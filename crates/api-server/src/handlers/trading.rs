//! Trading and order execution handlers.

use axum::extract::{Path, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use tracing::{info, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use auth::Claims;

use polymarket_core::types::{
    LimitOrder as CoreLimitOrder, MarketOrder as CoreMarketOrder, OrderSide as CoreOrderSide,
    OrderStatus as CoreOrderStatus,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::websocket::{SignalType, SignalUpdate};

/// Order side.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Order type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    Market,
    Limit,
    StopLoss,
    TakeProfit,
}

/// Order status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Pending,
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Expired,
}

/// Request to place an order.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PlaceOrderRequest {
    /// Market identifier.
    pub market_id: String,
    /// Outcome to trade (yes/no).
    pub outcome: String,
    /// Order side.
    pub side: OrderSide,
    /// Order type.
    pub order_type: OrderType,
    /// Quantity to trade.
    pub quantity: Decimal,
    /// Limit price (required for limit orders).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
    /// Stop price (for stop-loss/take-profit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<Decimal>,
    /// Time in force.
    #[serde(default = "default_time_in_force")]
    pub time_in_force: String,
    /// Client order ID for idempotency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
}

fn default_time_in_force() -> String {
    "GTC".to_string() // Good Till Cancelled
}

/// Order response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OrderResponse {
    /// Order identifier.
    pub id: Uuid,
    /// Client order ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<String>,
    /// Market identifier.
    pub market_id: String,
    /// Outcome.
    pub outcome: String,
    /// Order side.
    pub side: OrderSide,
    /// Order type.
    pub order_type: OrderType,
    /// Order status.
    pub status: OrderStatus,
    /// Original quantity.
    pub quantity: Decimal,
    /// Filled quantity.
    pub filled_quantity: Decimal,
    /// Remaining quantity.
    pub remaining_quantity: Decimal,
    /// Order price (for limit orders).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<Decimal>,
    /// Average fill price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_fill_price: Option<Decimal>,
    /// Stop price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_price: Option<Decimal>,
    /// Time in force.
    pub time_in_force: String,
    /// Created timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    /// Fill timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct OrderRow {
    id: Uuid,
    client_order_id: Option<String>,
    market_id: String,
    outcome: String,
    side: String,
    order_type: String,
    status: String,
    quantity: Decimal,
    filled_quantity: Decimal,
    price: Option<Decimal>,
    avg_fill_price: Option<Decimal>,
    stop_price: Option<Decimal>,
    time_in_force: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    filled_at: Option<DateTime<Utc>>,
}

