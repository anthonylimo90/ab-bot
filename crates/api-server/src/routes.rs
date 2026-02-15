//! API route definitions.

use axum::middleware as axum_middleware;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;
use std::sync::Arc;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::handlers::{
    activity, admin_workspaces, allocations, auth, auto_rotation, backtest, demo, discover, health,
    invites, markets, onboarding, order_signing, positions, recommendations, risk_allocations,
    trading, users, vault, wallet_auth, wallets, workspaces,
};
use crate::middleware::{require_admin, require_auth, require_trader};
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
        auth::logout,
        auth::get_current_user,
        auth::forgot_password,
        auth::reset_password,
        // Wallet auth
        wallet_auth::challenge,
        wallet_auth::verify,
        wallet_auth::link_wallet,
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
        wallets::get_wallet_trades,
        trading::place_order,
        trading::cancel_order,
        trading::get_order_status,
        backtest::run_backtest,
        backtest::list_backtest_results,
        backtest::get_backtest_result,
        discover::get_live_trades,
        discover::discover_wallets,
        discover::get_discovered_wallet,
        discover::simulate_demo_pnl,
        vault::store_wallet,
        vault::list_wallets,
        vault::get_wallet,
        vault::remove_wallet,
        vault::set_primary_wallet,
        vault::get_wallet_balance,
        recommendations::get_rotation_recommendations,
        recommendations::dismiss_recommendation,
        recommendations::accept_recommendation,
        users::list_users,
        users::create_user,
        users::get_user,
        users::update_user,
        users::delete_user,
        // Admin workspaces
        admin_workspaces::list_workspaces,
        admin_workspaces::create_workspace,
        admin_workspaces::get_workspace,
        admin_workspaces::update_workspace,
        admin_workspaces::delete_workspace,
        // User workspaces
        workspaces::list_workspaces,
        workspaces::get_current_workspace,
        workspaces::get_workspace,
        workspaces::update_workspace,
        workspaces::switch_workspace,
        workspaces::list_members,
        workspaces::update_member_role,
        workspaces::remove_member,
        workspaces::get_optimizer_status,
        workspaces::get_service_status,
        // Invites
        invites::list_invites,
        invites::create_invite,
        invites::revoke_invite,
        invites::get_invite_info,
        invites::accept_invite,
        // Allocations
        allocations::list_allocations,
        allocations::add_allocation,
        allocations::update_allocation,
        allocations::remove_allocation,
        allocations::promote_allocation,
        allocations::demote_allocation,
        allocations::pin_allocation,
        allocations::unpin_allocation,
        allocations::ban_wallet,
        allocations::unban_wallet,
        allocations::list_bans,
        // Auto-rotation
        auto_rotation::list_rotation_history,
        auto_rotation::acknowledge_entry,
        auto_rotation::trigger_optimization,
        // Onboarding
        onboarding::get_status,
        onboarding::set_mode,
        onboarding::set_budget,
        onboarding::auto_setup,
        onboarding::complete_onboarding,
        // Demo positions
        demo::list_demo_positions,
        demo::create_demo_position,
        demo::update_demo_position,
        demo::delete_demo_position,
        demo::get_demo_balance,
        demo::update_demo_balance,
        demo::reset_demo_portfolio,
        // Order signing (MetaMask)
        order_signing::prepare_order,
        order_signing::submit_order,
        // Activity
        activity::list_activity,
    ),
    components(
        schemas(
            crate::error::ErrorResponse,
            auth::RegisterRequest,
            auth::LoginRequest,
            auth::AuthResponse,
            auth::UserInfo,
            auth::ForgotPasswordRequest,
            auth::ForgotPasswordResponse,
            auth::ResetPasswordRequest,
            auth::ResetPasswordResponse,
            // Wallet auth schemas
            wallet_auth::ChallengeRequest,
            wallet_auth::ChallengeResponse,
            wallet_auth::VerifyRequest,
            wallet_auth::VerifyResponse,
            wallet_auth::WalletUserInfo,
            wallet_auth::LinkWalletRequest,
            wallet_auth::LinkWalletResponse,
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
            wallets::WalletTradeResponse,
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
            vault::WalletBalanceResponse,
            recommendations::RotationRecommendation,
            recommendations::RecommendationType,
            recommendations::RecommendationReason,
            recommendations::Urgency,
            users::UserListItem,
            users::CreateUserRequest,
            users::UpdateUserRequest,
            // Admin workspaces
            admin_workspaces::AdminWorkspaceListItem,
            admin_workspaces::CreateWorkspaceRequest,
            admin_workspaces::UpdateWorkspaceRequest,
            admin_workspaces::WorkspaceDetailResponse,
            // User workspaces
            workspaces::WorkspaceListItem,
            workspaces::WorkspaceResponse,
            workspaces::UpdateWorkspaceRequest,
            workspaces::WorkspaceMemberResponse,
            workspaces::UpdateMemberRoleRequest,
            workspaces::OptimizerStatusResponse,
            workspaces::OptimizerCriteria,
            workspaces::PortfolioMetrics,
            workspaces::ServiceStatusResponse,
            workspaces::ServiceStatusItem,
            // Invites
            invites::InviteResponse,
            invites::CreateInviteRequest,
            invites::AcceptInviteRequest,
            invites::AcceptInviteResponse,
            invites::InviteInfoResponse,
            // Allocations
            allocations::AllocationResponse,
            allocations::AddAllocationRequest,
            allocations::UpdateAllocationRequest,
            allocations::PinResponse,
            allocations::BanWalletRequest,
            allocations::BanResponse,
            allocations::BanListResponse,
            // Auto-rotation
            auto_rotation::RotationHistoryResponse,
            // Onboarding
            onboarding::OnboardingStatusResponse,
            onboarding::SetModeRequest,
            onboarding::SetBudgetRequest,
            onboarding::AutoSetupRequest,
            onboarding::AutoSetupResponse,
            onboarding::AutoSelectedWallet,
            // Demo
            demo::DemoPositionResponse,
            demo::CreateDemoPositionRequest,
            demo::UpdateDemoPositionRequest,
            demo::DemoBalanceResponse,
            demo::UpdateDemoBalanceRequest,
            // Order signing
            order_signing::PrepareOrderRequest,
            order_signing::PrepareOrderResponse,
            order_signing::SubmitOrderRequest,
            order_signing::SubmitOrderResponse,
            order_signing::Eip712TypedData,
            order_signing::Eip712Domain,
            order_signing::Eip712Order,
            order_signing::Eip712Types,
            order_signing::TypeDefinition,
            order_signing::OrderSummary,
            // Activity
            activity::ActivityResponse,
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
        (name = "users", description = "User management (admin only)"),
        (name = "admin_workspaces", description = "Workspace management (platform admin only)"),
        (name = "workspaces", description = "User workspace operations"),
        (name = "invites", description = "Workspace invite management"),
        (name = "allocations", description = "Wallet roster allocations"),
        (name = "auto_rotation", description = "Auto-rotation history and optimization"),
        (name = "onboarding", description = "Setup wizard and onboarding"),
        (name = "demo", description = "Demo trading positions and balance"),
        (name = "order_signing", description = "MetaMask/wallet-based order signing"),
        (name = "activity", description = "Activity feed from copy trade history"),
        (name = "websocket", description = "Real-time WebSocket endpoints"),
    )
)]
pub struct ApiDoc;

