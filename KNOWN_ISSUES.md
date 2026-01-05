# Known Issues & Future Improvements

## Critical - Funds at Risk

- ~~**Multiple deposits lost**~~ **ADDRESSED:** Multiple deposits to the same address are now summed together. The system collects all matching outputs across all transactions, uses the minimum confirmation count (waits for all to confirm), and checks eligibility criteria against all contributing transactions.

- **NWC "assume success" behavior** - If no NWC response is received after 5 attempts, the payment is assumed successful and a fake preimage (`unconfirmed-{event_id}`) is stored. If the payment actually failed, the user loses funds and there's no proof of payment. See `nwc.rs:154-161`.

- **No payment retry limit** - The `MAX_PAYMENT_ATTEMPTS` constant exists but is never used. Failed payments retry forever every 30 seconds, risking double-payments if NWC is intermittently failing.

## Economic / Business Model

- ~~**Dust creation incentive**~~ **ADDRESSED:** Implemented block height cutoff (default: block 930,400). Only UTXOs created before this block are eligible for payout.

- ~~**Large UTXO abuse**~~ **ADDRESSED:** Implemented input UTXO size validation. ALL input UTXOs must be below 1,000 sats (configurable via `MAX_INPUT_SATS`).

- **No minimum deposit** - Users can deposit dust amounts (546 sats) that cost more to process than they're worth. Consider enforcing a minimum of 5,000-10,000 sats.

- ~~**No maximum deposit**~~ **ADDRESSED:** The input size limit inherently caps deposit sizes since only dust UTXOs are accepted.

- **Lightning liquidity risk** - If on-chain deposits outpace outbound Lightning capacity, payouts will fail until rebalanced. No automated liquidity management.

- **Consolidation economics** - Collecting dust UTXOs has a cost: spending them later requires fees. At high fee rates, UTXOs under ~2,000 sats may never be economical to spend.

## Security

- ~~**No rate limiting**~~ **ADDRESSED:** Rate limiting added to `/confirm` and `/api/recycle` endpoints (default: 10 requests per 60 seconds per IP). Configurable via `RATE_LIMIT_MAX_REQUESTS` and `RATE_LIMIT_WINDOW_SECS`.

- **Lightning address staleness** - Address is validated at recycle creation, but may become invalid by payout time (6+ confirmations later). Consider re-validating before payment.

- **NWC URI is a hot key** - The NWC connection string grants payment permissions. Server compromise = wallet drain. Consider using a separate wallet with limited funds.

## Operational

- **Single Electrum server** - No fallback if the Electrum server goes down. Service stops working entirely. Consider adding multiple servers with failover.

- ~~**No monitoring/alerting**~~ **PARTIALLY ADDRESSED:** Added `/health` endpoint that returns DB status and last wallet sync time. Note: Fly.io health checks not yet configured in `fly.toml`.

- ~~**No admin dashboard**~~ **ADDRESSED:** Added `/admin/stats?token=<TOKEN>` endpoint with recycle counts, totals, and net sats.

- **SQLite on single volume** - No automated backups, no replication. Fly.io volume loss = data loss. Consider Litestream to S3.

- **No graceful shutdown** - Background workers don't handle SIGTERM. Could interrupt mid-payment.

- **Config inconsistency** - `fly.toml` has `PAYOUT_MULTIPLIER=1.01` but local `.env` may differ. Ensure production config is correct.

## User Experience

- **No notifications** - Users must manually refresh status page. No email, webhook, or push notification on completion.

- **No cancellation** - Can't cancel a pending recycle once created. Address is generated and waiting.

- **No transaction history** - Users can't see past recycles unless they bookmarked URLs.

- **QR code format** - Shows raw address only, not BIP21 URI with amount field.

## Compliance

- **No Terms of Service** - No legal agreement explaining service, risks, or limitations. Should add a ToS page.

- **Privacy considerations** - Lightning addresses stored in database could be considered PII in some jurisdictions.

## Technical Debt

- **No tests** - No unit or integration tests. High risk of regressions.

- **No CI/CD** - No automated testing or deployment pipeline.

- **Unused code** - 8 compiler warnings for unused functions/constants. Run `cargo fix` or remove dead code.

- **Hardcoded values** - Retry counts, timeouts, polling intervals are hardcoded rather than configurable.

---

## Priority Fixes

1. **Fix NWC payment verification** - Query payment status or track balance instead of assuming success
2. **Add payment retry limit** - Use `MAX_PAYMENT_ATTEMPTS` and mark as failed after limit
3. Add minimum deposit limit
4. Configure Fly.io health checks
5. Implement database backups (Litestream)
6. Add Terms of Service page
