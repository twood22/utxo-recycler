use crate::db::{RecycleRepository, RecycleStatus};
use crate::AppState;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

pub async fn run_deposit_monitor(state: Arc<AppState>) {
    let mut interval = time::interval(Duration::from_secs(30));

    loop {
        interval.tick().await;

        if let Err(e) = check_deposits(&state).await {
            tracing::error!("Deposit monitor error: {}", e);
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