/// Place a new order.
#[utoipa::path(
    post,
    path = "/api/v1/orders",
    tag = "trading",
    request_body = PlaceOrderRequest,
    responses(
        (status = 201, description = "Order placed", body = OrderResponse),
        (status = 400, description = "Invalid order request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn place_order(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(request): Json<PlaceOrderRequest>,
) -> ApiResult<Json<OrderResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_live_enabled: bool = sqlx::query_scalar(
        r#"
        SELECT COALESCE(w.live_trading_enabled, FALSE)
        FROM user_settings us
        JOIN workspaces w ON w.id = us.default_workspace_id
        WHERE us.user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(false);

    if workspace_live_enabled {
        if !state.order_executor.is_live() {
            return Err(ApiError::ServiceUnavailable(
                "Workspace is configured for live trading but executor is running in simulation mode. Enable LIVE_TRADING and restart."
                    .into(),
            ));
        }
        if !state.order_executor.is_live_ready().await {
            return Err(ApiError::ServiceUnavailable(
                "Workspace is configured for live trading but wallet/API credentials are not ready."
                    .into(),
            ));
        }
    }

    // Check circuit breaker before processing any orders
    if !state.circuit_breaker.can_trade().await {
        let cb_state = state.circuit_breaker.state().await;
        return Err(ApiError::ServiceUnavailable(format!(
            "Trading halted: {:?}. Resumes at {:?}",
            cb_state
                .trip_reason
                .unwrap_or(risk_manager::circuit_breaker::TripReason::Manual),
            cb_state
                .resume_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "unknown".to_string())
        )));
    }

    // Validate the order request
    validate_order_request(&request)?;

    let now = Utc::now();

    // Convert to core types
    let core_side = match request.side {
        OrderSide::Buy => CoreOrderSide::Buy,
        OrderSide::Sell => CoreOrderSide::Sell,
    };

    // Execute based on order type
    let report = match request.order_type {
        OrderType::Market => {
            let order = CoreMarketOrder::new(
                request.market_id.clone(),
                request.outcome.clone(),
                core_side,
                request.quantity,
            );
            state
                .order_executor
                .execute_market_order(order)
                .await
                .map_err(|e| ApiError::Internal(format!("Execution error: {}", e)))?
        }
        OrderType::Limit => {
            let price = request
                .price
                .ok_or(ApiError::BadRequest("Limit orders require a price".into()))?;
            let order = CoreLimitOrder::new(
                request.market_id.clone(),
                request.outcome.clone(),
                core_side,
                price,
                request.quantity,
            );
            state
                .order_executor
                .execute_limit_order(order)
                .await
                .map_err(|e| ApiError::Internal(format!("Execution error: {}", e)))?
        }
        OrderType::StopLoss | OrderType::TakeProfit => {
            // Stop-loss and take-profit orders are handled by the risk manager
            // For now, just store them as pending orders
            return store_conditional_order(&state, &request, now).await;
        }
    };

    // Determine final status from execution report
    let (status, filled_qty, avg_price, filled_at) = match report.status {
        CoreOrderStatus::Filled => (
            OrderStatus::Filled,
            report.filled_quantity,
            Some(report.average_price),
            Some(report.executed_at),
        ),
        CoreOrderStatus::PartiallyFilled => (
            OrderStatus::PartiallyFilled,
            report.filled_quantity,
            Some(report.average_price),
            Some(report.executed_at),
        ),
        CoreOrderStatus::Rejected => {
            return Err(ApiError::BadRequest(
                report.error_message.unwrap_or("Order rejected".into()),
            ));
        }
        _ => (OrderStatus::Pending, Decimal::ZERO, None, None),
    };

    let order_id = report.order_id;

    // Insert order into database with execution results
    sqlx::query(
        r#"
        INSERT INTO orders
        (id, client_order_id, market_id, outcome, side, order_type, status,
         quantity, filled_quantity, price, avg_fill_price, stop_price, time_in_force,
         created_at, updated_at, filled_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14, $15)
        "#,
    )
    .bind(order_id)
    .bind(&request.client_order_id)
    .bind(&request.market_id)
    .bind(&request.outcome)
    .bind(format!("{:?}", request.side).to_lowercase())
    .bind(format!("{:?}", request.order_type).to_lowercase())
    .bind(format!("{:?}", status).to_lowercase())
    .bind(request.quantity)
    .bind(filled_qty)
    .bind(request.price)
    .bind(avg_price)
    .bind(request.stop_price)
    .bind(&request.time_in_force)
    .bind(now)
    .bind(filled_at)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Log execution
    info!(
        order_id = %order_id,
        market = %request.market_id,
        status = ?status,
        filled = %filled_qty,
        live = %state.order_executor.is_live(),
        workspace_live_enabled = %workspace_live_enabled,
        "Order executed"
    );

    // Record trade for circuit breaker tracking
    // For buy orders (opening positions), we record the cost as negative PnL (unrealized)
    // For sell orders (closing positions), we record proceeds as positive PnL (realized estimate)
    // Real PnL accuracy requires position-level tracking (entry price vs exit price)
    if report.is_success() {
        let trade_value = report.total_value();
        let (pnl, is_win) = match request.side {
            // Buys are costs — record as negative so circuit breaker tracks exposure
            OrderSide::Buy => (-trade_value, false),
            // Sells are proceeds — approximate win (actual PnL needs entry price)
            OrderSide::Sell => (trade_value, true),
        };
        let _ = state.circuit_breaker.record_trade(pnl, is_win).await;
    }

    // Publish signal for successful fills
    if report.is_success() {
        let _ = state.publish_signal(SignalUpdate {
            signal_id: Uuid::new_v4(),
            signal_type: SignalType::Alert,
            market_id: request.market_id.clone(),
            outcome_id: request.outcome.clone(),
            action: format!("order_{}", format!("{:?}", status).to_lowercase()),
            confidence: 1.0,
            timestamp: now,
            metadata: serde_json::json!({
                "order_id": order_id,
                "side": format!("{:?}", request.side).to_lowercase(),
                "filled_quantity": filled_qty,
                "avg_price": avg_price,
            }),
        });
    }

    Ok(Json(OrderResponse {
        id: order_id,
        client_order_id: request.client_order_id,
        market_id: request.market_id,
        outcome: request.outcome,
        side: request.side,
        order_type: request.order_type,
        status,
        quantity: request.quantity,
        filled_quantity: filled_qty,
        remaining_quantity: request.quantity - filled_qty,
        price: request.price,
        avg_fill_price: avg_price,
        stop_price: request.stop_price,
        time_in_force: request.time_in_force,
        created_at: now,
        updated_at: now,
        filled_at,
    }))
}

