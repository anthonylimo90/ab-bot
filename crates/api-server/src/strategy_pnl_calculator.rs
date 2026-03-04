//! Strategy P&L calculator.
//!
//! Background task that periodically computes per-strategy performance snapshots
//! by joining `quant_signals → positions` (for quant strategies) and querying
//! `positions` directly (for arb source).
//!
//! Results are written to `strategy_pnl_snapshots` for dashboard display
//! and dynamic tuner feedback.

use chrono::{Duration, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use std::time;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Configuration for the strategy P&L calculator.
#[derive(Debug, Clone)]
pub struct StrategyPnlConfig {
    /// Whether the calculator is enabled.
    pub enabled: bool,
    /// Interval between computation cycles in seconds.
    pub interval_secs: u64,
    /// Rolling windows to compute (in days).
    pub periods: Vec<i32>,
}

impl StrategyPnlConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("STRATEGY_PNL_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("STRATEGY_PNL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(21600), // 6 hours
            periods: vec![7, 30],
        }
    }
}

/// Spawn the strategy P&L calculator background task.
pub fn spawn_strategy_pnl_calculator(
    config: StrategyPnlConfig,
    pool: PgPool,
    db_semaphore: Arc<Semaphore>,
) {
    if !config.enabled {
        info!("Strategy P&L calculator disabled (STRATEGY_PNL_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        periods = ?config.periods,
        "Spawning strategy P&L calculator"
    );

    tokio::spawn(calculator_loop(config, pool, db_semaphore));
}

async fn calculator_loop(config: StrategyPnlConfig, pool: PgPool, db_semaphore: Arc<Semaphore>) {
    let interval = time::Duration::from_secs(config.interval_secs);

    // Startup delay — let positions accumulate
    tokio::time::sleep(time::Duration::from_secs(120)).await;

    loop {
        match compute_cycle(&pool, &db_semaphore, &config).await {
            Ok(rows) => {
                info!(rows_upserted = rows, "Strategy P&L computation completed");
            }
            Err(e) => {
                warn!(error = %e, "Strategy P&L computation failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

/// Row type for quant strategy P&L aggregation.
#[derive(Debug, sqlx::FromRow)]
struct StrategyPnlRow {
    strategy: String,
    total_signals: i64,
    executed: i64,
    wins: i64,
    losses: i64,
    net_pnl: Option<Decimal>,
    avg_pnl: Option<Decimal>,
    avg_hold_hours: Option<f64>,
}

/// Row type for source-based P&L (arb).
#[derive(Debug, sqlx::FromRow)]
struct SourcePnlRow {
    total_positions: i64,
    wins: i64,
    losses: i64,
    net_pnl: Option<Decimal>,
    avg_pnl: Option<Decimal>,
    avg_hold_hours: Option<f64>,
}

/// Execute a single computation cycle.
async fn compute_cycle(
    pool: &PgPool,
    db_semaphore: &Semaphore,
    config: &StrategyPnlConfig,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();
    let mut total_rows = 0usize;

    for &period_days in &config.periods {
        let window_start = now - Duration::days(period_days as i64);

        // ── Quant strategies: join quant_signals → positions ──
        let quant_rows = sqlx::query_as::<_, StrategyPnlRow>(
            r#"
            SELECT
                qs.kind AS strategy,
                COUNT(*)::bigint AS total_signals,
                COUNT(*) FILTER (WHERE qs.execution_status = 'executed')::bigint AS executed,
                COUNT(*) FILTER (
                    WHERE qs.execution_status = 'executed'
                      AND p.realized_pnl > 0
                      AND p.state = 4
                )::bigint AS wins,
                COUNT(*) FILTER (
                    WHERE qs.execution_status = 'executed'
                      AND p.realized_pnl <= 0
                      AND p.state = 4
                )::bigint AS losses,
                SUM(p.realized_pnl) FILTER (
                    WHERE qs.execution_status = 'executed'
                      AND p.state = 4
                ) AS net_pnl,
                AVG(p.realized_pnl) FILTER (
                    WHERE qs.execution_status = 'executed'
                      AND p.state = 4
                ) AS avg_pnl,
                AVG(
                    EXTRACT(EPOCH FROM (p.exit_timestamp - p.entry_timestamp)) / 3600.0
                ) FILTER (
                    WHERE qs.execution_status = 'executed'
                      AND p.state = 4
                      AND p.exit_timestamp IS NOT NULL
                ) AS avg_hold_hours
            FROM quant_signals qs
            LEFT JOIN positions p ON qs.position_id = p.id
            WHERE qs.generated_at >= $1
              AND qs.generated_at <= $2
            GROUP BY qs.kind
            "#,
        )
        .bind(window_start)
        .bind(now)
        .fetch_all(pool)
        .await?;

        // ── Arb strategy: positions with source = 1 ──
        let arb_row = sqlx::query_as::<_, SourcePnlRow>(
            r#"
            SELECT
                COUNT(*)::bigint AS total_positions,
                COUNT(*) FILTER (WHERE realized_pnl > 0 AND state = 4)::bigint AS wins,
                COUNT(*) FILTER (WHERE realized_pnl <= 0 AND state = 4)::bigint AS losses,
                SUM(realized_pnl) FILTER (WHERE state = 4) AS net_pnl,
                AVG(realized_pnl) FILTER (WHERE state = 4) AS avg_pnl,
                AVG(
                    EXTRACT(EPOCH FROM (exit_timestamp - entry_timestamp)) / 3600.0
                ) FILTER (
                    WHERE state = 4
                      AND exit_timestamp IS NOT NULL
                ) AS avg_hold_hours
            FROM positions
            WHERE source = 1
              AND entry_timestamp >= $1
              AND entry_timestamp <= $2
            "#,
        )
        .bind(window_start)
        .bind(now)
        .fetch_optional(pool)
        .await?;

        // ── Acquire semaphore for DB writes ──
        let _permit = db_semaphore.acquire().await.expect("semaphore closed");

        // Upsert quant strategy snapshots
        for row in &quant_rows {
            let win_rate = if row.executed > 0 {
                Some(row.wins as f64 / row.executed as f64)
            } else {
                None
            };

            let sharpe = compute_sharpe(pool, &row.strategy, window_start, now).await;

            upsert_snapshot(
                pool,
                &row.strategy,
                now,
                period_days,
                row.total_signals as i32,
                row.executed as i32,
                row.wins as i32,
                row.losses as i32,
                row.net_pnl.unwrap_or(Decimal::ZERO),
                row.avg_pnl.unwrap_or(Decimal::ZERO),
                win_rate,
                sharpe,
                row.avg_hold_hours.unwrap_or(0.0),
            )
            .await?;
            total_rows += 1;
        }

        // Upsert arb snapshot
        if let Some(row) = &arb_row {
            if row.total_positions > 0 {
                let win_rate = if (row.wins + row.losses) > 0 {
                    Some(row.wins as f64 / (row.wins + row.losses) as f64)
                } else {
                    None
                };

                upsert_snapshot(
                    pool,
                    "arb",
                    now,
                    period_days,
                    row.total_positions as i32,
                    row.total_positions as i32, // arb positions are always "executed"
                    row.wins as i32,
                    row.losses as i32,
                    row.net_pnl.unwrap_or(Decimal::ZERO),
                    row.avg_pnl.unwrap_or(Decimal::ZERO),
                    win_rate,
                    None, // arb Sharpe computed separately if needed
                    row.avg_hold_hours.unwrap_or(0.0),
                )
                .await?;
                total_rows += 1;
            }
        }

        drop(_permit);

        debug!(
            period_days = period_days,
            quant_strategies = quant_rows.len(),
            "Computed strategy P&L snapshots"
        );
    }

    Ok(total_rows)
}

/// Compute Sharpe ratio for a quant strategy over a period.
/// Uses individual position returns from closed quant signal positions.
async fn compute_sharpe(
    pool: &PgPool,
    strategy: &str,
    window_start: chrono::DateTime<Utc>,
    window_end: chrono::DateTime<Utc>,
) -> Option<f64> {
    // Fetch individual realized PnL values for Sharpe calculation
    let returns: Vec<(Decimal,)> = sqlx::query_as(
        r#"
        SELECT p.realized_pnl
        FROM quant_signals qs
        JOIN positions p ON qs.position_id = p.id
        WHERE qs.kind = $1
          AND qs.execution_status = 'executed'
          AND qs.generated_at >= $2
          AND qs.generated_at <= $3
          AND p.state = 4
          AND p.realized_pnl IS NOT NULL
        "#,
    )
    .bind(strategy)
    .bind(window_start)
    .bind(window_end)
    .fetch_all(pool)
    .await
    .ok()?;

    if returns.len() < 3 {
        return None; // insufficient data for meaningful Sharpe
    }

    let vals: Vec<f64> = returns.iter().map(|(pnl,)| decimal_to_f64(*pnl)).collect();

    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;

    if n < 2.0 {
        return None;
    }

    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std_dev = variance.sqrt();

    if std_dev < 1e-10 {
        return None; // no volatility
    }

    // Annualized Sharpe (assuming ~2 trades/day, ~730 trades/year)
    let daily_sharpe = mean / std_dev;
    let annualized = daily_sharpe * (365.0_f64).sqrt();

    Some(annualized)
}

/// Upsert a single strategy P&L snapshot row.
#[allow(clippy::too_many_arguments)]
async fn upsert_snapshot(
    pool: &PgPool,
    strategy: &str,
    period_end: chrono::DateTime<Utc>,
    period_days: i32,
    total_signals: i32,
    executed: i32,
    wins: i32,
    losses: i32,
    net_pnl: Decimal,
    avg_pnl: Decimal,
    win_rate: Option<f64>,
    sharpe: Option<f64>,
    avg_hold_hours: f64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        INSERT INTO strategy_pnl_snapshots (
            strategy, period_end, period_days,
            total_signals, executed, wins, losses,
            net_pnl, avg_pnl, win_rate, sharpe, avg_hold_hours
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        ON CONFLICT (strategy, period_end, period_days) DO UPDATE SET
            total_signals = EXCLUDED.total_signals,
            executed = EXCLUDED.executed,
            wins = EXCLUDED.wins,
            losses = EXCLUDED.losses,
            net_pnl = EXCLUDED.net_pnl,
            avg_pnl = EXCLUDED.avg_pnl,
            win_rate = EXCLUDED.win_rate,
            sharpe = EXCLUDED.sharpe,
            avg_hold_hours = EXCLUDED.avg_hold_hours
        "#,
    )
    .bind(strategy)
    .bind(period_end)
    .bind(period_days)
    .bind(total_signals)
    .bind(executed)
    .bind(wins)
    .bind(losses)
    .bind(net_pnl)
    .bind(avg_pnl)
    .bind(win_rate)
    .bind(sharpe)
    .bind(avg_hold_hours)
    .execute(pool)
    .await?;

    Ok(())
}

/// Convert Decimal to f64.
fn decimal_to_f64(d: Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.to_f64().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = StrategyPnlConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.interval_secs, 21600); // 6 hours
        assert_eq!(config.periods, vec![7, 30]);
    }

    #[test]
    fn test_sharpe_calculation_logic() {
        // Simulate: [10.0, -5.0, 8.0, -2.0, 12.0]
        let vals = [10.0_f64, -5.0, 8.0, -2.0, 12.0];
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n; // 4.6
        let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let std_dev = variance.sqrt();

        assert!((mean - 4.6).abs() < 0.01);
        assert!(std_dev > 0.0);

        let daily_sharpe = mean / std_dev;
        let annualized = daily_sharpe * 365.0_f64.sqrt();
        assert!(annualized > 0.0); // positive because mean > 0
    }
}
