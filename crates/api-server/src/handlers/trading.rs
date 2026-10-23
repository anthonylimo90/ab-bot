//! Trading and order execution handlers.

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use tracing::{info, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use polymarket_core::types::{
    LimitOrder as CoreLimitOrder, Market, MarketOrder as CoreMarketOrder,
    OrderSide as CoreOrderSide, OrderStatus as CoreOrderStatus,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::websocket::{SignalType, SignalUpdate};
use crate::workspace_scope::{
    load_canonical_workspace_flags, WorkspaceReadAccess, WorkspaceTradeAccess,
};

#[cfg(test)]
#[path = "trading/intent_tests.rs"]
mod intent_tests;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange_order_id: Option<String>,
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
    /// Fill timestamp: when the order filled completely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filled_at: Option<DateTime<Utc>>,
    /// Why the exchange or the executor rejected the order, when it did
    /// (in the response to placing it; not stored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

#[derive(Debug, FromRow)]
struct OrderRow {
    id: Uuid,
    exchange_order_id: Option<String>,
    execution_mode: String,
    wallet_address: Option<String>,
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

impl From<OrderRow> for OrderResponse {
    fn from(row: OrderRow) -> Self {
        Self {
            id: row.id,
            exchange_order_id: row.exchange_order_id,
            client_order_id: row.client_order_id,
            market_id: row.market_id,
            outcome: row.outcome,
            side: parse_order_side(&row.side),
            order_type: parse_order_type(&row.order_type),
            status: parse_order_status(&row.status),
            quantity: row.quantity,
            filled_quantity: row.filled_quantity,
            // A buy may fill a little over its size (fee shares).
            remaining_quantity: (row.quantity - row.filled_quantity).max(Decimal::ZERO),
            price: row.price,
            avg_fill_price: row.avg_fill_price,
            stop_price: row.stop_price,
            time_in_force: row.time_in_force,
            created_at: row.created_at,
            updated_at: row.updated_at,
            filled_at: row.filled_at,
            rejection_reason: None,
        }
    }
}

async fn load_order(pool: &PgPool, id: Uuid) -> ApiResult<OrderRow> {
    sqlx::query_as("SELECT * FROM orders WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Order {id} not found")))
}

async fn reserve_order(
    pool: &PgPool,
    request: &PlaceOrderRequest,
    live: bool,
    wallet: Option<&str>,
    max_exposure: Decimal,
) -> ApiResult<(Uuid, bool)> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('position-exposure'))")
        .execute(&mut *tx)
        .await?;
    let id = Uuid::new_v4();
    let side = serde_json::to_value(request.side)?
        .as_str()
        .unwrap()
        .to_owned();
    let kind = serde_json::to_value(request.order_type)?
        .as_str()
        .unwrap()
        .to_owned();
    let inserted: Option<Uuid> = sqlx::query_scalar("INSERT INTO orders (id, client_order_id, market_id, outcome, side, order_type, status, quantity, price, stop_price, time_in_force, execution_mode, wallet_address) VALUES ($1,$2,$3,$4,$5,$6,'pending',$7,$8,$9,$10,$11,$12) ON CONFLICT (client_order_id) WHERE client_order_id IS NOT NULL DO NOTHING RETURNING id")
        .bind(id).bind(&request.client_order_id).bind(&request.market_id).bind(&request.outcome)
        .bind(&side).bind(&kind).bind(request.quantity).bind(if request.order_type == OrderType::Market { None } else { request.price }).bind(request.stop_price).bind(&request.time_in_force).bind(if live { "live" } else { "paper" }).bind(wallet)
        .fetch_optional(&mut *tx).await?;
    if inserted.is_some() {
        if request.side == OrderSide::Buy {
            let exposure: Decimal =
                sqlx::query_scalar(polymarket_core::db::positions::TOTAL_ACTIVE_EXPOSURE_SQL)
                    .fetch_one(&mut *tx)
                    .await?;
            if exposure > max_exposure {
                return Err(ApiError::BadRequest(
                    "Order would exceed shared exposure limit".into(),
                ));
            }
        } else {
            let available: Decimal = sqlx::query_scalar("SELECT COALESCE((SELECT quantity FROM manual_inventory WHERE execution_mode=$1 AND wallet_address=COALESCE(LOWER($2),'unknown') AND market_id=$3 AND outcome=$4),0) - COALESCE((SELECT SUM(quantity-filled_quantity) FROM orders WHERE execution_mode=$1 AND wallet_address IS NOT DISTINCT FROM $2 AND market_id=$3 AND outcome=$4 AND side='sell' AND status IN ('pending','open','partially_filled') AND id<>$5),0)")
                .bind(if live { "live" } else { "paper" }).bind(wallet).bind(&request.market_id).bind(&request.outcome).bind(id)
                .fetch_one(&mut *tx).await?;
            if request.quantity > available {
                return Err(ApiError::BadRequest("Sale exceeds available manual inventory; use position exit for automated holdings".into()));
            }
        }
        tx.commit().await?;
        return Ok((id, true));
    }
    let row: OrderRow = sqlx::query_as("SELECT * FROM orders WHERE client_order_id=$1")
        .bind(&request.client_order_id)
        .fetch_one(&mut *tx)
        .await?;
    if row.market_id != request.market_id
        || row.outcome != request.outcome
        || row.side != side
        || row.order_type != kind
        || row.quantity != request.quantity
        || row.price
            != if request.order_type == OrderType::Market {
                None
            } else {
                request.price
            }
        || row.stop_price != request.stop_price
        || row.time_in_force != request.time_in_force
    {
        return Err(ApiError::Conflict(
            "client_order_id was already used for a different order".into(),
        ));
    }
    Ok((row.id, false))
}