/// Store a conditional order (stop-loss/take-profit) without immediate execution.
///
/// WARNING: These orders are stored in the database but automatic trigger monitoring
/// is not yet implemented. They will not execute automatically when the stop price
/// is reached. Use the risk-manager crate's stop-loss rules for monitored stops.
async fn store_conditional_order(
    state: &Arc<AppState>,
    request: &PlaceOrderRequest,
    now: DateTime<Utc>,
) -> ApiResult<Json<OrderResponse>> {
    let order_id = Uuid::new_v4();

    sqlx::query(
        r#"
        INSERT INTO orders
        (id, client_order_id, market_id, outcome, side, order_type, status,
         quantity, filled_quantity, price, stop_price, time_in_force,
         created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9, $10, $11, $12, $12)
        "#,
    )
    .bind(order_id)
    .bind(&request.client_order_id)
    .bind(&request.market_id)
    .bind(&request.outcome)
    .bind(format!("{:?}", request.side).to_lowercase())
    .bind(format!("{:?}", request.order_type).to_lowercase())
    .bind("pending")
    .bind(request.quantity)
    .bind(request.price)
    .bind(request.stop_price)
    .bind(&request.time_in_force)
    .bind(now)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    warn!(
        order_id = %order_id,
        order_type = ?request.order_type,
        stop_price = ?request.stop_price,
        "Conditional order stored — automatic trigger monitoring not yet implemented"
    );

    Ok(Json(OrderResponse {
        id: order_id,
        client_order_id: request.client_order_id.clone(),
        market_id: request.market_id.clone(),
        outcome: request.outcome.clone(),
        side: request.side,
        order_type: request.order_type,
        status: OrderStatus::Pending,
        quantity: request.quantity,
        filled_quantity: Decimal::ZERO,
        remaining_quantity: request.quantity,
        price: request.price,
        avg_fill_price: None,
        stop_price: request.stop_price,
        time_in_force: request.time_in_force.clone(),
        created_at: now,
        updated_at: now,
        filled_at: None,
    }))
}

/// Maximum order quantity to prevent accidental enormous orders.
const MAX_ORDER_QUANTITY: Decimal = Decimal::from_parts(1_000_000, 0, 0, false, 0); // 1,000,000

