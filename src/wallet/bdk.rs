use anyhow::Result;
use bdk_electrum::electrum_client::{Client, ConfigBuilder, ElectrumApi, Socks5Config};
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::{Network, Txid};
use bdk_wallet::{KeychainKind, Wallet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct BdkWallet {
    wallet: Arc<Mutex<Wallet>>,
    electrum_url: String,
    tor_proxy: Option<String>,
}

/// Information about a single deposit transaction
#[derive(Debug, Clone)]
pub struct SingleDeposit {
    pub txid: String,
    pub amount_sats: u64,
    pub confirmations: u32,
    /// The block height where this transaction was confirmed.
    /// None if the transaction is still unconfirmed.
    pub block_height: Option<u32>,
}

/// Aggregated information about all deposits to an address.
/// Handles the case where a user sends multiple transactions to the same address.
#[derive(Debug, Clone)]
pub struct DepositInfo {
    /// All transaction IDs contributing to this deposit (comma-separated when stored)
    pub txids: Vec<String>,
    /// Total amount across all deposits
    pub amount_sats: u64,
    /// Minimum confirmations across all deposits (conservative - wait for all to confirm)
    pub min_confirmations: u32,
    /// All individual deposits with their block heights (for eligibility checks)
    pub deposits: Vec<SingleDeposit>,
}

impl DepositInfo {
    /// Get the first/primary txid (for backward compatibility in display)
    pub fn primary_txid(&self) -> &str {
        self.txids.first().map(|s| s.as_str()).unwrap_or("")
    }

    /// Get txids as comma-separated string for DB storage
    pub fn txids_csv(&self) -> String {
        self.txids.join(",")
    }

    /// Get the minimum block height across all confirmed deposits
    /// Returns None if any deposit is unconfirmed
    pub fn min_block_height(&self) -> Option<u32> {
        let heights: Vec<u32> = self.deposits
            .iter()
            .filter_map(|d| d.block_height)
            .collect();

        // If not all deposits are confirmed, return None
        if heights.len() != self.deposits.len() {
            return None;
        }

        heights.into_iter().min()
    }

    /// Get the maximum block height across all deposits (for cutoff check)
    /// Any deposit after the cutoff makes the whole thing ineligible
    pub fn max_block_height(&self) -> Option<u32> {
        self.deposits
            .iter()
            .filter_map(|d| d.block_height)
            .max()
    }

    /// Check if all deposits are confirmed
    pub fn all_confirmed(&self) -> bool {
        self.deposits.iter().all(|d| d.block_height.is_some())
    }

    /// Number of individual deposits
    pub fn deposit_count(&self) -> usize {
        self.deposits.len()
    }
}

impl BdkWallet {
    pub async fn new(descriptor: &str, electrum_url: &str, tor_proxy: Option<String>) -> Result<Self> {
        let wallet = Wallet::create_single(descriptor.to_string())
            .network(Network::Bitcoin)
            .create_wallet_no_persist()?;

        Ok(Self {
            wallet: Arc::new(Mutex::new(wallet)),
            electrum_url: electrum_url.to_string(),
            tor_proxy,
        })
    }

    fn create_client(&self) -> Result<BdkElectrumClient<Client>> {
        let config = if let Some(ref proxy) = self.tor_proxy {
            tracing::debug!("Connecting via Tor proxy: {}", proxy);
            ConfigBuilder::new()
                .socks5(Some(Socks5Config {
                    addr: proxy.clone(),
                    credentials: None,
                }))
                .timeout(Some(30))
                .build()
        } else {
            ConfigBuilder::new()
                .timeout(Some(30))
                .build()
        };

        let client = Client::from_config(&self.electrum_url, config)?;
        Ok(BdkElectrumClient::new(client))
    }

    pub async fn get_address(&self, index: u32) -> Result<String> {
        let wallet = self.wallet.lock().await;
        let address = wallet.peek_address(KeychainKind::External, index);
        Ok(address.address.to_string())
    }

    pub async fn full_scan(&self) -> Result<()> {
        let electrum_url = self.electrum_url.clone();
        let tor_proxy = self.tor_proxy.clone();
        let wallet = self.wallet.clone();

        // Electrum client is synchronous, so run in blocking task
        tokio::task::spawn_blocking(move || -> Result<()> {
            let config = if let Some(ref proxy) = tor_proxy {
                tracing::debug!("Connecting via Tor proxy: {}", proxy);
                ConfigBuilder::new()
                    .socks5(Some(Socks5Config {
                        addr: proxy.clone(),
                        credentials: None,
                    }))
                    .timeout(Some(60))
                    .build()
            } else {
                ConfigBuilder::new()
                    .timeout(Some(60))
                    .build()
            };

            let client = Client::from_config(&electrum_url, config)?;
            let electrum_client = BdkElectrumClient::new(client);

            let mut wallet_guard = wallet.blocking_lock();
            let request = wallet_guard.start_full_scan();
            let update = electrum_client.full_scan(request, 20, 5, false)?;
            wallet_guard.apply_update(update)?;

            Ok(())
        })
        .await??;

        Ok(())
    }

    pub async fn sync(&self) -> Result<()> {
        let electrum_url = self.electrum_url.clone();
        let tor_proxy = self.tor_proxy.clone();
        let wallet = self.wallet.clone();

        // Electrum client is synchronous, so run in blocking task
        tokio::task::spawn_blocking(move || -> Result<()> {
            let config = if let Some(ref proxy) = tor_proxy {
                ConfigBuilder::new()
                    .socks5(Some(Socks5Config {
                        addr: proxy.clone(),
                        credentials: None,
                    }))
                    .timeout(Some(30))
                    .build()
            } else {
                ConfigBuilder::new()
                    .timeout(Some(30))
                    .build()
            };

            let client = Client::from_config(&electrum_url, config)?;
            let electrum_client = BdkElectrumClient::new(client);

            let mut wallet_guard = wallet.blocking_lock();
            let request = wallet_guard.start_sync_with_revealed_spks();
            let update = electrum_client.sync(request, 5, false)?;
            wallet_guard.apply_update(update)?;

            Ok(())
        })
        .await??;

        Ok(())
    }

    /// Check for deposits to a specific address.
    /// Returns aggregated info from ALL deposits to this address (handles multiple transactions).
    pub async fn check_address_deposit(
        &self,
        _address: &str,
        address_index: u32,
    ) -> Result<Option<DepositInfo>> {
        let wallet = self.wallet.lock().await;

        let expected_address = wallet.peek_address(KeychainKind::External, address_index);
        let expected_script = expected_address.address.script_pubkey();

        let mut deposits: Vec<SingleDeposit> = Vec::new();

        // Get ALL transactions for this wallet and collect ALL matching outputs
        for tx in wallet.transactions() {
            let txid = tx.tx_node.txid.to_string();

            // Check all outputs in this transaction
            for output in tx.tx_node.tx.output.iter() {
                if output.script_pubkey == expected_script {
                    let (confirmations, block_height) = match tx.chain_position {
                        bdk_wallet::chain::ChainPosition::Confirmed {
                            anchor,
                            transitively: _,
                        } => {
                            let current_height = wallet.latest_checkpoint().height();
                            let confs = current_height.saturating_sub(anchor.block_id.height) + 1;
                            (confs, Some(anchor.block_id.height))
                        }
                        bdk_wallet::chain::ChainPosition::Unconfirmed { .. } => (0, None),
                    };

                    deposits.push(SingleDeposit {
                        txid: txid.clone(),
                        amount_sats: output.value.to_sat(),
                        confirmations,
                        block_height,
                    });
                }
            }
        }

        if deposits.is_empty() {
            return Ok(None);
        }

        // Log if multiple deposits found
        if deposits.len() > 1 {
            tracing::info!(
                "Found {} deposits to address index {} (total: {} sats)",
                deposits.len(),
                address_index,
                deposits.iter().map(|d| d.amount_sats).sum::<u64>()
            );
        }

        // Aggregate the deposits
        let txids: Vec<String> = deposits.iter().map(|d| d.txid.clone()).collect();
        let total_amount: u64 = deposits.iter().map(|d| d.amount_sats).sum();
        let min_confirmations: u32 = deposits.iter().map(|d| d.confirmations).min().unwrap_or(0);

        Ok(Some(DepositInfo {
            txids,
            amount_sats: total_amount,
            min_confirmations,
            deposits,
        }))
    }

    pub async fn reveal_addresses_up_to(&self, index: u32) -> Result<()> {
        let mut wallet = self.wallet.lock().await;

        // Reveal addresses up to and including the given index
        let _ = wallet.reveal_addresses_to(KeychainKind::External, index);

        Ok(())
    }

    /// Get the maximum input UTXO value across multiple transactions.
    /// Checks ALL transactions to ensure we're only accepting true dust consolidation.
    pub async fn get_max_input_value_for_txids(&self, txids: &[String]) -> Result<Option<u64>> {
        let mut overall_max: u64 = 0;

        for txid in txids {
            match self.get_max_input_value(txid).await? {
                Some(max) => {
                    if max > overall_max {
                        overall_max = max;
                    }
                }
                None => {
                    // Couldn't verify this tx - return None to be safe
                    return Ok(None);
                }
            }
        }

        if overall_max == 0 && !txids.is_empty() {
            return Ok(None);
        }

        Ok(Some(overall_max))
    }

    /// Get the maximum input UTXO value for a transaction.
    /// This looks up each input's previous output to determine the original UTXO sizes.
    /// Returns the largest input value, or None if the transaction couldn't be found/parsed.
    pub async fn get_max_input_value(&self, txid_str: &str) -> Result<Option<u64>> {
        let electrum_url = self.electrum_url.clone();
        let tor_proxy = self.tor_proxy.clone();
        let txid_string = txid_str.to_string();

        // Electrum client is synchronous, so run in blocking task
        tokio::task::spawn_blocking(move || -> Result<Option<u64>> {
            let config = if let Some(ref proxy) = tor_proxy {
                ConfigBuilder::new()
                    .socks5(Some(Socks5Config {
                        addr: proxy.clone(),
                        credentials: None,
                    }))
                    .timeout(Some(30))
                    .build()
            } else {
                ConfigBuilder::new()
                    .timeout(Some(30))
                    .build()
            };

            let client = Client::from_config(&electrum_url, config)?;

            // Parse the txid
            let txid = match Txid::from_str(&txid_string) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("Failed to parse txid {}: {}", txid_string, e);
                    return Ok(None);
                }
            };

            // Fetch the transaction
            let tx = match client.transaction_get(&txid) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("Failed to fetch transaction {}: {}", txid_string, e);
                    return Ok(None);
                }
            };

            let mut max_input_value: u64 = 0;

            // For each input, look up the previous output's value
            for input in tx.input.iter() {
                let prev_txid = input.previous_output.txid;
                let prev_vout = input.previous_output.vout as usize;

                // Fetch the previous transaction
                let prev_tx = match client.transaction_get(&prev_txid) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to fetch parent tx {} for input: {}",
                            prev_txid,
                            e
                        );
                        // Can't verify this input - skip and continue
                        // We'll be conservative and not reject based on unknown inputs
                        continue;
                    }
                };

                // Get the output value at the specified index
                if let Some(output) = prev_tx.output.get(prev_vout) {
                    let value = output.value.to_sat();
                    if value > max_input_value {
                        max_input_value = value;
                    }
                    tracing::debug!(
                        "Input from {}:{} has value {} sats",
                        prev_txid,
                        prev_vout,
                        value
                    );
                }
            }

            if max_input_value == 0 && !tx.input.is_empty() {
                // Couldn't determine any input values
                tracing::warn!("Could not determine input values for tx {}", txid_string);
                return Ok(None);
            }

            Ok(Some(max_input_value))
        })
        .await?
    }
}
