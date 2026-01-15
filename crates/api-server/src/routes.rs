//! API route definitions.

use axum::middleware as axum_middleware;
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::handlers::{
    auth, backtest, discover, health, markets, positions, recommendations, trading, vault, wallets,
};
use crate::middleware::{require_auth, require_trader};
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
        auth::register,
        auth::login,
        auth::refresh_token,
        auth::get_current_user,
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
        discover::get_live_trades,
        discover::discover_wallets,
        discover::simulate_demo_pnl,
        vault::store_wallet,
        vault::list_wallets,
        vault::get_wallet,
        vault::remove_wallet,
        vault::set_primary_wallet,
        recommendations::get_rotation_recommendations,
        recommendations::dismiss_recommendation,
        recommendations::accept_recommendation,
    ),
    components(
        schemas(
            crate::error::ErrorResponse,
            auth::RegisterRequest,
            auth::LoginRequest,
            auth::AuthResponse,
            auth::UserInfo,
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
            discover::LiveTrade,
            discover::DiscoveredWallet,
            discover::PredictionCategory,
            discover::DemoPnlSimulation,
            discover::WalletSimulation,
            discover::EquityPoint,
            vault::StoreWalletRequest,
            vault::WalletInfo,
            recommendations::RotationRecommendation,
            recommendations::RecommendationType,
            recommendations::RecommendationReason,
            recommendations::Urgency,
        )
    ),
    tags(
        (name = "auth", description = "Authentication endpoints"),
        (name = "health", description = "Health check endpoints"),
        (name = "markets", description = "Market data endpoints"),
        (name = "positions", description = "Position management"),
        (name = "wallets", description = "Wallet tracking for copy trading"),
        (name = "trading", description = "Order execution"),
        (name = "backtest", description = "Backtesting operations"),
        (name = "discover", description = "Wallet discovery and live trade monitoring"),
        (name = "vault", description = "Secure wallet key management"),
        (name = "recommendations", description = "AI-powered rotation recommendations"),
        (name = "websocket", description = "Real-time WebSocket endpoints"),
    )
)]
pub struct ApiDoc;

/// Create the main router with all routes.
pub fn create_router(state: Arc<AppState>) -> Router {
    // Public routes - no authentication required
    let public_routes = Router::new()
        .route("/health", get(health::health_check))
        .route("/ready", get(health::readiness))
        // Auth endpoints (public)
        .route("/api/v1/auth/register", post(auth::register))
        .route("/api/v1/auth/login", post(auth::login))
        // Discovery/demo endpoints (public for demo purposes)
        .route("/api/v1/discover/trades", get(discover::get_live_trades))
        .route("/api/v1/discover/wallets", get(discover::discover_wallets))
        .route(
            "/api/v1/discover/simulate",
            get(discover::simulate_demo_pnl),
        )
        // Recommendations (public for demo purposes)
        .route(
            "/api/v1/recommendations/rotation",
            get(recommendations::get_rotation_recommendations),
        )
        .route(
            "/api/v1/recommendations/:id/dismiss",
            post(recommendations::dismiss_recommendation),
        )
        .route(
            "/api/v1/recommendations/:id/accept",
            post(recommendations::accept_recommendation),
        )
        // WebSocket endpoints (auth handled via query param or message)
        .route("/ws/orderbook", get(websocket::ws_orderbook_handler))
        .route("/ws/positions", get(websocket::ws_positions_handler))
        .route("/ws/signals", get(websocket::ws_signals_handler))
        .route("/ws/all", get(websocket::ws_all_handler));

    // Protected read-only routes - require authentication (any role)
    let protected_routes = Router::new()
        // Auth endpoints (protected)
        .route("/api/v1/auth/refresh", post(auth::refresh_token))
        .route("/api/v1/auth/me", get(auth::get_current_user))
        // Market endpoints (read-only)
        .route("/api/v1/markets", get(markets::list_markets))
        .route("/api/v1/markets/:market_id", get(markets::get_market))
        .route(
            "/api/v1/markets/:market_id/orderbook",
            get(markets::get_market_orderbook),
        )
        // Position endpoints (read-only)
        .route("/api/v1/positions", get(positions::list_positions))
        .route(
            "/api/v1/positions/:position_id",
            get(positions::get_position),
        )
        // Wallet endpoints (read-only)
        .route("/api/v1/wallets", get(wallets::list_tracked_wallets))
        .route("/api/v1/wallets/:address", get(wallets::get_wallet))
        .route(
            "/api/v1/wallets/:address/metrics",
            get(wallets::get_wallet_metrics),
        )
        // Vault endpoints (read-only)
        .route("/api/v1/vault/wallets", get(vault::list_wallets))
        .route("/api/v1/vault/wallets/:address", get(vault::get_wallet))
        // Order endpoints (read-only)
        .route("/api/v1/orders/:order_id", get(trading::get_order_status))
        // Backtest endpoints (read-only)
        .route(
            "/api/v1/backtest/results",
            get(backtest::list_backtest_results),
        )
        .route(
            "/api/v1/backtest/results/:result_id",
            get(backtest::get_backtest_result),
        )
        // Apply auth middleware
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    // Trader routes - require Trader or Admin role
    let trader_routes = Router::new()
        // Trading operations
        .route("/api/v1/orders", post(trading::place_order))
        .route(
            "/api/v1/orders/:order_id/cancel",
            post(trading::cancel_order),
        )
        // Position operations
        .route(
            "/api/v1/positions/:position_id/close",
            post(positions::close_position),
        )
        // Wallet management
        .route("/api/v1/wallets", post(wallets::add_tracked_wallet))
        .route("/api/v1/wallets/:address", put(wallets::update_wallet))
        .route("/api/v1/wallets/:address", delete(wallets::remove_wallet))
        // Backtest operations
        .route("/api/v1/backtest", post(backtest::run_backtest))
        // Vault operations (write)
        .route("/api/v1/vault/wallets", post(vault::store_wallet))
        .route(
            "/api/v1/vault/wallets/:address",
            delete(vault::remove_wallet),
        )
        .route(
            "/api/v1/vault/wallets/:address/primary",
            put(vault::set_primary_wallet),
        )
        // Apply trader check first, then auth
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_trader,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .merge(trader_routes)
        // Swagger UI (public for development)
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
