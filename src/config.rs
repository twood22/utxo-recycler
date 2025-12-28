use std::env;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub nwc_uri: String,
    pub wallet_descriptor: String,
    pub esplora_url: String,
    pub payout_multiplier: f64,
    pub required_confirmations: u32,
    pub server_host: String,
    pub server_port: u16,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenvy::dotenv().ok();

        Ok(Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:utxo_recycler.db?mode=rwc".to_string()),
            nwc_uri: env::var("NWC_URI")
                .map_err(|_| anyhow::anyhow!("NWC_URI environment variable required"))?,
            wallet_descriptor: env::var("WALLET_DESCRIPTOR")
                .map_err(|_| anyhow::anyhow!("WALLET_DESCRIPTOR environment variable required"))?,
            esplora_url: env::var("ESPLORA_URL")
                .unwrap_or_else(|_| "https://blockstream.info/api".to_string()),
            payout_multiplier: env::var("PAYOUT_MULTIPLIER")
                .unwrap_or_else(|_| "1.01".to_string())
                .parse()
                .unwrap_or(1.01),
            required_confirmations: env::var("REQUIRED_CONFIRMATIONS")
                .unwrap_or_else(|_| "6".to_string())
                .parse()
                .unwrap_or(6),
            server_host: env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            server_port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "3000".to_string())
                .parse()
                .unwrap_or(3000),
        })
    }
}
