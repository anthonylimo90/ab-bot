//! Trading and order execution handlers.

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

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
#[derive(Debug, Deserialize, ToSchema)]
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
    Json(request): Json<PlaceOrderRequest>,
) -> ApiResult<Json<OrderResponse>> {
    // Validate the order request
    validate_order_request(&request)?;

    let order_id = Uuid::new_v4();
    let now = Utc::now();

    // Insert order into database
    sqlx::query!(
        r#"
        INSERT INTO orders
        (id, client_order_id, market_id, outcome, side, order_type, status,
         quantity, filled_quantity, price, stop_price, time_in_force,
         created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9, $10, $11, $12, $12)
        "#,
        order_id,
        request.client_order_id,
        request.market_id,
        request.outcome,
        format!("{:?}", request.side).to_lowercase(),
        format!("{:?}", request.order_type).to_lowercase(),
        "pending",
        request.quantity,
        request.price,
        request.stop_price,
        request.time_in_force,
        now
    )
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // In a real implementation, this would submit to the order execution engine
    // For now, we just return the created order

    Ok(Json(OrderResponse {
        id: order_id,
        client_order_id: request.client_order_id,
        market_id: request.market_id,
        outcome: request.outcome,
        side: request.side,
        order_type: request.order_type,
        status: OrderStatus::Pending,
        quantity: request.quantity,
        filled_quantity: Decimal::ZERO,
        remaining_quantity: request.quantity,
        price: request.price,
        avg_fill_price: None,
        stop_price: request.stop_price,
        time_in_force: request.time_in_force,
        created_at: now,
        updated_at: now,
        filled_at: None,
    }))
}

fn validate_order_request(request: &PlaceOrderRequest) -> ApiResult<()> {
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
                "Price must be between 0 and 1".to_string(),
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
    let row = sqlx::query!(
        r#"
        SELECT id, client_order_id, market_id, outcome, side, order_type, status,
               quantity, filled_quantity, price, avg_fill_price, stop_price,
               time_in_force, created_at, updated_at, filled_at
        FROM orders
        WHERE id = $1
        "#,
        order_id
    )
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
    let row = sqlx::query!(
        r#"
        SELECT id, client_order_id, market_id, outcome, side, order_type, status,
               quantity, filled_quantity, price, avg_fill_price, stop_price,
               time_in_force, created_at, updated_at, filled_at
        FROM orders
        WHERE id = $1
        "#,
        order_id
    )
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
    sqlx::query!(
        "UPDATE orders SET status = 'cancelled', updated_at = $1 WHERE id = $2",
        now,
        order_id
    )
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

        // Limit order without price
        let limit_no_price = PlaceOrderRequest {
            order_type: OrderType::Limit,
            price: None,
            ..valid.clone()
        };
        assert!(validate_order_request(&limit_no_price).is_err());
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
