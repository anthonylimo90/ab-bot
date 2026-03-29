//! Periodic reconciliation for open position marks and hidden one-legged rows.
//!
//! This keeps `positions.current_price`, `positions.unrealized_pnl`, and
//! `positions.is_open` aligned with the latest market snapshot in `markets`.
//! It also promotes legacy one-legged `EntryFailed` arb rows into `ExitFailed`
//! so the recovery loop can see and unwind them.

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
                .unwrap_or(60),
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

async fn reconcile_cycle(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        WITH candidate_positions AS (
            SELECT
                p.id,
                p.state,
                p.is_open,
                p.current_price,
                p.unrealized_pnl,
                p.quantity,
                p.yes_entry_price,
                p.no_entry_price,
                p.yes_exit_price,
                p.no_exit_price,
                p.failure_reason,
                CASE
                    WHEN p.state = 5
                        AND (
                            p.failure_reason::jsonb ? 'one_legged_entry'
                            OR p.failure_reason ILIKE '%one-legged%'
                            OR p.failure_reason ILIKE '%one_legged%'
                        )
                    THEN TRUE
                    ELSE FALSE
                END AS one_legged_entry_failed,
                CASE
                    WHEN p.state = 6
                        AND (
                            p.failure_reason::jsonb ? 'one_legged_entry'
                            OR p.failure_reason ILIKE '%one-legged%'
                            OR p.failure_reason ILIKE '%one_legged%'
                            OR p.failure_reason ILIKE '%sell%'
                            OR p.failure_reason ILIKE '%exit%'
                            OR p.failure_reason ILIKE '%token id%'
                        )
                    THEN TRUE
                    WHEN p.state = 6
                        AND (p.yes_exit_price IS NOT NULL OR p.no_exit_price IS NOT NULL)
                    THEN TRUE
                    ELSE FALSE
                END AS exit_failure_with_live_exposure,
                m.yes_price,
                m.no_price
            FROM positions p
            LEFT JOIN markets m
              ON m.id = p.market_id
            WHERE p.state IN (1, 2, 3, 5, 6, 7)
        ),
        derived_marks AS (
            SELECT
                id,
                state,
                is_open,
                current_price,
                unrealized_pnl,
                quantity,
                yes_entry_price,
                no_entry_price,
                CASE
                    WHEN one_legged_entry_failed THEN TRUE
                    WHEN state = 6 AND NOT exit_failure_with_live_exposure THEN FALSE
                    ELSE yes_entry_price > 0 AND yes_exit_price IS NULL
                END AS holds_yes,
                CASE
                    WHEN one_legged_entry_failed THEN FALSE
                    WHEN state = 6 AND NOT exit_failure_with_live_exposure THEN FALSE
                    ELSE no_entry_price > 0 AND no_exit_price IS NULL
                END AS holds_no,
                yes_price,
                no_price,
                one_legged_entry_failed,
                exit_failure_with_live_exposure
            FROM candidate_positions
        ),
        candidate_updates AS (
            SELECT
                id,
                state,
                is_open,
                current_price,
                unrealized_pnl,
                holds_yes,
                holds_no,
                quantity,
                (
                    CASE
                        WHEN holds_yes OR holds_no THEN
                            CASE
                                WHEN holds_yes THEN COALESCE(yes_price, yes_entry_price)
                                ELSE 0::numeric
                            END +
                            CASE
                                WHEN holds_no THEN COALESCE(no_price, no_entry_price)
                                ELSE 0::numeric
                            END
                        ELSE current_price
                    END
                ) AS effective_current_price,
                CASE
                    WHEN holds_yes OR holds_no THEN quantity * (
                        (
                            CASE
                                WHEN holds_yes THEN COALESCE(yes_price, yes_entry_price)
                                ELSE 0::numeric
                            END +
                            CASE
                                WHEN holds_no THEN COALESCE(no_price, no_entry_price)
                                ELSE 0::numeric
                            END
                        ) - (
                            CASE WHEN holds_yes THEN yes_entry_price ELSE 0::numeric END +
                            CASE WHEN holds_no THEN no_entry_price ELSE 0::numeric END
                        )
                    )
                    WHEN state = 5 THEN 0::numeric
                    ELSE unrealized_pnl
                END AS effective_unrealized_pnl,
                CASE
                    WHEN state = 5 THEN one_legged_entry_failed
                    WHEN state = 6 AND NOT exit_failure_with_live_exposure THEN FALSE
                    WHEN state = 4 THEN FALSE
                    ELSE TRUE
                END AS effective_is_open,
                CASE
                    WHEN state = 5 AND one_legged_entry_failed THEN 6
                    WHEN state = 6 AND NOT exit_failure_with_live_exposure THEN 5
                    ELSE state
                END AS effective_state
            FROM derived_marks
        ),
        updated AS (
            UPDATE positions p
            SET current_price = c.effective_current_price,
                unrealized_pnl = c.effective_unrealized_pnl,
                is_open = c.effective_is_open,
                state = c.effective_state,
                -- Sync per-leg qty as a floor: bump up if inferred exposure exists
                -- but the explicit qty field is zero (pre-migration rows).
                -- Never reduce — only PositionService exit fills do that.
                held_yes_qty = CASE
                    WHEN c.effective_state IN (4, 5) THEN 0
                    WHEN c.holds_yes AND p.held_yes_qty = 0 THEN p.quantity
                    ELSE p.held_yes_qty
                END,
                held_no_qty = CASE
                    WHEN c.effective_state IN (4, 5) THEN 0
                    WHEN c.holds_no AND p.held_no_qty = 0 THEN p.quantity
                    ELSE p.held_no_qty
                END,
                last_updated = CASE
                    WHEN p.state IS DISTINCT FROM c.effective_state
                      OR p.is_open IS DISTINCT FROM c.effective_is_open
                    THEN NOW()
                    ELSE p.last_updated
                END,
                updated_at = CASE
                    WHEN p.current_price IS DISTINCT FROM c.effective_current_price
                      OR p.unrealized_pnl IS DISTINCT FROM c.effective_unrealized_pnl
                      OR p.is_open IS DISTINCT FROM c.effective_is_open
                      OR p.state IS DISTINCT FROM c.effective_state
                    THEN NOW()
                    ELSE p.updated_at
                END
            FROM candidate_updates c
            WHERE p.id = c.id
              AND (
                    p.current_price IS DISTINCT FROM c.effective_current_price
                 OR p.unrealized_pnl IS DISTINCT FROM c.effective_unrealized_pnl
                 OR p.is_open IS DISTINCT FROM c.effective_is_open
                 OR p.state IS DISTINCT FROM c.effective_state
                 OR (c.holds_yes AND p.held_yes_qty = 0)
                 OR (c.holds_no AND p.held_no_qty = 0)
                 OR (c.effective_state IN (4, 5) AND (p.held_yes_qty > 0 OR p.held_no_qty > 0))
              )
            RETURNING 1
        )
        SELECT COUNT(*)::bigint
        FROM updated
        "#,
    )
    .fetch_one(pool)
    .await
}
