//! Strategy health calculator.
//!
//! Periodically summarizes live trade telemetry into operator-facing health
//! snapshots with simple scale/hold/reduce/paper recommendations.

use chrono::{Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::time;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct StrategyHealthConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub periods: Vec<i32>,
}

impl StrategyHealthConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("STRATEGY_HEALTH_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("STRATEGY_HEALTH_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(21600),
            periods: vec![7, 30],
        }
    }
}

#[derive(Debug, FromRow)]
struct StrategyHealthAggRow {
    strategy: String,
    generated_signals: i64,
    executed_signals: i64,
    skipped_signals: i64,
    expired_signals: i64,
    open_positions: i64,
    exit_ready_positions: i64,
    closed_positions: i64,
    entry_failed_positions: i64,
    exit_failed_positions: i64,
    total_expected_edge: Decimal,
    total_observed_edge: Decimal,
    total_realized_pnl: Decimal,
    avg_hold_hours: Option<f64>,
}

#[derive(Debug, FromRow)]
struct LatestBacktestRow {
    strategy: String,
    id: uuid::Uuid,
    return_pct: Option<Decimal>,
    created_at: chrono::DateTime<Utc>,
}

pub fn spawn_strategy_health_calculator(config: StrategyHealthConfig, pool: PgPool) {
    if !config.enabled {
        info!("Strategy health calculator disabled (STRATEGY_HEALTH_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        periods = ?config.periods,
        "Spawning strategy health calculator"
    );

    tokio::spawn(run_loop(config, pool));
}

async fn run_loop(config: StrategyHealthConfig, pool: PgPool) {
    tokio::time::sleep(time::Duration::from_secs(180)).await;
    let interval = time::Duration::from_secs(config.interval_secs);

    loop {
        match compute_cycle(&pool, &config).await {
            Ok(rows) => info!(
                rows_upserted = rows,
                "Strategy health computation completed"
            ),
            Err(error) => warn!(error = %error, "Strategy health computation failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

async fn compute_cycle(
    pool: &PgPool,
    config: &StrategyHealthConfig,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();
    let latest_backtests = load_latest_backtests(pool).await?;
    let mut upserted = 0usize;

    for &period_days in &config.periods {
        let window_start = now - Duration::days(period_days as i64);
        let rows = load_strategy_health(pool, window_start, now).await?;

        for row in rows {
            let latest_backtest = latest_backtests
                .iter()
                .find(|candidate| candidate.strategy == row.strategy);
            let recommendation = recommend(&row);
            sqlx::query(
                r#"
                INSERT INTO strategy_health_snapshots (
                    strategy, period_end, period_days, generated_signals, executed_signals,
                    skipped_signals, expired_signals, open_positions, exit_ready_positions,
                    closed_positions, entry_failed_positions, exit_failed_positions,
                    total_expected_edge, total_observed_edge, total_realized_pnl,
                    avg_hold_hours, skip_rate, failure_rate, edge_capture_ratio,
                    recommendation, rationale, latest_backtest_id,
                    latest_backtest_return_pct, latest_backtest_created_at
                )
                VALUES (
                    $1, $2, $3, $4, $5,
                    $6, $7, $8, $9,
                    $10, $11, $12,
                    $13, $14, $15,
                    $16, $17, $18, $19,
                    $20, $21, $22,
                    $23, $24
                )
                ON CONFLICT (strategy, period_end, period_days) DO UPDATE SET
                    generated_signals = EXCLUDED.generated_signals,
                    executed_signals = EXCLUDED.executed_signals,
                    skipped_signals = EXCLUDED.skipped_signals,
                    expired_signals = EXCLUDED.expired_signals,
                    open_positions = EXCLUDED.open_positions,
                    exit_ready_positions = EXCLUDED.exit_ready_positions,
                    closed_positions = EXCLUDED.closed_positions,
                    entry_failed_positions = EXCLUDED.entry_failed_positions,
                    exit_failed_positions = EXCLUDED.exit_failed_positions,
                    total_expected_edge = EXCLUDED.total_expected_edge,
                    total_observed_edge = EXCLUDED.total_observed_edge,
                    total_realized_pnl = EXCLUDED.total_realized_pnl,
                    avg_hold_hours = EXCLUDED.avg_hold_hours,
                    skip_rate = EXCLUDED.skip_rate,
                    failure_rate = EXCLUDED.failure_rate,
                    edge_capture_ratio = EXCLUDED.edge_capture_ratio,
                    recommendation = EXCLUDED.recommendation,
                    rationale = EXCLUDED.rationale,
                    latest_backtest_id = EXCLUDED.latest_backtest_id,
                    latest_backtest_return_pct = EXCLUDED.latest_backtest_return_pct,
                    latest_backtest_created_at = EXCLUDED.latest_backtest_created_at
                "#,
            )
            .bind(&row.strategy)
            .bind(now)
            .bind(period_days)
            .bind(row.generated_signals as i32)
            .bind(row.executed_signals as i32)
            .bind(row.skipped_signals as i32)
            .bind(row.expired_signals as i32)
            .bind(row.open_positions as i32)
            .bind(row.exit_ready_positions as i32)
            .bind(row.closed_positions as i32)
            .bind(row.entry_failed_positions as i32)
            .bind(row.exit_failed_positions as i32)
            .bind(row.total_expected_edge)
            .bind(row.total_observed_edge)
            .bind(row.total_realized_pnl)
            .bind(row.avg_hold_hours)
            .bind(skip_rate(&row))
            .bind(failure_rate(&row))
            .bind(edge_capture_ratio(&row))
            .bind(recommendation.label)
            .bind(recommendation.rationale)
            .bind(latest_backtest.map(|bt| bt.id))
            .bind(latest_backtest.and_then(|bt| bt.return_pct))
            .bind(latest_backtest.map(|bt| bt.created_at))
            .execute(pool)
            .await?;
            upserted += 1;
        }

        debug!(
            period_days,
            rows = upserted,
            "Computed strategy health snapshots"
        );
    }

    Ok(upserted)
}

async fn load_strategy_health(
    pool: &PgPool,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
) -> Result<Vec<StrategyHealthAggRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        WITH filtered AS (
            SELECT *
            FROM trade_events
            WHERE occurred_at >= $1
              AND occurred_at <= $2
        ),
        latest_positions AS (
            SELECT DISTINCT ON (strategy, position_id)
                strategy,
                position_id,
                state_to,
                occurred_at
            FROM filtered
            WHERE position_id IS NOT NULL
            ORDER BY strategy, position_id, occurred_at DESC, id DESC
        ),
        position_state_counts AS (
            SELECT
                strategy,
                COUNT(*) FILTER (WHERE state_to = 'open')::bigint AS open_positions,
                COUNT(*) FILTER (WHERE state_to = 'exit_ready')::bigint AS exit_ready_positions
            FROM latest_positions
            GROUP BY strategy
        ),
        hold_metrics AS (
            SELECT
                strategy,
                AVG((EXTRACT(EPOCH FROM (closed_at - opened_at)) / 3600.0)::double precision) AS avg_hold_hours
            FROM (
                SELECT
                    strategy,
                    position_id,
                    MIN(occurred_at) FILTER (
                        WHERE event_type IN ('position_open', 'entry_filled')
                           OR state_to = 'open'
                    ) AS opened_at,
                    MAX(occurred_at) FILTER (
                        WHERE event_type IN ('position_closed', 'closed_via_resolution')
                           OR state_to = 'closed'
                    ) AS closed_at
                FROM filtered
                WHERE position_id IS NOT NULL
                GROUP BY strategy, position_id
            ) spans
            WHERE opened_at IS NOT NULL
              AND closed_at IS NOT NULL
            GROUP BY strategy
        ),
        event_metrics AS (
            SELECT
                strategy,
                COUNT(DISTINCT signal_id) FILTER (WHERE event_type = 'signal_generated')::bigint AS generated_signals,
                COUNT(DISTINCT signal_id) FILTER (
                    WHERE event_type IN ('entry_requested', 'entry_filled', 'position_open')
                       OR state_to = 'open'
                )::bigint AS executed_signals,
                COUNT(DISTINCT signal_id) FILTER (WHERE event_type = 'signal_skipped')::bigint AS skipped_signals,
                COUNT(DISTINCT signal_id) FILTER (WHERE event_type = 'signal_expired')::bigint AS expired_signals,
                COUNT(DISTINCT position_id) FILTER (
                    WHERE event_type = 'entry_rejected'
                       OR state_to = 'entry_failed'
                )::bigint AS entry_failed_positions,
                COUNT(DISTINCT position_id) FILTER (
                    WHERE event_type = 'exit_rejected'
                       OR state_to = 'exit_failed'
                )::bigint AS exit_failed_positions,
                COUNT(DISTINCT position_id) FILTER (
                    WHERE event_type IN ('position_closed', 'closed_via_resolution')
                       OR state_to = 'closed'
                )::bigint AS closed_positions,
                COALESCE(SUM(expected_edge) FILTER (
                    WHERE event_type IN ('entry_filled', 'position_open')
                       OR state_to = 'open'
                ), 0) AS total_expected_edge,
                COALESCE(SUM(observed_edge) FILTER (
                    WHERE event_type IN ('entry_filled', 'position_open')
                       OR state_to = 'open'
                ), 0) AS total_observed_edge,
                COALESCE(SUM(realized_pnl) FILTER (
                    WHERE event_type IN ('position_closed', 'closed_via_resolution')
                       OR state_to = 'closed'
                ), 0) AS total_realized_pnl
            FROM filtered
            GROUP BY strategy
        )
        SELECT
            e.strategy,
            e.generated_signals,
            e.executed_signals,
            e.skipped_signals,
            e.expired_signals,
            COALESCE(p.open_positions, 0) AS open_positions,
            COALESCE(p.exit_ready_positions, 0) AS exit_ready_positions,
            e.closed_positions,
            e.entry_failed_positions,
            e.exit_failed_positions,
            e.total_expected_edge,
            e.total_observed_edge,
            e.total_realized_pnl,
            h.avg_hold_hours
        FROM event_metrics e
        LEFT JOIN position_state_counts p USING (strategy)
        LEFT JOIN hold_metrics h USING (strategy)
        ORDER BY e.strategy ASC
        "#,
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
}

async fn load_latest_backtests(pool: &PgPool) -> Result<Vec<LatestBacktestRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT DISTINCT ON (strategy_name)
            strategy_name AS strategy,
            id,
            COALESCE(total_return_pct, return_pct) AS return_pct,
            created_at
        FROM (
            SELECT
                id,
                CASE
                    WHEN strategy->>'type' = 'arbitrage' THEN 'arb'
                    ELSE strategy->>'type'
                END AS strategy_name,
                total_return_pct,
                return_pct,
                created_at
            FROM backtest_results
            WHERE status = 'completed'
        ) ranked
        WHERE strategy_name IS NOT NULL
        ORDER BY strategy_name, created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

fn skip_rate(row: &StrategyHealthAggRow) -> Option<f64> {
    let total = row.generated_signals + row.skipped_signals + row.expired_signals;
    if total <= 0 {
        return None;
    }
    Some(row.skipped_signals as f64 / total as f64)
}

fn failure_rate(row: &StrategyHealthAggRow) -> Option<f64> {
    let total = row.executed_signals + row.entry_failed_positions + row.exit_failed_positions;
    if total <= 0 {
        return None;
    }
    Some((row.entry_failed_positions + row.exit_failed_positions) as f64 / total as f64)
}

fn edge_capture_ratio(row: &StrategyHealthAggRow) -> Option<Decimal> {
    if row.total_expected_edge <= Decimal::ZERO {
        return None;
    }
    Some(row.total_realized_pnl / row.total_expected_edge)
}

struct Recommendation {
    label: &'static str,
    rationale: String,
}

fn recommend(row: &StrategyHealthAggRow) -> Recommendation {
    let skip_rate = skip_rate(row).unwrap_or(0.0);
    let failure_rate = failure_rate(row).unwrap_or(0.0);
    let edge_capture = edge_capture_ratio(row)
        .and_then(|value| value.to_f64())
        .unwrap_or(0.0);
    let pnl = row.total_realized_pnl.to_f64().unwrap_or(0.0);

    let baseline = match row.strategy.as_str() {
        "arb" => "scale",
        "flow" => "hold",
        "mean_reversion" => "paper",
        "cross_market" => "paper",
        "resolution_proximity" => "paper",
        _ => "hold",
    };

    let label = if matches!(
        row.strategy.as_str(),
        "cross_market" | "resolution_proximity"
    ) {
        "paper"
    } else if failure_rate >= 0.25 || pnl < 0.0 || edge_capture < 0.20 {
        "reduce"
    } else if baseline == "scale" && edge_capture >= 0.75 && failure_rate <= 0.10 {
        "scale"
    } else if baseline == "hold" && edge_capture >= 0.50 && skip_rate <= 0.65 {
        "hold"
    } else {
        baseline
    };

    Recommendation {
        label,
        rationale: format!(
            "baseline={} edge_capture={:.2} failure_rate={:.2} skip_rate={:.2} realized_pnl={:.2}",
            baseline, edge_capture, failure_rate, skip_rate, pnl
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row(strategy: &str) -> StrategyHealthAggRow {
        StrategyHealthAggRow {
            strategy: strategy.to_string(),
            generated_signals: 10,
            executed_signals: 8,
            skipped_signals: 1,
            expired_signals: 1,
            open_positions: 1,
            exit_ready_positions: 0,
            closed_positions: 7,
            entry_failed_positions: 0,
            exit_failed_positions: 0,
            total_expected_edge: Decimal::new(100, 0),
            total_observed_edge: Decimal::new(90, 0),
            total_realized_pnl: Decimal::new(80, 0),
            avg_hold_hours: Some(2.0),
        }
    }

    #[test]
    fn test_recommend_arb_scale_when_capture_is_strong() {
        let rec = recommend(&sample_row("arb"));
        assert_eq!(rec.label, "scale");
    }

    #[test]
    fn test_recommend_reduce_on_negative_pnl() {
        let mut row = sample_row("flow");
        row.total_realized_pnl = Decimal::new(-10, 0);
        let rec = recommend(&row);
        assert_eq!(rec.label, "reduce");
    }
}
