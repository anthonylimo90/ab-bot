//! Periodic reconciliation of position marks and visibility from explicit holdings.
//!
//! This keeps `positions.current_price`, `positions.unrealized_pnl`, and
//! `positions.is_open` aligned with the latest market snapshot in `markets`.
//! Holdings and lifecycle state are never inferred from failure text or exit prices.

use sqlx::PgPool;
use std::time;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct PositionReconcilerConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub startup_delay_secs: u64,
}

impl PositionReconcilerConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("POSITION_RECONCILER_ENABLED")
                .map(|value| value == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("POSITION_RECONCILER_INTERVAL_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(300),
            startup_delay_secs: std::env::var("POSITION_RECONCILER_STARTUP_DELAY_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        }
    }
}

pub fn spawn_position_reconciler(config: PositionReconcilerConfig, pool: PgPool) {
    if !config.enabled {
        info!("Position reconciler disabled (POSITION_RECONCILER_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        startup_delay_secs = config.startup_delay_secs,
        "Spawning position reconciler"
    );

    tokio::spawn(run_loop(config, pool));
}

async fn run_loop(config: PositionReconcilerConfig, pool: PgPool) {
    if config.startup_delay_secs > 0 {
        tokio::time::sleep(time::Duration::from_secs(config.startup_delay_secs)).await;
    }

    loop {
        match reconcile_cycle(&pool).await {
            Ok(updated_rows) => info!(updated_rows, "Position reconciler cycle completed"),
            Err(error) => warn!(error = %error, "Position reconciler cycle failed"),
        }

        tokio::time::sleep(time::Duration::from_secs(config.interval_secs)).await;
    }
}

/// Refresh every open or holding row's marks; returns the rows that changed.
///
/// `current_price` is the market value of the held shares per unit of
/// `quantity`, so `quantity * current_price` is that value and compares with
/// `quantity * entry_price` (for a YES+NO pair, both are per pair). Rows whose
/// marks are unchanged are not rewritten.
pub(crate) async fn reconcile_cycle(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        WITH marks AS (
            SELECT
                p.id,
                (p.state IN (0,1,2,3) OR p.held_yes_qty > 0 OR p.held_no_qty > 0) AS is_open,
                -- Rounded to the columns' scale, so an unchanged mark compares
                -- equal to the stored one.
                CASE WHEN p.held_yes_qty + p.held_no_qty > 0 AND p.quantity > 0
                    THEN ROUND((p.held_yes_qty * COALESCE(m.yes_price, p.yes_entry_price)
                        + p.held_no_qty * COALESCE(m.no_price, p.no_entry_price))
                        / p.quantity, 8)
                    ELSE p.current_price
                END AS current_price,
                ROUND(p.held_yes_qty * (COALESCE(m.yes_price, p.yes_entry_price) - p.yes_entry_price)
                    + p.held_no_qty * (COALESCE(m.no_price, p.no_entry_price) - p.no_entry_price),
                    8) AS unrealized_pnl
            FROM positions p
            LEFT JOIN markets m ON m.id = p.market_id
            WHERE p.state IN (0, 1, 2, 3, 6, 7)
               OR p.held_yes_qty > 0 OR p.held_no_qty > 0
               -- A failed entry holding nothing is settled once (closed, no
               -- open P&L) and then never read again.
               OR (p.state = 5 AND (p.is_open OR p.unrealized_pnl <> 0))
        ), changed AS (
            UPDATE positions p SET
                is_open = k.is_open,
                current_price = k.current_price,
                unrealized_pnl = k.unrealized_pnl
            FROM marks k
            WHERE k.id = p.id
              AND (p.is_open IS DISTINCT FROM k.is_open
                   OR p.current_price IS DISTINCT FROM k.current_price
                   OR p.unrealized_pnl IS DISTINCT FROM k.unrealized_pnl)
            RETURNING p.id
        ) SELECT COUNT(*) FROM changed
    "#,
    )
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use polymarket_core::types::{ExitStrategy, Position, PositionState};
    use rust_decimal::Decimal;

    #[sqlx::test(migrations = false)]
    async fn marks_value_held_shares_per_unit_and_skip_unchanged_rows(pool: PgPool) {
        let service = crate::exit_handler::correctness_tests::setup(&pool).await;
        sqlx::query("INSERT INTO markets (id, yes_price, no_price) VALUES ('m', 0.55, 0.45)")
            .execute(&pool)
            .await
            .unwrap();
        let mut pair = Position::new(
            "m".into(),
            Decimal::new(48, 2),
            Decimal::new(50, 2),
            Decimal::TEN,
            ExitStrategy::HoldToResolution,
        );
        pair.state = PositionState::Open;
        pair.held_yes_qty = Decimal::TEN;
        pair.held_no_qty = Decimal::TEN;
        // A single leg with 4 of 10 shares left after a partial exit.
        let mut leg = Position::new(
            "m".into(),
            Decimal::new(40, 2),
            Decimal::ZERO,
            Decimal::TEN,
            ExitStrategy::ExitOnCorrection,
        );
        leg.state = PositionState::ExitFailed;
        leg.held_yes_qty = Decimal::from(4);
        // Arb quantities are size / cost: never round numbers.
        sqlx::query("INSERT INTO markets (id, yes_price, no_price) VALUES ('f', 0.553, 0.441)")
            .execute(&pool)
            .await
            .unwrap();
        let shares: Decimal = "10.20408163".parse().unwrap();
        let mut fractional = Position::new(
            "f".into(),
            Decimal::new(48, 2),
            Decimal::new(50, 2),
            shares,
            ExitStrategy::HoldToResolution,
        );
        fractional.state = PositionState::Open;
        fractional.held_yes_qty = shares;
        fractional.held_no_qty = shares;
        let mut failed = Position::new(
            "m".into(),
            Decimal::new(48, 2),
            Decimal::new(50, 2),
            Decimal::TEN,
            ExitStrategy::HoldToResolution,
        );
        failed.state = PositionState::EntryFailed;
        for position in [&pair, &leg, &fractional, &failed] {
            service.repo().insert(position).await.unwrap();
        }
        sqlx::query("UPDATE positions SET is_open = TRUE, unrealized_pnl = 1 WHERE id = $1")
            .bind(failed.id)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(reconcile_cycle(&pool).await.unwrap(), 4);
        let mark = |id| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (Decimal, Decimal, Decimal)>(
                    "SELECT quantity, current_price, unrealized_pnl FROM positions WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        // The pair is worth 10 × (0.55 + 0.45) = $10, comparable to its
        // 10 × 0.98 cost, not half of it.
        let (quantity, price, pnl) = mark(pair.id).await;
        assert_eq!(quantity * price, Decimal::TEN);
        assert_eq!(pnl, Decimal::new(20, 2));
        // The leg's 4 shares are worth $2.20.
        let (quantity, price, pnl) = mark(leg.id).await;
        assert_eq!(quantity * price, Decimal::new(220, 2));
        assert_eq!(pnl, Decimal::new(60, 2));

        // 10.20408163 × (0.073 − 0.059), rounded to the column's 8 places.
        let (_, _, pnl) = mark(fractional.id).await;
        assert_eq!(pnl, "0.14285714".parse::<Decimal>().unwrap());
        // The failed entry is closed with no open P&L.
        let (open, pnl): (bool, Decimal) =
            sqlx::query_as("SELECT is_open, unrealized_pnl FROM positions WHERE id = $1")
                .bind(failed.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!open);
        assert_eq!(pnl, Decimal::ZERO);

        // Nothing moved: nothing is rewritten.
        assert_eq!(reconcile_cycle(&pool).await.unwrap(), 0);
    }
}
