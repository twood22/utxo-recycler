# Known Issues & Future Improvements

## Economic / Business Model

- **Dust creation incentive** - ~~The bonus payment (e.g., 5%) incentivizes users to intentionally create dust UTXOs just to claim the reward. This defeats the purpose of consolidating existing dust.~~ **ADDRESSED:** Implemented block height cutoff (default: block 930,400). Only UTXOs created before this block are eligible for payout. UTXOs created at or after the cutoff are kept as donations.

- **Large UTXO abuse** - ~~Users could send small amounts from large UTXOs to claim the bonus without actually consolidating dust.~~ **ADDRESSED:** Implemented input UTXO size validation. The service verifies that ALL input UTXOs in the deposit transaction are below 1,000 sats (configurable via `MAX_INPUT_SATS`). This ensures only true dust consolidation qualifies.

- **No minimum deposit** - Users can deposit dust amounts (546 sats) that cost more to spend than they're worth. Consider enforcing a minimum of 5,000-10,000 sats.

- ~~**No maximum deposit** - Large deposits could drain Lightning liquidity.~~ **ADDRESSED:** The input size limit (1,000 sats) inherently caps deposit sizes since only dust UTXOs are accepted.

- **Lightning liquidity risk** - If on-chain deposits outpace outbound Lightning capacity, payouts will fail until rebalanced. No automated liquidity management.

- **Consolidation economics** - Collecting dust UTXOs has a cost: spending them later requires fees. At high fee rates, UTXOs under ~2,000 sats may never be economical to spend.

## Security

- ~~**No rate limiting** - The `/api/recycle` endpoint has no rate limiting. Attackers could spam address generation, bloating the database and wallet index.~~ **ADDRESSED:** Rate limiting added to `/confirm` and `/api/recycle` endpoints (default: 10 requests per 60 seconds per IP). Configurable via `RATE_LIMIT_MAX_REQUESTS` and `RATE_LIMIT_WINDOW_SECS`.

- **Lightning address staleness** - Address is validated at recycle creation, but may become invalid by payout time (6+ confirmations later).

- ~~**NWC "assume success" behavior**~~ **ADDRESSED:** NWC now returns an error when no response is received instead of assuming success. Payment processor retries up to `MAX_PAYMENT_ATTEMPTS` (10) before marking as failed.

- **NWC URI is a hot key** - The NWC connection string grants payment permissions. Server compromise = wallet drain.

## Operational

- **Single Electrum server** - No fallback if personal Electrum server goes down. Service stops working entirely.

- ~~**No monitoring/alerting** - No health checks, no alerts for failures. Must watch logs manually.~~ **ADDRESSED:** Added `/health` endpoint that returns DB status and last wallet sync time. Fly.io health checks configured in `fly.toml` to auto-restart unhealthy instances.

- ~~**No admin dashboard** - Can't view pending volume, Lightning balance, failed payments, or service stats without manual database queries.~~ **ADDRESSED:** Added `/admin/stats?token=<TOKEN>` endpoint that returns recycle counts by status, total deposited/paid/donated sats, and net sats. Protected by `ADMIN_TOKEN` env var.

- **SQLite on single volume** - No automated backups, no replication. Fly.io volume loss = data loss.

- **No graceful shutdown** - Background workers don't have clean shutdown handling.

## User Experience

- **No notifications** - Users must manually refresh status page. No email, webhook, or push notification on completion.

- **Multiple deposits ignored** - If user sends multiple UTXOs to same address, only first is processed. Subsequent deposits are effectively lost.

- **No cancellation** - Can't cancel a pending recycle once created.

- **No transaction history** - Users can't see past recycles unless they bookmarked URLs.

- **QR code format** - Shows raw address only, not BIP21 URI with amount field.

## Compliance

- **Potential money transmission** - Depending on jurisdiction, service may require money transmitter licensing.

- **No AML/KYC** - No identity verification. Could be used for money laundering at scale.

- **No Terms of Service** - No legal agreement explaining service, risks, or limitations.

- **Privacy considerations** - Lightning addresses stored in database could be considered PII.

## Technical Debt

- **No tests** - No unit or integration tests.

- **No CI/CD** - No automated testing or deployment pipeline.

- **Unused code** - Several unused functions generating compiler warnings.

- **Hardcoded values** - Retry counts, timeouts, and other values are hardcoded rather than configurable.

---

## Priority Fixes

1. ~~Add rate limiting on `/api/recycle`~~ ✅ Done
2. ~~Add health check endpoint (`/health`)~~ ✅ Done
3. ~~Add basic monitoring/alerting~~ ✅ Done (health endpoint + admin stats)
4. ~~Fix NWC "assume success"~~ ✅ Done (returns error, payment processor retries with limit)
5. ~~Configure Fly.io health checks~~ ✅ Done
6. Implement database backups (Litestream)
7. Add Terms of Service page