/// Place a new order.
#[utoipa::path(
    post,
    path = "/api/v1/orders",
    tag = "trading",
    request_body = PlaceOrderRequest,
    responses(
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 201, description = "Order placed", body = OrderResponse),
        (status = 400, description = "Invalid order request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn place_order(
    _access: WorkspaceTradeAccess,
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<auth::Claims>,
    Json(mut request): Json<PlaceOrderRequest>,
) -> ApiResult<Json<OrderResponse>> {
    if request.order_type == OrderType::Market && request.time_in_force == "GTC" {
        request.time_in_force = "FOK".into();
    }
    validate_order_request(&request)?;
    // Held until the order's outcome is recorded; refused once shutdown began.
    let Some(operation) = state.order_executor.begin_operation() else {
        return Err(ApiError::ServiceUnavailable(SHUTTING_DOWN.into()));
    };
    // Runs to the end even if the client goes away: an order handed to the
    // exchange must have its outcome recorded, not be dropped mid-request.
    tokio::spawn(async move {
        let _operation = operation;
        let requested = serde_json::to_value(&request).unwrap_or_default();
        let result = reserve_and_execute_order(state.clone(), request).await;
        super::audit_change(
            &state,
            Uuid::parse_str(&claims.sub).unwrap_or_default(),
            ::auth::AuditAction::Custom("order_placed".into()),
            "orders",
            serde_json::json!({
                "request": requested,
                "order_id": result.as_ref().ok().map(|order| order.id),
                "status": result.as_ref().ok().map(|order| order.status),
                "error": result.as_ref().err().map(ToString::to_string),
            }),
        )
        .await;
        result
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Order task failed: {e}")))?
}

/// What a request refused at shutdown is told. It sent nothing, but an
/// earlier request with the same client order ID may have: retrying with that
/// ID finds the earlier order instead of placing another.
pub(crate) const SHUTTING_DOWN: &str = "The server is shutting down and this request sent \
     nothing; retry it with the same client order ID once the server is back";

async fn reserve_and_execute_order(
    state: Arc<AppState>,
    request: PlaceOrderRequest,
) -> ApiResult<Json<OrderResponse>> {
    let wallet = state.order_executor.wallet_address().await;
    let max_exposure = if let Some(cfg) = &state.arb_executor_config {
        cfg.read().await.max_total_exposure
    } else {
        crate::arb_executor::ArbExecutorConfig::from_env().max_total_exposure
    };
    let (order_id, fresh) = reserve_order(
        &state.pool,
        &request,
        state.order_executor.is_live(),
        wallet.as_deref(),
        max_exposure,
    )
    .await?;
    if !fresh {
        return Ok(Json(load_order(&state.pool, order_id).await?.into()));
    }
    let result = execute_reserved_order(&state, request, order_id).await;
    if result.is_err() {
        reject_unsubmitted_order(&state.pool, order_id).await?;
    }
    result
}

async fn reject_unsubmitted_order(pool: &PgPool, id: Uuid) -> ApiResult<()> {
    sqlx::query("UPDATE orders SET status='rejected', updated_at=NOW() WHERE id=$1 AND status='pending' AND NOT EXISTS(SELECT 1 FROM execution_intents WHERE id=$1)")
        .bind(id).execute(pool).await?;
    Ok(())
}

async fn execute_reserved_order(
    state: &Arc<AppState>,
    request: PlaceOrderRequest,
    order_id: Uuid,
) -> ApiResult<Json<OrderResponse>> {
    let origin = load_order(&state.pool, order_id).await?;
    let workspace_live_enabled = load_canonical_workspace_flags(&state.pool)
        .await?
        .map(|flags| flags.live_trading_enabled)
        .ok_or_else(|| ApiError::NotFound("Canonical workspace not found".into()))?;

    // The order took this process's live mode when it was reserved; a
    // process that has not yet taken up an operator switching live trading
    // off (it re-reads every few seconds) must not send it.
    if origin.execution_mode == "live"
        && !workspace_live_enabled
        && !crate::runtime_sync::env_enabled("LIVE_TRADING")
    {
        return Err(ApiError::Conflict(
            "Live trading is switched off; nothing was sent".into(),
        ));
    }

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

    // A tripped circuit breaker refuses buys. A sale only reduces exposure,
    // and manual holdings have no automatic exit: during a trip (a latched
    // one holds until reset) selling them is how an operator de-risks.
    if request.side == OrderSide::Buy && !state.circuit_breaker.can_trade().await {
        // A hold (settings unread, closes uncounted) blocks whatever the trip.
        let holds = state.circuit_breaker.entry_holds();
        if !holds.is_empty() {
            return Err(ApiError::ServiceUnavailable(format!(
                "New entries are held: {}",
                holds.join("; ")
            )));
        }
        let cb_state = state.circuit_breaker.state().await;
        if !cb_state.tripped {
            return Err(ApiError::ServiceUnavailable(
                "The circuit breaker's trip has just been lifted; try again".into(),
            ));
        }
        let max_daily_loss = state.circuit_breaker.config().await.max_daily_loss;
        // Counters of a day that has ended are not this day's loss.
        let day_pnl = if cb_state.last_reset_date < Utc::now().date_naive() {
            Decimal::ZERO
        } else {
            cb_state.daily_pnl
        };
        let day_loss_over_limit = day_pnl < Decimal::ZERO && -day_pnl >= max_daily_loss;
        let resumes = match cb_state.resume_at {
            None => "Holds until an owner or admin resets it".to_string(),
            // A trip holds while the day's loss is over the limit: until
            // the UTC day ends.
            Some(at) if day_loss_over_limit => {
                let day_ends = Utc::now()
                    .date_naive()
                    .succ_opt()
                    .map(|day| day.and_time(chrono::NaiveTime::MIN).and_utc())
                    .unwrap_or(at);
                format!(
                    "Holds until {}: the day's loss is over the limit",
                    at.max(day_ends).to_rfc3339()
                )
            }
            Some(at) if at > Utc::now() => format!("Resumes at {}", at.to_rfc3339()),
            // A process that does not lead follows the trading process
            // within seconds of it lifting the trip.
            Some(_) => "The cooldown is over; trading resumes once the trading process lifts \
                        the trip"
                .to_string(),
        };
        return Err(ApiError::ServiceUnavailable(format!(
            "Trading halted: {:?}. {resumes}",
            cb_state
                .trip_reason
                .unwrap_or(risk_manager::circuit_breaker::TripReason::Manual),
        )));
    }

    let now = Utc::now();

    // Convert to core types
    let core_side = match request.side {
        OrderSide::Buy => CoreOrderSide::Buy,
        OrderSide::Sell => CoreOrderSide::Sell,
    };
    let outcome_token_id = resolve_market_outcome_token_id(
        state.clob_client.as_ref(),
        &request.market_id,
        &request.outcome,
    )
    .await?;

    // Execute based on order type
    let report = match request.order_type {
        OrderType::Market => {
            let expected_price = load_expected_market_price(
                state.clob_client.as_ref(),
                &outcome_token_id,
                core_side,
            )
            .await?;
            let mut order = CoreMarketOrder::new(
                request.market_id.clone(),
                outcome_token_id.clone(),
                core_side,
                request.quantity,
            )
            .with_expected_price(expected_price)
            .with_slippage(state.order_executor.default_slippage());
            order.id = order_id;
            state
                .order_executor
                .execute_reserved_market_order(
                    order,
                    &origin.execution_mode,
                    origin.wallet_address.as_deref(),
                )
                .await
                .map_err(manual_execution_error)?
        }
        OrderType::Limit => {
            let price = request
                .price
                .ok_or(ApiError::BadRequest("Limit orders require a price".into()))?;
            let mut order = CoreLimitOrder::new(
                request.market_id.clone(),
                outcome_token_id.clone(),
                core_side,
                price,
                request.quantity,
            );
            order.id = order_id;
            if request.time_in_force == "FOK" {
                order = order.fok();
            }
            state
                .order_executor
                .execute_reserved_limit_order(
                    order,
                    &origin.execution_mode,
                    origin.wallet_address.as_deref(),
                )
                .await
                .map_err(manual_execution_error)?
        }
        OrderType::StopLoss | OrderType::TakeProfit => {
            return Err(ApiError::BadRequest(
                "Protective order types are not supported".into(),
            ));
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
        // Filled only in part: no fill time until it fills completely.
        CoreOrderStatus::PartiallyFilled => (
            OrderStatus::PartiallyFilled,
            report.filled_quantity,
            Some(report.average_price),
            None,
        ),
        CoreOrderStatus::Rejected => (OrderStatus::Rejected, Decimal::ZERO, None, None),
        _ => (
            if report.exchange_order_id.is_some() {
                OrderStatus::Open
            } else {
                OrderStatus::Pending
            },
            Decimal::ZERO,
            None,
            None,
        ),
    };

    // The row was reserved before submission. A persistence failure leaves a
    // replayable pending intent rather than allowing the caller to trade twice.
    sqlx::query("UPDATE orders SET status=$2, filled_quantity=$3, avg_fill_price=$4, filled_at=$5, updated_at=NOW(), exchange_order_id=COALESCE($6, (SELECT exchange_order_id FROM execution_intents WHERE id=$1)) WHERE id=$1")
        .bind(order_id).bind(serde_json::to_value(status)?.as_str().unwrap())
        .bind(filled_qty).bind(avg_price).bind(filled_at).bind(&report.exchange_order_id).execute(&state.pool).await?;

    // As an exchange's refusal is reported (its first 300 characters).
    let rejection_reason = (status == OrderStatus::Rejected).then(|| {
        report
            .error_message
            .as_deref()
            .unwrap_or("rejected without a reason")
            .chars()
            .take(300)
            .collect::<String>()
    });
    if let Some(reason) = &rejection_reason {
        warn!(order_id = %order_id, market = %request.market_id, %reason, "Manual order rejected");
    }

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

    // A fill is a cash/inventory movement, not realized P&L. The exit handler
    // records realized results only when a cost-basis-tracked position closes.

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
        exchange_order_id: report.exchange_order_id,
        client_order_id: request.client_order_id,
        market_id: request.market_id,
        outcome: request.outcome,
        side: request.side,
        order_type: request.order_type,
        status,
        quantity: request.quantity,
        filled_quantity: filled_qty,
        remaining_quantity: (request.quantity - filled_qty).max(Decimal::ZERO),
        price: request.price,
        avg_fill_price: avg_price,
        stop_price: request.stop_price,
        time_in_force: request.time_in_force,
        created_at: now,
        updated_at: now,
        filled_at,
        rejection_reason,
    }))
}

