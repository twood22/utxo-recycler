-- Initial schema for UTXO Recycler

CREATE TABLE IF NOT EXISTS recycles (
    id TEXT PRIMARY KEY,
    lightning_address TEXT NOT NULL,
    deposit_address TEXT NOT NULL UNIQUE,
    address_index INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'awaiting_deposit',
    deposit_txid TEXT,
    deposit_amount_sats INTEGER,
    deposit_confirmations INTEGER DEFAULT 0,
    payout_amount_sats INTEGER,
    payment_preimage TEXT,
    payment_hash TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    paid_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_recycles_status ON recycles(status);
CREATE INDEX IF NOT EXISTS idx_recycles_deposit_address ON recycles(deposit_address);

-- Track the next address index for the HD wallet
CREATE TABLE IF NOT EXISTS wallet_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    next_address_index INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO wallet_state (id, next_address_index) VALUES (1, 0);
