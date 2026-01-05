use crate::db::{RecycleRepository, RecycleStatus};
use crate::AppState;
use chrono::Utc;
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

    // Update last sync time
    {
        let mut last_sync = state.last_sync.write().await;
        *last_sync = Some(Utc::now());
    }

    // Get all pending recycles (awaiting_deposit or confirming)
    let pending = RecycleRepository::find_pending_deposits(&state.db).await?;

    for recycle in pending {
        match state
            .wallet
            .check_address_deposit(&recycle.deposit_address, recycle.address_index)
            .await
        {
            Ok(Some(deposit)) => {
                // Log deposit info (note if multiple deposits)
                if deposit.deposit_count() > 1 {
                    tracing::info!(
                        "Found {} deposits for recycle {}: {} sats total, {} min confirmations",
                        deposit.deposit_count(),
                        recycle.id,
                        deposit.amount_sats,
                        deposit.min_confirmations
                    );
                } else {
                    tracing::info!(
                        "Found deposit for recycle {}: {} sats, {} confirmations, block {:?}",
                        recycle.id,
                        deposit.amount_sats,
                        deposit.min_confirmations,
                        deposit.min_block_height()
                    );
                }

                match recycle.status {
                    RecycleStatus::AwaitingDeposit => {
                        // First time seeing this deposit - check eligibility
                        // For multiple deposits, we check ALL of them
                        if deposit.all_confirmed() {
                            let max_block = deposit.max_block_height().unwrap_or(0);
                            let min_block = deposit.min_block_height().unwrap_or(0);

                            // Check 1: Block height cutoff - ANY deposit after cutoff = ineligible
                            if max_block >= state.config.cutoff_block_height {
                                tracing::info!(
                                    "Recycle {} has deposit at block {} (AFTER cutoff {}) - marking as donation",
                                    recycle.id,
                                    max_block,
                                    state.config.cutoff_block_height
                                );
                                RecycleRepository::update_as_donation(
                                    &state.db,
                                    &recycle.id,
                                    &deposit.txids_csv(),
                                    deposit.amount_sats,
                                    Some(max_block),
                                    None,
                                    "block_height",
                                )
                                .await?;
                                continue;
                            }

                            // Check 2: Input UTXO sizes - check ALL transactions
                            let max_input = state.wallet.get_max_input_value_for_txids(&deposit.txids).await?;
                            if let Some(max_input_value) = max_input {
                                if max_input_value >= state.config.max_input_sats {
                                    tracing::info!(
                                        "Recycle {} has input of {} sats (>= {} limit) - marking as donation",
                                        recycle.id,
                                        max_input_value,
                                        state.config.max_input_sats
                                    );
                                    RecycleRepository::update_as_donation(
                                        &state.db,
                                        &recycle.id,
                                        &deposit.txids_csv(),
                                        deposit.amount_sats,
                                        Some(min_block),
                                        Some(max_input_value),
                                        "input_too_large",
                                    )
                                    .await?;
                                    continue;
                                }

                                // All checks passed - eligible for payout
                                tracing::info!(
                                    "Recycle {} passed all checks ({} deposits, blocks {}-{}, max input {} sats) - eligible for payout",
                                    recycle.id,
                                    deposit.deposit_count(),
                                    min_block,
                                    max_block,
                                    max_input_value
                                );
                                RecycleRepository::update_deposit_detected(
                                    &state.db,
                                    &recycle.id,
                                    &deposit.txids_csv(),
                                    deposit.amount_sats,
                                    deposit.min_confirmations,
                                    Some(min_block),
                                    Some(max_input_value),
                                    state.config.required_confirmations,
                                )
                                .await?;
                            } else {
                                // Couldn't determine input values - allow it (benefit of doubt)
                                tracing::warn!(
                                    "Recycle {} - couldn't verify input values, allowing",
                                    recycle.id
                                );
                                RecycleRepository::update_deposit_detected(
                                    &state.db,
                                    &recycle.id,
                                    &deposit.txids_csv(),
                                    deposit.amount_sats,
                                    deposit.min_confirmations,
                                    Some(min_block),
                                    None,
                                    state.config.required_confirmations,
                                )
                                .await?;
                            }

                            if deposit.min_confirmations >= state.config.required_confirmations {
                                tracing::info!("Recycle {} confirmed immediately!", recycle.id);
                            }
                        } else {
                            // Not all deposits confirmed - can't determine eligibility yet
                            tracing::debug!(
                                "Recycle {} has {} deposits, waiting for all to confirm",
                                recycle.id,
                                deposit.deposit_count()
                            );
                            RecycleRepository::update_deposit_detected(
                                &state.db,
                                &recycle.id,
                                &deposit.txids_csv(),
                                deposit.amount_sats,
                                deposit.min_confirmations,
                                None,
                                None,
                                state.config.required_confirmations,
                            )
                            .await?;
                        }
                    }
                    RecycleStatus::Confirming => {
                        // Check if all deposits are now confirmed
                        if deposit.all_confirmed() {
                            // Check if we've already determined eligibility
                            if recycle.deposit_block_height.is_none() {
                                // First time all confirmed - run full eligibility checks
                                let max_block = deposit.max_block_height().unwrap_or(0);
                                let min_block = deposit.min_block_height().unwrap_or(0);

                                // Check 1: Block height cutoff - ANY deposit after cutoff = ineligible
                                if max_block >= state.config.cutoff_block_height {
                                    tracing::info!(
                                        "Recycle {} has deposit at block {} (AFTER cutoff {}) - marking as donation",
                                        recycle.id,
                                        max_block,
                                        state.config.cutoff_block_height
                                    );
                                    RecycleRepository::update_as_donation(
                                        &state.db,
                                        &recycle.id,
                                        &deposit.txids_csv(),
                                        deposit.amount_sats,
                                        Some(max_block),
                                        None,
                                        "block_height",
                                    )
                                    .await?;
                                    continue;
                                }

                                // Check 2: Input UTXO sizes - check ALL transactions
                                let max_input = state.wallet.get_max_input_value_for_txids(&deposit.txids).await?;
                                if let Some(max_input_value) = max_input {
                                    if max_input_value >= state.config.max_input_sats {
                                        tracing::info!(
                                            "Recycle {} has input of {} sats (>= {} limit) - marking as donation",
                                            recycle.id,
                                            max_input_value,
                                            state.config.max_input_sats
                                        );
                                        RecycleRepository::update_as_donation(
                                            &state.db,
                                            &recycle.id,
                                            &deposit.txids_csv(),
                                            deposit.amount_sats,
                                            Some(min_block),
                                            Some(max_input_value),
                                            "input_too_large",
                                        )
                                        .await?;
                                        continue;
                                    }
                                }
                            }
                        }

                        // Update confirmation count (already known to be eligible)
                        RecycleRepository::update_confirmations(
                            &state.db,
                            &recycle.id,
                            deposit.min_confirmations,
                            state.config.required_confirmations,
                        )
                        .await?;

                        if deposit.min_confirmations >= state.config.required_confirmations {
                            tracing::info!(
                                "Recycle {} reached {} confirmations!",
                                recycle.id,
                                deposit.min_confirmations
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
