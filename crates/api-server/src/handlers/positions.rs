//! Position management handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use polymarket_core::db::positions::{
    PositionRepository, SOURCE_ARBITRAGE, SOURCE_COPY_TRADE, SOURCE_MANUAL, SOURCE_RECOMMENDATION,
};
use polymarket_core::types::PositionState;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::trade_events::{NewTradeEvent, TradeEventRecorder};
use crate::websocket::{PositionUpdate, PositionUpdateType};

/// Position response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PositionResponse {
    /// Position identifier.
    pub id: Uuid,
    /// Market identifier.
    pub market_id: String,
    /// Outcome (yes/no).
    pub outcome: String,
    /// Position side (long/short).
    pub side: String,
    /// Quantity held.
    pub quantity: Decimal,
    /// Average entry price.
    pub entry_price: Decimal,
    /// Current market price.
    pub current_price: Decimal,
    /// Unrealized P&L.
    pub unrealized_pnl: Decimal,
    /// Unrealized P&L percentage.
    pub unrealized_pnl_pct: Decimal,
    /// Stop loss price (if set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<Decimal>,
    /// Take profit price (if set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit: Option<Decimal>,
    /// Realized P&L (for closed positions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_pnl: Option<Decimal>,
    /// Full lifecycle state.
    pub state: String,
    /// Position opened timestamp.
    pub opened_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Summary response for dashboard/overview metrics.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PositionsSummaryResponse {
    /// Deduplicated active position count (latest row per market/source).
    pub open_positions: i64,
    /// Distinct active markets after deduplication.
    pub open_markets: i64,
    /// Active markets with duplicate rows before deduplication.
    pub duplicate_open_markets: i64,
    /// Deduplicated marked position value based on current prices only.
    pub portfolio_value: Decimal,
    /// Deduplicated position count with a current mark.
    pub priced_open_positions: i64,
    /// Deduplicated position count missing a current mark.
    pub unpriced_open_positions: i64,
    /// Deduplicated cost basis for positions missing a current mark.
    pub unpriced_position_cost_basis: Decimal,
    /// Deduplicated unrealized P&L.
    pub unrealized_pnl: Decimal,
    /// Raw active row count before deduplication.
    pub raw_open_positions: i64,
    /// Raw marked position value before deduplication.
    pub raw_portfolio_value: Decimal,
    /// Raw active row count missing a current mark before deduplication.
    pub raw_unpriced_open_positions: i64,
    /// Raw cost basis for positions missing a current mark before deduplication.
    pub raw_unpriced_position_cost_basis: Decimal,
    /// Raw unrealized P&L before deduplication.
    pub raw_unrealized_pnl: Decimal,
    /// All closed positions, including flat closes.
    pub closed_positions: i64,
    /// Count of winning closed positions.
    pub wins: i64,
    /// Count of losing closed positions.
    pub losses: i64,
    /// Count of closed positions with zero realized P&L.
    pub flat_closes: i64,
    /// Win rate based on resolved non-flat closes only.
    pub win_rate: Decimal,
}

/// Request to close a position.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ClosePositionRequest {
    /// Quantity to close (None = close all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<Decimal>,
    /// Limit price (None = market order).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<Decimal>,
}

