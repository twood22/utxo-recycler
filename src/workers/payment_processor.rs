use crate::db::{RecycleRepository, RecycleStatus};
use crate::lightning::LnurlClient;
use crate::AppState;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

const MAX_PAYMENT_ATTEMPTS: u32 = 10;

pub async fn run_payment_processor(state: Arc<AppState>) {
    let mut interval = time::interval(Duration::from_secs(30)); // Check every 30 seconds

    loop {
        interval.tick().await;

        if let Err(e) = process_confirmed_recycles(&state).await {
            tracing::error!("Payment processor error: {}", e);
        }
    }
}

async fn process_confirmed_recycles(state: &AppState) -> anyhow::Result<()> {
    // Get all confirmed recycles ready for payout
    let confirmed = RecycleRepository::find_by_status(&state.db, RecycleStatus::Confirmed).await?;

    for recycle in confirmed {
        let deposit_amount = match recycle.deposit_amount_sats {
            Some(amount) => amount,
            None => {
                tracing::warn!("Recycle {} is confirmed but has no deposit amount", recycle.id);
                continue;
            }
        };

        // Calculate payout amount (101% or configured multiplier)
        let payout_amount = (deposit_amount as f64 * state.config.payout_multiplier) as u64;

        tracing::info!(
            "Processing payout for recycle {}: {} sats deposit -> {} sats payout",
            recycle.id,
            deposit_amount,
            payout_amount
        );

        // Get invoice from lightning address
        let lnurl_client = LnurlClient::new();
        let invoice = match lnurl_client
            .get_invoice_for_address(&recycle.lightning_address, payout_amount)
            .await
        {
            Ok(inv) => inv,
            Err(e) => {
                tracing::error!(
                    "Failed to get invoice for recycle {}: {}",
                    recycle.id,
                    e
                );
                // Don't mark as failed, will retry on next loop
                continue;
            }
        };

        tracing::debug!("Got invoice for recycle {}: {}", recycle.id, invoice);

        // Pay the invoice via NWC
        match state.nwc.pay_invoice(&invoice).await {
            Ok(result) => {
                tracing::info!(
                    "Payment successful for recycle {}: preimage={}",
                    recycle.id,
                    result.preimage
                );

                RecycleRepository::mark_paid(
                    &state.db,
                    &recycle.id,
                    payout_amount,
                    &result.preimage,
                    &result.payment_hash,
                )
                .await?;
            }
            Err(e) => {
                tracing::warn!("Payment attempt failed for recycle {}: {} (will retry)", recycle.id, e);
                // Don't mark as failed - will retry on next loop
                // The recycle stays in "confirmed" status
            }
        }
    }

    Ok(())
}
