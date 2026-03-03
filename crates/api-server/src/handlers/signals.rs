//! Signal-related API handlers.
//!
//! Provides endpoints for querying market flow features,
//! quant signal performance, and recent signals.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiResult;
use crate::state::AppState;

/// Query parameters for the flow features endpoint.
#[derive(Debug, Deserialize, IntoParams)]
pub struct FlowQuery {
    /// Polymarket condition ID.
    pub condition_id: String,
    /// Time window in minutes (default: 60).
    pub window_minutes: Option<i32>,
}

/// Flow feature response for a market.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FlowFeatureResponse {
    pub condition_id: String,
    pub window_end: DateTime<Utc>,
    pub window_minutes: i32,
    pub buy_volume: Decimal,
    pub sell_volume: Decimal,
    pub net_flow: Decimal,
    pub imbalance_ratio: Decimal,
    pub unique_buyers: i32,
    pub unique_sellers: i32,
    pub smart_money_flow: Decimal,
    pub trade_count: i32,
}

/// Market metadata response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MarketMetadataResponse {
    pub condition_id: String,
    pub question: String,
    pub category: Option<String>,
    pub tags: Vec<String>,
    pub end_date: Option<DateTime<Utc>>,
    pub volume: Decimal,
    pub liquidity: Decimal,
    pub active: bool,
    pub fetched_at: DateTime<Utc>,
}

/// GET /api/v1/signals/flow — Get flow features for a market.
#[utoipa::path(
    get,
    path = "/api/v1/signals/flow",
    tag = "signals",
    params(FlowQuery),
    responses(
        (status = 200, description = "Flow features for the market", body = Vec<FlowFeatureResponse>),
        (status = 404, description = "No flow data found")
    )
)]
pub async fn get_flow_features(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FlowQuery>,
) -> ApiResult<Json<Vec<FlowFeatureResponse>>> {
    let window = params.window_minutes.unwrap_or(60);

    let rows = sqlx::query_as::<_, FlowRow>(
        r#"
        SELECT condition_id, window_end, window_minutes,
               buy_volume, sell_volume, net_flow, imbalance_ratio,
               unique_buyers, unique_sellers, smart_money_flow, trade_count
        FROM market_flow_features
        WHERE condition_id = $1
          AND window_minutes = $2
        ORDER BY window_end DESC
        LIMIT 24
        "#,
    )
    .bind(&params.condition_id)
    .bind(window)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {}", e)))?;

    let response: Vec<FlowFeatureResponse> = rows
        .into_iter()
        .map(|r| FlowFeatureResponse {
            condition_id: r.condition_id,
            window_end: r.window_end,
            window_minutes: r.window_minutes,
            buy_volume: r.buy_volume,
            sell_volume: r.sell_volume,
            net_flow: r.net_flow,
            imbalance_ratio: r.imbalance_ratio,
            unique_buyers: r.unique_buyers,
            unique_sellers: r.unique_sellers,
            smart_money_flow: r.smart_money_flow,
            trade_count: r.trade_count,
        })
        .collect();

    Ok(Json(response))
}

/// Query parameters for market metadata.
#[derive(Debug, Deserialize, IntoParams)]
pub struct MetadataQuery {
    /// Filter by category (optional).
    pub category: Option<String>,
    /// Only active markets (default: true).
    pub active: Option<bool>,
    /// Limit results (default: 50).
    pub limit: Option<i64>,
}