/// Query parameters for listing positions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListPositionsQuery {
    /// Filter by market.
    pub market_id: Option<String>,
    /// Filter by outcome.
    pub outcome: Option<String>,
    /// Filter by status (open/closed/all).
    #[serde(default = "default_status")]
    pub status: String,
    /// Maximum results.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_status() -> String {
    "open".to_string()
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, FromRow)]
struct PositionRow {
    id: Uuid,
    market_id: String,
    outcome: String,
    side: String,
    quantity: Decimal,
    entry_price: Decimal,
    current_price: Decimal,
    unrealized_pnl: Decimal,
    stop_loss: Option<Decimal>,
    take_profit: Option<Decimal>,
    realized_pnl: Option<Decimal>,
    state: i16,
    opened_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct CloseablePositionRow {
    id: Uuid,
    market_id: String,
    outcome: String,
    side: String,
    quantity: Decimal,
    entry_price: Decimal,
    current_price: Decimal,
    stop_loss: Option<Decimal>,
    take_profit: Option<Decimal>,
    realized_pnl: Option<Decimal>,
    source: i16,
    source_signal_id: Option<Uuid>,
    opened_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct PositionSummaryRow {
    open_positions: i64,
    open_markets: i64,
    duplicate_open_markets: i64,
    portfolio_value: Decimal,
    priced_open_positions: i64,
    unpriced_open_positions: i64,
    unpriced_position_cost_basis: Decimal,
    unrealized_pnl: Decimal,
    raw_open_positions: i64,
    raw_portfolio_value: Decimal,
    raw_unpriced_open_positions: i64,
    raw_unpriced_position_cost_basis: Decimal,
    raw_unrealized_pnl: Decimal,
    closed_positions: i64,
    wins: i64,
    losses: i64,
    flat_closes: i64,
    win_rate: Decimal,
}

fn position_state_name(state: i16) -> &'static str {
    match state {
        0 => "pending",
        1 => "open",
        2 => "exit_ready",
        3 => "closing",
        4 => "closed",
        5 => "entry_failed",
        6 => "exit_failed",
        7 => "stalled",
        _ => "closed",
    }
}

fn position_state_label(state: PositionState) -> &'static str {
    match state {
        PositionState::Pending => "pending",
        PositionState::Open => "open",
        PositionState::ExitReady => "exit_ready",
        PositionState::Closing => "closing",
        PositionState::Closed => "closed",
        PositionState::EntryFailed => "entry_failed",
        PositionState::ExitFailed => "exit_failed",
        PositionState::Stalled => "stalled",
    }
}

fn trade_event_labels(source: i16) -> (&'static str, &'static str) {
    match source {
        SOURCE_ARBITRAGE => ("arb", "arb"),
        SOURCE_COPY_TRADE => ("copy_trade", "copy_trade"),
        SOURCE_RECOMMENDATION => ("quant", "quant"),
        SOURCE_MANUAL => ("manual", "manual"),
        _ => ("manual", "manual"),
    }
}

async fn current_execution_mode(state: &AppState) -> &'static str {
    if state.order_executor.is_live_ready().await {
        "live"
    } else {
        "paper"
    }
}

fn validate_status_filter(status: &str) -> ApiResult<&str> {
    match status {
        "open" | "closed" | "all" => Ok(status),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid status '{}'. Expected open, closed, or all",
            status
        ))),
    }
}

