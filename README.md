# UTXO Recycler

A Bitcoin dust recycling service that accepts small on-chain deposits and pays back 101% via Lightning Network.

## What It Does

1. User provides their Lightning address (e.g., `user@getalby.com`)
2. Service generates a unique Bitcoin deposit address
3. User sends on-chain Bitcoin (dust UTXOs they want to consolidate)
4. After 6 confirmations, service pays 101% of the deposit amount to their Lightning address via LNURL-pay
5. User receives instant Lightning sats, service consolidates the on-chain UTXOs

## Architecture

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│   Web Frontend  │────▶│   Axum Server    │────▶│   SQLite DB     │
│   (Askama HTML) │     │                  │     │                 │
└─────────────────┘     └──────────────────┘     └─────────────────┘
                               │
                               ▼
        ┌──────────────────────┴──────────────────────┐
        │                                             │
        ▼                                             ▼
┌───────────────────┐                    ┌────────────────────┐
│  Deposit Monitor  │                    │ Payment Processor  │
│  (BDK + Electrum) │                    │   (NWC + LNURL)    │
└───────────────────┘                    └────────────────────┘
```

**Components:**
- **BDK (Bitcoin Dev Kit)**: HD wallet for generating deposit addresses and monitoring the blockchain via Electrum
- **NWC (Nostr Wallet Connect)**: Pays Lightning invoices through your connected wallet (e.g., Alby Hub)
- **LNURL-pay**: Resolves Lightning addresses to BOLT11 invoices
- **Askama**: Server-rendered HTML templates

## Prerequisites

- Rust 1.75+
- A BDK-compatible wallet descriptor (e.g., from Sparrow, BlueWallet, or generated)
- An NWC-compatible Lightning wallet (e.g., [Alby Hub](https://albyhub.com))

## Configuration

Copy `.env.example` to `.env` and configure:

```bash
cp .env.example .env
```

| Variable | Required | Description |
|----------|----------|-------------|
| `NWC_URI` | Yes | Nostr Wallet Connect URI from your Lightning wallet |
| `WALLET_DESCRIPTOR` | Yes | BDK wallet descriptor for deposit addresses |
| `DATABASE_URL` | No | SQLite path (default: `sqlite:utxo_recycler.db?mode=rwc`) |
| `ELECTRUM_URL` | No | Electrum server (default: `ssl://electrum.blockstream.info:50002`) |
| `TOR_PROXY` | No | SOCKS5 proxy for Tor (e.g., `127.0.0.1:9050`) |
| `PAYOUT_MULTIPLIER` | No | Payout ratio (default: `1.01` for 101%) |
| `REQUIRED_CONFIRMATIONS` | No | Confirmations before payout (default: `6`) |
| `SERVER_HOST` | No | Bind address (default: `0.0.0.0`) |
| `SERVER_PORT` | No | Port (default: `3000`) |

### Getting an NWC URI

1. Install [Alby Hub](https://albyhub.com) or use another NWC-compatible wallet
2. Create a new app connection with `pay_invoice` permission
3. Copy the connection string (starts with `nostr+walletconnect://`)

### Getting a Wallet Descriptor

Generate a descriptor using a wallet like Sparrow, or create one manually:

```
wpkh([fingerprint/84'/0'/0']xpub.../0/*)
```

The service derives fresh addresses from this descriptor for each recycle request.

### Using Your Own Electrum Server (with Tor)

For maximum privacy, you can run your own Electrum server and connect via Tor:

```bash
# In your .env file:
ELECTRUM_URL=tcp://your-server.onion:50001
TOR_PROXY=127.0.0.1:9050
```

Make sure Tor is running locally (it listens on port 9050 by default).

## Local Development

```bash
# Install dependencies and run
cargo run

# Or with specific log level
RUST_LOG=debug cargo run
```

Visit `http://localhost:3000` to use the service.

## Deployment (Fly.io)

### Initial Setup

```bash
# Install Fly CLI
brew install flyctl  # or curl -L https://fly.io/install.sh | sh

# Login
fly auth login

# Initialize app (don't deploy yet)
fly launch --no-deploy

# Create persistent volume for SQLite database
fly volumes create data --size 1 --region sjc

# Set secrets (never commit these!)
fly secrets set NWC_URI="nostr+walletconnect://..."
fly secrets set WALLET_DESCRIPTOR="wpkh([...])"

# Deploy
fly deploy
```

### Subsequent Deployments

```bash
fly deploy
```

### Useful Commands

```bash
fly logs              # View logs
fly status            # Check app status
fly ssh console       # SSH into the machine
fly secrets list      # List configured secrets
```

### Configuration

The `fly.toml` is pre-configured with:
- Persistent volume at `/data` for SQLite
- `auto_stop_machines = false` to keep background workers running
- Sensible defaults for non-sensitive environment variables

## Database

The SQLite database stores recycle records with the following states:

| Status | Description |
|--------|-------------|
| `awaiting_deposit` | Waiting for on-chain deposit |
| `confirming` | Deposit detected, waiting for confirmations |
| `confirmed` | Ready for Lightning payout |
| `paid` | Successfully paid via Lightning |
| `failed` | Payment failed (will retry) |

### Manual Database Access

```bash
# Local
sqlite3 utxo_recycler.db

# On Fly.io
fly ssh console
sqlite3 /data/utxo_recycler.db
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Landing page with form |
| `POST` | `/api/recycle` | Create new recycle request |
| `GET` | `/recycle/:id` | Status page (HTML) |
| `GET` | `/api/recycle/:id` | Status (JSON) |

## How It Works

1. **Create Recycle**: User submits Lightning address → service validates via LNURL, generates deposit address from HD wallet, stores in DB

2. **Deposit Monitor** (runs every 30s): Syncs wallet with Electrum server, checks for deposits to pending addresses, updates confirmation counts

3. **Payment Processor** (runs every 30s): For confirmed deposits, fetches BOLT11 invoice via LNURL-pay, pays via NWC, stores preimage as proof

## Security Considerations

- Never commit `.env` or expose your `NWC_URI` / `WALLET_DESCRIPTOR`
- The NWC URI grants payment permissions - treat it like a private key
- The wallet descriptor can derive all your deposit addresses
- Run behind HTTPS in production (Fly.io handles this automatically)

## License

MIT
