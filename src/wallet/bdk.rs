use anyhow::Result;
use bdk_electrum::electrum_client::{Client, ConfigBuilder, Socks5Config};
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::{KeychainKind, Wallet};
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
                    let confirmations = match tx.chain_position {
                        bdk_wallet::chain::ChainPosition::Confirmed {
                            anchor,
                            transitively: _,
                        } => {
                            let current_height = wallet.latest_checkpoint().height();
                            current_height.saturating_sub(anchor.block_id.height) + 1
                        }
                        bdk_wallet::chain::ChainPosition::Unconfirmed { .. } => 0,
                    };

                    return Ok(Some(DepositInfo {
                        txid,
                        amount_sats: output.value.to_sat(),
                        confirmations,
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
}