/// List positions.
#[utoipa::path(
    get,
    path = "/api/v1/positions",
    tag = "positions",
    params(ListPositionsQuery),
    responses(
        (status = 200, description = "List of positions", body = Vec<PositionResponse>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_positions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListPositionsQuery>,
) -> ApiResult<Json<Vec<PositionResponse>>> {
    let status_filter = validate_status_filter(&query.status)?;

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        r#"
        SELECT id, market_id,
               COALESCE(outcome, 'both') AS outcome,
               COALESCE(side, 'long') AS side,
               quantity,
               COALESCE(entry_price, (yes_entry_price + no_entry_price)) AS entry_price,
               COALESCE(current_price, (yes_entry_price + no_entry_price)) AS current_price,
               unrealized_pnl, stop_loss, take_profit,
               realized_pnl,
               state,
               COALESCE(opened_at, entry_timestamp) AS opened_at,
               COALESCE(last_updated, updated_at, entry_timestamp) AS updated_at
        FROM positions
        WHERE 1 = 1
        "#,
    );

    if let Some(market_id) = &query.market_id {
        qb.push(" AND market_id = ");
        qb.push_bind(market_id);
    }

    if let Some(outcome) = &query.outcome {
        qb.push(" AND COALESCE(outcome, 'both') = ");
        qb.push_bind(outcome);
    }

    match status_filter {
        "open" => {
            qb.push(" AND is_open = TRUE");
        }
        "closed" => {
            qb.push(" AND is_open = FALSE");
        }
        "all" => {}
        _ => unreachable!("status_filter already validated"),
    }

    qb.push(" ORDER BY COALESCE(opened_at, entry_timestamp) DESC");
    qb.push(" LIMIT ");
    qb.push_bind(query.limit);
    qb.push(" OFFSET ");
    qb.push_bind(query.offset);

    let rows: Vec<PositionRow> = qb
        .build_query_as()
        .fetch_all(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let positions: Vec<PositionResponse> = rows
        .into_iter()
        .map(|row| {
            let entry_value = row.entry_price * row.quantity;
            let current_value = row.current_price * row.quantity;
            let pnl_pct = if entry_value > Decimal::ZERO {
                (current_value - entry_value) / entry_value * Decimal::new(100, 0)
            } else {
                Decimal::ZERO
            };

            PositionResponse {
                id: row.id,
                market_id: row.market_id,
                outcome: row.outcome,
                side: row.side,
                quantity: row.quantity,
                entry_price: row.entry_price,
                current_price: row.current_price,
                unrealized_pnl: row.unrealized_pnl,
                unrealized_pnl_pct: pnl_pct,
                stop_loss: row.stop_loss,
                take_profit: row.take_profit,
                realized_pnl: row.realized_pnl,
                state: position_state_name(row.state).to_string(),
                opened_at: row.opened_at,
                updated_at: row.updated_at,
            }
        })
        .collect();

    Ok(Json(positions))
}

/// Get aggregate position metrics for dashboards.
#[utoipa::path(
    get,
    path = "/api/v1/positions/summary",
    tag = "positions",
    responses(
        (status = 200, description = "Aggregate position metrics", body = PositionsSummaryResponse),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_positions_summary(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<PositionsSummaryResponse>> {
    let row: PositionSummaryRow = sqlx::query_as(
        r#"
        WITH active_positions AS (
            SELECT
                id,
                market_id,
                COALESCE(source, 0) AS source,
                quantity,
                current_price,
                COALESCE(entry_price, (yes_entry_price + no_entry_price), 0) AS entry_price,
                (quantity * COALESCE(entry_price, (yes_entry_price + no_entry_price), 0)) AS entry_value,
                unrealized_pnl,
                COALESCE(last_updated, updated_at, entry_timestamp) AS sort_updated
            FROM positions
            WHERE is_open = TRUE
        ),
        ranked_active AS (
            SELECT
                *,
                ROW_NUMBER() OVER (
                    PARTITION BY market_id, source
                    ORDER BY sort_updated DESC, id DESC
                ) AS rn
            FROM active_positions
        ),
        effective_active AS (
            SELECT *
            FROM ranked_active
            WHERE rn = 1
        ),
        duplicate_active_markets AS (
            SELECT COUNT(*)::bigint AS duplicate_open_markets
            FROM (
                SELECT market_id, source
                FROM active_positions
                GROUP BY market_id, source
                HAVING COUNT(*) > 1
            ) dup
        ),
        closed_stats AS (
            SELECT
                COUNT(*) FILTER (WHERE state = 4)::bigint AS closed_positions,
                COUNT(*) FILTER (WHERE state = 4 AND COALESCE(realized_pnl, 0) > 0)::bigint AS wins,
                COUNT(*) FILTER (WHERE state = 4 AND COALESCE(realized_pnl, 0) < 0)::bigint AS losses,
                COUNT(*) FILTER (WHERE state = 4 AND COALESCE(realized_pnl, 0) = 0)::bigint AS flat_closes
            FROM positions
        )
        SELECT
            COALESCE((SELECT COUNT(*) FROM effective_active), 0)::bigint AS open_positions,
            COALESCE((SELECT COUNT(DISTINCT market_id) FROM effective_active), 0)::bigint AS open_markets,
            COALESCE((SELECT duplicate_open_markets FROM duplicate_active_markets), 0)::bigint AS duplicate_open_markets,
            COALESCE((SELECT SUM(CASE
                WHEN current_price IS NOT NULL THEN quantity * current_price
                ELSE GREATEST(entry_value + COALESCE(unrealized_pnl, 0), 0)
            END) FROM effective_active), 0) AS portfolio_value,
            COALESCE((SELECT COUNT(*) FROM effective_active WHERE current_price IS NOT NULL), 0)::bigint AS priced_open_positions,
            COALESCE((SELECT COUNT(*) FROM effective_active WHERE current_price IS NULL), 0)::bigint AS unpriced_open_positions,
            COALESCE((SELECT SUM(quantity * entry_price) FROM effective_active WHERE current_price IS NULL), 0) AS unpriced_position_cost_basis,
            COALESCE((SELECT SUM(unrealized_pnl) FROM effective_active), 0) AS unrealized_pnl,
            COALESCE((SELECT COUNT(*) FROM active_positions), 0)::bigint AS raw_open_positions,
            COALESCE((SELECT SUM(CASE
                WHEN current_price IS NOT NULL THEN quantity * current_price
                ELSE GREATEST(entry_value + COALESCE(unrealized_pnl, 0), 0)
            END) FROM active_positions), 0) AS raw_portfolio_value,
            COALESCE((SELECT COUNT(*) FROM active_positions WHERE current_price IS NULL), 0)::bigint AS raw_unpriced_open_positions,
            COALESCE((SELECT SUM(quantity * entry_price) FROM active_positions WHERE current_price IS NULL), 0) AS raw_unpriced_position_cost_basis,
            COALESCE((SELECT SUM(unrealized_pnl) FROM active_positions), 0) AS raw_unrealized_pnl,
            closed_positions,
            wins,
            losses,
            flat_closes,
            CASE
                WHEN (wins + losses) > 0
                    THEN (wins::numeric / (wins + losses)::numeric) * 100
                ELSE 0
            END AS win_rate
        FROM closed_stats
        "#,
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(PositionsSummaryResponse {
        open_positions: row.open_positions,
        open_markets: row.open_markets,
        duplicate_open_markets: row.duplicate_open_markets,
        portfolio_value: row.portfolio_value,
        priced_open_positions: row.priced_open_positions,
        unpriced_open_positions: row.unpriced_open_positions,
        unpriced_position_cost_basis: row.unpriced_position_cost_basis,
        unrealized_pnl: row.unrealized_pnl,
        raw_open_positions: row.raw_open_positions,
        raw_portfolio_value: row.raw_portfolio_value,
        raw_unpriced_open_positions: row.raw_unpriced_open_positions,
        raw_unpriced_position_cost_basis: row.raw_unpriced_position_cost_basis,
        raw_unrealized_pnl: row.raw_unrealized_pnl,
        closed_positions: row.closed_positions,
        wins: row.wins,
        losses: row.losses,
        flat_closes: row.flat_closes,
        win_rate: row.win_rate,
    }))
}

/// Get a specific position.
#[utoipa::path(
    get,
    path = "/api/v1/positions/{position_id}",
    tag = "positions",
    params(
        ("position_id" = Uuid, Path, description = "Position identifier")
    ),
    responses(
        (status = 200, description = "Position details", body = PositionResponse),
        (status = 404, description = "Position not found")
    )
)]
pub async fn get_position(
    State(state): State<Arc<AppState>>,
    Path(position_id): Path<Uuid>,
) -> ApiResult<Json<PositionResponse>> {
    let row: Option<PositionRow> = sqlx::query_as(
        r#"
        SELECT id, market_id,
               COALESCE(outcome, 'both') AS outcome,
               COALESCE(side, 'long') AS side,
               quantity,
               COALESCE(entry_price, (yes_entry_price + no_entry_price)) AS entry_price,
               COALESCE(current_price, (yes_entry_price + no_entry_price)) AS current_price,
               unrealized_pnl, stop_loss, take_profit,
               realized_pnl,
               state,
               COALESCE(opened_at, entry_timestamp) AS opened_at,
               COALESCE(last_updated, updated_at, entry_timestamp) AS updated_at
        FROM positions
        WHERE id = $1
        "#,
    )
    .bind(position_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(row) => {
            let entry_value = row.entry_price * row.quantity;
            let current_value = row.current_price * row.quantity;
            let pnl_pct = if entry_value > Decimal::ZERO {
                (current_value - entry_value) / entry_value * Decimal::new(100, 0)
            } else {
                Decimal::ZERO
            };

            Ok(Json(PositionResponse {
                id: row.id,
                market_id: row.market_id,
                outcome: row.outcome,
                side: row.side,
                quantity: row.quantity,
                entry_price: row.entry_price,
                current_price: row.current_price,
                unrealized_pnl: row.unrealized_pnl,
                unrealized_pnl_pct: pnl_pct,
                stop_loss: row.stop_loss,
                take_profit: row.take_profit,
                realized_pnl: row.realized_pnl,
                state: position_state_name(row.state).to_string(),
                opened_at: row.opened_at,
                updated_at: row.updated_at,
            }))
        }
        None => Err(ApiError::NotFound(format!(
            "Position {} not found",
            position_id
        ))),
    }
}

/// Close a position.
#[utoipa::path(
    post,
    path = "/api/v1/positions/{position_id}/close",
    tag = "positions",
    params(
        ("position_id" = Uuid, Path, description = "Position identifier")
    ),
    request_body = ClosePositionRequest,
    responses(
        (status = 200, description = "Position closed", body = PositionResponse),
        (status = 404, description = "Position not found"),
        (status = 400, description = "Invalid request")
    )
)]
pub async fn close_position(
    State(state): State<Arc<AppState>>,
    Path(position_id): Path<Uuid>,
    Json(request): Json<ClosePositionRequest>,
) -> ApiResult<Json<PositionResponse>> {
    if request.limit_price.is_some() {
        return Err(ApiError::BadRequest(
            "Manual close queues a market exit; limit_price is not supported".to_string(),
        ));
    }

    // First, fetch the position
    let row: Option<CloseablePositionRow> = sqlx::query_as(
        r#"
        SELECT id, market_id,
               COALESCE(outcome, 'both') AS outcome,
               COALESCE(side, 'long') AS side,
               quantity,
               COALESCE(entry_price, (yes_entry_price + no_entry_price)) AS entry_price,
               COALESCE(current_price, (yes_entry_price + no_entry_price)) AS current_price,
               stop_loss, take_profit,
               realized_pnl,
               COALESCE(source, 0) AS source,
               source_signal_id,
               COALESCE(opened_at, entry_timestamp) AS opened_at
        FROM positions
        WHERE id = $1
          AND is_open = TRUE
        "#,
    )
    .bind(position_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let row = match row {
        Some(r) => r,
        None => {
            return Err(ApiError::NotFound(format!(
                "Open position {} not found",
                position_id
            )))
        }
    };

    let close_quantity = request.quantity.unwrap_or(row.quantity);

    if close_quantity > row.quantity {
        return Err(ApiError::BadRequest(
            "Close quantity exceeds position size".to_string(),
        ));
    }
    if close_quantity != row.quantity {
        return Err(ApiError::BadRequest(
            "Partial manual close is not supported; queueing exits is full-position only"
                .to_string(),
        ));
    }

    let position_repo = PositionRepository::new(state.pool.clone());
    let mut position = position_repo
        .get(position_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Open position {} not found", position_id)))?;

    let previous_state = position.state;
    match position.state {
        PositionState::Open => position.mark_exit_ready().map_err(ApiError::BadRequest)?,
        PositionState::ExitReady | PositionState::Closing => position.touch(),
        PositionState::ExitFailed => {
            if !position.attempt_exit_recovery() {
                return Err(ApiError::BadRequest(
                    "Position has exhausted exit retries and cannot be re-queued".to_string(),
                ));
            }
        }
        PositionState::Stalled => match position.attempt_stalled_recovery() {
            Some(PositionState::Open) => {
                position.mark_exit_ready().map_err(ApiError::BadRequest)?
            }
            Some(PositionState::ExitReady | PositionState::Closing) => {}
            Some(state) => {
                return Err(ApiError::BadRequest(format!(
                    "Position recovered to {} and cannot be manually closed yet",
                    position_state_label(state)
                )))
            }
            None => {
                return Err(ApiError::BadRequest(
                    "Position could not be recovered for manual close".to_string(),
                ))
            }
        },
        PositionState::EntryFailed if position.is_one_legged_entry_fail() => {
            position
                .recover_one_legged_to_open()
                .map_err(ApiError::BadRequest)?;
            position.mark_exit_ready().map_err(ApiError::BadRequest)?;
        }
        PositionState::Pending => {
            return Err(ApiError::BadRequest(
                "Position is still entering and cannot be manually closed yet".to_string(),
            ))
        }
        PositionState::EntryFailed => {
            return Err(ApiError::BadRequest(
                "Position failed to enter and has no exit flow to queue".to_string(),
            ))
        }
        PositionState::Closed => {
            return Err(ApiError::NotFound(format!(
                "Open position {} not found",
                position_id
            )))
        }
    }

    position_repo
        .update(&position)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let execution_mode = current_execution_mode(state.as_ref()).await;
    let (strategy, source_label) = trade_event_labels(row.source);
    let mut event = NewTradeEvent::new(
        strategy,
        execution_mode,
        source_label,
        row.market_id.clone(),
        "manual_exit_requested",
    );
    event.position_id = Some(position.id);
    event.signal_id = row.source_signal_id;
    event.state_from = Some(position_state_label(previous_state).to_string());
    event.state_to = Some(position_state_label(position.state).to_string());
    event.reason = Some("manual_close_request".to_string());
    event.unrealized_pnl = Some(position.unrealized_pnl);
    event.metadata = serde_json::json!({
        "requested_quantity": close_quantity.to_string(),
        "limit_price": request.limit_price.map(|price| price.to_string()),
        "manual_close": true,
    });
    TradeEventRecorder::new(state.pool.clone(), state.trade_event_tx.clone())
        .record_warn(event)
        .await;

    // Publish position update via WebSocket
    let update = PositionUpdate {
        position_id,
        market_id: row.market_id.clone(),
        update_type: PositionUpdateType::Updated,
        quantity: row.quantity,
        current_price: row.current_price,
        unrealized_pnl: position.unrealized_pnl,
        timestamp: Utc::now(),
    };
    let _ = state.publish_position(update);

    let entry_value = row.entry_price * row.quantity;
    let current_value = row.current_price * row.quantity;
    let pnl_pct = if entry_value > Decimal::ZERO {
        (current_value - entry_value) / entry_value * Decimal::new(100, 0)
    } else {
        Decimal::ZERO
    };

    Ok(Json(PositionResponse {
        id: row.id,
        market_id: row.market_id,
        outcome: row.outcome,
        side: row.side,
        quantity: row.quantity,
        entry_price: row.entry_price,
        current_price: row.current_price,
        unrealized_pnl: position.unrealized_pnl,
        unrealized_pnl_pct: pnl_pct,
        stop_loss: row.stop_loss,
        take_profit: row.take_profit,
        realized_pnl: row.realized_pnl,
        state: position_state_label(position.state).to_string(),
        opened_at: row.opened_at,
        updated_at: position.last_updated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_response_serialization() {
        let position = PositionResponse {
            id: Uuid::new_v4(),
            market_id: "market1".to_string(),
            outcome: "yes".to_string(),
            side: "long".to_string(),
            quantity: Decimal::new(100, 0),
            entry_price: Decimal::new(50, 2),
            current_price: Decimal::new(55, 2),
            unrealized_pnl: Decimal::new(5, 0),
            unrealized_pnl_pct: Decimal::new(10, 0),
            stop_loss: Some(Decimal::new(45, 2)),
            take_profit: None,
            realized_pnl: None,
            state: "open".to_string(),
            opened_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&position).unwrap();
        assert!(json.contains("market1"));
        assert!(json.contains("yes"));
        assert!(json.contains("stop_loss"));
        assert!(!json.contains("take_profit")); // skipped when None
    }

    #[test]
    fn test_close_position_request() {
        let request = ClosePositionRequest {
            quantity: Some(Decimal::new(50, 0)),
            limit_price: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("50"));
    }

    #[test]
    fn test_position_state_name() {
        assert_eq!(position_state_name(1), "open");
        assert_eq!(position_state_name(4), "closed");
        assert_eq!(position_state_name(5), "entry_failed");
    }

    #[test]
    fn test_validate_status_filter() {
        assert!(validate_status_filter("open").is_ok());
        assert!(validate_status_filter("closed").is_ok());
        assert!(validate_status_filter("all").is_ok());
        assert!(validate_status_filter("bad").is_err());
    }
}
