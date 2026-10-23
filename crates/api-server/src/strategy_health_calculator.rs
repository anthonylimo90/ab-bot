//! Strategy health calculator.
//!
//! Periodically summarizes live trade telemetry into operator-facing health
//! snapshots with simple scale/hold/reduce/paper recommendations.

use chrono::{Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use std::time;
use tracing::{debug, info, warn};
use trading_engine::OrderExecutor;

#[derive(Debug, Clone)]
pub struct StrategyHealthConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub periods: Vec<i32>,
}

impl StrategyHealthConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_flag("STRATEGY_HEALTH_ENABLED", true),
            interval_secs: polymarket_core::config::env_count(
                "STRATEGY_HEALTH_INTERVAL_SECS",
                21600,
            ),
            periods: vec![7, 30],
        }
    }
}

#[derive(Debug, Clone, FromRow)]
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
    /// Sum of the expected edge recorded per executed signal. The unit depends on
    /// the strategy: basis points for quant strategies (`expected_edge_bps`), USD
    /// per share for `arb` (`net_profit`). Use [`expected_edge_bps`] to compare.
    total_expected_edge: Decimal,
    /// Sum of the observed edge on the same events (gross profit per share for `arb`).
    total_observed_edge: Decimal,
    /// Realized P&L of closed positions, in USD.
    total_realized_pnl: Decimal,
    /// Number of events/signals that contributed to `total_expected_edge`.
    expected_edge_events: i64,
    /// USD entry cost (filled notional) of the closed positions whose realized
    /// P&L is in `total_realized_pnl`.
    closed_filled_notional: Decimal,
    avg_hold_hours: Option<f64>,
}

#[derive(Debug, FromRow)]
struct LatestBacktestRow {
    strategy: String,
    id: uuid::Uuid,
    return_pct: Option<Decimal>,
    created_at: chrono::DateTime<Utc>,
}