/// Maximum order quantity to prevent accidental enormous orders.
const MAX_ORDER_QUANTITY: Decimal = Decimal::from_parts(1_000_000, 0, 0, false, 0); // 1,000,000

fn resolve_market_outcome_token(market: &Market, requested_outcome: &str) -> Option<String> {
    market
        .outcomes
        .iter()
        .find(|outcome| outcome.name.eq_ignore_ascii_case(requested_outcome.trim()))
        .map(|outcome| outcome.token_id.clone())
}

async fn resolve_market_outcome_token_id(
    clob_client: &polymarket_core::api::ClobClient,
    market_id: &str,
    requested_outcome: &str,
) -> ApiResult<String> {
    let market = clob_client
        .get_market_by_id(market_id)
        .await
        .map_err(|error| {
            ApiError::Internal(format!("Failed to load market {}: {}", market_id, error))
        })?;

    resolve_market_outcome_token(&market, requested_outcome).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "Outcome '{}' is not available for market {}",
            requested_outcome, market_id
        ))
    })
}

async fn load_expected_market_price(
    clob_client: &polymarket_core::api::ClobClient,
    token_id: &str,
    side: CoreOrderSide,
) -> ApiResult<Decimal> {
    let book = clob_client
        .get_order_book(token_id)
        .await
        .map_err(|error| {
            ApiError::Internal(format!(
                "Failed to load orderbook for {}: {}",
                token_id, error
            ))
        })?;

    let level = match side {
        CoreOrderSide::Buy => book.best_ask_level(),
        CoreOrderSide::Sell => book.best_bid_level(),
    };

    level.map(|price_level| price_level.price).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "No {} liquidity is currently available for outcome {}",
            match side {
                CoreOrderSide::Buy => "ask",
                CoreOrderSide::Sell => "bid",
            },
            token_id
        ))
    })
}

