mod api;
mod config;
mod db;
mod lightning;
mod rate_limit;
mod wallet;
mod workers;

use crate::api::create_router;
use crate::config::Config;
use crate::lightning::NwcClient;
use crate::rate_limit::RateLimiter;
use crate::wallet::BdkWallet;
use crate::workers::{run_deposit_monitor, run_payment_processor};
use chrono::{DateTime, Utc};
use rustls::crypto::ring::default_provider;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub struct AppState {
    pub db: SqlitePool,
    pub wallet: BdkWallet,
    pub nwc: NwcClient,
    pub config: Config,
    pub last_sync: RwLock<Option<DateTime<Utc>>>,
    pub rate_limiter: RateLimiter,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install rustls crypto provider (required for TLS connections)
    default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Initialize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "utxo_recycler=info,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting UTXO Recycler...");

    // Load configuration
    let config = Config::from_env()?;
    tracing::info!("Configuration loaded");
    tracing::info!("  - Electrum URL: {}", config.electrum_url);
    if let Some(ref proxy) = config.tor_proxy {
        tracing::info!("  - Tor proxy: {}", proxy);
    }
    tracing::info!("  - Payout multiplier: {}x", config.payout_multiplier);
    tracing::info!("  - Required confirmations: {}", config.required_confirmations);

    // Initialize database
    let db = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;

    // Run migrations
    tracing::info!("Running database migrations...");
    sqlx::query(include_str!("../migrations/001_initial.sql"))
        .execute(&db)
        .await?;
    // Run migration 002 - adds blockheight cutoff and input size columns
    // Each statement is run individually to handle SQLite ALTER TABLE limitations
    let migration_002 = include_str!("../migrations/002_blockheight_cutoff.sql");
    for statement in migration_002.split(';') {
        // Remove comments and whitespace
        let cleaned: String = statement
            .lines()
            .filter(|line| !line.trim().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let cleaned = cleaned.trim();

        if cleaned.is_empty() {
            continue;
        }

        if let Err(e) = sqlx::query(cleaned).execute(&db).await {
            let err_str = e.to_string();
            // Ignore "duplicate column" and "already exists" errors
            if !err_str.contains("duplicate column") && !err_str.contains("already exists") {
                tracing::warn!("Migration statement failed (may be expected): {}", e);
            }
        }
    }
    tracing::info!("Database ready");

    // Initialize BDK wallet
    tracing::info!("Initializing BDK wallet...");
    let wallet = BdkWallet::new(&config.wallet_descriptor, &config.electrum_url, config.tor_proxy.clone()).await?;

    // Do initial full scan (non-fatal if it fails - background worker will retry)
    tracing::info!("Performing initial wallet sync (this may take a moment)...");
    let initial_sync_time = match wallet.full_scan().await {
        Ok(_) => {
            tracing::info!("Wallet synced");
            Some(Utc::now())
        }
        Err(e) => {
            tracing::warn!("Initial wallet sync failed (will retry in background): {}", e);
            None
        }
    };

    // Initialize NWC client
    tracing::info!("Connecting to Lightning wallet via NWC...");
    let nwc = NwcClient::new(&config.nwc_uri).await?;
    tracing::info!("NWC connected");

    // Initialize rate limiter
    let rate_limiter = RateLimiter::new(
        config.rate_limit_max_requests,
        config.rate_limit_window_secs,
    );

    // Create shared state
    let state = Arc::new(AppState {
        db,
        wallet,
        nwc,
        config: config.clone(),
        last_sync: RwLock::new(initial_sync_time),
        rate_limiter,
    });

    // Spawn background workers
    let monitor_state = Arc::clone(&state);
    tokio::spawn(async move {
        tracing::info!("Starting deposit monitor worker...");
        run_deposit_monitor(monitor_state).await;
    });

    let processor_state = Arc::clone(&state);
    tokio::spawn(async move {
        tracing::info!("Starting payment processor worker...");
        run_payment_processor(processor_state).await;
    });

    // Create router
    let app = create_router()
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state);

    // Start server
    let addr = SocketAddr::new(
        config.server_host.parse()?,
        config.server_port,
    );
    tracing::info!("Server listening on http://{}", addr);
    if config.admin_token.is_some() {
        tracing::info!("Admin endpoint enabled at /admin/stats?token=<ADMIN_TOKEN>");
    }

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Use into_make_service_with_connect_info to get client IP addresses
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