/// GET /api/v1/signals/metadata — Get market metadata.
#[utoipa::path(
    get,
    path = "/api/v1/signals/metadata",
    tag = "signals",
    params(MetadataQuery),
    responses(
        (status = 200, description = "Market metadata list", body = Vec<MarketMetadataResponse>)
    )
)]
pub async fn get_market_metadata(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MetadataQuery>,
) -> ApiResult<Json<Vec<MarketMetadataResponse>>> {
    let active = params.active.unwrap_or(true);
    let limit = params.limit.unwrap_or(50).min(200);

    let rows = if let Some(ref category) = params.category {
        sqlx::query_as::<_, MetadataRow>(
            r#"
            SELECT condition_id, question, category, tags, end_date,
                   volume, liquidity, active, fetched_at
            FROM market_metadata
            WHERE active = $1
              AND category = $2
            ORDER BY volume DESC
            LIMIT $3
            "#,
        )
        .bind(active)
        .bind(category)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, MetadataRow>(
            r#"
            SELECT condition_id, question, category, tags, end_date,
                   volume, liquidity, active, fetched_at
            FROM market_metadata
            WHERE active = $1
            ORDER BY volume DESC
            LIMIT $2
            "#,
        )
        .bind(active)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    }
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {}", e)))?;

    let response: Vec<MarketMetadataResponse> = rows
        .into_iter()
        .map(|r| MarketMetadataResponse {
            condition_id: r.condition_id,
            question: r.question,
            category: r.category,
            tags: r.tags,
            end_date: r.end_date,
            volume: r.volume,
            liquidity: r.liquidity,
            active: r.active,
            fetched_at: r.fetched_at,
        })
        .collect();

    Ok(Json(response))
}

// ── Phase 4: Performance + Recent Signals ──

/// Query parameters for recent quant signals.
#[derive(Debug, Deserialize, IntoParams)]
pub struct RecentSignalsQuery {
    /// Filter by signal kind (optional): flow, cross_market, mean_reversion, resolution_proximity.
    pub kind: Option<String>,
    /// Maximum number of results (default: 50, max: 200).
    pub limit: Option<i64>,
}

/// Response for a recent quant signal.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RecentSignalResponse {
    pub id: String,
    pub kind: String,
    pub condition_id: String,
    pub direction: String,
    pub confidence: f64,
    pub size_usd: Option<Decimal>,
    pub execution_status: Option<String>,
    pub skip_reason: Option<String>,
    pub position_id: Option<String>,
    pub generated_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

/// GET /api/v1/signals/recent — Get recent quant signals.
#[utoipa::path(
    get,
    path = "/api/v1/signals/recent",
    tag = "signals",
    params(RecentSignalsQuery),
    responses(
        (status = 200, description = "Recent quant signals", body = Vec<RecentSignalResponse>)
    )
)]
pub async fn get_recent_signals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecentSignalsQuery>,
) -> ApiResult<Json<Vec<RecentSignalResponse>>> {
    let limit = params.limit.unwrap_or(50).min(200);

    let rows = if let Some(ref kind) = params.kind {
        sqlx::query_as::<_, RecentSignalRow>(
            r#"
            SELECT id, kind, condition_id, direction, confidence, size_usd,
                   execution_status, skip_reason, position_id, generated_at, metadata
            FROM quant_signals
            WHERE kind = $1
            ORDER BY generated_at DESC
            LIMIT $2
            "#,
        )
        .bind(kind)
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as::<_, RecentSignalRow>(
            r#"
            SELECT id, kind, condition_id, direction, confidence, size_usd,
                   execution_status, skip_reason, position_id, generated_at, metadata
            FROM quant_signals
            ORDER BY generated_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&state.pool)
        .await
    }
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {}", e)))?;

    let response: Vec<RecentSignalResponse> = rows
        .into_iter()
        .map(|r| RecentSignalResponse {
            id: r.id.to_string(),
            kind: r.kind,
            condition_id: r.condition_id,
            direction: r.direction,
            confidence: r.confidence,
            size_usd: r.size_usd,
            execution_status: r.execution_status,
            skip_reason: r.skip_reason,
            position_id: r.position_id.map(|id| id.to_string()),
            generated_at: r.generated_at,
            metadata: r.metadata,
        })
        .collect();

    Ok(Json(response))
}

/// Query parameters for strategy performance.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PerformanceQuery {
    /// Rolling period in days (default: 7). Options: 7, 30.
    pub period_days: Option<i32>,
}

