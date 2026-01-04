use std::env;

/// The cutoff block height for UTXO eligibility.
/// Only UTXOs created BEFORE this block are eligible for payout.
/// UTXOs created at or after this block are kept as donations.
pub const DEFAULT_CUTOFF_BLOCK_HEIGHT: u32 = 930_400;

/// Maximum input UTXO size in satoshis.
/// Only transactions where ALL inputs are below this threshold are eligible.
/// This ensures we're only accepting true dust consolidation, not regular transactions.
pub const DEFAULT_MAX_INPUT_SATS: u64 = 1_000;

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub nwc_uri: String,
    pub wallet_descriptor: String,
    pub electrum_url: String,
    pub tor_proxy: Option<String>,
    pub payout_multiplier: f64,
    pub required_confirmations: u32,
    pub server_host: String,
    pub server_port: u16,
    /// Only UTXOs created before this block height are eligible for payout.
    /// UTXOs at or after this height are treated as donations.
    pub cutoff_block_height: u32,
    /// Maximum input UTXO size in satoshis.
    /// Transactions with any input larger than this are rejected (kept as donations).
    pub max_input_sats: u64,
    /// Admin token for accessing /admin routes. If not set, admin routes are disabled.
    pub admin_token: Option<String>,
    /// Rate limit: max requests per window (default: 10)
    pub rate_limit_max_requests: u32,
    /// Rate limit: window duration in seconds (default: 60)
    pub rate_limit_window_secs: u64,
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
            electrum_url: env::var("ELECTRUM_URL")
                .unwrap_or_else(|_| "ssl://electrum.blockstream.info:50002".to_string()),
            tor_proxy: env::var("TOR_PROXY").ok(),
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
            cutoff_block_height: env::var("CUTOFF_BLOCK_HEIGHT")
                .unwrap_or_else(|_| DEFAULT_CUTOFF_BLOCK_HEIGHT.to_string())
                .parse()
                .unwrap_or(DEFAULT_CUTOFF_BLOCK_HEIGHT),
            max_input_sats: env::var("MAX_INPUT_SATS")
                .unwrap_or_else(|_| DEFAULT_MAX_INPUT_SATS.to_string())
                .parse()
                .unwrap_or(DEFAULT_MAX_INPUT_SATS),
            admin_token: env::var("ADMIN_TOKEN").ok(),
            rate_limit_max_requests: env::var("RATE_LIMIT_MAX_REQUESTS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .unwrap_or(10),
            rate_limit_window_secs: env::var("RATE_LIMIT_WINDOW_SECS")
                .unwrap_or_else(|_| "60".to_string())
                .parse()
                .unwrap_or(60),
        })
    }
}
