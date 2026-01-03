use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecycleStatus {
    AwaitingDeposit,
    Confirming,
    Confirmed,
    Paid,
    Failed,
    /// Deposit received but UTXO was created after the cutoff block.
    /// No payout will be made - kept as donation.
    Donation,
}

impl RecycleStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingDeposit => "awaiting_deposit",
            Self::Confirming => "confirming",
            Self::Confirmed => "confirmed",
            Self::Paid => "paid",
            Self::Failed => "failed",
            Self::Donation => "donation",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "awaiting_deposit" => Self::AwaitingDeposit,
            "confirming" => Self::Confirming,
            "confirmed" => Self::Confirmed,
            "paid" => Self::Paid,
            "failed" => Self::Failed,
            "donation" => Self::Donation,
            _ => Self::Failed,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::AwaitingDeposit => "Awaiting Deposit",
            Self::Confirming => "Confirming",
            Self::Confirmed => "Confirmed",
            Self::Paid => "Paid",
            Self::Failed => "Failed",
            Self::Donation => "Donation Received",
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct RecycleRow {
    pub id: String,
    pub lightning_address: String,
    pub deposit_address: String,
    pub address_index: i64,
    pub status: String,
    pub deposit_txid: Option<String>,
    pub deposit_amount_sats: Option<i64>,
    pub deposit_confirmations: Option<i64>,
    pub deposit_block_height: Option<i64>,
    pub is_eligible: Option<i64>,
    pub donation_reason: Option<String>,
    pub max_input_sats: Option<i64>,
    pub payout_amount_sats: Option<i64>,
    pub payment_preimage: Option<String>,
    pub payment_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub paid_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Recycle {
    pub id: String,
    pub lightning_address: String,
    pub deposit_address: String,
    pub address_index: u32,
    pub status: RecycleStatus,
    pub deposit_txid: Option<String>,
    pub deposit_amount_sats: Option<u64>,
    pub deposit_confirmations: u32,
    /// The block height where the deposit was confirmed
    pub deposit_block_height: Option<u32>,
    /// Whether the UTXO is eligible for payout (created before cutoff block, small inputs)
    pub is_eligible: bool,
    /// Reason for donation status: "block_height" or "input_too_large"
    pub donation_reason: Option<String>,
    /// Maximum input UTXO value in the deposit transaction
    pub max_input_sats: Option<u64>,
    pub payout_amount_sats: Option<u64>,
    pub payment_preimage: Option<String>,
    pub payment_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub paid_at: Option<DateTime<Utc>>,
}

impl From<RecycleRow> for Recycle {
    fn from(row: RecycleRow) -> Self {
        Self {
            id: row.id,
            lightning_address: row.lightning_address,
            deposit_address: row.deposit_address,
            address_index: row.address_index as u32,
            status: RecycleStatus::from_str(&row.status),
            deposit_txid: row.deposit_txid,
            deposit_amount_sats: row.deposit_amount_sats.map(|v| v as u64),
            deposit_confirmations: row.deposit_confirmations.unwrap_or(0) as u32,
            deposit_block_height: row.deposit_block_height.map(|v| v as u32),
            is_eligible: row.is_eligible.unwrap_or(1) == 1,
            donation_reason: row.donation_reason,
            max_input_sats: row.max_input_sats.map(|v| v as u64),
            payout_amount_sats: row.payout_amount_sats.map(|v| v as u64),
            payment_preimage: row.payment_preimage,
            payment_hash: row.payment_hash,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: DateTime::parse_from_rfc3339(&row.updated_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            paid_at: row.paid_at.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }),
        }
    }
}

pub struct RecycleRepository;

