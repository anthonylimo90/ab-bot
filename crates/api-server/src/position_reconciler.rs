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

pub(crate) async fn reconcile_cycle(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(r#"
        WITH marks AS (
            SELECT p.id,m.yes_price,m.no_price FROM positions p
            LEFT JOIN markets m ON m.id=p.market_id
            WHERE p.state<>4 OR p.held_yes_qty>0 OR p.held_no_qty>0
        ), changed AS (
            UPDATE positions p SET
                is_open = (p.state IN (0,1,2,3) OR p.held_yes_qty > 0 OR p.held_no_qty > 0),
                current_price = CASE WHEN p.held_yes_qty + p.held_no_qty > 0
                    THEN (p.held_yes_qty * COALESCE(m.yes_price,p.yes_entry_price)
                        + p.held_no_qty * COALESCE(m.no_price,p.no_entry_price))
                        / (p.held_yes_qty + p.held_no_qty) ELSE p.current_price END,
                unrealized_pnl = p.held_yes_qty * (COALESCE(m.yes_price,p.yes_entry_price)-p.yes_entry_price)
                    + p.held_no_qty * (COALESCE(m.no_price,p.no_entry_price)-p.no_entry_price)
            FROM marks m WHERE m.id = p.id
            RETURNING p.id
        ) SELECT COUNT(*) FROM changed
    "#).fetch_one(pool).await
}
