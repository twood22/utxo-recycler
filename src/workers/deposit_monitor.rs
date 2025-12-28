use crate::db::{RecycleRepository, RecycleStatus};
use crate::AppState;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

const BASE_INTERVAL_SECS: u64 = 30;
const MAX_BACKOFF_SECS: u64 = 300; // 5 minutes max

pub async fn run_deposit_monitor(state: Arc<AppState>) {
    let mut consecutive_errors: u32 = 0;

    loop {
        // Calculate delay with exponential backoff on errors
        let delay_secs = if consecutive_errors == 0 {
            BASE_INTERVAL_SECS
        } else {
            (BASE_INTERVAL_SECS * 2u64.pow(consecutive_errors.min(4))).min(MAX_BACKOFF_SECS)
        };

        time::sleep(Duration::from_secs(delay_secs)).await;

        match check_deposits(&state).await {
            Ok(_) => {
                consecutive_errors = 0;
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors <= 2 {
                    tracing::warn!("Deposit monitor error (will retry): {}", e);
                } else {
                    tracing::error!(
                        "Deposit monitor error (attempt {}, backing off to {}s): {}",
                        consecutive_errors,
                        (BASE_INTERVAL_SECS * 2u64.pow(consecutive_errors.min(4))).min(MAX_BACKOFF_SECS),
                        e
                    );
                }
            }
        }
    }
}

async fn check_deposits(state: &AppState) -> anyhow::Result<()> {
    // Sync the wallet with the blockchain
    tracing::debug!("Syncing wallet with blockchain...");
    state.wallet.sync().await?;

    // Get all pending recycles (awaiting_deposit or confirming)
    let pending = RecycleRepository::find_pending_deposits(&state.db).await?;

    for recycle in pending {
        match state
            .wallet
            .check_address_deposit(&recycle.deposit_address, recycle.address_index)
            .await
        {
            Ok(Some(deposit)) => {
                tracing::info!(
                    "Found deposit for recycle {}: {} sats, {} confirmations",
                    recycle.id,
                    deposit.amount_sats,
                    deposit.confirmations
                );

                match recycle.status {
                    RecycleStatus::AwaitingDeposit => {
                        // First time seeing this deposit
                        RecycleRepository::update_deposit_detected(
                            &state.db,
                            &recycle.id,
                            &deposit.txid,
                            deposit.amount_sats,
                            deposit.confirmations,
                        )
                        .await?;

                        if deposit.confirmations >= state.config.required_confirmations {
                            tracing::info!("Recycle {} confirmed immediately!", recycle.id);
                        }
                    }
                    RecycleStatus::Confirming => {
                        // Update confirmation count
                        RecycleRepository::update_confirmations(
                            &state.db,
                            &recycle.id,
                            deposit.confirmations,
                            state.config.required_confirmations,
                        )
                        .await?;

                        if deposit.confirmations >= state.config.required_confirmations {
                            tracing::info!(
                                "Recycle {} reached {} confirmations!",
                                recycle.id,
                                deposit.confirmations
                            );
                        }
                    }
                    _ => {}
                }
            }
            Ok(None) => {
                // No deposit yet
                tracing::debug!("No deposit found for recycle {}", recycle.id);
            }
            Err(e) => {
                tracing::warn!(
                    "Error checking deposit for recycle {}: {}",
                    recycle.id,
                    e
                );
            }
        }
    }

    Ok(())
}
