use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnurlPayResponse {
    pub callback: String,
    pub min_sendable: u64, // millisats
    pub max_sendable: u64, // millisats
    pub metadata: String,
    pub tag: String,
}

#[derive(Debug, Deserialize)]
pub struct LnurlInvoiceResponse {
    pub pr: String, // BOLT11 invoice
    pub routes: Option<Vec<serde_json::Value>>,
}

pub struct LnurlClient {
    client: Client,
}

impl LnurlClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Validate a lightning address format (user@domain)
    pub fn validate_lightning_address(address: &str) -> bool {
        let parts: Vec<&str> = address.split('@').collect();
        if parts.len() != 2 {
            return false;
        }

        let user = parts[0];
        let domain = parts[1];

        // User part should not be empty and should be valid
        if user.is_empty() {
            return false;
        }

        // Domain should be valid
        if domain.is_empty() || !domain.contains('.') {
            return false;
        }

        true
    }

    /// Convert a lightning address to its LNURL-pay endpoint
    fn lightning_address_to_url(address: &str) -> Result<String> {
        let parts: Vec<&str> = address.split('@').collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid lightning address format"));
        }

        let user = parts[0];
        let domain = parts[1];

        Ok(format!("https://{}/.well-known/lnurlp/{}", domain, user))
    }

    /// Fetch the LNURL-pay metadata for a lightning address
    pub async fn fetch_pay_params(&self, lightning_address: &str) -> Result<LnurlPayResponse> {
        let url = Self::lightning_address_to_url(lightning_address)?;

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to fetch LNURL params: {}",
                response.status()
            ));
        }

        let params: LnurlPayResponse = response.json().await?;

        if params.tag != "payRequest" {
            return Err(anyhow!("Invalid LNURL tag: expected payRequest"));
        }

        Ok(params)
    }

    /// Request a BOLT11 invoice for a specific amount (in millisats)
    pub async fn fetch_invoice(
        &self,
        callback: &str,
        amount_msats: u64,
    ) -> Result<LnurlInvoiceResponse> {
        let mut url = Url::parse(callback)?;
        url.query_pairs_mut()
            .append_pair("amount", &amount_msats.to_string());

        let response = self
            .client
            .get(url.as_str())
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch invoice: {}", response.status()));
        }

        let invoice: LnurlInvoiceResponse = response.json().await?;
        Ok(invoice)
    }

    /// Get a BOLT11 invoice for a lightning address and amount (in sats)
    pub async fn get_invoice_for_address(
        &self,
        lightning_address: &str,
        amount_sats: u64,
    ) -> Result<String> {
        let params = self.fetch_pay_params(lightning_address).await?;

        let amount_msats = amount_sats * 1000;

        if amount_msats < params.min_sendable {
            return Err(anyhow!(
                "Amount {} msats is below minimum {} msats",
                amount_msats,
                params.min_sendable
            ));
        }

        if amount_msats > params.max_sendable {
            return Err(anyhow!(
                "Amount {} msats is above maximum {} msats",
                amount_msats,
                params.max_sendable
            ));
        }

        let invoice_response = self.fetch_invoice(&params.callback, amount_msats).await?;
        Ok(invoice_response.pr)
    }
}

impl Default for LnurlClient {
    fn default() -> Self {
        Self::new()
    }
}
