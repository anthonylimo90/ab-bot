//! Database operations for reconciled wallet inventory.

use crate::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool, QueryBuilder};
use uuid::Uuid;

#[cfg(test)]
#[path = "inventory/tests.rs"]
mod tests;

/// Repository for reconciled wallet inventory rows.
pub struct WalletInventoryRepository {
    pool: PgPool,
}

#[derive(Debug, Clone, FromRow)]
pub struct WalletInventoryEntry {
    pub id: Uuid,
    pub wallet_address: String,
    pub token_id: String,
    pub condition_id: Option<String>,
    pub outcome: Option<String>,
    pub linked_position_id: Option<Uuid>,
    pub quantity: Decimal,
    pub cost_basis: Option<Decimal>,
    pub current_price: Option<Decimal>,
    pub marked_value: Option<Decimal>,
    pub is_orphan: bool,
    pub discovery_source: String,
    pub recovery_status: String,
    pub last_exit_error: Option<String>,
    pub last_exit_attempted_at: Option<DateTime<Utc>>,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct WalletInventoryUpsert {
    pub wallet_address: String,
    pub token_id: String,
    pub condition_id: Option<String>,
    pub outcome: Option<String>,
    pub linked_position_id: Option<Uuid>,
    pub quantity: Decimal,
    pub cost_basis: Option<Decimal>,
    pub current_price: Option<Decimal>,
    pub marked_value: Option<Decimal>,
    pub is_orphan: bool,
    pub discovery_source: String,
    pub recovery_status: String,
    pub last_exit_error: Option<String>,
    pub last_exit_attempted_at: Option<DateTime<Utc>>,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct WalletInventorySummary {
    pub open_positions: i64,
    pub open_markets: i64,
    pub position_value: Decimal,
    pub unrealized_pnl: Decimal,
    pub unpriced_open_positions: i64,
    pub unpriced_position_cost_basis: Decimal,
    pub orphan_positions: i64,
    pub orphan_marked_value: Decimal,
    pub inventory_last_synced_at: Option<DateTime<Utc>>,
    pub inventory_last_scanned_block: Option<i64>,
    pub inventory_backfill_cursor_block: Option<i64>,
    pub inventory_backfill_completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct WalletInventorySyncState {
    pub wallet_address: String,
    pub last_scanned_block: i64,
    pub backfill_cursor_block: Option<i64>,
    pub backfill_completed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl WalletInventoryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_wallet_entries(
        &self,
        wallet_address: &str,
    ) -> Result<Vec<WalletInventoryEntry>> {
        let rows = sqlx::query_as::<_, WalletInventoryEntry>(
            r#"
            SELECT
                id,
                wallet_address,
                token_id,
                condition_id,
                outcome,
                linked_position_id,
                quantity,
                cost_basis,
                current_price,
                marked_value,
                is_orphan,
                discovery_source,
                recovery_status,
                last_exit_error,
                last_exit_attempted_at,
                first_observed_at,
                last_observed_at,
                updated_at
            FROM wallet_inventory
            WHERE LOWER(wallet_address) = LOWER($1)
            ORDER BY quantity DESC, last_observed_at DESC, token_id ASC
            "#,
        )
        .bind(wallet_address)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn list_wallet_token_ids(&self, wallet_address: &str) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            r#"
            SELECT token_id
            FROM wallet_inventory
            WHERE LOWER(wallet_address) = LOWER($1)
            ORDER BY token_id ASC
            "#,
        )
        .bind(wallet_address)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn get_recoverable_orphans(
        &self,
        wallet_address: &str,
        retry_backoff_secs: u64,
    ) -> Result<Vec<WalletInventoryEntry>> {
        let rows = sqlx::query_as::<_, WalletInventoryEntry>(
            r#"
            SELECT
                id,
                wallet_address,
                token_id,
                condition_id,
                outcome,
                linked_position_id,
                quantity,
                cost_basis,
                current_price,
                marked_value,
                is_orphan,
                discovery_source,
                recovery_status,
                last_exit_error,
                last_exit_attempted_at,
                first_observed_at,
                last_observed_at,
                updated_at
            FROM wallet_inventory
            WHERE LOWER(wallet_address) = LOWER($1)
              -- Balances under one sellable lot (0.01 shares) cannot be sold.
              AND quantity >= 0.01
              AND is_orphan = TRUE
              -- Owners are matched by market and outcome: a token whose
              -- market or outcome is unknown cannot be told apart from one a
              -- position or manual order holds, so it is never sold here.
              AND condition_id IS NOT NULL
              AND outcome IN ('yes', 'no')
              -- Manual inventory awaiting wallet verification may be this
              -- wallet's too.
              AND NOT EXISTS (SELECT 1 FROM manual_inventory mi
                  WHERE mi.execution_mode IN ('live','unknown')
                    AND mi.wallet_address IN (LOWER(wallet_inventory.wallet_address),'unknown')
                    AND mi.market_id=wallet_inventory.condition_id AND mi.outcome=wallet_inventory.outcome AND mi.quantity>0)
              AND NOT EXISTS (SELECT 1 FROM orders o
                  WHERE o.execution_mode='live' AND o.status IN ('pending','open','partially_filled')
                    AND (LOWER(o.wallet_address)=LOWER(wallet_inventory.wallet_address) OR o.wallet_address IS NULL)
                    AND o.market_id=wallet_inventory.condition_id AND o.outcome=wallet_inventory.outcome)
              AND NOT EXISTS (SELECT 1 FROM positions p WHERE p.market_id=wallet_inventory.condition_id
                  AND p.execution_mode<>'paper' AND (LOWER(p.wallet_address)=LOWER(wallet_inventory.wallet_address) OR p.execution_mode='unknown')
                  AND (p.state=0 OR (wallet_inventory.outcome='yes' AND p.held_yes_qty>0) OR (wallet_inventory.outcome='no' AND p.held_no_qty>0)))
              AND (
                    last_exit_attempted_at IS NULL
                    OR last_exit_attempted_at <= NOW() - ($2 * INTERVAL '1 second')
                  )
            ORDER BY last_observed_at ASC, token_id ASC
            "#,
        )
        .bind(wallet_address)
        .bind(retry_backoff_secs as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn summarize_wallet(&self, wallet_address: &str) -> Result<WalletInventorySummary> {
        let row = sqlx::query_as::<_, WalletInventorySummary>(
            r#"
            SELECT
                COUNT(DISTINCT COALESCE(linked_position_id::text, token_id)) FILTER (WHERE quantity > 0)::bigint AS open_positions,
                COUNT(DISTINCT condition_id) FILTER (WHERE quantity > 0 AND condition_id IS NOT NULL)::bigint AS open_markets,
                COALESCE(SUM(COALESCE(marked_value, cost_basis, 0)) FILTER (WHERE quantity > 0), 0) AS position_value,
                COALESCE(SUM(
                    CASE
                        WHEN quantity > 0 AND marked_value IS NOT NULL AND cost_basis IS NOT NULL
                        THEN marked_value - cost_basis
                        ELSE 0
                    END
                ), 0) AS unrealized_pnl,
                COUNT(*) FILTER (WHERE quantity > 0 AND current_price IS NULL)::bigint AS unpriced_open_positions,
                COALESCE(SUM(COALESCE(cost_basis, 0)) FILTER (WHERE quantity > 0 AND current_price IS NULL), 0) AS unpriced_position_cost_basis,
                COUNT(*) FILTER (WHERE quantity > 0 AND is_orphan = TRUE)::bigint AS orphan_positions,
                COALESCE(SUM(COALESCE(marked_value, cost_basis, 0)) FILTER (WHERE quantity > 0 AND is_orphan = TRUE), 0) AS orphan_marked_value,
                MAX(last_observed_at) AS inventory_last_synced_at,
                (
                    SELECT last_scanned_block
                    FROM wallet_inventory_sync_state sync
                    WHERE LOWER(sync.wallet_address) = LOWER($1)
                ) AS inventory_last_scanned_block,
                (
                    SELECT backfill_cursor_block
                    FROM wallet_inventory_sync_state sync
                    WHERE LOWER(sync.wallet_address) = LOWER($1)
                ) AS inventory_backfill_cursor_block,
                (
                    SELECT backfill_completed_at
                    FROM wallet_inventory_sync_state sync
                    WHERE LOWER(sync.wallet_address) = LOWER($1)
                ) AS inventory_backfill_completed_at
            FROM wallet_inventory
            WHERE LOWER(wallet_address) = LOWER($1)
            "#,
        )
        .bind(wallet_address)
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn upsert_entries(&self, entries: &[WalletInventoryUpsert]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        // Batch all rows into a single INSERT ... ON CONFLICT statement.
        // PostgreSQL parameter limit is 65535; each row uses 16 params → max ~4096
        // rows per batch. A typical wallet has 5–50 tokens so one batch suffices.
        const BATCH_SIZE: usize = 500;
        let mut tx = self.pool.begin().await?;
        for chunk in entries.chunks(BATCH_SIZE) {
            let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
                "INSERT INTO wallet_inventory (\
                    wallet_address, token_id, condition_id, outcome, \
                    linked_position_id, quantity, cost_basis, current_price, \
                    marked_value, is_orphan, discovery_source, recovery_status, \
                    last_exit_error, last_exit_attempted_at, \
                    first_observed_at, last_observed_at\
                ) ",
            );

            qb.push_values(chunk, |mut b, e| {
                b.push_bind(&e.wallet_address)
                    .push_bind(&e.token_id)
                    .push_bind(&e.condition_id)
                    .push_bind(&e.outcome)
                    .push_bind(e.linked_position_id)
                    .push_bind(e.quantity)
                    .push_bind(e.cost_basis)
                    .push_bind(e.current_price)
                    .push_bind(e.marked_value)
                    .push_bind(e.is_orphan)
                    .push_bind(&e.discovery_source)
                    .push_bind(&e.recovery_status)
                    .push_bind(&e.last_exit_error)
                    .push_bind(e.last_exit_attempted_at)
                    .push_bind(e.first_observed_at)
                    .push_bind(e.last_observed_at);
            });

            qb.push(
                " ON CONFLICT (wallet_address, token_id) DO UPDATE SET \
                    condition_id           = EXCLUDED.condition_id, \
                    outcome                = EXCLUDED.outcome, \
                    linked_position_id     = EXCLUDED.linked_position_id, \
                    quantity               = EXCLUDED.quantity, \
                    cost_basis             = EXCLUDED.cost_basis, \
                    current_price          = EXCLUDED.current_price, \
                    marked_value           = EXCLUDED.marked_value, \
                    is_orphan              = EXCLUDED.is_orphan, \
                    discovery_source       = EXCLUDED.discovery_source, \
                    recovery_status        = EXCLUDED.recovery_status, \
                    last_exit_error        = EXCLUDED.last_exit_error, \
                    last_exit_attempted_at = EXCLUDED.last_exit_attempted_at, \
                    first_observed_at      = LEAST(wallet_inventory.first_observed_at, EXCLUDED.first_observed_at), \
                    last_observed_at       = EXCLUDED.last_observed_at, \
                    updated_at             = NOW()",
            );

            qb.build().execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn mark_missing_tokens_zero(
        &self,
        wallet_address: &str,
        retained_token_ids: &[String],
        observed_at: DateTime<Utc>,
    ) -> Result<()> {
        if retained_token_ids.is_empty() {
            return Ok(());
        }

        sqlx::query(
            r#"
            UPDATE wallet_inventory
            SET
                quantity = 0,
                current_price = NULL,
                marked_value = NULL,
                is_orphan = FALSE,
                recovery_status = CASE
                    WHEN quantity > 0 THEN 'recovered'
                    ELSE recovery_status
                END,
                last_observed_at = $2,
                updated_at = $2
            WHERE LOWER(wallet_address) = LOWER($1)
              AND NOT (token_id = ANY($3))
            "#,
        )
        .bind(wallet_address)
        .bind(observed_at)
        .bind(retained_token_ids)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Take `sold` shares out of a row: its quantity, and the same share of
    /// its cost basis, so a later sale of the rest charges only what is left.
    pub async fn record_disposal(
        &self,
        wallet_address: &str,
        token_id: &str,
        sold: Decimal,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE wallet_inventory
            SET
                cost_basis = CASE
                    WHEN cost_basis IS NULL OR quantity <= 0 THEN cost_basis
                    ELSE cost_basis * GREATEST(quantity - $3, 0) / quantity
                END,
                quantity = GREATEST(quantity - $3, 0),
                updated_at = NOW()
            WHERE LOWER(wallet_address) = LOWER($1)
              AND token_id = $2
            "#,
        )
        .bind(wallet_address)
        .bind(token_id)
        .bind(sold)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_recovery_attempt(
        &self,
        wallet_address: &str,
        token_id: &str,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE wallet_inventory
            SET
                last_exit_attempted_at = NOW(),
                last_exit_error = $3,
                recovery_status = CASE
                    WHEN $3 IS NULL THEN recovery_status
                    ELSE 'sell_failed'
                END,
                updated_at = NOW()
            WHERE LOWER(wallet_address) = LOWER($1)
              AND token_id = $2
            "#,
        )
        .bind(wallet_address)
        .bind(token_id)
        .bind(error)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_sync_state(
        &self,
        wallet_address: &str,
    ) -> Result<Option<WalletInventorySyncState>> {
        let row = sqlx::query_as::<_, WalletInventorySyncState>(
            r#"
            SELECT
                wallet_address,
                last_scanned_block,
                backfill_cursor_block,
                backfill_completed_at,
                updated_at
            FROM wallet_inventory_sync_state
            WHERE LOWER(wallet_address) = LOWER($1)
            "#,
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row)
    }

    pub async fn upsert_sync_state(
        &self,
        wallet_address: &str,
        last_scanned_block: u64,
        backfill_cursor_block: Option<u64>,
        backfill_completed_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO wallet_inventory_sync_state (
                wallet_address,
                last_scanned_block,
                backfill_cursor_block,
                backfill_completed_at
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (wallet_address) DO UPDATE SET
                last_scanned_block = EXCLUDED.last_scanned_block,
                backfill_cursor_block = EXCLUDED.backfill_cursor_block,
                backfill_completed_at = EXCLUDED.backfill_completed_at,
                updated_at = NOW()
            "#,
        )
        .bind(wallet_address)
        .bind(last_scanned_block as i64)
        .bind(backfill_cursor_block.map(|value| value as i64))
        .bind(backfill_completed_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