/// Create the main router with all routes.
pub fn create_router(state: Arc<AppState>) -> Router {
    // Rate limiter for auth endpoints: 5 requests per 60 seconds per IP
    // Uses SmartIpKeyExtractor to handle X-Forwarded-For from Railway's proxy
    let auth_rate_limit_config = GovernorConfigBuilder::default()
        .per_second(60)
        .burst_size(5)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("Failed to create auth rate limiter config");

    // Rate limiter for admin endpoints: 30 requests per 60 seconds per IP
    // Uses SmartIpKeyExtractor to handle X-Forwarded-For from Railway's proxy
    // Higher burst to accommodate cascading refetches after bulk deletions
    let admin_rate_limit_config = GovernorConfigBuilder::default()
        .per_second(60)
        .burst_size(30)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("Failed to create admin rate limiter config");

    // Rate limiter for workspace config updates: 10 requests per 60 seconds per IP
    // Tighter limit for sensitive config changes (API keys, trading toggles)
    let config_rate_limit_config = GovernorConfigBuilder::default()
        .per_second(60)
        .burst_size(10)
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .expect("Failed to create config rate limiter config");

    // Auth routes with rate limiting (SmartIpKeyExtractor handles proxy IPs)
    let auth_routes = Router::new()
        .route("/api/v1/auth/register", post(auth::register))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/forgot-password", post(auth::forgot_password))
        .route("/api/v1/auth/reset-password", post(auth::reset_password))
        // Wallet auth (SIWE)
        .route(
            "/api/v1/auth/wallet/challenge",
            post(wallet_auth::challenge),
        )
        .route("/api/v1/auth/wallet/verify", post(wallet_auth::verify))
        .layer(GovernorLayer {
            config: Arc::new(auth_rate_limit_config),
        });

    // Public routes - no authentication required
    let public_routes = Router::new()
        .route("/health", get(health::health_check))
        .route("/ready", get(health::readiness))
        // Discovery/demo endpoints (public for demo purposes)
        .route("/api/v1/discover/trades", get(discover::get_live_trades))
        .route("/api/v1/discover/wallets", get(discover::discover_wallets))
        .route(
            "/api/v1/discover/wallets/:address",
            get(discover::get_discovered_wallet),
        )
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
        // Invite info and acceptance (public - token validates access)
        .route("/api/v1/invites/:token", get(invites::get_invite_info))
        .route(
            "/api/v1/invites/:token/accept",
            post(invites::accept_invite),
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
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/me", get(auth::get_current_user))
        // Wallet linking (requires auth)
        .route("/api/v1/auth/wallet/link", post(wallet_auth::link_wallet))
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
        .route(
            "/api/v1/wallets/:address/trades",
            get(wallets::get_wallet_trades),
        )
        // Vault endpoints (read-only)
        .route("/api/v1/vault/wallets", get(vault::list_wallets))
        .route("/api/v1/vault/wallets/:address", get(vault::get_wallet))
        .route(
            "/api/v1/vault/wallets/:address/balance",
            get(vault::get_wallet_balance),
        )
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
        // Workspace endpoints (read-only for all members)
        .route("/api/v1/workspaces", get(workspaces::list_workspaces))
        .route(
            "/api/v1/workspaces/current",
            get(workspaces::get_current_workspace),
        )
        .route(
            "/api/v1/workspaces/:workspace_id",
            get(workspaces::get_workspace),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/members",
            get(workspaces::list_members),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/optimizer-status",
            get(workspaces::get_optimizer_status),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/service-status",
            get(workspaces::get_service_status),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/invites",
            get(invites::list_invites),
        )
        // Activity feed (read-only for all members)
        .route("/api/v1/activity", get(activity::list_activity))
        // Allocations (read-only for all members)
        .route("/api/v1/allocations", get(allocations::list_allocations))
        // Auto-rotation history (read-only for all members)
        .route(
            "/api/v1/auto-rotation/history",
            get(auto_rotation::list_rotation_history),
        )
        // Onboarding status (read for all)
        .route("/api/v1/onboarding/status", get(onboarding::get_status))
        // Demo positions (read for all workspace members)
        .route("/api/v1/demo/positions", get(demo::list_demo_positions))
        .route("/api/v1/demo/balance", get(demo::get_demo_balance))
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
        // Wallet-based order signing (MetaMask)
        .route("/api/v1/orders/prepare", post(order_signing::prepare_order))
        .route("/api/v1/orders/submit", post(order_signing::submit_order))
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
        // Workspace operations (owner/admin can modify)
        // NOTE: PUT /workspaces/:id is in config_routes with stricter rate limiting
        .route(
            "/api/v1/workspaces/:workspace_id/switch",
            post(workspaces::switch_workspace),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/members/:member_id",
            put(workspaces::update_member_role),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/members/:member_id",
            delete(workspaces::remove_member),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/invites",
            post(invites::create_invite),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/invites/:invite_id",
            delete(invites::revoke_invite),
        )
        // Allocation management (owner/admin can modify)
        .route(
            "/api/v1/allocations/:address",
            post(allocations::add_allocation),
        )
        .route(
            "/api/v1/allocations/:address",
            put(allocations::update_allocation),
        )
        .route(
            "/api/v1/allocations/:address",
            delete(allocations::remove_allocation),
        )
        .route(
            "/api/v1/allocations/:address/promote",
            post(allocations::promote_allocation),
        )
        .route(
            "/api/v1/allocations/:address/demote",
            post(allocations::demote_allocation),
        )
        // Pin/unpin wallet (prevents auto-demotion)
        .route(
            "/api/v1/allocations/:address/pin",
            put(allocations::pin_allocation),
        )
        .route(
            "/api/v1/allocations/:address/pin",
            delete(allocations::unpin_allocation),
        )
        // Wallet bans (prevents auto-promotion)
        .route("/api/v1/allocations/bans", post(allocations::ban_wallet))
        .route("/api/v1/allocations/bans", get(allocations::list_bans))
        .route(
            "/api/v1/allocations/bans/:address",
            delete(allocations::unban_wallet),
        )
        // Risk-based allocation recalculation
        .route(
            "/api/v1/allocations/risk/recalculate",
            post(risk_allocations::recalculate_allocations),
        )
        // Auto-rotation operations
        .route(
            "/api/v1/auto-rotation/:entry_id/acknowledge",
            put(auto_rotation::acknowledge_entry),
        )
        .route(
            "/api/v1/auto-rotation/trigger",
            post(auto_rotation::trigger_optimization),
        )
        // Onboarding operations
        .route("/api/v1/onboarding/mode", put(onboarding::set_mode))
        .route("/api/v1/onboarding/budget", put(onboarding::set_budget))
        .route(
            "/api/v1/onboarding/auto-setup",
            post(onboarding::auto_setup),
        )
        .route(
            "/api/v1/onboarding/complete",
            put(onboarding::complete_onboarding),
        )
        // Demo positions (write requires trader role)
        .route("/api/v1/demo/positions", post(demo::create_demo_position))
        .route(
            "/api/v1/demo/positions/:position_id",
            put(demo::update_demo_position),
        )
        .route(
            "/api/v1/demo/positions/:position_id",
            delete(demo::delete_demo_position),
        )
        .route("/api/v1/demo/balance", put(demo::update_demo_balance))
        .route("/api/v1/demo/reset", post(demo::reset_demo_portfolio))
        // Apply trader check first, then auth
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_trader,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    // Admin routes - require Admin role with rate limiting
    let admin_routes = Router::new()
        // User management
        .route("/api/v1/users", get(users::list_users))
        .route("/api/v1/users", post(users::create_user))
        .route("/api/v1/users/:user_id", get(users::get_user))
        .route("/api/v1/users/:user_id", patch(users::update_user))
        .route("/api/v1/users/:user_id", delete(users::delete_user))
        // Admin workspace management
        .route(
            "/api/v1/admin/workspaces",
            get(admin_workspaces::list_workspaces),
        )
        .route(
            "/api/v1/admin/workspaces",
            post(admin_workspaces::create_workspace),
        )
        .route(
            "/api/v1/admin/workspaces/:workspace_id",
            get(admin_workspaces::get_workspace),
        )
        .route(
            "/api/v1/admin/workspaces/:workspace_id",
            put(admin_workspaces::update_workspace),
        )
        .route(
            "/api/v1/admin/workspaces/:workspace_id",
            delete(admin_workspaces::delete_workspace),
        )
        // Apply rate limiting first (outermost layer runs last)
        .layer(GovernorLayer {
            config: Arc::new(admin_rate_limit_config),
        })
        // Apply admin check, then auth
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_admin,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    // Config routes - sensitive workspace config with tighter rate limiting (10 req/min)
    let config_routes = Router::new()
        .route(
            "/api/v1/workspaces/:workspace_id",
            put(workspaces::update_workspace),
        )
        .layer(GovernorLayer {
            config: Arc::new(config_rate_limit_config),
        })
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_trader,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ));

    Router::new()
        .merge(auth_routes)
        .merge(public_routes)
        .merge(protected_routes)
        .merge(trader_routes)
        .merge(config_routes)
        .merge(admin_routes)
        // Swagger UI (public for development)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        // Add state
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_spec() {
        let doc = ApiDoc::openapi();
        let json = doc.to_json().unwrap();
        assert!(json.contains("Polymarket Trading API"));
        assert!(json.contains("health"));
        assert!(json.contains("markets"));
        assert!(json.contains("workspaces"));
        assert!(json.contains("allocations"));
    }
}