/// Strategy performance snapshot response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StrategyPerformanceResponse {
    pub strategy: String,
    pub period_end: DateTime<Utc>,
    pub period_days: i32,
    pub total_signals: i32,
    pub executed: i32,
    pub wins: i32,
    pub losses: i32,
    pub net_pnl: Decimal,
    pub avg_pnl: Decimal,
    pub win_rate: Option<f64>,
    pub sharpe: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub avg_hold_hours: Option<f64>,
}

/// GET /api/v1/signals/performance — Get per-strategy performance.
#[utoipa::path(
    get,
    path = "/api/v1/signals/performance",
    tag = "signals",
    params(PerformanceQuery),
    responses(
        (status = 200, description = "Strategy performance snapshots", body = Vec<StrategyPerformanceResponse>)
    )
)]
pub async fn get_strategy_performance(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PerformanceQuery>,
) -> ApiResult<Json<Vec<StrategyPerformanceResponse>>> {
    let period = params.period_days.unwrap_or(7);

    // Get the most recent snapshot for each strategy at the requested period
    let rows = sqlx::query_as::<_, PerformanceRow>(
        r#"
        SELECT DISTINCT ON (strategy)
            strategy, period_end, period_days,
            total_signals, executed, wins, losses,
            net_pnl, avg_pnl, win_rate, sharpe,
            max_drawdown_pct, avg_hold_hours
        FROM strategy_pnl_snapshots
        WHERE period_days = $1
        ORDER BY strategy, period_end DESC
        "#,
    )
    .bind(period)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {}", e)))?;

    let response: Vec<StrategyPerformanceResponse> = rows
        .into_iter()
        .map(|r| StrategyPerformanceResponse {
            strategy: r.strategy,
            period_end: r.period_end,
            period_days: r.period_days,
            total_signals: r.total_signals,
            executed: r.executed,
            wins: r.wins,
            losses: r.losses,
            net_pnl: r.net_pnl,
            avg_pnl: r.avg_pnl,
            win_rate: r.win_rate,
            sharpe: r.sharpe,
            max_drawdown_pct: r.max_drawdown_pct,
            avg_hold_hours: r.avg_hold_hours,
        })
        .collect();

    Ok(Json(response))
}

// ── Internal row types ──

#[derive(Debug, sqlx::FromRow)]
struct RecentSignalRow {
    id: uuid::Uuid,
    kind: String,
    condition_id: String,
    direction: String,
    confidence: f64,
    size_usd: Option<Decimal>,
    execution_status: Option<String>,
    skip_reason: Option<String>,
    position_id: Option<uuid::Uuid>,
    generated_at: DateTime<Utc>,
    metadata: serde_json::Value,
}

#[derive(Debug, sqlx::FromRow)]
struct PerformanceRow {
    strategy: String,
    period_end: DateTime<Utc>,
    period_days: i32,
    total_signals: i32,
    executed: i32,
    wins: i32,
    losses: i32,
    net_pnl: Decimal,
    avg_pnl: Decimal,
    win_rate: Option<f64>,
    sharpe: Option<f64>,
    max_drawdown_pct: Option<f64>,
    avg_hold_hours: Option<f64>,
}

#[derive(Debug, sqlx::FromRow)]
struct FlowRow {
    condition_id: String,
    window_end: DateTime<Utc>,
    window_minutes: i32,
    buy_volume: Decimal,
    sell_volume: Decimal,
    net_flow: Decimal,
    imbalance_ratio: Decimal,
    unique_buyers: i32,
    unique_sellers: i32,
    smart_money_flow: Decimal,
    trade_count: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct MetadataRow {
    condition_id: String,
    question: String,
    category: Option<String>,
    tags: Vec<String>,
    end_date: Option<DateTime<Utc>>,
    volume: Decimal,
    liquidity: Decimal,
    active: bool,
    fetched_at: DateTime<Utc>,
}
