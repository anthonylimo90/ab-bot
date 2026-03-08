//! Strategy health API handlers.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Debug, Deserialize, IntoParams)]
pub struct StrategyHealthQuery {
    /// Rolling window in days.
    pub period_days: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StrategyHealthItemResponse {
    pub strategy: String,
    pub period_end: DateTime<Utc>,
    pub period_days: i32,
    pub generated_signals: i32,
    pub executed_signals: i32,
    pub skipped_signals: i32,
    pub expired_signals: i32,
    pub open_positions: i32,
    pub exit_ready_positions: i32,
    pub closed_positions: i32,
    pub entry_failed_positions: i32,
    pub exit_failed_positions: i32,
    pub total_expected_edge: Decimal,
    pub total_observed_edge: Decimal,
    pub total_realized_pnl: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_hold_hours: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_capture_ratio: Option<Decimal>,
    pub recommendation: String,
    pub rationale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_backtest_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_backtest_return_pct: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_backtest_created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StrategyHealthResponse {
    pub period_days: i32,
    pub snapshots: Vec<StrategyHealthItemResponse>,
}

#[derive(Debug, FromRow)]
struct StrategyHealthRow {
    strategy: String,
    period_end: DateTime<Utc>,
    period_days: i32,
    generated_signals: i32,
    executed_signals: i32,
    skipped_signals: i32,
    expired_signals: i32,
    open_positions: i32,
    exit_ready_positions: i32,
    closed_positions: i32,
    entry_failed_positions: i32,
    exit_failed_positions: i32,
    total_expected_edge: Decimal,
    total_observed_edge: Decimal,
    total_realized_pnl: Decimal,
    avg_hold_hours: Option<f64>,
    skip_rate: Option<f64>,
    failure_rate: Option<f64>,
    edge_capture_ratio: Option<Decimal>,
    recommendation: String,
    rationale: String,
    latest_backtest_id: Option<Uuid>,
    latest_backtest_return_pct: Option<Decimal>,
    latest_backtest_created_at: Option<DateTime<Utc>>,
}

/// Get latest strategy-health snapshots.
#[utoipa::path(
    get,
    path = "/api/v1/signals/health",
    tag = "signals",
    params(StrategyHealthQuery),
    responses(
        (status = 200, description = "Strategy health snapshots", body = StrategyHealthResponse)
    )
)]
pub async fn get_strategy_health(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StrategyHealthQuery>,
) -> ApiResult<Json<StrategyHealthResponse>> {
    let period_days = query.period_days.unwrap_or(7).clamp(1, 90);
    let rows: Vec<StrategyHealthRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (strategy)
            strategy, period_end, period_days, generated_signals, executed_signals,
            skipped_signals, expired_signals, open_positions, exit_ready_positions,
            closed_positions, entry_failed_positions, exit_failed_positions,
            total_expected_edge, total_observed_edge, total_realized_pnl,
            avg_hold_hours, skip_rate, failure_rate, edge_capture_ratio,
            recommendation, rationale, latest_backtest_id,
            latest_backtest_return_pct, latest_backtest_created_at
        FROM strategy_health_snapshots
        WHERE period_days = $1
        ORDER BY strategy, period_end DESC
        "#,
    )
    .bind(period_days)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;

    let snapshots = rows
        .into_iter()
        .map(|row| StrategyHealthItemResponse {
            strategy: row.strategy,
            period_end: row.period_end,
            period_days: row.period_days,
            generated_signals: row.generated_signals,
            executed_signals: row.executed_signals,
            skipped_signals: row.skipped_signals,
            expired_signals: row.expired_signals,
            open_positions: row.open_positions,
            exit_ready_positions: row.exit_ready_positions,
            closed_positions: row.closed_positions,
            entry_failed_positions: row.entry_failed_positions,
            exit_failed_positions: row.exit_failed_positions,
            total_expected_edge: row.total_expected_edge,
            total_observed_edge: row.total_observed_edge,
            total_realized_pnl: row.total_realized_pnl,
            avg_hold_hours: row.avg_hold_hours,
            skip_rate: row.skip_rate,
            failure_rate: row.failure_rate,
            edge_capture_ratio: row.edge_capture_ratio,
            recommendation: row.recommendation,
            rationale: row.rationale,
            latest_backtest_id: row.latest_backtest_id,
            latest_backtest_return_pct: row.latest_backtest_return_pct,
            latest_backtest_created_at: row.latest_backtest_created_at,
        })
        .collect();

    Ok(Json(StrategyHealthResponse {
        period_days,
        snapshots,
    }))
}