fn validate_order_request(request: &PlaceOrderRequest) -> ApiResult<()> {
    // Validate market_id is non-empty and reasonable length
    let market_id = request.market_id.trim();
    if market_id.is_empty() {
        return Err(ApiError::BadRequest(
            "market_id must not be empty".to_string(),
        ));
    }
    if market_id.len() > 256 {
        return Err(ApiError::BadRequest(
            "market_id exceeds maximum length of 256 characters".to_string(),
        ));
    }

    // Validate outcome
    if request.outcome != "yes" && request.outcome != "no" {
        return Err(ApiError::BadRequest(
            "Outcome must be 'yes' or 'no'".to_string(),
        ));
    }

    // Validate quantity
    if request.quantity <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Quantity must be positive".to_string(),
        ));
    }
    if request.quantity > MAX_ORDER_QUANTITY {
        return Err(ApiError::BadRequest(format!(
            "Quantity exceeds maximum of {}",
            MAX_ORDER_QUANTITY
        )));
    }

    // Validate price for limit orders
    if request.order_type == OrderType::Limit && request.price.is_none() {
        return Err(ApiError::BadRequest(
            "Limit orders require a price".to_string(),
        ));
    }

    // Validate price range
    if let Some(price) = request.price {
        if price <= Decimal::ZERO || price >= Decimal::ONE {
            return Err(ApiError::BadRequest(
                "Price must be between 0 and 1 (exclusive)".to_string(),
            ));
        }
    }

    // Validate stop price for stop orders
    if (request.order_type == OrderType::StopLoss || request.order_type == OrderType::TakeProfit)
        && request.stop_price.is_none()
    {
        return Err(ApiError::BadRequest(
            "Stop orders require a stop_price".to_string(),
        ));
    }

    // Validate stop price range (must also be valid prediction market price)
    if let Some(stop_price) = request.stop_price {
        if stop_price <= Decimal::ZERO || stop_price >= Decimal::ONE {
            return Err(ApiError::BadRequest(
                "stop_price must be between 0 and 1 (exclusive)".to_string(),
            ));
        }
    }

    // Validate time_in_force
    match request.time_in_force.as_str() {
        "GTC" | "IOC" | "FOK" | "GTD" => {}
        _ => {
            return Err(ApiError::BadRequest(
                "time_in_force must be one of: GTC, IOC, FOK, GTD".to_string(),
            ));
        }
    }

    // Validate client_order_id length (if provided)
    if let Some(ref client_id) = request.client_order_id {
        if client_id.len() > 128 {
            return Err(ApiError::BadRequest(
                "client_order_id exceeds maximum length of 128 characters".to_string(),
            ));
        }
    }

    Ok(())
}

/// Get order status.
#[utoipa::path(
    get,
    path = "/api/v1/orders/{order_id}",
    tag = "trading",
    params(
        ("order_id" = Uuid, Path, description = "Order identifier")
    ),
    responses(
        (status = 200, description = "Order details", body = OrderResponse),
        (status = 404, description = "Order not found")
    )
)]
pub async fn get_order_status(
    State(state): State<Arc<AppState>>,
    Path(order_id): Path<Uuid>,
) -> ApiResult<Json<OrderResponse>> {
    let row: Option<OrderRow> = sqlx::query_as(
        r#"
        SELECT id, client_order_id, market_id, outcome, side, order_type, status,
               quantity, filled_quantity, price, avg_fill_price, stop_price,
               time_in_force, created_at, updated_at, filled_at
        FROM orders
        WHERE id = $1
        "#,
    )
    .bind(order_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(row) => Ok(Json(OrderResponse {
            id: row.id,
            client_order_id: row.client_order_id,
            market_id: row.market_id,
            outcome: row.outcome,
            side: parse_order_side(&row.side),
            order_type: parse_order_type(&row.order_type),
            status: parse_order_status(&row.status),
            quantity: row.quantity,
            filled_quantity: row.filled_quantity,
            remaining_quantity: row.quantity - row.filled_quantity,
            price: row.price,
            avg_fill_price: row.avg_fill_price,
            stop_price: row.stop_price,
            time_in_force: row.time_in_force,
            created_at: row.created_at,
            updated_at: row.updated_at,
            filled_at: row.filled_at,
        })),
        None => Err(ApiError::NotFound(format!("Order {} not found", order_id))),
    }
}

fn parse_order_side(s: &str) -> OrderSide {
    match s {
        "buy" => OrderSide::Buy,
        "sell" => OrderSide::Sell,
        _ => OrderSide::Buy,
    }
}

