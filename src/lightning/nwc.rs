use anyhow::{anyhow, Result};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub struct NwcClient {
    client: Client,
    wallet_pubkey: PublicKey,
    secret_key: SecretKey,
}

#[derive(Debug, Clone)]
pub struct PaymentResult {
    pub preimage: String,
    pub payment_hash: String,
}

// NIP-47 request/response structures
#[derive(Serialize)]
struct Nip47Request {
    method: String,
    params: Nip47PayInvoiceParams,
}

#[derive(Serialize)]
struct Nip47PayInvoiceParams {
    invoice: String,
}

#[derive(Deserialize)]
struct Nip47Response {
    result_type: Option<String>,
    result: Option<Nip47PayInvoiceResult>,
    error: Option<Nip47Error>,
}

#[derive(Deserialize)]
struct Nip47PayInvoiceResult {
    preimage: String,
}

#[derive(Deserialize)]
struct Nip47Error {
    code: String,
    message: String,
}

impl NwcClient {
    pub async fn new(nwc_uri: &str) -> Result<Self> {
        // Parse the NWC URI manually
        // Format: nostr+walletconnect://pubkey?relay=wss://...&secret=hex
        let uri = nwc_uri.strip_prefix("nostr+walletconnect://")
            .ok_or_else(|| anyhow!("Invalid NWC URI format"))?;

        let parts: Vec<&str> = uri.splitn(2, '?').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid NWC URI format: missing query params"));
        }

        let wallet_pubkey_hex = parts[0];
        let query = parts[1];

        let mut relay_url = None;
        let mut secret_hex = None;

        for param in query.split('&') {
            let kv: Vec<&str> = param.splitn(2, '=').collect();
            if kv.len() == 2 {
                match kv[0] {
                    "relay" => relay_url = Some(urlencoding::decode(kv[1])?.into_owned()),
                    "secret" => secret_hex = Some(kv[1].to_string()),
                    _ => {}
                }
            }
        }

        let relay_url = relay_url.ok_or_else(|| anyhow!("Missing relay in NWC URI"))?;
        let secret_hex = secret_hex.ok_or_else(|| anyhow!("Missing secret in NWC URI"))?;

        let wallet_pubkey = PublicKey::from_hex(wallet_pubkey_hex)?;
        let secret_key = SecretKey::from_hex(&secret_hex)?;
        let keys = Keys::new(secret_key.clone());

        let client = Client::new(keys);
        client.add_relay(&relay_url).await?;
        client.connect().await;

        Ok(Self {
            client,
            wallet_pubkey,
            secret_key,
        })
    }

    pub async fn pay_invoice(&self, bolt11: &str) -> Result<PaymentResult> {
        // Create the NIP-47 pay_invoice request
        let request = Nip47Request {
            method: "pay_invoice".to_string(),
            params: Nip47PayInvoiceParams {
                invoice: bolt11.to_string(),
            },
        };
        let request_json = serde_json::to_string(&request)?;

        // Encrypt using NIP-04
        let encrypted = nip04::encrypt(&self.secret_key, &self.wallet_pubkey, &request_json)?;

        // Get our public key from the keys
        let keys = Keys::new(self.secret_key.clone());
        let our_pubkey = keys.public_key();

        // Create and send the event
        let event_builder = EventBuilder::new(Kind::WalletConnectRequest, encrypted)
            .tag(Tag::public_key(self.wallet_pubkey));

        let output = self.client.send_event_builder(event_builder).await?;
        let event_id = *output.id();

        tracing::debug!("Sent NWC payment request, event_id: {}", event_id);

        // Wait for the response - try multiple times with short intervals
        let filter = Filter::new()
            .kind(Kind::WalletConnectResponse)
            .author(self.wallet_pubkey)
            .pubkey(our_pubkey)
            .event(event_id);

        // Poll for response multiple times
        for attempt in 1..=5 {
            let timeout = Duration::from_secs(3);
            let events = self.client.fetch_events(vec![filter.clone()], timeout).await?;

            if let Some(response_event) = events.into_iter().next() {
                // Decrypt the response
                let decrypted = nip04::decrypt(&self.secret_key, &self.wallet_pubkey, &response_event.content)?;
                let response: Nip47Response = serde_json::from_str(&decrypted)?;

                if let Some(error) = response.error {
                    return Err(anyhow!("Payment failed: {} - {}", error.code, error.message));
                }

                if let Some(result) = response.result {
                    return Ok(PaymentResult {
                        preimage: result.preimage.clone(),
                        payment_hash: result.preimage,
                    });
                }
            }

            tracing::debug!("NWC response attempt {}/5 - no response yet", attempt);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        // No response received - return an error so the payment processor can decide
        // whether to retry. Do NOT assume success as this could cause fund loss.
        tracing::warn!("No NWC response received after 5 attempts for event {}", event_id);
        Err(anyhow!("No response from wallet after 5 attempts (event_id: {}). Payment status unknown - will retry.", event_id))
    }
}
