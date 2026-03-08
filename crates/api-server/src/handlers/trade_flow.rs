//! Trade flow analytics backed by canonical `trade_events` when available,
//! with explicit fallback to derived history for older windows.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::HashSet;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
pub struct TradeFlowQuery {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub strategy: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TradeFlowStrategySummary {
    pub strategy: String,
    pub source: String,
    pub supports_signal_history: bool,
    pub generated_signals: i64,
    pub executed_signals: i64,
    pub skipped_signals: i64,
    pub expired_signals: i64,
    pub open_positions: i64,
    pub exit_ready_positions: i64,
    pub closed_positions: i64,
    pub entry_failed_positions: i64,
    pub exit_failed_positions: i64,
    pub net_pnl: Decimal,
    pub avg_hold_hours: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TradeFlowSummaryResponse {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub total_generated_signals: i64,
    pub total_executed_signals: i64,
    pub total_skipped_signals: i64,
    pub total_expired_signals: i64,
    pub total_open_positions: i64,
    pub total_exit_ready_positions: i64,
    pub total_closed_positions: i64,
    pub total_entry_failed_positions: i64,
    pub total_exit_failed_positions: i64,
    pub total_realized_pnl: Decimal,
    pub strategies: Vec<TradeFlowStrategySummary>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TradeJourneyResponse {
    pub strategy: String,
    pub source: String,
    pub supports_signal_history: bool,
    pub lifecycle_stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_state: Option<String>,
    pub market_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_generated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_pnl: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_hours: Option<f64>,
    pub synthetic_history: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TradeFlowMarketResponse {
    pub market_id: String,
    pub summary: TradeFlowSummaryResponse,
    pub journeys: Vec<TradeJourneyResponse>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ArbMarketScorecardItem {
    pub market_id: String,
    pub signals_generated: i64,
    pub signals_skipped: i64,
    pub executed_positions: i64,
    pub entry_failed_positions: i64,
    pub closed_positions: i64,
    pub wins: i64,
    pub losses: i64,
    pub win_rate: Decimal,
    pub total_expected_edge: Decimal,
    pub total_observed_edge: Decimal,
    pub total_realized_pnl: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_expected_edge: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_observed_edge: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_realized_pnl: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_capture_ratio: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_hold_hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_filled_size_usd: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_traded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ArbMarketScorecardResponse {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub markets: Vec<ArbMarketScorecardItem>,
}

#[derive(Debug, FromRow)]
struct StrategySummaryRow {
    strategy: String,
    source: String,
    supports_signal_history: bool,
    generated_signals: i64,
    executed_signals: i64,
    skipped_signals: i64,
    expired_signals: i64,
    open_positions: i64,
    exit_ready_positions: i64,
    closed_positions: i64,
    entry_failed_positions: i64,
    exit_failed_positions: i64,
    net_pnl: Decimal,
    avg_hold_hours: Option<f64>,
}

#[derive(Debug, FromRow)]
struct TradeJourneyRow {
    strategy: String,
    source: String,
    supports_signal_history: bool,
    lifecycle_stage: String,
    execution_status: Option<String>,
    position_state: Option<String>,
    market_id: String,
    signal_id: Option<Uuid>,
    position_id: Option<Uuid>,
    direction: Option<String>,
    confidence: Option<f64>,
    skip_reason: Option<String>,
    signal_generated_at: Option<DateTime<Utc>>,
    opened_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    realized_pnl: Option<Decimal>,
    unrealized_pnl: Option<Decimal>,
    hold_hours: Option<f64>,
    synthetic_history: bool,
}

#[derive(Debug, FromRow)]
struct ArbMarketScorecardRow {
    market_id: String,
    signals_generated: i64,
    signals_skipped: i64,
    executed_positions: i64,
    entry_failed_positions: i64,
    closed_positions: i64,
    wins: i64,
    losses: i64,
    win_rate: Decimal,
    total_expected_edge: Decimal,
    total_observed_edge: Decimal,
    total_realized_pnl: Decimal,
    avg_expected_edge: Option<Decimal>,
    avg_observed_edge: Option<Decimal>,
    avg_realized_pnl: Option<Decimal>,
    edge_capture_ratio: Option<Decimal>,
    avg_hold_hours: Option<f64>,
    avg_filled_size_usd: Option<Decimal>,
    last_traded_at: Option<DateTime<Utc>>,
}

fn effective_window(query: &TradeFlowQuery) -> (DateTime<Utc>, DateTime<Utc>, i64) {
    let to = query.to.unwrap_or_else(Utc::now);
    let from = query.from.unwrap_or_else(|| to - Duration::days(7));
    let limit = query.limit.unwrap_or(100).clamp(1, 200);
    (from, to, limit)
}

async fn load_arb_market_scorecard_rows(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    market_id: Option<&str>,
    limit: i64,
) -> Result<Vec<ArbMarketScorecardItem>, sqlx::Error> {
    let rows: Vec<ArbMarketScorecardRow> = sqlx::query_as(
        r#"
        WITH filtered AS (
            SELECT *
            FROM trade_events
            WHERE strategy = 'arb'
              AND occurred_at >= $1
              AND occurred_at <= $2
              AND ($3::text IS NULL OR market_id = $3)
        ),
        markets AS (
            SELECT DISTINCT market_id
            FROM filtered
            WHERE market_id IS NOT NULL
        ),
        entry_events AS (
            SELECT
                market_id,
                position_id,
                MAX(expected_edge) FILTER (WHERE expected_edge IS NOT NULL) AS expected_edge,
                MAX(observed_edge) FILTER (WHERE observed_edge IS NOT NULL) AS observed_edge,
                MAX(filled_size_usd) FILTER (WHERE filled_size_usd IS NOT NULL) AS filled_size_usd,
                MAX(occurred_at) FILTER (
                    WHERE event_type IN ('entry_filled', 'position_open')
                       OR state_to = 'open'
                ) AS traded_at
            FROM filtered
            WHERE position_id IS NOT NULL
            GROUP BY market_id, position_id
        ),
        close_events AS (
            SELECT DISTINCT ON (position_id)
                market_id,
                position_id,
                realized_pnl,
                occurred_at AS closed_at
            FROM filtered
            WHERE position_id IS NOT NULL
              AND (
                    event_type IN ('position_closed', 'closed_via_resolution')
                    OR state_to = 'closed'
                  )
            ORDER BY position_id, occurred_at DESC
        ),
        open_events AS (
            SELECT
                market_id,
                position_id,
                MIN(occurred_at) AS opened_at
            FROM filtered
            WHERE position_id IS NOT NULL
              AND (event_type = 'position_open' OR state_to = 'open')
            GROUP BY market_id, position_id
        ),
        skipped AS (
            SELECT market_id, COUNT(*)::bigint AS signals_skipped
            FROM filtered
            WHERE event_type = 'signal_skipped'
            GROUP BY market_id
        ),
        generated AS (
            SELECT market_id, COUNT(*)::bigint AS signals_generated
            FROM filtered
            WHERE event_type = 'signal_generated'
            GROUP BY market_id
        ),
        failures AS (
            SELECT market_id, COUNT(DISTINCT position_id)::bigint AS entry_failed_positions
            FROM filtered
            WHERE event_type = 'position_failed'
              AND position_id IS NOT NULL
            GROUP BY market_id
        ),
        per_position AS (
            SELECT
                e.market_id,
                e.position_id,
                e.expected_edge,
                e.observed_edge,
                e.filled_size_usd,
                e.traded_at,
                c.realized_pnl,
                c.closed_at,
                o.opened_at
            FROM entry_events e
            LEFT JOIN close_events c
                ON c.position_id = e.position_id
            LEFT JOIN open_events o
                ON o.position_id = e.position_id
        ),
        positions_agg AS (
            SELECT
                p.market_id,
                COUNT(DISTINCT p.position_id)::bigint AS executed_positions,
                COUNT(DISTINCT p.position_id) FILTER (WHERE p.closed_at IS NOT NULL)::bigint AS closed_positions,
                COUNT(DISTINCT p.position_id) FILTER (
                    WHERE p.closed_at IS NOT NULL AND p.realized_pnl > 0
                )::bigint AS wins,
                COUNT(DISTINCT p.position_id) FILTER (
                    WHERE p.closed_at IS NOT NULL AND COALESCE(p.realized_pnl, 0) < 0
                )::bigint AS losses,
                CASE
                    WHEN COUNT(DISTINCT p.position_id) FILTER (
                        WHERE p.closed_at IS NOT NULL AND COALESCE(p.realized_pnl, 0) <> 0
                    ) = 0 THEN 0
                    ELSE (
                        COUNT(DISTINCT p.position_id) FILTER (
                            WHERE p.closed_at IS NOT NULL AND p.realized_pnl > 0
                        )::numeric /
                        COUNT(DISTINCT p.position_id) FILTER (
                            WHERE p.closed_at IS NOT NULL AND COALESCE(p.realized_pnl, 0) <> 0
                        )::numeric
                    )
                END AS win_rate,
                COALESCE(SUM(p.expected_edge), 0) AS total_expected_edge,
                COALESCE(SUM(p.observed_edge), 0) AS total_observed_edge,
                COALESCE(SUM(p.realized_pnl), 0) AS total_realized_pnl,
                AVG(p.expected_edge) AS avg_expected_edge,
                AVG(p.observed_edge) AS avg_observed_edge,
                AVG(p.realized_pnl) FILTER (WHERE p.closed_at IS NOT NULL) AS avg_realized_pnl,
                COALESCE(SUM(p.expected_edge) FILTER (WHERE p.closed_at IS NOT NULL), 0) AS closed_expected_edge,
                AVG((EXTRACT(EPOCH FROM (p.closed_at - p.opened_at)) / 3600.0)::double precision)
                    FILTER (WHERE p.closed_at IS NOT NULL AND p.opened_at IS NOT NULL) AS avg_hold_hours,
                AVG(p.filled_size_usd) AS avg_filled_size_usd,
                MAX(COALESCE(p.closed_at, p.traded_at, p.opened_at)) AS last_traded_at
            FROM per_position p
            GROUP BY p.market_id
        )
        SELECT
            m.market_id,
            COALESCE(g.signals_generated, 0) AS signals_generated,
            COALESCE(s.signals_skipped, 0) AS signals_skipped,
            COALESCE(pa.executed_positions, 0) AS executed_positions,
            COALESCE(f.entry_failed_positions, 0) AS entry_failed_positions,
            COALESCE(pa.closed_positions, 0) AS closed_positions,
            COALESCE(pa.wins, 0) AS wins,
            COALESCE(pa.losses, 0) AS losses,
            COALESCE(pa.win_rate, 0) AS win_rate,
            COALESCE(pa.total_expected_edge, 0) AS total_expected_edge,
            COALESCE(pa.total_observed_edge, 0) AS total_observed_edge,
            COALESCE(pa.total_realized_pnl, 0) AS total_realized_pnl,
            pa.avg_expected_edge,
            pa.avg_observed_edge,
            pa.avg_realized_pnl,
            CASE
                WHEN COALESCE(pa.closed_expected_edge, 0) = 0 THEN NULL
                ELSE COALESCE(pa.total_realized_pnl, 0) / NULLIF(pa.closed_expected_edge, 0)
            END AS edge_capture_ratio,
            pa.avg_hold_hours,
            pa.avg_filled_size_usd,
            pa.last_traded_at
        FROM markets m
        LEFT JOIN positions_agg pa ON pa.market_id = m.market_id
        LEFT JOIN generated g ON g.market_id = m.market_id
        LEFT JOIN skipped s ON s.market_id = m.market_id
        LEFT JOIN failures f ON f.market_id = m.market_id
        ORDER BY COALESCE(pa.total_realized_pnl, 0) DESC, m.market_id
        LIMIT $4
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(market_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| ArbMarketScorecardItem {
            market_id: row.market_id,
            signals_generated: row.signals_generated,
            signals_skipped: row.signals_skipped,
            executed_positions: row.executed_positions,
            entry_failed_positions: row.entry_failed_positions,
            closed_positions: row.closed_positions,
            wins: row.wins,
            losses: row.losses,
            win_rate: row.win_rate,
            total_expected_edge: row.total_expected_edge,
            total_observed_edge: row.total_observed_edge,
            total_realized_pnl: row.total_realized_pnl,
            avg_expected_edge: row.avg_expected_edge,
            avg_observed_edge: row.avg_observed_edge,
            avg_realized_pnl: row.avg_realized_pnl,
            edge_capture_ratio: row.edge_capture_ratio,
            avg_hold_hours: row.avg_hold_hours,
            avg_filled_size_usd: row.avg_filled_size_usd,
            last_traded_at: row.last_traded_at,
        })
        .collect())
}

#[cfg(test)]
fn state_name(state: i16) -> &'static str {
    match state {
        0 => "pending",
        1 => "open",
        2 => "exit_ready",
        3 => "closing",
        4 => "closed",
        5 => "entry_failed",
        6 => "exit_failed",
        7 => "stalled",
        _ => "unknown",
    }
}

async fn load_canonical_event_strategies(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    strategy: Option<&str>,
    market_id: Option<&str>,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT strategy
        FROM trade_events
        WHERE occurred_at >= $1
          AND occurred_at <= $2
          AND ($3::text IS NULL OR strategy = $3)
          AND ($4::text IS NULL OR market_id = $4)
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(strategy)
    .bind(market_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(strategy,)| strategy).collect())
}

async fn load_trade_event_summary_rows(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    strategy: Option<&str>,
    market_id: Option<&str>,
) -> Result<Vec<TradeFlowStrategySummary>, sqlx::Error> {
    let rows: Vec<StrategySummaryRow> = sqlx::query_as(
        r#"
        WITH filtered AS (
            SELECT *
            FROM trade_events
            WHERE occurred_at >= $1
              AND occurred_at <= $2
              AND ($3::text IS NULL OR strategy = $3)
              AND ($4::text IS NULL OR market_id = $4)
        ),
        position_scope AS (
            SELECT DISTINCT position_id
            FROM filtered
            WHERE position_id IS NOT NULL
        ),
        latest_positions AS (
            SELECT DISTINCT ON (te.position_id)
                te.position_id,
                te.strategy,
                te.source,
                COALESCE(
                    te.state_to,
                    CASE
                        WHEN te.event_type = 'position_failed' THEN 'entry_failed'
                        WHEN te.event_type IN ('position_closed', 'closed_via_resolution') THEN 'closed'
                        ELSE NULL
                    END
                ) AS lifecycle_stage
            FROM trade_events te
            JOIN position_scope ps ON ps.position_id = te.position_id
            WHERE te.occurred_at <= $2
            ORDER BY te.position_id, te.occurred_at DESC
        ),
        hold_stats AS (
            SELECT
                strategy,
                source,
                AVG((EXTRACT(EPOCH FROM (closed_at - opened_at)) / 3600.0)::double precision) AS avg_hold_hours
            FROM (
                SELECT
                    te.position_id,
                    MAX(te.strategy) AS strategy,
                    MAX(te.source) AS source,
                    MIN(te.occurred_at) FILTER (
                        WHERE te.state_to = 'open' OR te.event_type = 'position_open'
                    ) AS opened_at,
                    MIN(te.occurred_at) FILTER (
                        WHERE te.state_to = 'closed'
                           OR te.event_type IN ('position_closed', 'closed_via_resolution')
                    ) AS closed_at
                FROM trade_events te
                JOIN position_scope ps ON ps.position_id = te.position_id
                WHERE te.occurred_at <= $2
                GROUP BY te.position_id
            ) durations
            WHERE opened_at IS NOT NULL
              AND closed_at IS NOT NULL
              AND closed_at >= $1
            GROUP BY strategy, source
        ),
        signal_metrics AS (
            SELECT
                strategy,
                source,
                (strategy <> 'arb') AS supports_signal_history,
                COUNT(*) FILTER (WHERE event_type = 'signal_generated')::bigint AS generated_signals,
                COUNT(DISTINCT position_id) FILTER (
                    WHERE event_type = 'position_open' OR state_to = 'open'
                )::bigint AS executed_signals,
                COUNT(*) FILTER (WHERE event_type = 'signal_skipped')::bigint AS skipped_signals,
                COUNT(*) FILTER (WHERE event_type = 'signal_expired')::bigint AS expired_signals,
                COALESCE(SUM(realized_pnl) FILTER (
                    WHERE state_to = 'closed'
                       OR event_type IN ('position_closed', 'closed_via_resolution')
                ), 0) AS net_pnl
            FROM filtered
            GROUP BY strategy, source
        ),
        position_metrics AS (
            SELECT
                strategy,
                source,
                COUNT(DISTINCT position_id) FILTER (WHERE lifecycle_stage = 'open')::bigint AS open_positions,
                COUNT(DISTINCT position_id) FILTER (WHERE lifecycle_stage = 'exit_ready')::bigint AS exit_ready_positions,
                COUNT(DISTINCT position_id) FILTER (WHERE lifecycle_stage = 'closed')::bigint AS closed_positions,
                COUNT(DISTINCT position_id) FILTER (WHERE lifecycle_stage = 'entry_failed')::bigint AS entry_failed_positions,
                COUNT(DISTINCT position_id) FILTER (WHERE lifecycle_stage = 'exit_failed')::bigint AS exit_failed_positions
            FROM latest_positions
            GROUP BY strategy, source
        )
        SELECT
            COALESCE(sm.strategy, pm.strategy) AS strategy,
            COALESCE(sm.source, pm.source) AS source,
            COALESCE(sm.supports_signal_history, COALESCE(pm.strategy, '') <> 'arb') AS supports_signal_history,
            COALESCE(sm.generated_signals, 0) AS generated_signals,
            COALESCE(sm.executed_signals, 0) AS executed_signals,
            COALESCE(sm.skipped_signals, 0) AS skipped_signals,
            COALESCE(sm.expired_signals, 0) AS expired_signals,
            COALESCE(pm.open_positions, 0) AS open_positions,
            COALESCE(pm.exit_ready_positions, 0) AS exit_ready_positions,
            COALESCE(pm.closed_positions, 0) AS closed_positions,
            COALESCE(pm.entry_failed_positions, 0) AS entry_failed_positions,
            COALESCE(pm.exit_failed_positions, 0) AS exit_failed_positions,
            COALESCE(sm.net_pnl, 0) AS net_pnl,
            hs.avg_hold_hours
        FROM signal_metrics sm
        FULL OUTER JOIN position_metrics pm
            ON pm.strategy = sm.strategy
           AND pm.source = sm.source
        LEFT JOIN hold_stats hs
            ON hs.strategy = COALESCE(sm.strategy, pm.strategy)
           AND hs.source = COALESCE(sm.source, pm.source)
        ORDER BY COALESCE(sm.strategy, pm.strategy)
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(strategy)
    .bind(market_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| TradeFlowStrategySummary {
            strategy: row.strategy,
            source: row.source,
            supports_signal_history: row.supports_signal_history,
            generated_signals: row.generated_signals,
            executed_signals: row.executed_signals,
            skipped_signals: row.skipped_signals,
            expired_signals: row.expired_signals,
            open_positions: row.open_positions,
            exit_ready_positions: row.exit_ready_positions,
            closed_positions: row.closed_positions,
            entry_failed_positions: row.entry_failed_positions,
            exit_failed_positions: row.exit_failed_positions,
            net_pnl: row.net_pnl,
            avg_hold_hours: row.avg_hold_hours,
        })
        .collect())
}

async fn load_derived_summary_rows(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    strategy: Option<&str>,
    market_id: Option<&str>,
) -> Result<Vec<TradeFlowStrategySummary>, sqlx::Error> {
    let mut rows = Vec::new();

    let quant_rows: Vec<StrategySummaryRow> = sqlx::query_as(
        r#"
        SELECT
            qs.kind AS strategy,
            'quant'::text AS source,
            TRUE AS supports_signal_history,
            COUNT(*)::bigint AS generated_signals,
            COUNT(*) FILTER (WHERE qs.execution_status = 'executed')::bigint AS executed_signals,
            COUNT(*) FILTER (WHERE qs.execution_status = 'skipped')::bigint AS skipped_signals,
            COUNT(*) FILTER (WHERE qs.execution_status = 'expired')::bigint AS expired_signals,
            COUNT(*) FILTER (WHERE p.state = 1)::bigint AS open_positions,
            COUNT(*) FILTER (WHERE p.state = 2)::bigint AS exit_ready_positions,
            COUNT(*) FILTER (WHERE p.state = 4)::bigint AS closed_positions,
            COUNT(*) FILTER (WHERE p.state = 5)::bigint AS entry_failed_positions,
            COUNT(*) FILTER (WHERE p.state = 6)::bigint AS exit_failed_positions,
            COALESCE(SUM(p.realized_pnl) FILTER (WHERE p.state = 4), 0) AS net_pnl,
            AVG((EXTRACT(EPOCH FROM (p.exit_timestamp - p.entry_timestamp)) / 3600.0)::double precision)
                FILTER (WHERE p.state = 4 AND p.exit_timestamp IS NOT NULL) AS avg_hold_hours
        FROM quant_signals qs
        LEFT JOIN positions p ON p.id = qs.position_id
        WHERE qs.generated_at >= $1
          AND qs.generated_at <= $2
          AND ($3::text IS NULL OR qs.kind = $3)
          AND ($4::text IS NULL OR qs.condition_id = $4)
        GROUP BY qs.kind
        ORDER BY qs.kind
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(strategy)
    .bind(market_id)
    .fetch_all(pool)
    .await?;

    rows.extend(quant_rows.into_iter().map(|row| TradeFlowStrategySummary {
        strategy: row.strategy,
        source: row.source,
        supports_signal_history: row.supports_signal_history,
        generated_signals: row.generated_signals,
        executed_signals: row.executed_signals,
        skipped_signals: row.skipped_signals,
        expired_signals: row.expired_signals,
        open_positions: row.open_positions,
        exit_ready_positions: row.exit_ready_positions,
        closed_positions: row.closed_positions,
        entry_failed_positions: row.entry_failed_positions,
        exit_failed_positions: row.exit_failed_positions,
        net_pnl: row.net_pnl,
        avg_hold_hours: row.avg_hold_hours,
    }));

    if strategy.is_none() || strategy == Some("arb") {
        let arb_rows: Vec<StrategySummaryRow> = sqlx::query_as(
            r#"
            SELECT
                'arb'::text AS strategy,
                'arb'::text AS source,
                FALSE AS supports_signal_history,
                0::bigint AS generated_signals,
                COUNT(*)::bigint AS executed_signals,
                0::bigint AS skipped_signals,
                0::bigint AS expired_signals,
                COUNT(*) FILTER (WHERE state = 1)::bigint AS open_positions,
                COUNT(*) FILTER (WHERE state = 2)::bigint AS exit_ready_positions,
                COUNT(*) FILTER (WHERE state = 4)::bigint AS closed_positions,
                COUNT(*) FILTER (WHERE state = 5)::bigint AS entry_failed_positions,
                COUNT(*) FILTER (WHERE state = 6)::bigint AS exit_failed_positions,
                COALESCE(SUM(realized_pnl) FILTER (WHERE state = 4), 0) AS net_pnl,
                AVG((EXTRACT(EPOCH FROM (exit_timestamp - entry_timestamp)) / 3600.0)::double precision)
                    FILTER (WHERE state = 4 AND exit_timestamp IS NOT NULL) AS avg_hold_hours
            FROM positions
            WHERE source = 1
              AND entry_timestamp >= $1
              AND entry_timestamp <= $2
              AND ($3::text IS NULL OR market_id = $3)
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(market_id)
        .fetch_all(pool)
        .await?;

        rows.extend(arb_rows.into_iter().map(|row| TradeFlowStrategySummary {
            strategy: row.strategy,
            source: row.source,
            supports_signal_history: row.supports_signal_history,
            generated_signals: row.generated_signals,
            executed_signals: row.executed_signals,
            skipped_signals: row.skipped_signals,
            expired_signals: row.expired_signals,
            open_positions: row.open_positions,
            exit_ready_positions: row.exit_ready_positions,
            closed_positions: row.closed_positions,
            entry_failed_positions: row.entry_failed_positions,
            exit_failed_positions: row.exit_failed_positions,
            net_pnl: row.net_pnl,
            avg_hold_hours: row.avg_hold_hours,
        }));
    }

    Ok(rows)
}

async fn load_trade_event_journeys(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    strategy: Option<&str>,
    market_id: Option<&str>,
    limit: i64,
) -> Result<Vec<TradeJourneyResponse>, sqlx::Error> {
    let rows: Vec<TradeJourneyRow> = sqlx::query_as(
        r#"
        WITH scope AS (
            SELECT DISTINCT
                strategy,
                COALESCE(signal_id::text, position_id::text) AS journey_key
            FROM trade_events
            WHERE occurred_at >= $1
              AND occurred_at <= $2
              AND ($3::text IS NULL OR strategy = $3)
              AND ($4::text IS NULL OR market_id = $4)
              AND (signal_id IS NOT NULL OR position_id IS NOT NULL)
        ),
        history AS (
            SELECT
                te.*,
                COALESCE(te.signal_id::text, te.position_id::text) AS journey_key
            FROM trade_events te
            JOIN scope s
              ON s.strategy = te.strategy
             AND s.journey_key = COALESCE(te.signal_id::text, te.position_id::text)
            WHERE te.occurred_at <= $2
        ),
        latest AS (
            SELECT DISTINCT ON (strategy, journey_key)
                strategy,
                journey_key,
                source,
                market_id,
                signal_id,
                position_id,
                event_type,
                state_to,
                reason,
                direction,
                confidence,
                realized_pnl,
                unrealized_pnl
            FROM history
            ORDER BY strategy, journey_key, occurred_at DESC
        ),
        aggregates AS (
            SELECT
                strategy,
                journey_key,
                MAX(source) AS source,
                MAX(market_id) AS market_id,
                MAX(signal_id) AS signal_id,
                MAX(position_id) AS position_id,
                MAX(direction) FILTER (WHERE direction IS NOT NULL) AS direction,
                MAX(confidence) FILTER (WHERE confidence IS NOT NULL) AS confidence,
                MIN(occurred_at) FILTER (WHERE event_type = 'signal_generated') AS signal_generated_at,
                MIN(occurred_at) FILTER (
                    WHERE state_to = 'open' OR event_type = 'position_open'
                ) AS opened_at,
                MIN(occurred_at) FILTER (
                    WHERE state_to = 'closed'
                       OR event_type IN ('position_closed', 'closed_via_resolution')
                ) AS closed_at
            FROM history
            GROUP BY strategy, journey_key
        )
        SELECT
            a.strategy,
            a.source,
            (a.strategy <> 'arb') AS supports_signal_history,
            COALESCE(
                l.state_to,
                CASE
                    WHEN l.event_type = 'signal_skipped' THEN 'skipped'
                    WHEN l.event_type = 'signal_expired' THEN 'expired'
                    WHEN l.event_type = 'position_failed' THEN 'entry_failed'
                    WHEN l.event_type IN ('position_closed', 'closed_via_resolution') THEN 'closed'
                    ELSE l.event_type
                END
            ) AS lifecycle_stage,
            l.event_type AS execution_status,
            CASE
                WHEN a.position_id IS NULL THEN NULL
                ELSE COALESCE(
                    l.state_to,
                    CASE
                        WHEN l.event_type = 'position_failed' THEN 'entry_failed'
                        WHEN l.event_type IN ('position_closed', 'closed_via_resolution') THEN 'closed'
                        ELSE NULL
                    END
                )
            END AS position_state,
            a.market_id,
            a.signal_id,
            a.position_id,
            a.direction,
            a.confidence,
            l.reason AS skip_reason,
            a.signal_generated_at,
            a.opened_at,
            a.closed_at,
            l.realized_pnl,
            l.unrealized_pnl,
            CASE
                WHEN a.opened_at IS NULL THEN NULL
                ELSE (EXTRACT(EPOCH FROM (COALESCE(a.closed_at, $2) - a.opened_at)) / 3600.0)::double precision
            END AS hold_hours,
            FALSE AS synthetic_history
        FROM aggregates a
        JOIN latest l
          ON l.strategy = a.strategy
         AND l.journey_key = a.journey_key
        ORDER BY COALESCE(a.signal_generated_at, a.opened_at, a.closed_at) DESC NULLS LAST
        LIMIT $5
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(strategy)
    .bind(market_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| TradeJourneyResponse {
            strategy: row.strategy,
            source: row.source,
            supports_signal_history: row.supports_signal_history,
            lifecycle_stage: row.lifecycle_stage,
            execution_status: row.execution_status,
            position_state: row.position_state,
            market_id: row.market_id,
            signal_id: row.signal_id,
            position_id: row.position_id,
            direction: row.direction,
            confidence: row.confidence,
            skip_reason: row.skip_reason,
            signal_generated_at: row.signal_generated_at,
            opened_at: row.opened_at,
            closed_at: row.closed_at,
            realized_pnl: row.realized_pnl,
            unrealized_pnl: row.unrealized_pnl,
            hold_hours: row.hold_hours,
            synthetic_history: row.synthetic_history,
        })
        .collect())
}

async fn load_derived_journeys(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    strategy: Option<&str>,
    market_id: Option<&str>,
    limit: i64,
) -> Result<Vec<TradeJourneyResponse>, sqlx::Error> {
    let mut journeys = Vec::new();

    let quant_rows: Vec<TradeJourneyRow> = sqlx::query_as(
        r#"
        SELECT
            qs.kind AS strategy,
            'quant'::text AS source,
            TRUE AS supports_signal_history,
            CASE
                WHEN qs.execution_status = 'skipped' THEN 'skipped'
                WHEN qs.execution_status = 'expired' THEN 'expired'
                WHEN p.state IS NOT NULL THEN
                    CASE p.state
                        WHEN 0 THEN 'pending'
                        WHEN 1 THEN 'open'
                        WHEN 2 THEN 'exit_ready'
                        WHEN 3 THEN 'closing'
                        WHEN 4 THEN 'closed'
                        WHEN 5 THEN 'entry_failed'
                        WHEN 6 THEN 'exit_failed'
                        WHEN 7 THEN 'stalled'
                        ELSE 'unknown'
                    END
                ELSE COALESCE(qs.execution_status, 'pending')
            END AS lifecycle_stage,
            qs.execution_status,
            CASE
                WHEN p.state IS NULL THEN NULL
                ELSE
                    CASE p.state
                        WHEN 0 THEN 'pending'
                        WHEN 1 THEN 'open'
                        WHEN 2 THEN 'exit_ready'
                        WHEN 3 THEN 'closing'
                        WHEN 4 THEN 'closed'
                        WHEN 5 THEN 'entry_failed'
                        WHEN 6 THEN 'exit_failed'
                        WHEN 7 THEN 'stalled'
                        ELSE 'unknown'
                    END
            END AS position_state,
            qs.condition_id AS market_id,
            qs.id AS signal_id,
            qs.position_id,
            qs.direction,
            qs.confidence,
            qs.skip_reason,
            qs.generated_at AS signal_generated_at,
            p.entry_timestamp AS opened_at,
            p.exit_timestamp AS closed_at,
            p.realized_pnl,
            p.unrealized_pnl,
            (EXTRACT(EPOCH FROM (COALESCE(p.exit_timestamp, NOW()) - p.entry_timestamp)) / 3600.0)::double precision AS hold_hours,
            FALSE AS synthetic_history
        FROM quant_signals qs
        LEFT JOIN positions p ON p.id = qs.position_id
        WHERE qs.generated_at >= $1
          AND qs.generated_at <= $2
          AND ($3::text IS NULL OR qs.kind = $3)
          AND ($4::text IS NULL OR qs.condition_id = $4)
        ORDER BY qs.generated_at DESC
        LIMIT $5
        "#,
    )
    .bind(from)
    .bind(to)
    .bind(strategy)
    .bind(market_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    journeys.extend(quant_rows.into_iter().map(|row| TradeJourneyResponse {
        strategy: row.strategy,
        source: row.source,
        supports_signal_history: row.supports_signal_history,
        lifecycle_stage: row.lifecycle_stage,
        execution_status: row.execution_status,
        position_state: row.position_state,
        market_id: row.market_id,
        signal_id: row.signal_id,
        position_id: row.position_id,
        direction: row.direction,
        confidence: row.confidence,
        skip_reason: row.skip_reason,
        signal_generated_at: row.signal_generated_at,
        opened_at: row.opened_at,
        closed_at: row.closed_at,
        realized_pnl: row.realized_pnl,
        unrealized_pnl: row.unrealized_pnl,
        hold_hours: row.hold_hours,
        synthetic_history: row.synthetic_history,
    }));

    if strategy.is_none() || strategy == Some("arb") {
        let arb_rows: Vec<TradeJourneyRow> = sqlx::query_as(
            r#"
            SELECT
                'arb'::text AS strategy,
                'arb'::text AS source,
                FALSE AS supports_signal_history,
                CASE state
                    WHEN 0 THEN 'pending'
                    WHEN 1 THEN 'open'
                    WHEN 2 THEN 'exit_ready'
                    WHEN 3 THEN 'closing'
                    WHEN 4 THEN 'closed'
                    WHEN 5 THEN 'entry_failed'
                    WHEN 6 THEN 'exit_failed'
                    WHEN 7 THEN 'stalled'
                    ELSE 'unknown'
                END AS lifecycle_stage,
                NULL::text AS execution_status,
                CASE state
                    WHEN 0 THEN 'pending'
                    WHEN 1 THEN 'open'
                    WHEN 2 THEN 'exit_ready'
                    WHEN 3 THEN 'closing'
                    WHEN 4 THEN 'closed'
                    WHEN 5 THEN 'entry_failed'
                    WHEN 6 THEN 'exit_failed'
                    WHEN 7 THEN 'stalled'
                    ELSE 'unknown'
                END AS position_state,
                market_id,
                NULL::uuid AS signal_id,
                id AS position_id,
                'both'::text AS direction,
                NULL::double precision AS confidence,
                NULL::text AS skip_reason,
                NULL::timestamptz AS signal_generated_at,
                entry_timestamp AS opened_at,
                exit_timestamp AS closed_at,
                realized_pnl,
                unrealized_pnl,
                (EXTRACT(EPOCH FROM (COALESCE(exit_timestamp, NOW()) - entry_timestamp)) / 3600.0)::double precision AS hold_hours,
                TRUE AS synthetic_history
            FROM positions
            WHERE source = 1
              AND entry_timestamp >= $1
              AND entry_timestamp <= $2
              AND ($3::text IS NULL OR market_id = $3)
            ORDER BY entry_timestamp DESC
            LIMIT $4
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(market_id)
        .bind(limit)
        .fetch_all(pool)
        .await?;

        journeys.extend(arb_rows.into_iter().map(|row| TradeJourneyResponse {
            strategy: row.strategy,
            source: row.source,
            supports_signal_history: row.supports_signal_history,
            lifecycle_stage: row.lifecycle_stage,
            execution_status: row.execution_status,
            position_state: row.position_state,
            market_id: row.market_id,
            signal_id: row.signal_id,
            position_id: row.position_id,
            direction: row.direction,
            confidence: row.confidence,
            skip_reason: row.skip_reason,
            signal_generated_at: row.signal_generated_at,
            opened_at: row.opened_at,
            closed_at: row.closed_at,
            realized_pnl: row.realized_pnl,
            unrealized_pnl: row.unrealized_pnl,
            hold_hours: row.hold_hours,
            synthetic_history: row.synthetic_history,
        }));
    }

    Ok(journeys)
}

fn merge_strategy_summaries(
    canonical: Vec<TradeFlowStrategySummary>,
    fallback: Vec<TradeFlowStrategySummary>,
) -> Vec<TradeFlowStrategySummary> {
    let mut covered: HashSet<String> = canonical.iter().map(|row| row.strategy.clone()).collect();
    let mut merged = canonical;
    for row in fallback {
        if covered.insert(row.strategy.clone()) {
            merged.push(row);
        }
    }
    merged.sort_by(|a, b| a.strategy.cmp(&b.strategy));
    merged
}

fn merge_journeys(
    mut canonical: Vec<TradeJourneyResponse>,
    fallback: Vec<TradeJourneyResponse>,
    from: DateTime<Utc>,
    limit: i64,
) -> Vec<TradeJourneyResponse> {
    let covered: HashSet<String> = canonical.iter().map(|row| row.strategy.clone()).collect();
    canonical.extend(
        fallback
            .into_iter()
            .filter(|row| !covered.contains(&row.strategy)),
    );
    canonical.sort_by(|a, b| {
        let a_ts = a
            .signal_generated_at
            .or(a.opened_at)
            .or(a.closed_at)
            .unwrap_or(from);
        let b_ts = b
            .signal_generated_at
            .or(b.opened_at)
            .or(b.closed_at)
            .unwrap_or(from);
        b_ts.cmp(&a_ts)
    });
    canonical.truncate(limit as usize);
    canonical
}

fn build_summary(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    strategies: Vec<TradeFlowStrategySummary>,
) -> TradeFlowSummaryResponse {
    TradeFlowSummaryResponse {
        from,
        to,
        total_generated_signals: strategies.iter().map(|s| s.generated_signals).sum(),
        total_executed_signals: strategies.iter().map(|s| s.executed_signals).sum(),
        total_skipped_signals: strategies.iter().map(|s| s.skipped_signals).sum(),
        total_expired_signals: strategies.iter().map(|s| s.expired_signals).sum(),
        total_open_positions: strategies.iter().map(|s| s.open_positions).sum(),
        total_exit_ready_positions: strategies.iter().map(|s| s.exit_ready_positions).sum(),
        total_closed_positions: strategies.iter().map(|s| s.closed_positions).sum(),
        total_entry_failed_positions: strategies.iter().map(|s| s.entry_failed_positions).sum(),
        total_exit_failed_positions: strategies.iter().map(|s| s.exit_failed_positions).sum(),
        total_realized_pnl: strategies
            .iter()
            .fold(Decimal::ZERO, |acc, s| acc + s.net_pnl),
        strategies,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/trade-flow/summary",
    params(TradeFlowQuery),
    responses(
        (status = 200, description = "Trade flow summary", body = TradeFlowSummaryResponse),
        (status = 500, description = "Internal server error"),
    ),
    tag = "trade_flow"
)]
pub async fn get_trade_flow_summary(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TradeFlowQuery>,
) -> ApiResult<Json<TradeFlowSummaryResponse>> {
    let (from, to, _) = effective_window(&query);
    let canonical_strategies =
        load_canonical_event_strategies(&state.pool, from, to, query.strategy.as_deref(), None)
            .await
            .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?;
    let canonical = if canonical_strategies.is_empty() {
        Vec::new()
    } else {
        load_trade_event_summary_rows(&state.pool, from, to, query.strategy.as_deref(), None)
            .await
            .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?
    };
    let fallback = if query
        .strategy
        .as_deref()
        .map(|strategy| !canonical_strategies.contains(strategy))
        .unwrap_or(true)
    {
        load_derived_summary_rows(&state.pool, from, to, query.strategy.as_deref(), None)
            .await
            .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?
    } else {
        Vec::new()
    };

    Ok(Json(build_summary(
        from,
        to,
        merge_strategy_summaries(canonical, fallback),
    )))
}

#[utoipa::path(
    get,
    path = "/api/v1/trade-flow/journeys",
    params(TradeFlowQuery),
    responses(
        (status = 200, description = "Trade flow journeys", body = Vec<TradeJourneyResponse>),
        (status = 500, description = "Internal server error"),
    ),
    tag = "trade_flow"
)]
pub async fn get_trade_flow_journeys(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TradeFlowQuery>,
) -> ApiResult<Json<Vec<TradeJourneyResponse>>> {
    let (from, to, limit) = effective_window(&query);
    let canonical = load_trade_event_journeys(
        &state.pool,
        from,
        to,
        query.strategy.as_deref(),
        None,
        limit,
    )
    .await
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?;
    let canonical_strategies: HashSet<String> =
        canonical.iter().map(|row| row.strategy.clone()).collect();
    let fallback = if query
        .strategy
        .as_deref()
        .map(|strategy| !canonical_strategies.contains(strategy))
        .unwrap_or(true)
    {
        load_derived_journeys(
            &state.pool,
            from,
            to,
            query.strategy.as_deref(),
            None,
            limit,
        )
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?
    } else {
        Vec::new()
    };

    Ok(Json(merge_journeys(canonical, fallback, from, limit)))
}

#[utoipa::path(
    get,
    path = "/api/v1/trade-flow/markets/{market_id}",
    params(
        ("market_id" = String, Path, description = "Market identifier"),
        TradeFlowQuery
    ),
    responses(
        (status = 200, description = "Trade flow for a single market", body = TradeFlowMarketResponse),
        (status = 500, description = "Internal server error"),
    ),
    tag = "trade_flow"
)]
pub async fn get_market_trade_flow(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<String>,
    Query(query): Query<TradeFlowQuery>,
) -> ApiResult<Json<TradeFlowMarketResponse>> {
    let (from, to, limit) = effective_window(&query);
    let canonical_summary_strategies = load_canonical_event_strategies(
        &state.pool,
        from,
        to,
        query.strategy.as_deref(),
        Some(&market_id),
    )
    .await
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?;
    let canonical_summary = if canonical_summary_strategies.is_empty() {
        Vec::new()
    } else {
        load_trade_event_summary_rows(
            &state.pool,
            from,
            to,
            query.strategy.as_deref(),
            Some(&market_id),
        )
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?
    };
    let fallback_summary = if query
        .strategy
        .as_deref()
        .map(|strategy| !canonical_summary_strategies.contains(strategy))
        .unwrap_or(true)
    {
        load_derived_summary_rows(
            &state.pool,
            from,
            to,
            query.strategy.as_deref(),
            Some(&market_id),
        )
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?
    } else {
        Vec::new()
    };

    let canonical_journeys = load_trade_event_journeys(
        &state.pool,
        from,
        to,
        query.strategy.as_deref(),
        Some(&market_id),
        limit,
    )
    .await
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?;
    let canonical_journey_strategies: HashSet<String> = canonical_journeys
        .iter()
        .map(|row| row.strategy.clone())
        .collect();
    let fallback_journeys = if query
        .strategy
        .as_deref()
        .map(|strategy| !canonical_journey_strategies.contains(strategy))
        .unwrap_or(true)
    {
        load_derived_journeys(
            &state.pool,
            from,
            to,
            query.strategy.as_deref(),
            Some(&market_id),
            limit,
        )
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?
    } else {
        Vec::new()
    };

    Ok(Json(TradeFlowMarketResponse {
        market_id,
        summary: build_summary(
            from,
            to,
            merge_strategy_summaries(canonical_summary, fallback_summary),
        ),
        journeys: merge_journeys(canonical_journeys, fallback_journeys, from, limit),
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/trade-flow/strategies/arb/scorecard",
    params(TradeFlowQuery),
    responses(
        (status = 200, description = "Arbitrage market scorecard", body = ArbMarketScorecardResponse),
        (status = 500, description = "Internal server error"),
    ),
    tag = "trade_flow"
)]
pub async fn get_arb_market_scorecard(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TradeFlowQuery>,
) -> ApiResult<Json<ArbMarketScorecardResponse>> {
    let (from, to, limit) = effective_window(&query);
    if matches!(query.strategy.as_deref(), Some(strategy) if strategy != "arb") {
        return Ok(Json(ArbMarketScorecardResponse {
            from,
            to,
            markets: Vec::new(),
        }));
    }
    let markets = load_arb_market_scorecard_rows(&state.pool, from, to, None, limit)
        .await
        .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {e}")))?;

    Ok(Json(ArbMarketScorecardResponse { from, to, markets }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_name_maps_known_values() {
        assert_eq!(state_name(0), "pending");
        assert_eq!(state_name(4), "closed");
        assert_eq!(state_name(99), "unknown");
    }

    #[test]
    fn merge_strategy_summaries_prefers_canonical_rows() {
        let merged = merge_strategy_summaries(
            vec![TradeFlowStrategySummary {
                strategy: "flow".to_string(),
                source: "quant".to_string(),
                supports_signal_history: true,
                generated_signals: 4,
                executed_signals: 3,
                skipped_signals: 1,
                expired_signals: 0,
                open_positions: 1,
                exit_ready_positions: 0,
                closed_positions: 2,
                entry_failed_positions: 0,
                exit_failed_positions: 0,
                net_pnl: Decimal::new(12, 0),
                avg_hold_hours: Some(1.5),
            }],
            vec![
                TradeFlowStrategySummary {
                    strategy: "flow".to_string(),
                    source: "quant".to_string(),
                    supports_signal_history: true,
                    generated_signals: 99,
                    executed_signals: 99,
                    skipped_signals: 0,
                    expired_signals: 0,
                    open_positions: 0,
                    exit_ready_positions: 0,
                    closed_positions: 0,
                    entry_failed_positions: 0,
                    exit_failed_positions: 0,
                    net_pnl: Decimal::ZERO,
                    avg_hold_hours: None,
                },
                TradeFlowStrategySummary {
                    strategy: "arb".to_string(),
                    source: "arb".to_string(),
                    supports_signal_history: false,
                    generated_signals: 0,
                    executed_signals: 1,
                    skipped_signals: 0,
                    expired_signals: 0,
                    open_positions: 1,
                    exit_ready_positions: 0,
                    closed_positions: 0,
                    entry_failed_positions: 0,
                    exit_failed_positions: 0,
                    net_pnl: Decimal::ZERO,
                    avg_hold_hours: None,
                },
            ],
        );

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].strategy, "flow");
        assert_eq!(merged[1].generated_signals, 4);
    }
}