impl RecycleRepository {
    pub async fn create(
        pool: &SqlitePool,
        id: &str,
        lightning_address: &str,
        deposit_address: &str,
        address_index: u32,
    ) -> anyhow::Result<Recycle> {
        let now = Utc::now().to_rfc3339();
        let status = RecycleStatus::AwaitingDeposit.as_str();

        sqlx::query(
            r#"
            INSERT INTO recycles (id, lightning_address, deposit_address, address_index, status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(id)
        .bind(lightning_address)
        .bind(deposit_address)
        .bind(address_index as i64)
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;

        Self::find_by_id(pool, id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to create recycle"))
    }

    pub async fn find_by_id(pool: &SqlitePool, id: &str) -> anyhow::Result<Option<Recycle>> {
        let row: Option<RecycleRow> = sqlx::query_as("SELECT * FROM recycles WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;

        Ok(row.map(Recycle::from))
    }

    pub async fn find_by_deposit_address(
        pool: &SqlitePool,
        address: &str,
    ) -> anyhow::Result<Option<Recycle>> {
        let row: Option<RecycleRow> =
            sqlx::query_as("SELECT * FROM recycles WHERE deposit_address = ?")
                .bind(address)
                .fetch_optional(pool)
                .await?;

        Ok(row.map(Recycle::from))
    }

    pub async fn find_by_status(
        pool: &SqlitePool,
        status: RecycleStatus,
    ) -> anyhow::Result<Vec<Recycle>> {
        let rows: Vec<RecycleRow> = sqlx::query_as("SELECT * FROM recycles WHERE status = ?")
            .bind(status.as_str())
            .fetch_all(pool)
            .await?;

        Ok(rows.into_iter().map(Recycle::from).collect())
    }

    pub async fn find_pending_deposits(pool: &SqlitePool) -> anyhow::Result<Vec<Recycle>> {
        let rows: Vec<RecycleRow> =
            sqlx::query_as("SELECT * FROM recycles WHERE status IN ('awaiting_deposit', 'confirming')")
                .fetch_all(pool)
                .await?;

        Ok(rows.into_iter().map(Recycle::from).collect())
    }

    pub async fn update_deposit_detected(
        pool: &SqlitePool,
        id: &str,
        txid: &str,
        amount_sats: u64,
        confirmations: u32,
        block_height: Option<u32>,
        max_input_sats: Option<u64>,
        required_confirmations: u32,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let status = if confirmations >= required_confirmations {
            RecycleStatus::Confirmed.as_str()
        } else {
            RecycleStatus::Confirming.as_str()
        };

        sqlx::query(
            r#"
            UPDATE recycles
            SET status = ?, deposit_txid = ?, deposit_amount_sats = ?, deposit_confirmations = ?,
                deposit_block_height = ?, max_input_sats = ?, is_eligible = 1, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(status)
        .bind(txid)
        .bind(amount_sats as i64)
        .bind(confirmations as i64)
        .bind(block_height.map(|h| h as i64))
        .bind(max_input_sats.map(|v| v as i64))
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Mark a deposit as a donation. No payout will be processed.
    /// reason: "block_height" (after cutoff) or "input_too_large" (input > max allowed)
    pub async fn update_as_donation(
        pool: &SqlitePool,
        id: &str,
        txid: &str,
        amount_sats: u64,
        block_height: Option<u32>,
        max_input_sats: Option<u64>,
        reason: &str,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            UPDATE recycles
            SET status = 'donation', deposit_txid = ?, deposit_amount_sats = ?,
                deposit_block_height = ?, max_input_sats = ?, is_eligible = 0,
                donation_reason = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(txid)
        .bind(amount_sats as i64)
        .bind(block_height.map(|h| h as i64))
        .bind(max_input_sats.map(|v| v as i64))
        .bind(reason)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn update_confirmations(
        pool: &SqlitePool,
        id: &str,
        confirmations: u32,
        required_confirmations: u32,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let status = if confirmations >= required_confirmations {
            RecycleStatus::Confirmed.as_str()
        } else {
            RecycleStatus::Confirming.as_str()
        };

        sqlx::query(
            r#"
            UPDATE recycles
            SET status = ?, deposit_confirmations = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(status)
        .bind(confirmations as i64)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn mark_paid(
        pool: &SqlitePool,
        id: &str,
        payout_amount_sats: u64,
        payment_preimage: &str,
        payment_hash: &str,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            UPDATE recycles
            SET status = 'paid', payout_amount_sats = ?, payment_preimage = ?, payment_hash = ?, updated_at = ?, paid_at = ?
            WHERE id = ?
            "#,
        )
        .bind(payout_amount_sats as i64)
        .bind(payment_preimage)
        .bind(payment_hash)
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn mark_failed(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            UPDATE recycles
            SET status = 'failed', updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn get_next_address_index(pool: &SqlitePool) -> anyhow::Result<u32> {
        let row: (i64,) = sqlx::query_as("SELECT next_address_index FROM wallet_state WHERE id = 1")
            .fetch_one(pool)
            .await?;

        Ok(row.0 as u32)
    }

    pub async fn increment_address_index(pool: &SqlitePool) -> anyhow::Result<u32> {
        let current = Self::get_next_address_index(pool).await?;

        sqlx::query("UPDATE wallet_state SET next_address_index = next_address_index + 1 WHERE id = 1")
            .execute(pool)
            .await?;

        Ok(current)
    }
}
