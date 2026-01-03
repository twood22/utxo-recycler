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
                    "Found deposit for recycle {}: {} sats, {} confirmations, block {:?}",
                    recycle.id,
                    deposit.amount_sats,
                    deposit.confirmations,
                    deposit.block_height
                );

                match recycle.status {
                    RecycleStatus::AwaitingDeposit => {
                        // First time seeing this deposit - check eligibility
                        if let Some(block_height) = deposit.block_height {
                            // Check 1: Block height cutoff
                            if block_height >= state.config.cutoff_block_height {
                                tracing::info!(
                                    "Recycle {} deposit at block {} is AFTER cutoff {} - marking as donation",
                                    recycle.id,
                                    block_height,
                                    state.config.cutoff_block_height
                                );
                                RecycleRepository::update_as_donation(
                                    &state.db,
                                    &recycle.id,
                                    &deposit.txid,
                                    deposit.amount_sats,
                                    Some(block_height),
                                    None,
                                    "block_height",
                                )
                                .await?;
                                continue;
                            }

                            // Check 2: Input UTXO sizes - are they actually dust?
                            let max_input = state.wallet.get_max_input_value(&deposit.txid).await?;
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
                                        &deposit.txid,
                                        deposit.amount_sats,
                                        Some(block_height),
                                        Some(max_input_value),
                                        "input_too_large",
                                    )
                                    .await?;
                                    continue;
                                }

                                // Both checks passed - eligible for payout
                                tracing::info!(
                                    "Recycle {} passed all checks (block {}, max input {} sats) - eligible for payout",
                                    recycle.id,
                                    block_height,
                                    max_input_value
                                );
                                RecycleRepository::update_deposit_detected(
                                    &state.db,
                                    &recycle.id,
                                    &deposit.txid,
                                    deposit.amount_sats,
                                    deposit.confirmations,
                                    Some(block_height),
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
                                    &deposit.txid,
                                    deposit.amount_sats,
                                    deposit.confirmations,
                                    Some(block_height),
                                    None,
                                    state.config.required_confirmations,
                                )
                                .await?;
                            }

                            if deposit.confirmations >= state.config.required_confirmations {
                                tracing::info!("Recycle {} confirmed immediately!", recycle.id);
                            }
                        } else {
                            // Unconfirmed - can't determine eligibility yet, just track the deposit
                            tracing::debug!(
                                "Recycle {} deposit is unconfirmed, waiting for confirmation to determine eligibility",
                                recycle.id
                            );
                            RecycleRepository::update_deposit_detected(
                                &state.db,
                                &recycle.id,
                                &deposit.txid,
                                deposit.amount_sats,
                                deposit.confirmations,
                                None,
                                None,
                                state.config.required_confirmations,
                            )
                            .await?;
                        }
                    }
                    RecycleStatus::Confirming => {
                        // Check if we now have block height info (tx just confirmed)
                        if let Some(block_height) = deposit.block_height {
                            // Check if we've already determined eligibility
                            if recycle.deposit_block_height.is_none() {
                                // First time seeing block height - run full eligibility checks

                                // Check 1: Block height cutoff
                                if block_height >= state.config.cutoff_block_height {
                                    tracing::info!(
                                        "Recycle {} deposit at block {} is AFTER cutoff {} - marking as donation",
                                        recycle.id,
                                        block_height,
                                        state.config.cutoff_block_height
                                    );
                                    RecycleRepository::update_as_donation(
                                        &state.db,
                                        &recycle.id,
                                        &deposit.txid,
                                        deposit.amount_sats,
                                        Some(block_height),
                                        None,
                                        "block_height",
                                    )
                                    .await?;
                                    continue;
                                }

                                // Check 2: Input UTXO sizes
                                let max_input = state.wallet.get_max_input_value(&deposit.txid).await?;
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
                                            &deposit.txid,
                                            deposit.amount_sats,
                                            Some(block_height),
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
