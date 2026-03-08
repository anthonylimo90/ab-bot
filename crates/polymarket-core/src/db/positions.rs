//! Database operations for positions.

use crate::types::{
    ArbOpportunity, ExitStrategy, FailureReason, Position, PositionFeeModel, PositionState,
    PositionStats,
};
use crate::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Repository for position data.
pub struct PositionRepository {
    pool: PgPool,
}

/// Active exit-on-correction position plus its persisted source.
pub struct ExitCandidate {
    pub position: Position,
    pub source: i16,
}

const SOURCE_COPY_TRADE: i16 = 2;
const SOURCE_RECOMMENDATION: i16 = 3;

impl PositionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new position.
    pub async fn insert(&self, position: &Position) -> Result<()> {
        let failure_reason_json = position
            .failure_reason
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap_or_default());

        sqlx::query(
            r#"
            INSERT INTO positions (
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares,
                is_open
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17
            )
            "#,
        )
        .bind(position.id)
        .bind(&position.market_id)
        .bind(position.yes_entry_price)
        .bind(position.no_entry_price)
        .bind(position.quantity)
        .bind(position.entry_timestamp)
        .bind(position.exit_strategy as i16)
        .bind(position.state as i16)
        .bind(position.unrealized_pnl)
        .bind(failure_reason_json)
        .bind(position.retry_count as i32)
        .bind(position.last_updated)
        .bind(position.fee_model.as_i16())
        .bind(position.resolution_payout_per_share)
        .bind(position.yes_entry_fee_shares)
        .bind(position.no_entry_fee_shares)
        .bind(!matches!(
            position.state,
            PositionState::Closed | PositionState::EntryFailed
        ))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update position state and P&L.
    pub async fn update(&self, position: &Position) -> Result<()> {
        let failure_reason_json = position
            .failure_reason
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap_or_default());

        sqlx::query(
            r#"
            UPDATE positions SET
                state = $2,
                unrealized_pnl = $3,
                realized_pnl = $4,
                exit_timestamp = $5,
                yes_exit_price = $6,
                no_exit_price = $7,
                failure_reason = $8,
                retry_count = $9,
                last_updated = $10,
                fee_model = $11,
                resolution_payout_per_share = $12,
                yes_entry_fee_shares = $13,
                no_entry_fee_shares = $14,
                is_open = $15
            WHERE id = $1
            "#,
        )
        .bind(position.id)
        .bind(position.state as i16)
        .bind(position.unrealized_pnl)
        .bind(position.realized_pnl)
        .bind(position.exit_timestamp)
        .bind(position.yes_exit_price)
        .bind(position.no_exit_price)
        .bind(failure_reason_json)
        .bind(position.retry_count as i32)
        .bind(position.last_updated)
        .bind(position.fee_model.as_i16())
        .bind(position.resolution_payout_per_share)
        .bind(position.yes_entry_fee_shares)
        .bind(position.no_entry_fee_shares)
        .bind(!matches!(
            position.state,
            PositionState::Closed | PositionState::EntryFailed
        ))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get a position by ID.
    pub async fn get(&self, id: Uuid) -> Result<Option<Position>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Self::row_to_position(&r)))
    }

    /// Convert a database row to a Position.
    fn row_to_position(r: &sqlx::postgres::PgRow) -> Position {
        let failure_reason: Option<FailureReason> = r
            .get::<Option<String>, _>("failure_reason")
            .and_then(|s| serde_json::from_str(&s).ok());

        let last_updated = r
            .get::<Option<chrono::DateTime<Utc>>, _>("last_updated")
            .unwrap_or_else(|| r.get("entry_timestamp"));

        let yes_entry_price: Decimal = r.get("yes_entry_price");
        let no_entry_price: Decimal = r.get("no_entry_price");
        let legacy_resolution_payout = (Decimal::ONE
            - ((yes_entry_price + no_entry_price) * ArbOpportunity::DEFAULT_FEE))
            .max(Decimal::ZERO);

        Position {
            id: r.get("id"),
            market_id: r.get("market_id"),
            yes_entry_price,
            no_entry_price,
            quantity: r.get("quantity"),
            entry_timestamp: r.get("entry_timestamp"),
            exit_strategy: match r.get::<i16, _>("exit_strategy") {
                0 => ExitStrategy::HoldToResolution,
                _ => ExitStrategy::ExitOnCorrection,
            },
            state: match r.get::<i16, _>("state") {
                0 => PositionState::Pending,
                1 => PositionState::Open,
                2 => PositionState::ExitReady,
                3 => PositionState::Closing,
                4 => PositionState::Closed,
                5 => PositionState::EntryFailed,
                6 => PositionState::ExitFailed,
                7 => PositionState::Stalled,
                _ => PositionState::Closed,
            },
            unrealized_pnl: r.get("unrealized_pnl"),
            realized_pnl: r.get("realized_pnl"),
            exit_timestamp: r.get("exit_timestamp"),
            yes_exit_price: r.get("yes_exit_price"),
            no_exit_price: r.get("no_exit_price"),
            failure_reason,
            retry_count: r.get::<Option<i32>, _>("retry_count").unwrap_or(0) as u32,
            last_updated,
            pre_stall_state: None, // Runtime-only; not persisted to DB
            fee_model: PositionFeeModel::from_i16(
                r.get::<Option<i16>, _>("fee_model").unwrap_or(0),
            ),
            resolution_payout_per_share: r
                .get::<Option<Decimal>, _>("resolution_payout_per_share")
                .unwrap_or(legacy_resolution_payout),
            yes_entry_fee_shares: r
                .get::<Option<Decimal>, _>("yes_entry_fee_shares")
                .unwrap_or(Decimal::ZERO),
            no_entry_fee_shares: r
                .get::<Option<Decimal>, _>("no_entry_fee_shares")
                .unwrap_or(Decimal::ZERO),
        }
    }

    /// Get all active (non-closed, non-entry-failed) positions.
    /// This includes positions needing recovery (ExitFailed, Stalled).
    pub async fn get_active(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state NOT IN (4, 5)
            ORDER BY entry_timestamp DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get active positions managed by the arb monitor.
    ///
    /// Excludes recommendation and legacy copy-trade rows because their
    /// lifecycle is owned elsewhere and the arb monitor's stale watchdog
    /// should not mutate them.
    pub async fn get_active_for_arb_monitor(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state NOT IN (4, 5)
              AND COALESCE(source, 0) NOT IN ($1, $2)
            ORDER BY entry_timestamp DESC
            "#,
        )
        .bind(SOURCE_COPY_TRADE)
        .bind(SOURCE_RECOMMENDATION)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get positions that need recovery (ExitFailed or Stalled).
    pub async fn get_needing_recovery(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state IN (6, 7)
            ORDER BY last_updated ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get arb-monitor-managed positions that need recovery.
    pub async fn get_needing_recovery_for_arb_monitor(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state IN (6, 7)
              AND COALESCE(source, 0) NOT IN ($1, $2)
            ORDER BY last_updated ASC
            "#,
        )
        .bind(SOURCE_COPY_TRADE)
        .bind(SOURCE_RECOMMENDATION)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get positions in ExitReady state (for ExitOnCorrection exits).
    pub async fn get_exit_ready(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state = 2
            ORDER BY last_updated ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get HoldToResolution positions that are Open or ExitReady.
    pub async fn get_hold_to_resolution(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE exit_strategy = 0 AND state IN (1, 2)
            ORDER BY entry_timestamp ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get open ExitOnCorrection positions that still need exit evaluation.
    pub async fn get_open_exit_candidates(&self) -> Result<Vec<ExitCandidate>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares,
                source
            FROM positions
            WHERE exit_strategy = 1 AND state = 1
            ORDER BY entry_timestamp ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|row| ExitCandidate {
                position: Self::row_to_position(row),
                source: row.get::<Option<i16>, _>("source").unwrap_or(0),
            })
            .collect())
    }

    /// Get ExitFailed positions eligible for retry (retry_count < 3).
    pub async fn get_failed_exits(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state = 6 AND retry_count < 3
            ORDER BY last_updated ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get one-legged entry failures eligible for recovery (retry_count < 3).
    /// These are positions where YES filled but NO failed, identifiable by
    /// failure_reason containing "one-legged" or "one_legged".
    pub async fn get_one_legged_entry_failed(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price,
                failure_reason, retry_count, last_updated, fee_model,
                resolution_payout_per_share, yes_entry_fee_shares, no_entry_fee_shares
            FROM positions
            WHERE state = 5
              AND retry_count < 3
              AND (failure_reason ILIKE '%one-legged%' OR failure_reason ILIKE '%one_legged%')
            ORDER BY last_updated ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(Self::row_to_position).collect())
    }

    /// Get position statistics.
    pub async fn get_stats(&self) -> Result<PositionStats> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) as total,
                COUNT(*) FILTER (WHERE state < 4) as open,
                COUNT(*) FILTER (WHERE state = 4) as closed,
                COALESCE(SUM(realized_pnl), 0) as total_realized,
                COALESCE(SUM(unrealized_pnl) FILTER (WHERE state < 4), 0) as total_unrealized,
                COUNT(*) FILTER (WHERE realized_pnl > 0) as wins,
                COUNT(*) FILTER (WHERE realized_pnl <= 0 AND state = 4) as losses
            FROM positions
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(PositionStats {
            total_positions: row.get::<Option<i64>, _>("total").unwrap_or(0) as u64,
            open_positions: row.get::<Option<i64>, _>("open").unwrap_or(0) as u64,
            closed_positions: row.get::<Option<i64>, _>("closed").unwrap_or(0) as u64,
            total_realized_pnl: row
                .get::<Option<Decimal>, _>("total_realized")
                .unwrap_or_default(),
            total_unrealized_pnl: row
                .get::<Option<Decimal>, _>("total_unrealized")
                .unwrap_or_default(),
            win_count: row.get::<Option<i64>, _>("wins").unwrap_or(0) as u64,
            loss_count: row.get::<Option<i64>, _>("losses").unwrap_or(0) as u64,
        })
    }

    /// Returns true when a source already has an active position in the market.
    pub async fn active_position_exists_for_market_source(
        &self,
        market_id: &str,
        source: i16,
    ) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM positions
                WHERE market_id = $1
                  AND source = $2
                  AND state NOT IN (4, 5)
            )
            "#,
        )
        .bind(market_id)
        .bind(source)
        .fetch_one(&self.pool)
        .await?;

        Ok(exists)
    }

    /// Count active positions for a single source.
    pub async fn count_active_by_source(&self, source: i16) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM positions
            WHERE source = $1
              AND state NOT IN (4, 5)
            "#,
        )
        .bind(source)
        .fetch_one(&self.pool)
        .await?;

        Ok(count)
    }
}
