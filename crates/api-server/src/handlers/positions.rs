//! Position management handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
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
    /// Whether this is a copy trade.
    pub is_copy_trade: bool,
    /// Source wallet (for copy trades).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_wallet: Option<String>,
    /// Realized P&L (for closed positions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_pnl: Option<Decimal>,
    /// Position opened timestamp.
    pub opened_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
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
    /// Filter by copy trades only.
    pub copy_trades_only: Option<bool>,
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
    is_copy_trade: bool,
    source_wallet: Option<String>,
    realized_pnl: Option<Decimal>,
    opened_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[allow(dead_code)]
    is_open: bool,
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
    let status_filter = match query.status.as_str() {
        "open" => Some(true),
        "closed" => Some(false),
        _ => None,
    };

    let rows: Vec<PositionRow> = sqlx::query_as(
        r#"
        SELECT id, market_id, outcome, side, quantity, entry_price,
               current_price, unrealized_pnl, stop_loss, take_profit,
               is_copy_trade, source_wallet, realized_pnl,
               opened_at, updated_at, is_open
        FROM positions
        WHERE ($1::text IS NULL OR market_id = $1)
          AND ($2::text IS NULL OR outcome = $2)
          AND ($3::bool IS NULL OR is_copy_trade = $3)
          AND ($4::bool IS NULL OR is_open = $4)
        ORDER BY opened_at DESC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(&query.market_id)
    .bind(&query.outcome)
    .bind(query.copy_trades_only)
    .bind(status_filter)
    .bind(query.limit)
    .bind(query.offset)
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
                is_copy_trade: row.is_copy_trade,
                source_wallet: row.source_wallet,
                realized_pnl: row.realized_pnl,
                opened_at: row.opened_at,
                updated_at: row.updated_at,
            }
        })
        .collect();

    Ok(Json(positions))
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
        SELECT id, market_id, outcome, side, quantity, entry_price,
               current_price, unrealized_pnl, stop_loss, take_profit,
               is_copy_trade, source_wallet, realized_pnl,
               opened_at, updated_at, is_open
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
                is_copy_trade: row.is_copy_trade,
                source_wallet: row.source_wallet,
                realized_pnl: row.realized_pnl,
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
    // First, fetch the position
    let row: Option<PositionRow> = sqlx::query_as(
        r#"
        SELECT id, market_id, outcome, side, quantity, entry_price,
               current_price, unrealized_pnl, stop_loss, take_profit,
               is_copy_trade, source_wallet, realized_pnl,
               opened_at, updated_at, is_open
        FROM positions
        WHERE id = $1 AND is_open = true
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

    // Update the position in the database
    let remaining = row.quantity - close_quantity;
    let is_fully_closed = remaining == Decimal::ZERO;

    sqlx::query(
        r#"
        UPDATE positions
        SET quantity = $1, is_open = $2, updated_at = NOW()
        WHERE id = $3
        "#,
    )
    .bind(remaining)
    .bind(!is_fully_closed)
    .bind(position_id)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Publish position update via WebSocket
    let update = PositionUpdate {
        position_id,
        market_id: row.market_id.clone(),
        update_type: if is_fully_closed {
            PositionUpdateType::Closed
        } else {
            PositionUpdateType::Updated
        },
        quantity: remaining,
        current_price: row.current_price,
        unrealized_pnl: row.unrealized_pnl,
        timestamp: Utc::now(),
    };
    let _ = state.publish_position(update);

    let entry_value = row.entry_price * remaining;
    let current_value = row.current_price * remaining;
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
        quantity: remaining,
        entry_price: row.entry_price,
        current_price: row.current_price,
        unrealized_pnl: row.unrealized_pnl * (remaining / row.quantity),
        unrealized_pnl_pct: pnl_pct,
        stop_loss: row.stop_loss,
        take_profit: row.take_profit,
        is_copy_trade: row.is_copy_trade,
        source_wallet: row.source_wallet,
        realized_pnl: row.realized_pnl,
        opened_at: row.opened_at,
        updated_at: Utc::now(),
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
            is_copy_trade: false,
            source_wallet: None,
            realized_pnl: None,
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
}
