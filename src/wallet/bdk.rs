use anyhow::Result;
use bdk_esplora::esplora_client::r#async::AsyncClient;
use bdk_esplora::esplora_client::Builder;
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::{KeychainKind, Wallet};
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct BdkWallet {
    wallet: Arc<Mutex<Wallet>>,
    client: AsyncClient,
}

#[derive(Debug, Clone)]
pub struct DepositInfo {
    pub txid: String,
    pub amount_sats: u64,
    pub confirmations: u32,
}

impl BdkWallet {
    pub async fn new(descriptor: &str, esplora_url: &str) -> Result<Self> {
        let wallet = Wallet::create_single(descriptor.to_string())
            .network(Network::Bitcoin)
            .create_wallet_no_persist()?;

        let builder = Builder::new(esplora_url);
        let client = AsyncClient::from_builder(builder)?;

        Ok(Self {
            wallet: Arc::new(Mutex::new(wallet)),
            client,
        })
    }

    pub async fn get_address(&self, index: u32) -> Result<String> {
        let wallet = self.wallet.lock().await;
        let address = wallet.peek_address(KeychainKind::External, index);
        Ok(address.address.to_string())
    }

    pub async fn full_scan(&self) -> Result<()> {
        let mut wallet = self.wallet.lock().await;

        let request = wallet.start_full_scan();
        let update = self.client.full_scan(request, 20, 5).await?;

        wallet.apply_update(update)?;
        Ok(())
    }

    pub async fn sync(&self) -> Result<()> {
        let mut wallet = self.wallet.lock().await;

        let request = wallet.start_sync_with_revealed_spks();
        let update = self.client.sync(request, 5).await?;

        wallet.apply_update(update)?;
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