pub fn spawn_strategy_health_calculator(
    config: StrategyHealthConfig,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
) {
    if !config.enabled {
        info!("Strategy health calculator disabled (STRATEGY_HEALTH_ENABLED is off)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        periods = ?config.periods,
        "Spawning strategy health calculator"
    );

    tokio::spawn(run_loop(config, pool, order_executor));
}

async fn run_loop(config: StrategyHealthConfig, pool: PgPool, order_executor: Arc<OrderExecutor>) {
    tokio::time::sleep(time::Duration::from_secs(180)).await;
    let interval = time::Duration::from_secs(config.interval_secs);

    loop {
        // A live account leaves paper results out, as the account valuation does.
        let live = order_executor.is_live_ready().await;
        match compute_cycle(&pool, &config, live).await {
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
    live: bool,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();
    let latest_backtests = load_latest_backtests(pool).await?;
    let mut upserted = 0usize;

    for &period_days in &config.periods {
        let window_start = now - Duration::days(period_days as i64);
        let rows = load_strategy_health(pool, window_start, now, live).await?;

        // A period's snapshots are stored together: readers show the latest
        // cycle's strategies, never part of them.
        let mut tx = pool.begin().await?;
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
            .execute(&mut *tx)
            .await?;
            upserted += 1;
        }
        tx.commit().await?;

        debug!(
            period_days,
            rows = upserted,
            "Computed strategy health snapshots"
        );
    }

    Ok(upserted)
}

/// `live` leaves paper results out.
async fn load_strategy_health(
    pool: &PgPool,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    live: bool,
) -> Result<Vec<StrategyHealthAggRow>, sqlx::Error> {
    let canonical: Vec<StrategyHealthAggRow> = sqlx::query_as(
        r#"
        WITH filtered AS (
            SELECT *
            FROM trade_events
            WHERE occurred_at >= $1
              AND occurred_at <= $2
              AND ($3 OR execution_mode <> 'paper')
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
        closed_notional AS (
            -- Entry cost of exactly the positions whose realized P&L is summed
            -- below, including entry fills that happened before the window.
            SELECT
                cp.strategy,
                COALESCE(SUM(te.filled_size_usd), 0) AS closed_filled_notional
            FROM (
                SELECT DISTINCT strategy, position_id
                FROM filtered
                WHERE position_id IS NOT NULL
                  AND (event_type IN ('position_closed', 'closed_via_resolution')
                       OR state_to = 'closed')
            ) cp
            JOIN trade_events te
              ON te.position_id = cp.position_id
             AND te.event_type = 'entry_filled'
            GROUP BY cp.strategy
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
                -- Entries that opened (a failed one, or an arb pair left
                -- holding one leg, is counted apart). Arb entries carry no
                -- signal id: their position stands in.
                COUNT(DISTINCT COALESCE(signal_id, position_id)) FILTER (
                    WHERE event_type = 'position_open'
                       OR state_to = 'open'
                )::bigint AS executed_signals,
                COUNT(DISTINCT signal_id) FILTER (WHERE event_type = 'signal_skipped')::bigint AS skipped_signals,
                COUNT(DISTINCT signal_id) FILTER (WHERE event_type = 'signal_expired')::bigint AS expired_signals,
                COUNT(DISTINCT position_id) FILTER (
                    WHERE event_type IN ('entry_rejected', 'one_legged_exit_ready')
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
                -- Each position's promised edge is recorded once, on its
                -- entry request.
                COALESCE(SUM(expected_edge) FILTER (
                    WHERE event_type = 'entry_requested'
                ), 0) AS total_expected_edge,
                COALESCE(SUM(observed_edge) FILTER (
                    WHERE event_type = 'entry_requested'
                ), 0) AS total_observed_edge,
                COUNT(*) FILTER (
                    WHERE expected_edge IS NOT NULL
                      AND event_type = 'entry_requested'
                )::bigint AS expected_edge_events,
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
            e.expected_edge_events,
            COALESCE(cn.closed_filled_notional, 0) AS closed_filled_notional,
            h.avg_hold_hours
        FROM event_metrics e
        LEFT JOIN position_state_counts p USING (strategy)
        LEFT JOIN hold_metrics h USING (strategy)
        LEFT JOIN closed_notional cn USING (strategy)
        ORDER BY e.strategy ASC
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(!live)
    .fetch_all(pool)
    .await?;

    let derived = load_derived_strategy_health(pool, from, to, live).await?;
    Ok(merge_strategy_health_rows(canonical, derived))
}

async fn load_derived_strategy_health(
    pool: &PgPool,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    live: bool,
) -> Result<Vec<StrategyHealthAggRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT
            qs.kind AS strategy,
            COUNT(*)::bigint AS generated_signals,
            COUNT(*) FILTER (WHERE qs.execution_status = 'executed')::bigint AS executed_signals,
            COUNT(*) FILTER (WHERE qs.execution_status = 'skipped')::bigint AS skipped_signals,
            COUNT(*) FILTER (WHERE qs.execution_status = 'expired')::bigint AS expired_signals,
            COUNT(*) FILTER (WHERE p.state = 1)::bigint AS open_positions,
            COUNT(*) FILTER (WHERE p.state = 2)::bigint AS exit_ready_positions,
            COUNT(*) FILTER (WHERE p.state = 4)::bigint AS closed_positions,
            COUNT(*) FILTER (WHERE p.state = 5)::bigint AS entry_failed_positions,
            COUNT(*) FILTER (WHERE p.state = 6)::bigint AS exit_failed_positions,
            COALESCE(
                SUM(NULLIF(qs.metadata ->> 'expected_edge_bps', '')::numeric)
                    FILTER (WHERE qs.execution_status = 'executed'),
                0
            ) AS total_expected_edge,
            0::numeric AS total_observed_edge,
            COALESCE(SUM(p.realized_pnl) FILTER (WHERE p.state = 4), 0) AS total_realized_pnl,
            COUNT(*) FILTER (
                WHERE qs.execution_status = 'executed'
                  AND NULLIF(qs.metadata ->> 'expected_edge_bps', '') IS NOT NULL
            )::bigint AS expected_edge_events,
            COALESCE(
                SUM(
                    CASE
                        WHEN (p.held_yes_qty + p.exited_yes_qty) > 0
                          OR (p.held_no_qty + p.exited_no_qty) > 0
                        THEN COALESCE(p.yes_entry_price, 0) * (p.held_yes_qty + p.exited_yes_qty)
                           + COALESCE(p.no_entry_price, 0) * (p.held_no_qty + p.exited_no_qty)
                        ELSE (COALESCE(p.yes_entry_price, 0) + COALESCE(p.no_entry_price, 0))
                           * p.quantity
                    END
                ) FILTER (WHERE p.state = 4),
                0
            ) AS closed_filled_notional,
            AVG((EXTRACT(EPOCH FROM (p.exit_timestamp - p.entry_timestamp)) / 3600.0)::double precision)
                FILTER (WHERE p.state = 4 AND p.exit_timestamp IS NOT NULL) AS avg_hold_hours
        FROM quant_signals qs
        LEFT JOIN positions p
          ON p.id = qs.position_id
         AND ($3 OR p.execution_mode <> 'paper')
        WHERE qs.generated_at >= $1
          AND qs.generated_at <= $2
          -- A signal acted on in paper is left out whole, not only its position.
          AND ($3 OR qs.position_id IS NULL OR p.id IS NOT NULL)
        GROUP BY qs.kind
        ORDER BY qs.kind ASC
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(!live)
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
    if row.generated_signals <= 0 {
        return None;
    }
    Some(row.skipped_signals as f64 / row.generated_signals as f64)
}

/// Failed entries and failed exits over the entries tried: those that got in
/// (an exit can fail only on one of them) and those that failed. An exit
/// failing in the window for an entry made before it can push the ratio over
/// one: it is capped there.
fn failure_rate(row: &StrategyHealthAggRow) -> Option<f64> {
    let total = row.executed_signals + row.entry_failed_positions;
    if total <= 0 {
        return None;
    }
    let failed = row.entry_failed_positions + row.exit_failed_positions;
    Some((failed as f64 / total as f64).min(1.0))
}

const BASIS_POINTS: Decimal = Decimal::from_parts(10_000, 0, 0, false, 0);

/// Realized P&L as basis points of the filled notional of the closed positions
/// it came from. `None` without a positive notional.
fn realized_edge_bps(row: &StrategyHealthAggRow) -> Option<Decimal> {
    if row.closed_filled_notional <= Decimal::ZERO {
        return None;
    }
    Some(row.total_realized_pnl / row.closed_filled_notional * BASIS_POINTS)
}

/// Average expected edge per executed signal, in basis points of entry cost.
///
/// The recorded edge has strategy-specific units: quant strategies store basis
/// points already, while `arb` stores `net_profit` in USD per share (a $1-payout
/// pair). The arb figure is converted to bps of its entry cost per share,
/// `1 - gross_edge` (the pair costs `1 - gross profit` per share), using the
/// average observed (gross) edge; without one the cost is taken as $1.
/// `None` when no edge was recorded or it is not positive.
fn expected_edge_bps(row: &StrategyHealthAggRow) -> Option<Decimal> {
    if row.expected_edge_events <= 0 {
        return None;
    }
    let events = Decimal::from(row.expected_edge_events);
    let mean_edge = row.total_expected_edge / events;
    if mean_edge <= Decimal::ZERO {
        return None;
    }

    if row.strategy == "arb" {
        let mean_gross = row.total_observed_edge / events;
        let cost_per_share = if mean_gross > Decimal::ZERO && mean_gross < Decimal::ONE {
            Decimal::ONE - mean_gross
        } else {
            Decimal::ONE
        };
        Some(mean_edge / cost_per_share * BASIS_POINTS)
    } else {
        Some(mean_edge)
    }
}

/// Share of the expected edge that was actually realized: realized bps over
/// expected bps, both measured against filled notional, so the ratio is
/// dimensionless (1.0 = captured exactly the edge the signals promised).
///
/// Both sides are window-level averages: realized P&L and its notional come from
/// positions closed in the window, the expected edge from the executed signals
/// recorded in it. `None` if either side is unavailable.
fn edge_capture_ratio(row: &StrategyHealthAggRow) -> Option<Decimal> {
    let expected = expected_edge_bps(row)?;
    let realized = realized_edge_bps(row)?;
    Some(realized / expected)
}

fn merge_strategy_health_rows(
    canonical: Vec<StrategyHealthAggRow>,
    derived: Vec<StrategyHealthAggRow>,
) -> Vec<StrategyHealthAggRow> {
    let mut derived_by_strategy: std::collections::HashMap<String, StrategyHealthAggRow> = derived
        .into_iter()
        .map(|row| (row.strategy.clone(), row))
        .collect();

    let mut merged: Vec<StrategyHealthAggRow> = canonical
        .into_iter()
        .map(|canonical_row| {
            if let Some(derived_row) = derived_by_strategy.remove(&canonical_row.strategy) {
                hydrate_strategy_health_row(canonical_row, derived_row)
            } else {
                canonical_row
            }
        })
        .collect();

    merged.extend(derived_by_strategy.into_values());
    merged.sort_by(|a, b| a.strategy.cmp(&b.strategy));
    merged
}

fn hydrate_strategy_health_row(
    mut canonical: StrategyHealthAggRow,
    derived: StrategyHealthAggRow,
) -> StrategyHealthAggRow {
    canonical.generated_signals = canonical.generated_signals.max(derived.generated_signals);
    canonical.executed_signals = canonical.executed_signals.max(derived.executed_signals);
    canonical.skipped_signals = canonical.skipped_signals.max(derived.skipped_signals);
    canonical.expired_signals = canonical.expired_signals.max(derived.expired_signals);
    canonical.open_positions = canonical.open_positions.max(derived.open_positions);
    canonical.exit_ready_positions = canonical
        .exit_ready_positions
        .max(derived.exit_ready_positions);
    canonical.closed_positions = canonical.closed_positions.max(derived.closed_positions);
    canonical.entry_failed_positions = canonical
        .entry_failed_positions
        .max(derived.entry_failed_positions);
    canonical.exit_failed_positions = canonical
        .exit_failed_positions
        .max(derived.exit_failed_positions);
    if canonical.total_expected_edge.is_zero() && !derived.total_expected_edge.is_zero() {
        // The sum and the number of events it was built from travel together.
        canonical.total_expected_edge = derived.total_expected_edge;
        canonical.expected_edge_events = derived.expected_edge_events;
    }
    if canonical.total_observed_edge.is_zero() && !derived.total_observed_edge.is_zero() {
        canonical.total_observed_edge = derived.total_observed_edge;
    }
    if canonical.total_realized_pnl.is_zero() && !derived.total_realized_pnl.is_zero() {
        // Realized P&L and the notional it is measured against must come from
        // the same positions, so they are taken together or not at all.
        canonical.total_realized_pnl = derived.total_realized_pnl;
        canonical.closed_filled_notional = derived.closed_filled_notional;
    }
    canonical.avg_hold_hours = canonical.avg_hold_hours.or(derived.avg_hold_hours);
    canonical
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

    /// `arb` rows record edge in USD per share, every other strategy in bps.
    fn sample_row(strategy: &str) -> StrategyHealthAggRow {
        let is_arb = strategy == "arb";
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
            // arb: $0.03 net / $0.04 gross per share over 10 events;
            // others: 100 bps per event.
            total_expected_edge: if is_arb {
                Decimal::new(30, 2)
            } else {
                Decimal::new(1000, 0)
            },
            total_observed_edge: if is_arb {
                Decimal::new(40, 2)
            } else {
                Decimal::new(900, 0)
            },
            // $300 (arb) / $80 realized on $10,000 of closed notional.
            total_realized_pnl: if is_arb {
                Decimal::new(300, 0)
            } else {
                Decimal::new(80, 0)
            },
            expected_edge_events: 10,
            closed_filled_notional: Decimal::new(10_000, 0),
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

    #[test]
    fn test_skip_rate_uses_generated_signals_as_denominator() {
        let row = StrategyHealthAggRow {
            generated_signals: 2237,
            skipped_signals: 2237,
            expired_signals: 0,
            ..sample_row("flow")
        };

        assert_eq!(skip_rate(&row), Some(1.0));
    }

    #[test]
    fn test_merge_strategy_health_rows_hydrates_missing_execution_counts() {
        let merged = merge_strategy_health_rows(
            vec![StrategyHealthAggRow {
                strategy: "flow".to_string(),
                generated_signals: 10,
                executed_signals: 0,
                skipped_signals: 10,
                expired_signals: 0,
                open_positions: 0,
                exit_ready_positions: 0,
                closed_positions: 2,
                entry_failed_positions: 0,
                exit_failed_positions: 0,
                total_expected_edge: Decimal::ZERO,
                total_observed_edge: Decimal::ZERO,
                total_realized_pnl: Decimal::new(-5, 0),
                expected_edge_events: 0,
                closed_filled_notional: Decimal::ZERO,
                avg_hold_hours: None,
            }],
            vec![StrategyHealthAggRow {
                strategy: "flow".to_string(),
                generated_signals: 10,
                executed_signals: 3,
                skipped_signals: 10,
                expired_signals: 0,
                open_positions: 0,
                exit_ready_positions: 0,
                closed_positions: 2,
                entry_failed_positions: 0,
                exit_failed_positions: 0,
                total_expected_edge: Decimal::new(120, 0),
                total_observed_edge: Decimal::ZERO,
                total_realized_pnl: Decimal::new(-5, 0),
                expected_edge_events: 3,
                closed_filled_notional: Decimal::new(250, 0),
                avg_hold_hours: Some(4.0),
            }],
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].executed_signals, 3);
        assert_eq!(merged[0].total_expected_edge, Decimal::new(120, 0));
        assert_eq!(merged[0].avg_hold_hours, Some(4.0));
    }
}

/// `edge_capture_ratio` compares realized and expected edge in the same unit.
#[cfg(test)]
mod edge_capture_tests {
    use super::*;
    use sqlx::PgPool;
    use uuid::Uuid;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn approx(a: Decimal, b: &str) -> bool {
        (a - d(b)).abs() < d("0.0001")
    }

    fn row(strategy: &str) -> StrategyHealthAggRow {
        StrategyHealthAggRow {
            strategy: strategy.to_string(),
            generated_signals: 10,
            executed_signals: 5,
            skipped_signals: 0,
            expired_signals: 0,
            open_positions: 0,
            exit_ready_positions: 0,
            closed_positions: 5,
            entry_failed_positions: 0,
            exit_failed_positions: 0,
            total_expected_edge: Decimal::ZERO,
            total_observed_edge: Decimal::ZERO,
            total_realized_pnl: Decimal::ZERO,
            expected_edge_events: 0,
            closed_filled_notional: Decimal::ZERO,
            avg_hold_hours: None,
        }
    }

    #[test]
    fn quant_edge_is_compared_in_basis_points_of_notional() {
        // Five signals promised 120 bps each; the closed trades earned $12 on
        // $1,000 of filled notional = 120 bps: the edge was fully captured.
        let r = StrategyHealthAggRow {
            total_expected_edge: d("600"),
            expected_edge_events: 5,
            total_realized_pnl: d("12"),
            closed_filled_notional: d("1000"),
            ..row("flow")
        };
        assert!(approx(realized_edge_bps(&r).unwrap(), "120"));
        assert!(approx(expected_edge_bps(&r).unwrap(), "120"));
        assert!(approx(edge_capture_ratio(&r).unwrap(), "1"));
        // The old ratio divided USD by summed bps: 12 / 600 = 0.02 ("reduce").
        assert_eq!(r.total_realized_pnl / r.total_expected_edge, d("0.02"));
    }

    #[test]
    fn arb_per_share_edge_is_converted_to_bps_of_entry_cost() {
        // $0.03 net per share on a pair costing 1 - 0.04 = $0.96: 312.5 bps.
        // $24 earned on $1,000 of notional = 240 bps: 76.8% captured.
        let r = StrategyHealthAggRow {
            total_expected_edge: d("0.15"),
            total_observed_edge: d("0.20"),
            expected_edge_events: 5,
            total_realized_pnl: d("24"),
            closed_filled_notional: d("1000"),
            ..row("arb")
        };
        assert!(approx(expected_edge_bps(&r).unwrap(), "312.5"));
        assert!(approx(realized_edge_bps(&r).unwrap(), "240"));
        assert!(approx(edge_capture_ratio(&r).unwrap(), "0.768"));
        // The old ratio divided $24 by $0.15 of summed per-share edge = 160.
        assert_eq!(r.total_realized_pnl / r.total_expected_edge, d("160"));
    }

    #[test]
    fn arb_without_observed_edge_assumes_a_one_dollar_cost() {
        let r = StrategyHealthAggRow {
            total_expected_edge: d("0.15"),
            expected_edge_events: 5,
            total_realized_pnl: d("30"),
            closed_filled_notional: d("1000"),
            ..row("arb")
        };
        assert!(approx(expected_edge_bps(&r).unwrap(), "300"));
        assert!(approx(edge_capture_ratio(&r).unwrap(), "1"));
    }

    #[test]
    fn ratio_does_not_depend_on_trade_size() {
        let base = StrategyHealthAggRow {
            total_expected_edge: d("600"),
            expected_edge_events: 5,
            total_realized_pnl: d("9"),
            closed_filled_notional: d("1000"),
            ..row("mean_reversion")
        };
        let scaled = StrategyHealthAggRow {
            total_realized_pnl: d("90"),
            closed_filled_notional: d("10000"),
            ..base.clone()
        };
        assert_eq!(edge_capture_ratio(&base), edge_capture_ratio(&scaled));
        assert!(approx(edge_capture_ratio(&base).unwrap(), "0.75"));
    }

    #[test]
    fn losses_give_a_negative_ratio() {
        let r = StrategyHealthAggRow {
            total_expected_edge: d("300"),
            expected_edge_events: 3,
            total_realized_pnl: d("-5"),
            closed_filled_notional: d("1000"),
            ..row("flow")
        };
        assert!(edge_capture_ratio(&r).unwrap() < Decimal::ZERO);
        assert_eq!(recommend(&r).label, "reduce");
    }

    #[test]
    fn missing_inputs_give_no_ratio() {
        let ok = StrategyHealthAggRow {
            total_expected_edge: d("300"),
            expected_edge_events: 3,
            total_realized_pnl: d("5"),
            closed_filled_notional: d("1000"),
            ..row("flow")
        };
        assert!(edge_capture_ratio(&ok).is_some());

        let no_notional = StrategyHealthAggRow {
            closed_filled_notional: Decimal::ZERO,
            ..ok.clone()
        };
        assert_eq!(edge_capture_ratio(&no_notional), None);

        let no_events = StrategyHealthAggRow {
            expected_edge_events: 0,
            ..ok.clone()
        };
        assert_eq!(edge_capture_ratio(&no_events), None);

        let no_edge = StrategyHealthAggRow {
            total_expected_edge: Decimal::ZERO,
            ..ok.clone()
        };
        assert_eq!(edge_capture_ratio(&no_edge), None);

        let negative_edge = StrategyHealthAggRow {
            total_expected_edge: d("-10"),
            ..ok
        };
        assert_eq!(edge_capture_ratio(&negative_edge), None);
    }

    #[test]
    fn hydration_keeps_pnl_and_notional_paired() {
        let canonical = |pnl: &str, notional: &str| StrategyHealthAggRow {
            total_realized_pnl: d(pnl),
            closed_filled_notional: d(notional),
            ..row("flow")
        };
        let derived = StrategyHealthAggRow {
            total_expected_edge: d("300"),
            expected_edge_events: 3,
            total_realized_pnl: d("5"),
            closed_filled_notional: d("140"),
            ..row("flow")
        };

        // Canonical has no P&L: P&L, notional and the edge/event pair all come
        // from the derived row.
        let merged = merge_strategy_health_rows(vec![canonical("0", "0")], vec![derived.clone()]);
        assert_eq!(merged[0].total_realized_pnl, d("5"));
        assert_eq!(merged[0].closed_filled_notional, d("140"));
        assert_eq!(merged[0].expected_edge_events, 3);
        assert_eq!(merged[0].total_expected_edge, d("300"));
        assert!(approx(edge_capture_ratio(&merged[0]).unwrap(), "3.5714"));

        // Canonical has its own P&L but no notional: the derived notional is
        // NOT mixed in, the ratio is simply unavailable.
        let merged = merge_strategy_health_rows(vec![canonical("7", "0")], vec![derived]);
        assert_eq!(merged[0].total_realized_pnl, d("7"));
        assert_eq!(merged[0].closed_filled_notional, Decimal::ZERO);
        assert_eq!(edge_capture_ratio(&merged[0]), None);
    }

    #[test]
    fn recommendations_use_the_unit_consistent_ratio() {
        // arb: 96% capture, healthy execution -> scale.
        assert_eq!(recommend(&sample_arb_row()).label, "scale");

        // Weak capture (realized well below the promised edge) -> reduce.
        let weak = StrategyHealthAggRow {
            total_realized_pnl: d("30"),
            ..sample_arb_row()
        };
        assert!(edge_capture_ratio(&weak).unwrap() < d("0.20"));
        assert_eq!(recommend(&weak).label, "reduce");
    }

    fn sample_arb_row() -> StrategyHealthAggRow {
        StrategyHealthAggRow {
            executed_signals: 8,
            total_expected_edge: d("0.30"),
            total_observed_edge: d("0.40"),
            expected_edge_events: 10,
            total_realized_pnl: d("300"),
            closed_filled_notional: d("10000"),
            ..row("arb")
        }
    }

    // -- SQL ----------------------------------------------------------------

    async fn create_tables(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE trade_events (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                strategy TEXT NOT NULL,
                execution_mode TEXT NOT NULL DEFAULT 'paper',
                source TEXT NOT NULL DEFAULT 'test',
                market_id TEXT NOT NULL DEFAULT 'm',
                position_id UUID,
                signal_id UUID,
                event_type TEXT NOT NULL,
                state_from TEXT,
                state_to TEXT,
                reason TEXT,
                direction TEXT,
                confidence DOUBLE PRECISION,
                expected_edge DECIMAL(20, 10),
                observed_edge DECIMAL(20, 10),
                requested_size_usd DECIMAL(20, 10),
                filled_size_usd DECIMAL(20, 10),
                fill_price DECIMAL(20, 10),
                realized_pnl DECIMAL(20, 10),
                unrealized_pnl DECIMAL(20, 10),
                metadata JSONB NOT NULL DEFAULT '{}'::JSONB
            );
            CREATE TABLE positions (
                id UUID PRIMARY KEY,
                state SMALLINT NOT NULL DEFAULT 0,
                realized_pnl NUMERIC,
                entry_timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                exit_timestamp TIMESTAMPTZ,
                yes_entry_price NUMERIC NOT NULL DEFAULT 0,
                no_entry_price NUMERIC NOT NULL DEFAULT 0,
                quantity NUMERIC NOT NULL DEFAULT 0,
                held_yes_qty NUMERIC NOT NULL DEFAULT 0,
                held_no_qty NUMERIC NOT NULL DEFAULT 0,
                exited_yes_qty NUMERIC NOT NULL DEFAULT 0,
                exited_no_qty NUMERIC NOT NULL DEFAULT 0,
                execution_mode TEXT NOT NULL DEFAULT 'paper'
            );
            CREATE TABLE quant_signals (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                kind TEXT NOT NULL,
                execution_status TEXT,
                metadata JSONB NOT NULL DEFAULT '{}'::JSONB,
                generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                position_id UUID
            );
            "#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn event(
        pool: &PgPool,
        strategy: &str,
        position: Uuid,
        event_type: &str,
        state_to: Option<&str>,
        age: chrono::Duration,
        filled: Option<&str>,
        realized: Option<&str>,
        expected: Option<(&str, &str)>,
    ) {
        sqlx::query(
            "INSERT INTO trade_events (strategy, position_id, event_type, state_to, occurred_at, \
             filled_size_usd, realized_pnl, expected_edge, observed_edge) \
             VALUES ($1, $2, $3, $4, $5, $6::numeric, $7::numeric, $8::numeric, $9::numeric)",
        )
        .bind(strategy)
        .bind(position)
        .bind(event_type)
        .bind(state_to)
        .bind(Utc::now() - age)
        .bind(filled)
        .bind(realized)
        .bind(expected.map(|e| e.0))
        .bind(expected.map(|e| e.1))
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = false)]
    async fn canonical_query_measures_notional_of_the_closed_positions(pool: PgPool) {
        create_tables(&pool).await;
        let day = |n| chrono::Duration::days(n);
        let hour = |n| chrono::Duration::hours(n);

        // P1: entered 10 days ago (before the 7-day window), closed 1 day ago.
        let p1 = Uuid::new_v4();
        event(
            &pool,
            "arb",
            p1,
            "entry_filled",
            None,
            day(10),
            Some("300"),
            None,
            None,
        )
        .await;
        event(
            &pool,
            "arb",
            p1,
            "entry_filled",
            None,
            day(10),
            Some("280"),
            None,
            None,
        )
        .await;
        event(
            &pool,
            "arb",
            p1,
            "entry_requested",
            Some("pending"),
            day(3),
            None,
            None,
            Some(("0.03", "0.04")),
        )
        .await;
        event(
            &pool,
            "arb",
            p1,
            "position_closed",
            Some("closed"),
            day(1),
            None,
            Some("12"),
            None,
        )
        .await;

        // P2: entered and still open inside the window: not in the closed notional.
        let p2 = Uuid::new_v4();
        event(
            &pool,
            "arb",
            p2,
            "entry_filled",
            None,
            hour(5),
            Some("100"),
            None,
            None,
        )
        .await;
        event(
            &pool,
            "arb",
            p2,
            "entry_requested",
            Some("pending"),
            hour(5),
            None,
            None,
            Some(("0.05", "0.06")),
        )
        .await;

        let rows = load_strategy_health(&pool, Utc::now() - day(7), Utc::now(), false)
            .await
            .unwrap();
        let arb = rows.iter().find(|r| r.strategy == "arb").unwrap();
        assert_eq!(arb.total_realized_pnl, d("12"));
        assert_eq!(
            arb.closed_filled_notional,
            d("580"),
            "P1's pre-window fills, not P2"
        );
        assert_eq!(arb.expected_edge_events, 2);
        assert_eq!(arb.total_expected_edge, d("0.08"));
        assert_eq!(arb.total_observed_edge, d("0.10"));

        // mean net 0.04 / (1 - 0.05) = 421.05 bps; realized 12 / 580 = 206.9 bps.
        assert!(approx(expected_edge_bps(arb).unwrap(), "421.0526"));
        assert!(approx(realized_edge_bps(arb).unwrap(), "206.8966"));
        assert!(approx(edge_capture_ratio(arb).unwrap(), "0.4914"));
    }

    #[sqlx::test(migrations = false)]
    async fn live_results_leave_paper_out(pool: PgPool) {
        create_tables(&pool).await;
        for (mode, pnl) in [("paper", "50"), ("live", "7")] {
            sqlx::query(
                "INSERT INTO trade_events (strategy, execution_mode, position_id, event_type, \
                 state_to, realized_pnl) VALUES ('arb', $1, $2, 'position_closed', 'closed', \
                 $3::numeric)",
            )
            .bind(mode)
            .bind(Uuid::new_v4())
            .bind(pnl)
            .execute(&pool)
            .await
            .unwrap();
            let position = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO positions (id, state, realized_pnl, execution_mode) \
                 VALUES ($1, 4, $2::numeric, $3)",
            )
            .bind(position)
            .bind(pnl)
            .bind(mode)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO quant_signals (kind, execution_status, position_id) \
                 VALUES ('flow', 'executed', $1)",
            )
            .bind(position)
            .execute(&pool)
            .await
            .unwrap();
        }
        let from = Utc::now() - chrono::Duration::days(1);
        let pnl = |rows: &[StrategyHealthAggRow], strategy: &str| {
            rows.iter()
                .find(|r| r.strategy == strategy)
                .unwrap()
                .total_realized_pnl
        };

        let live = load_strategy_health(&pool, from, Utc::now(), true)
            .await
            .unwrap();
        assert_eq!(pnl(&live, "arb"), d("7"));
        assert_eq!(pnl(&live, "flow"), d("7"));

        // A paper account still sees everything it traded, live leftovers too.
        let paper = load_strategy_health(&pool, from, Utc::now(), false)
            .await
            .unwrap();
        assert_eq!(pnl(&paper, "arb"), d("57"));
        assert_eq!(pnl(&paper, "flow"), d("57"));
    }

    #[sqlx::test(migrations = false)]
    async fn derived_query_uses_position_entry_cost_of_closed_positions(pool: PgPool) {
        create_tables(&pool).await;

        // Closed single-leg positions: 100 YES @ 0.40 = $40 and 200 YES @ 0.50
        // = $100; plus one still-open position that must not count.
        let mut ids = Vec::new();
        for (state, qty, price, pnl) in [
            (4, "100", "0.40", "2"),
            (4, "200", "0.50", "3"),
            (1, "50", "0.60", "0"),
        ] {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO positions (id, state, realized_pnl, yes_entry_price, quantity, \
                 exited_yes_qty, held_yes_qty) \
                 VALUES ($1, $2, $3::numeric, $4::numeric, $5::numeric, \
                         CASE WHEN $2 = 4 THEN $5::numeric ELSE 0 END, \
                         CASE WHEN $2 = 4 THEN 0 ELSE $5::numeric END)",
            )
            .bind(id)
            .bind(state as i16)
            .bind(pnl)
            .bind(price)
            .bind(qty)
            .execute(&pool)
            .await
            .unwrap();
            ids.push(id);
        }
        for (id, bps) in ids.iter().zip(["100", "120", "80"]) {
            sqlx::query(
                "INSERT INTO quant_signals (kind, execution_status, metadata, position_id) \
                 VALUES ('flow', 'executed', jsonb_build_object('expected_edge_bps', $1::numeric), $2)",
            )
            .bind(bps)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }

        let rows = load_derived_strategy_health(
            &pool,
            Utc::now() - chrono::Duration::days(7),
            Utc::now() + chrono::Duration::minutes(1),
            false,
        )
        .await
        .unwrap();
        let flow = rows.iter().find(|r| r.strategy == "flow").unwrap();
        assert_eq!(flow.expected_edge_events, 3);
        assert_eq!(flow.total_expected_edge, d("300"));
        assert_eq!(flow.total_realized_pnl, d("5"));
        assert_eq!(flow.closed_filled_notional, d("140"));

        // 5 / 140 = 357.14 bps realized vs a 100 bps average promise.
        assert!(approx(realized_edge_bps(flow).unwrap(), "357.1429"));
        assert!(approx(expected_edge_bps(flow).unwrap(), "100"));
        assert!(approx(edge_capture_ratio(flow).unwrap(), "3.5714"));
    }
}
