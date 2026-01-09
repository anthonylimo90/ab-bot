//! API route definitions.

use axum::routing::{get, post, put, delete};
use axum::Router;
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::handlers::{health, markets, positions, wallets, trading, backtest};
use crate::state::AppState;
use crate::websocket;

/// OpenAPI documentation.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Polymarket Trading API",
        version = "1.0.0",
        description = "REST and WebSocket API for Polymarket trading platform"
    ),
    paths(
        health::health_check,
        health::readiness,
        markets::list_markets,
        markets::get_market,
        markets::get_market_orderbook,
        positions::list_positions,
        positions::get_position,
        positions::close_position,
        wallets::list_tracked_wallets,
        wallets::add_tracked_wallet,
        wallets::get_wallet,
        wallets::update_wallet,
        wallets::remove_wallet,
        wallets::get_wallet_metrics,
        trading::place_order,
        trading::cancel_order,
        trading::get_order_status,
        backtest::run_backtest,
        backtest::list_backtest_results,
        backtest::get_backtest_result,
    ),
    components(
        schemas(
            crate::error::ErrorResponse,
            crate::websocket::OrderbookUpdate,
            crate::websocket::PositionUpdate,
            crate::websocket::SignalUpdate,
            crate::websocket::PositionUpdateType,
            crate::websocket::SignalType,
            health::HealthResponse,
            markets::MarketResponse,
            markets::OrderbookResponse,
            markets::PriceLevel,
            markets::SpreadInfo,
            positions::PositionResponse,
            positions::ClosePositionRequest,
            wallets::TrackedWalletResponse,
            wallets::AddWalletRequest,
            wallets::UpdateWalletRequest,
            wallets::WalletMetricsResponse,
            trading::PlaceOrderRequest,
            trading::OrderResponse,
            trading::OrderSide,
            trading::OrderType,
            trading::OrderStatus,
            backtest::RunBacktestRequest,
            backtest::BacktestResultResponse,
            backtest::StrategyConfig,
            backtest::SlippageModel,
            backtest::EquityPoint,
        )
    ),
    tags(
        (name = "health", description = "Health check endpoints"),
        (name = "markets", description = "Market data endpoints"),
        (name = "positions", description = "Position management"),
        (name = "wallets", description = "Wallet tracking for copy trading"),
        (name = "trading", description = "Order execution"),
        (name = "backtest", description = "Backtesting operations"),
        (name = "websocket", description = "Real-time WebSocket endpoints"),
    )
)]
pub struct ApiDoc;

/// Create the main router with all routes.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Health endpoints
        .route("/health", get(health::health_check))
        .route("/ready", get(health::readiness))

        // Market endpoints
        .route("/api/v1/markets", get(markets::list_markets))
        .route("/api/v1/markets/:market_id", get(markets::get_market))
        .route("/api/v1/markets/:market_id/orderbook", get(markets::get_market_orderbook))

        // Position endpoints
        .route("/api/v1/positions", get(positions::list_positions))
        .route("/api/v1/positions/:position_id", get(positions::get_position))
        .route("/api/v1/positions/:position_id/close", post(positions::close_position))

        // Wallet endpoints
        .route("/api/v1/wallets", get(wallets::list_tracked_wallets))
        .route("/api/v1/wallets", post(wallets::add_tracked_wallet))
        .route("/api/v1/wallets/:address", get(wallets::get_wallet))
        .route("/api/v1/wallets/:address", put(wallets::update_wallet))
        .route("/api/v1/wallets/:address", delete(wallets::remove_wallet))
        .route("/api/v1/wallets/:address/metrics", get(wallets::get_wallet_metrics))

        // Trading endpoints
        .route("/api/v1/orders", post(trading::place_order))
        .route("/api/v1/orders/:order_id", get(trading::get_order_status))
        .route("/api/v1/orders/:order_id/cancel", post(trading::cancel_order))

        // Backtest endpoints
        .route("/api/v1/backtest", post(backtest::run_backtest))
        .route("/api/v1/backtest/results", get(backtest::list_backtest_results))
        .route("/api/v1/backtest/results/:result_id", get(backtest::get_backtest_result))

        // WebSocket endpoints
        .route("/ws/orderbook", get(websocket::ws_orderbook_handler))
        .route("/ws/positions", get(websocket::ws_positions_handler))
        .route("/ws/signals", get(websocket::ws_signals_handler))
        .route("/ws/all", get(websocket::ws_all_handler))

        // Swagger UI
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))

        // Add state
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn create_test_state() -> Arc<AppState> {
        // Create a mock pool - this would need a real database for integration tests
        // For now, we'll just verify the router compiles
        todo!("Need database for tests")
    }

    #[test]
    fn test_openapi_spec() {
        let doc = ApiDoc::openapi();
        let json = doc.to_json().unwrap();
        assert!(json.contains("Polymarket Trading API"));
        assert!(json.contains("health"));
        assert!(json.contains("markets"));
    }
}
