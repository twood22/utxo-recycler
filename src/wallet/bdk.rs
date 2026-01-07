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

#[derive(Debug, Clone)]
pub struct DepositInfo {
    pub txid: String,
    pub amount_sats: u64,
    pub confirmations: u32,
    /// The block height where this transaction was confirmed.
    /// None if the transaction is still unconfirmed.
    pub block_height: Option<u32>,
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

    pub async fn check_address_deposit(
        &self,
        _address: &str,
        address_index: u32,
    ) -> Result<Option<DepositInfo>> {
        let wallet = self.wallet.lock().await;

        // Get transactions for this wallet
        for tx in wallet.transactions() {
            let txid = tx.tx_node.txid.to_string();

            // Check if any output goes to our address
            for output in tx.tx_node.tx.output.iter() {
                let script = &output.script_pubkey;

                // Get the address for this index and compare
                let expected_address = wallet.peek_address(KeychainKind::External, address_index);
                if *script == expected_address.address.script_pubkey() {
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

                    return Ok(Some(DepositInfo {
                        txid,
                        amount_sats: output.value.to_sat(),
                        confirmations,
                        block_height,
                    }));
                }
            }
        }

        Ok(None)
    }

    pub async fn reveal_addresses_up_to(&self, index: u32) -> Result<()> {
        let mut wallet = self.wallet.lock().await;

        // Reveal addresses up to and including the given index
        let _ = wallet.reveal_addresses_to(KeychainKind::External, index);

        Ok(())
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

    /// Get the maximum creation block height of input UTXOs.
    /// This checks when the INPUT UTXOs were originally created (confirmed),
    /// NOT when the deposit transaction was confirmed.
    /// Used to verify UTXOs existed before the cutoff block.
    pub async fn get_max_input_creation_height(&self, txid_str: &str) -> Result<Option<u32>> {
        let electrum_url = self.electrum_url.clone();
        let tor_proxy = self.tor_proxy.clone();
        let txid_string = txid_str.to_string();

        tokio::task::spawn_blocking(move || -> Result<Option<u32>> {
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

            // Fetch the deposit transaction
            let tx = match client.transaction_get(&txid) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("Failed to fetch transaction {}: {}", txid_string, e);
                    return Ok(None);
                }
            };

            let mut max_creation_height: u32 = 0;

            // For each input, find when the parent UTXO was created
            for input in tx.input.iter() {
                let prev_txid = input.previous_output.txid;
                let prev_vout = input.previous_output.vout as usize;

                // Fetch the previous transaction to get the output script
                let prev_tx = match client.transaction_get(&prev_txid) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to fetch parent tx {} for input: {}",
                            prev_txid,
                            e
                        );
                        continue;
                    }
                };

                // Get the output script that was spent
                let output = match prev_tx.output.get(prev_vout) {
                    Some(o) => o,
                    None => {
                        tracing::warn!("Output {} not found in tx {}", prev_vout, prev_txid);
                        continue;
                    }
                };

                // Get history for this script to find the block height
                let script = &output.script_pubkey;
                let history = match client.script_get_history(script) {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to get script history for parent tx {}: {}",
                            prev_txid,
                            e
                        );
                        continue;
                    }
                };

                // Find our parent transaction in the history
                for entry in history {
                    if entry.tx_hash == prev_txid && entry.height > 0 {
                        let height = entry.height as u32;
                        tracing::debug!(
                            "Input UTXO {}:{} was created in block {}",
                            prev_txid,
                            prev_vout,
                            height
                        );
                        if height > max_creation_height {
                            max_creation_height = height;
                        }
                        break;
                    }
                }
            }

            if max_creation_height == 0 && !tx.input.is_empty() {
                tracing::warn!("Could not determine input creation heights for tx {}", txid_string);
                return Ok(None);
            }

            tracing::info!(
                "Max input UTXO creation height for tx {}: block {}",
                txid_string,
                max_creation_height
            );

            Ok(Some(max_creation_height))
        })
        .await?
    }
}