fn validate_order_request(request: &PlaceOrderRequest) -> ApiResult<()> {
    if matches!(
        request.order_type,
        OrderType::StopLoss | OrderType::TakeProfit
    ) {
        return Err(ApiError::BadRequest(
            "Protective order types are not supported".into(),
        ));
    }
    if request
        .client_order_id
        .as_deref()
        .is_none_or(|key| key.trim().is_empty() || key.len() > 255)
    {
        return Err(ApiError::BadRequest(
            "A nonempty client_order_id is required for safe retries".into(),
        ));
    }
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

    if request.quantity.normalize().scale() > 2 {
        return Err(ApiError::BadRequest(
            "Share quantity supports at most two decimal places".into(),
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
        "GTC" | "FOK" => {}
        _ => {
            return Err(ApiError::BadRequest(
                "Supported time_in_force values are GTC and FOK".to_string(),
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
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 200, description = "Order details", body = OrderResponse),
        (status = 404, description = "Order not found")
    )
)]
pub async fn get_order_status(
    _access: WorkspaceReadAccess,
    State(state): State<Arc<AppState>>,
    Path(order_id): Path<Uuid>,
) -> ApiResult<Json<OrderResponse>> {
    let row = load_order(&state.pool, order_id).await?;
    if row.execution_mode == "live"
        && matches!(row.status.as_str(), "pending" | "open" | "partially_filled")
    {
        if let Some(exchange_id) = row.exchange_order_id.as_deref() {
            let snapshot = state
                .order_executor
                .get_exchange_order(exchange_id)
                .await
                .map_err(|_| {
                    ApiError::ServiceUnavailable("Exchange order reconciliation unavailable".into())
                })?;
            let fill = confirmed_fill(&state, &row, &snapshot).await;
            apply_exchange_snapshot(&state.pool, &row, &snapshot, fill).await?;
            return Ok(Json(load_order(&state.pool, order_id).await?.into()));
        }
    }
    Ok(Json(row.into()))
}

/// What the order's settled trades filled and at what average price, or
/// `None` while some of its trades are still settling (or cannot be read).
async fn confirmed_fill(
    state: &AppState,
    row: &OrderRow,
    snapshot: &polymarket_core::api::clob::ExchangeOrder,
) -> Option<(Decimal, Decimal)> {
    let maker = row.wallet_address.as_deref()?;
    match state
        .order_executor
        .confirmed_exchange_fill(maker, snapshot)
        .await
    {
        Ok(fill) => Some(fill),
        Err(error) => {
            tracing::debug!(%error, order_id = %row.id, "Order's trades are not settled yet");
            None
        }
    }
}

/// Bring a live manual order up to the exchange's snapshot. Fills are booked
/// only from settled trades, at their average price (`fill`); `None`, while
/// some are still settling, books nothing and leaves the order as it is (its
/// cash stays reserved): the reconciler or a later read books them.
async fn apply_exchange_snapshot(
    pool: &PgPool,
    row: &OrderRow,
    snapshot: &polymarket_core::api::clob::ExchangeOrder,
    fill: Option<(Decimal, Decimal)>,
) -> ApiResult<()> {
    let matched = snapshot
        .size_matched
        .parse::<Decimal>()
        .map_err(|_| ApiError::ServiceUnavailable("Invalid exchange fill quantity".into()))?;
    let size = snapshot
        .original_size
        .parse::<Decimal>()
        .map_err(|_| ApiError::ServiceUnavailable("Invalid exchange size".into()))?;
    let mismatch =
        || ApiError::ServiceUnavailable("Exchange order does not match stored intent".into());
    if Some(snapshot.id.as_str()) != row.exchange_order_id.as_deref()
        || !snapshot_fits(row, size, matched)
    {
        return Err(mismatch());
    }
    let Some((confirmed, price)) = fill else {
        return Ok(());
    };
    if confirmed > matched {
        return Err(mismatch());
    }
    // Fills never decrease (the inventory trigger refuses it): one already
    // booked when the order was placed stays, at its stored price.
    let filled = confirmed.max(row.filled_quantity);
    let avg_fill_price = if confirmed > Decimal::ZERO && confirmed >= row.filled_quantity {
        Some(price)
    } else {
        row.avg_fill_price
    };
    let state = snapshot.state();
    let status = match state.as_str() {
        _ if snapshot_complete(row, size, filled) => "filled",
        "expired" => "expired",
        _ if snapshot.is_closed() => "cancelled",
        _ if filled > Decimal::ZERO => "partially_filled",
        "live" | "matched" | "delayed" | "unmatched" => "open",
        _ => {
            return Err(ApiError::ServiceUnavailable(
                "Unrecognized exchange order status".into(),
            ))
        }
    };
    sqlx::query("UPDATE orders SET status=$2, avg_fill_price=$4, filled_quantity=$3, filled_at=CASE WHEN $2='filled' THEN COALESCE(filled_at,NOW()) ELSE filled_at END, updated_at=NOW() WHERE id=$1 AND filled_quantity<=$3 AND (status NOT IN ('filled','cancelled','expired') OR $2='filled')")
        .bind(row.id).bind(status).bind(filled).bind(avg_fill_price).execute(pool).await?;
    Ok(())
}

/// Whether an exchange snapshot of size `size`, `filled` matched, can be
/// this manual order's. A limit order is signed for exactly its quantity; a
/// market order's signed size is its quantity rounded as the exchange
/// accepts it, so it is not compared. A buy may receive more shares than it
/// asked for (the exchange passes a better price on as shares); a sell never
/// parts with more than its quantity. Confirmed fills never go back.
fn snapshot_fits(row: &OrderRow, size: Decimal, filled: Decimal) -> bool {
    (row.order_type == "market" || size == row.quantity)
        && filled >= row.filled_quantity
        && (row.side == "buy" || filled <= row.quantity)
}

/// Whether a snapshot shows the order filled in full: all of the size the
/// exchange holds it to, or all of its quantity.
fn snapshot_complete(row: &OrderRow, size: Decimal, filled: Decimal) -> bool {
    filled > Decimal::ZERO && (filled >= size || filled >= row.quantity)
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
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 200, description = "Order cancelled", body = OrderResponse),
        (status = 404, description = "Order not found"),
        (status = 409, description = "Order already filled, rejected or expired, or its submission is unresolved"),
        (status = 503, description = "The cancellation's outcome needs exchange reconciliation")
    )
)]
pub async fn cancel_order(
    _access: WorkspaceTradeAccess,
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<auth::Claims>,
    Path(order_id): Path<Uuid>,
) -> ApiResult<Json<OrderResponse>> {
    // Waited for by a shutdown's drain, but never refused by it: a
    // cancellation only takes risk away.
    let operation = state.order_executor.track_operation();
    // As placing an order: runs to the end even if the client goes away, so
    // a cancellation the exchange took is recorded, not left open.
    tokio::spawn(async move {
        let _operation = operation;
        let result = cancel_order_inner(&state, order_id).await;
        super::audit_change(
            &state,
            Uuid::parse_str(&claims.sub).unwrap_or_default(),
            ::auth::AuditAction::Custom("order_cancel_requested".into()),
            format!("orders/{order_id}"),
            serde_json::json!({
                "status": result.as_ref().ok().map(|order| order.status),
                "error": result.as_ref().err().map(ToString::to_string),
            }),
        )
        .await;
        result
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Cancel task failed: {e}")))?
}

async fn cancel_order_inner(
    state: &Arc<AppState>,
    order_id: Uuid,
) -> ApiResult<Json<OrderResponse>> {
    let row = load_order(&state.pool, order_id).await?;
    if matches!(row.status.as_str(), "filled" | "rejected" | "expired") {
        return Err(ApiError::Conflict(format!(
            "Order is already {}",
            row.status
        )));
    }
    if row.status == "cancelled" {
        return Ok(Json(row.into()));
    }
    match row.execution_mode.as_str() {
        "paper" => {
            sqlx::query("UPDATE orders SET status='cancelled', updated_at=NOW() WHERE id=$1 AND status IN ('pending','open','partially_filled')")
                .bind(order_id).execute(&state.pool).await?;
        }
        "live" => {
            let exchange_id = row.exchange_order_id.as_deref().ok_or_else(|| ApiError::Conflict("Submission is unresolved; exchange identity must be reconciled before cancellation".into()))?;
            let cancel_result = state
                .order_executor
                .cancel_exchange_order(exchange_id)
                .await;
            // A fill may win the cancellation race. Always read confirmed fills,
            // including after an unacknowledged or repeated cancellation request.
            let snapshot = state
                .order_executor
                .get_exchange_order(exchange_id)
                .await
                .map_err(|_| {
                    ApiError::ServiceUnavailable(
                        "Cancellation outcome requires exchange reconciliation".into(),
                    )
                })?;
            let fill = confirmed_fill(state, &row, &snapshot).await;
            apply_exchange_snapshot(&state.pool, &row, &snapshot, fill).await?;
            let current = load_order(&state.pool, order_id).await?;
            if !matches!(current.status.as_str(), "cancelled" | "filled" | "expired") {
                return Err(ApiError::Conflict(if cancel_result.is_err() {
                    "Exchange did not confirm cancellation".into()
                } else {
                    "Cancellation is awaiting exchange confirmation".into()
                }));
            }
        }
        _ => {
            return Err(ApiError::Conflict(
                "Historical order execution mode is unknown; reconcile with the exchange".into(),
            ))
        }
    }
    Ok(Json(load_order(&state.pool, order_id).await?.into()))
}

/// Settle a manual order the exchange never received and release its fence.
pub(crate) async fn reject_never_executed_manual_intent(
    pool: &PgPool,
    id: Uuid,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('position-exposure'))")
        .execute(&mut *tx)
        .await?;
    let row: OrderRow = sqlx::query_as("SELECT * FROM orders WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if row.execution_mode != "live" || row.filled_quantity > Decimal::ZERO {
        anyhow::bail!("Recorded fills conflict with an order the exchange never received");
    }
    sqlx::query("UPDATE orders SET status='rejected',updated_at=NOW() WHERE id=$1 AND status IN ('pending','open')")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE execution_intents SET status='rejected',updated_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Apply a terminal authenticated snapshot and release its fence atomically.
/// `quantity` is what confirmed trades filled: matched trades that failed to
/// settle are not part of it.
pub(crate) async fn reconcile_manual_intent(
    pool: &PgPool,
    id: Uuid,
    snapshot: &polymarket_core::api::clob::ExchangeOrder,
    quantity: Decimal,
    price: Decimal,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('position-exposure'))")
        .execute(&mut *tx)
        .await?;
    let row: OrderRow = sqlx::query_as("SELECT * FROM orders WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    let size = snapshot.original_size.parse::<Decimal>()?;
    // Checked against what the exchange matched (a read of the order may
    // already have stored that); booked as what confirmed trades filled.
    let matched = snapshot.size_matched.parse::<Decimal>()?;
    if row.execution_mode != "live" || !snapshot_fits(&row, size, matched) || quantity > matched {
        anyhow::bail!("Snapshot conflicts with stored manual order");
    }
    // Fills never decrease (the inventory trigger refuses it): a match a
    // read of the order already stored stays booked, at its stored price.
    let confirmed = quantity;
    let quantity = confirmed.max(row.filled_quantity);
    let avg_fill_price = if confirmed > Decimal::ZERO && confirmed >= row.filled_quantity {
        Some(price)
    } else {
        row.avg_fill_price
    };
    let status = if snapshot_complete(&row, size, quantity) {
        "filled"
    } else {
        "cancelled"
    };
    sqlx::query("UPDATE orders SET exchange_order_id=$2,status=$3,filled_quantity=$4,avg_fill_price=$5,updated_at=NOW() WHERE id=$1")
        .bind(id).bind(&snapshot.id).bind(status).bind(quantity).bind(avg_fill_price).execute(&mut *tx).await?;
    sqlx::query("UPDATE execution_intents SET status='reconciled',updated_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A client-safe error for a manual order the executor did not complete. The
/// caller must be able to tell an order whose outcome is unknown (never resend
/// it) from one that was certainly not sent; internal detail stays in the log.
fn manual_execution_error(error: anyhow::Error) -> ApiError {
    let text = error.to_string();
    tracing::warn!(error = %text, "Manual order execution failed");
    if text.contains("submission_requires_reconciliation") {
        ApiError::ServiceUnavailable(
            "The order's outcome is not yet known and will be reconciled automatically; \
             do not resubmit it, check its status instead"
                .into(),
        )
    } else if text.contains("live_submission_fence_busy") {
        ApiError::ServiceUnavailable(
            "Another live order is still being submitted; nothing was sent. Retry shortly \
             with a new client_order_id (this one now names the refused attempt)"
                .into(),
        )
    } else if text.contains("nothing was sent") {
        ApiError::ServiceUnavailable(
            "The order could not be prepared in time; nothing was sent. Retry shortly with a \
             new client_order_id (this one now names the refused attempt)"
                .into(),
        )
    } else if text.starts_with("Insufficient ") {
        // Our own balance/allowance preflight: amounts of the caller's wallet.
        ApiError::Validation(text)
    } else if let Some((_, reason)) = text.split_once("Failed to post order: ") {
        // The exchange's rejection of the order (nothing was filled).
        let reason: String = reason.chars().take(300).collect();
        ApiError::Validation(format!("The exchange rejected the order: {reason}"))
    } else if text.contains("Live trading not initialized")
        || text.contains("Live client unavailable")
        || text.contains("No authenticated client")
        || text.contains("not initialized for collateral preflight")
        || text.contains("balance preflight unavailable")
    {
        ApiError::ServiceUnavailable("Live trading is not available right now".into())
    } else {
        ApiError::Internal(format!("Execution error: {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_execution_errors_tell_unknown_outcomes_from_unsent_orders() {
        let map = |text: &str| manual_execution_error(anyhow::anyhow!(text.to_string()));
        let unknown = map("submission_requires_reconciliation: network lost response");
        assert_eq!(
            unknown.status_code(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(unknown.to_string().contains("do not resubmit"));

        let busy = map("live_submission_fence_busy: another live order is still being posted; nothing was sent");
        assert!(busy.to_string().contains("nothing was sent"));
        // Its key now names the refused attempt: a retry needs a new one.
        assert!(busy.to_string().contains("new client_order_id"));
        let late = map("Live order preparation exceeded its deadline; nothing was sent");
        assert!(late.to_string().contains("nothing was sent"));
        assert!(!late.to_string().contains("Another live order"));

        let funds = map("Insufficient USDC balance for live order: available 3, required 5");
        assert_eq!(
            funds.status_code(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(funds.to_string().contains("required 5"));

        let rejected =
            map("refreshable_balance_allowance: Failed to post order: not enough balance");
        assert!(rejected
            .to_string()
            .contains("The exchange rejected the order: not enough balance"));

        let unexpected = map("Failed to derive API credentials: secret detail");
        assert_eq!(
            unexpected.status_code(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

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
            exchange_order_id: None,
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
            rejection_reason: None,
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
            client_order_id: Some("test-intent".into()),
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
            assert_eq!(
                validate_order_request(&req).is_ok(),
                matches!(*tif, "GTC" | "FOK")
            );
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
        assert!(validate_order_request(&good_stop).is_err());

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

    #[test]
    fn test_resolve_market_outcome_token() {
        let market = Market {
            id: "market1".to_string(),
            question: "Question".to_string(),
            description: None,
            outcomes: vec![
                polymarket_core::types::Outcome {
                    id: "yes-token".to_string(),
                    name: "Yes".to_string(),
                    token_id: "yes-token".to_string(),
                    price: None,
                    winner: None,
                },
                polymarket_core::types::Outcome {
                    id: "no-token".to_string(),
                    name: "No".to_string(),
                    token_id: "no-token".to_string(),
                    price: None,
                    winner: None,
                },
            ],
            volume: Decimal::ZERO,
            liquidity: Decimal::ZERO,
            end_date: None,
            resolved: false,
            resolution: None,
            category: None,
            tags: Vec::new(),
            fees_enabled: false,
            fee_type: None,
            fee_schedule: None,
        };

        assert_eq!(
            resolve_market_outcome_token(&market, "yes"),
            Some("yes-token".to_string())
        );
        assert_eq!(
            resolve_market_outcome_token(&market, "NO"),
            Some("no-token".to_string())
        );
        assert_eq!(resolve_market_outcome_token(&market, "maybe"), None);
    }
}