fn parse_order_type(s: &str) -> OrderType {
    match s {
        "market" => OrderType::Market,
        "limit" => OrderType::Limit,
        "stop_loss" => OrderType::StopLoss,
        "take_profit" => OrderType::TakeProfit,
        _ => OrderType::Market,
    }
}

fn parse_order_status(s: &str) -> OrderStatus {
    match s {
        "pending" => OrderStatus::Pending,
        "open" => OrderStatus::Open,
        "partially_filled" => OrderStatus::PartiallyFilled,
        "filled" => OrderStatus::Filled,
        "cancelled" => OrderStatus::Cancelled,
        "rejected" => OrderStatus::Rejected,
        "expired" => OrderStatus::Expired,
        _ => OrderStatus::Pending,
    }
}

/// Cancel an order.
#[utoipa::path(
    post,
    path = "/api/v1/orders/{order_id}/cancel",
    tag = "trading",
    params(
        ("order_id" = Uuid, Path, description = "Order identifier")
    ),
    responses(
        (status = 200, description = "Order cancelled", body = OrderResponse),
        (status = 404, description = "Order not found"),
        (status = 400, description = "Order cannot be cancelled")
    )
)]
pub async fn cancel_order(
    State(state): State<Arc<AppState>>,
    Path(order_id): Path<Uuid>,
) -> ApiResult<Json<OrderResponse>> {
    // First, fetch the order
    let row: Option<OrderRow> = sqlx::query_as(
        r#"
        SELECT id, client_order_id, market_id, outcome, side, order_type, status,
               quantity, filled_quantity, price, avg_fill_price, stop_price,
               time_in_force, created_at, updated_at, filled_at
        FROM orders
        WHERE id = $1
        "#,
    )
    .bind(order_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let row = match row {
        Some(r) => r,
        None => return Err(ApiError::NotFound(format!("Order {} not found", order_id))),
    };

    // Check if order can be cancelled
    let status = parse_order_status(&row.status);
    if status == OrderStatus::Filled || status == OrderStatus::Cancelled {
        return Err(ApiError::BadRequest(format!(
            "Order cannot be cancelled (status: {:?})",
            status
        )));
    }

    // Update order status
    let now = Utc::now();
    sqlx::query("UPDATE orders SET status = 'cancelled', updated_at = $1 WHERE id = $2")
        .bind(now)
        .bind(order_id)
        .execute(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(OrderResponse {
        id: row.id,
        client_order_id: row.client_order_id,
        market_id: row.market_id,
        outcome: row.outcome,
        side: parse_order_side(&row.side),
        order_type: parse_order_type(&row.order_type),
        status: OrderStatus::Cancelled,
        quantity: row.quantity,
        filled_quantity: row.filled_quantity,
        remaining_quantity: row.quantity - row.filled_quantity,
        price: row.price,
        avg_fill_price: row.avg_fill_price,
        stop_price: row.stop_price,
        time_in_force: row.time_in_force,
        created_at: row.created_at,
        updated_at: now,
        filled_at: row.filled_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_request_serialization() {
        let request = PlaceOrderRequest {
            market_id: "market1".to_string(),
            outcome: "yes".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            quantity: Decimal::new(100, 0),
            price: Some(Decimal::new(55, 2)),
            stop_price: None,
            time_in_force: "GTC".to_string(),
            client_order_id: Some("client123".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("market1"));
        assert!(json.contains("buy"));
        assert!(json.contains("limit"));
    }

    #[test]
    fn test_order_response_serialization() {
        let order = OrderResponse {
            id: Uuid::new_v4(),
            client_order_id: Some("test".to_string()),
            market_id: "market1".to_string(),
            outcome: "yes".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            status: OrderStatus::Open,
            quantity: Decimal::new(100, 0),
            filled_quantity: Decimal::new(50, 0),
            remaining_quantity: Decimal::new(50, 0),
            price: Some(Decimal::new(55, 2)),
            avg_fill_price: Some(Decimal::new(54, 2)),
            stop_price: None,
            time_in_force: "GTC".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            filled_at: None,
        };

        let json = serde_json::to_string(&order).unwrap();
        assert!(json.contains("open"));
        assert!(json.contains("market1"));
    }

    #[test]
    fn test_validate_order_request() {
        // Valid request
        let valid = PlaceOrderRequest {
            market_id: "market1".to_string(),
            outcome: "yes".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            quantity: Decimal::new(100, 0),
            price: None,
            stop_price: None,
            time_in_force: "GTC".to_string(),
            client_order_id: None,
        };
        assert!(validate_order_request(&valid).is_ok());

        // Invalid outcome
        let invalid_outcome = PlaceOrderRequest {
            outcome: "maybe".to_string(),
            ..valid.clone()
        };
        assert!(validate_order_request(&invalid_outcome).is_err());

        // Zero quantity
        let zero_qty = PlaceOrderRequest {
            quantity: Decimal::ZERO,
            ..valid.clone()
        };
        assert!(validate_order_request(&zero_qty).is_err());

        // Quantity exceeds maximum
        let huge_qty = PlaceOrderRequest {
            quantity: Decimal::new(2_000_000, 0),
            ..valid.clone()
        };
        assert!(validate_order_request(&huge_qty).is_err());

        // Limit order without price
        let limit_no_price = PlaceOrderRequest {
            order_type: OrderType::Limit,
            price: None,
            ..valid.clone()
        };
        assert!(validate_order_request(&limit_no_price).is_err());

        // Empty market_id
        let empty_market = PlaceOrderRequest {
            market_id: "".to_string(),
            ..valid.clone()
        };
        assert!(validate_order_request(&empty_market).is_err());

        // Whitespace-only market_id
        let whitespace_market = PlaceOrderRequest {
            market_id: "   ".to_string(),
            ..valid.clone()
        };
        assert!(validate_order_request(&whitespace_market).is_err());

        // Invalid time_in_force
        let bad_tif = PlaceOrderRequest {
            time_in_force: "INVALID".to_string(),
            ..valid.clone()
        };
        assert!(validate_order_request(&bad_tif).is_err());

        // Valid time_in_force values
        for tif in &["GTC", "IOC", "FOK", "GTD"] {
            let req = PlaceOrderRequest {
                time_in_force: tif.to_string(),
                ..valid.clone()
            };
            assert!(validate_order_request(&req).is_ok());
        }

        // Stop price out of range
        let bad_stop = PlaceOrderRequest {
            order_type: OrderType::StopLoss,
            stop_price: Some(Decimal::new(150, 2)), // 1.50 — out of range
            ..valid.clone()
        };
        assert!(validate_order_request(&bad_stop).is_err());

        // Valid stop-loss order
        let good_stop = PlaceOrderRequest {
            order_type: OrderType::StopLoss,
            stop_price: Some(Decimal::new(40, 2)), // 0.40
            ..valid.clone()
        };
        assert!(validate_order_request(&good_stop).is_ok());

        // client_order_id too long
        let long_client_id = PlaceOrderRequest {
            client_order_id: Some("x".repeat(200)),
            ..valid.clone()
        };
        assert!(validate_order_request(&long_client_id).is_err());
    }

    #[test]
    fn test_parse_functions() {
        assert_eq!(parse_order_side("buy"), OrderSide::Buy);
        assert_eq!(parse_order_side("sell"), OrderSide::Sell);

        assert_eq!(parse_order_type("market"), OrderType::Market);
        assert_eq!(parse_order_type("limit"), OrderType::Limit);
        assert_eq!(parse_order_type("stop_loss"), OrderType::StopLoss);

        assert_eq!(parse_order_status("pending"), OrderStatus::Pending);
        assert_eq!(parse_order_status("filled"), OrderStatus::Filled);
        assert_eq!(parse_order_status("cancelled"), OrderStatus::Cancelled);
    }
}
