//! Database operations for positions.

use crate::types::{ExitStrategy, Position, PositionState, PositionStats};
use crate::Result;
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool, Row};
use uuid::Uuid;

/// Repository for position data.
pub struct PositionRepository {
    pool: PgPool,
}

impl PositionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new position.
    pub async fn insert(&self, position: &Position) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO positions (
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update position state and P&L.
    pub async fn update(&self, position: &Position) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE positions SET
                state = $2,
                unrealized_pnl = $3,
                realized_pnl = $4,
                exit_timestamp = $5,
                yes_exit_price = $6,
                no_exit_price = $7
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
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price
            FROM positions
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| Position {
            id: r.get("id"),
            market_id: r.get("market_id"),
            yes_entry_price: r.get("yes_entry_price"),
            no_entry_price: r.get("no_entry_price"),
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
                _ => PositionState::Closed,
            },
            unrealized_pnl: r.get("unrealized_pnl"),
            realized_pnl: r.get("realized_pnl"),
            exit_timestamp: r.get("exit_timestamp"),
            yes_exit_price: r.get("yes_exit_price"),
            no_exit_price: r.get("no_exit_price"),
        }))
    }

    /// Get all active (non-closed) positions.
    pub async fn get_active(&self) -> Result<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, market_id, yes_entry_price, no_entry_price, quantity,
                entry_timestamp, exit_strategy, state, unrealized_pnl,
                realized_pnl, exit_timestamp, yes_exit_price, no_exit_price
            FROM positions
            WHERE state < 4
            ORDER BY entry_timestamp DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| Position {
                id: r.get("id"),
                market_id: r.get("market_id"),
                yes_entry_price: r.get("yes_entry_price"),
                no_entry_price: r.get("no_entry_price"),
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
                    _ => PositionState::Closed,
                },
                unrealized_pnl: r.get("unrealized_pnl"),
                realized_pnl: r.get("realized_pnl"),
                exit_timestamp: r.get("exit_timestamp"),
                yes_exit_price: r.get("yes_exit_price"),
                no_exit_price: r.get("no_exit_price"),
            })
            .collect())
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
            total_realized_pnl: row.get::<Option<Decimal>, _>("total_realized").unwrap_or_default(),
            total_unrealized_pnl: row.get::<Option<Decimal>, _>("total_unrealized").unwrap_or_default(),
            win_count: row.get::<Option<i64>, _>("wins").unwrap_or(0) as u64,
            loss_count: row.get::<Option<i64>, _>("losses").unwrap_or(0) as u64,
        })
    }
}
