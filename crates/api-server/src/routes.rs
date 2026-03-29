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
    accounting, activity, admin_workspaces, auth, backtest, discover, health, markets,
    order_signing, positions, recommendations, recovery, risk, signals, strategy_health,
    trade_flow, trading, users, vault, wallet_auth, wallets, workspaces,
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
        wallets::get_wallet_metrics,
        wallets::get_wallet_trades,
        trading::place_order,
        trading::cancel_order,
        trading::get_order_status,
        backtest::run_backtest,
        backtest::list_backtest_results,
        backtest::get_backtest_result,
        backtest::list_backtest_schedules,
        backtest::create_backtest_schedule,
        backtest::update_backtest_schedule,
        backtest::delete_backtest_schedule,
        discover::get_live_trades,
        discover::discover_wallets,
        discover::get_discovered_wallet,
        discover::get_current_regime,

        vault::store_wallet,
        vault::list_wallets,
        vault::get_wallet,
        vault::remove_wallet,
        vault::set_primary_wallet,
        vault::get_wallet_balance,
        vault::list_withdrawals,
        vault::create_withdrawal,
        vault::withdrawal_preflight,
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
        workspaces::list_workspace_invites,
        workspaces::create_workspace_invite,
        workspaces::revoke_workspace_invite,
        workspaces::get_invite_info,
        workspaces::accept_invite,
        workspaces::update_member_role,
        workspaces::remove_member,
        workspaces::get_service_status,
        workspaces::get_dynamic_tuner_status,
        workspaces::update_opportunity_selection_settings,
        workspaces::get_dynamic_tuning_history,
        // Order signing (MetaMask)
        order_signing::prepare_order,
        order_signing::submit_order,
        // Activity
        activity::list_activity,
        accounting::get_account_summary,
        accounting::get_account_history,
        accounting::list_cash_flows,
        accounting::create_cash_flow,
        recovery::preview_recovery,
        recovery::run_recovery,
        // Trade flow
        trade_flow::get_trade_flow_summary,
        trade_flow::get_trade_flow_journeys,
        trade_flow::get_market_trade_flow,
        trade_flow::get_arb_market_scorecard,
        trade_flow::get_arb_execution_telemetry,
        trade_flow::get_learning_overview,
        trade_flow::get_learning_rollout_detail,
        trade_flow::create_learning_model,
        trade_flow::update_learning_model,
        trade_flow::activate_learning_model,
        trade_flow::disable_learning_model,
        trade_flow::retire_learning_model,
        trade_flow::create_learning_rollout,
        trade_flow::update_learning_rollout,
        trade_flow::pause_learning_rollout,
        trade_flow::resume_learning_rollout,
        trade_flow::complete_learning_rollout,
        // Risk monitoring
        risk::get_risk_status,
        risk::manual_trip_circuit_breaker,
        risk::reset_circuit_breaker,
        risk::update_circuit_breaker_config,
        // Arb executor config
        workspaces::update_arb_executor_config,
        // Signals (quant signal system)
        signals::get_flow_features,
        signals::get_market_metadata,
        signals::get_recent_signals,
        signals::get_strategy_performance,
        strategy_health::get_strategy_health,
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
            crate::trade_events::TradeEventUpdate,
            crate::websocket::PositionUpdateType,
            crate::websocket::SignalType,
            health::HealthResponse,
            markets::MarketResponse,
            markets::OrderbookResponse,
            markets::PriceLevel,
            markets::SpreadInfo,
            positions::PositionResponse,
            positions::ClosePositionRequest,
            wallets::WalletMetricsResponse,
            wallets::WalletTradeResponse,
            trading::PlaceOrderRequest,
            trading::OrderResponse,
            trading::OrderSide,
            trading::OrderType,
            trading::OrderStatus,
            backtest::RunBacktestRequest,
            backtest::BacktestResultResponse,
            backtest::BacktestScheduleResponse,
            backtest::CreateBacktestScheduleRequest,
            backtest::UpdateBacktestScheduleRequest,
            backtest::StrategyConfig,
            backtest::SlippageModel,
            backtest::EquityPoint,
            discover::LiveTrade,
            discover::DiscoveredWallet,
            discover::PredictionCategory,
            discover::MarketRegimeResponse,

            vault::StoreWalletRequest,
            vault::WalletInfo,
            vault::WalletBalanceResponse,
            vault::CreateWithdrawalRequest,
            vault::WalletWithdrawalResponse,
            vault::WithdrawalPreflightResponse,
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
            workspaces::WorkspaceInviteResponse,
            workspaces::CreateInviteRequest,
            workspaces::InviteInfoResponse,
            workspaces::AcceptInviteRequest,
            workspaces::AcceptInviteResponse,
            workspaces::UpdateMemberRoleRequest,
            workspaces::ServiceStatusResponse,
            workspaces::ServiceStatusItem,
            crate::strategy_modes::StrategyModeStatus,
            crate::strategy_modes::ResolvedStrategyMode,
            workspaces::DynamicTunerStatusResponse,
            workspaces::DynamicSignalThresholdsResponse,
            workspaces::DynamicConfigItemResponse,
            workspaces::DynamicConfigHistoryEntryResponse,
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
            accounting::AccountSummaryResponse,
            accounting::AccountEquityPointResponse,
            accounting::CashFlowEventResponse,
            accounting::AccountTradeEventResponse,
            accounting::AccountHistoryResponse,
            accounting::CreateCashFlowRequest,
            recovery::RecoveryBucketSummary,
            recovery::RecoveryPreviewResponse,
            recovery::RecoveryRunResponse,
            trade_flow::TradeFlowSummaryResponse,
            trade_flow::TradeFlowStrategySummary,
            trade_flow::TradeJourneyResponse,
            trade_flow::TradeFlowMarketResponse,
            trade_flow::ArbMarketScorecardResponse,
            trade_flow::ArbMarketScorecardItem,
            trade_flow::ArbExecutionTelemetryResponse,
            trade_flow::ArbExecutionTelemetrySummary,
            trade_flow::ArbExecutionLatencyMetric,
            trade_flow::ArbExecutionOutcomeBreakdown,
            trade_flow::ArbExecutionAttempt,
            trade_flow::LearningOverviewResponse,
            trade_flow::LearningDatasetReadiness,
            trade_flow::LearningModelSummary,
            trade_flow::CreateLearningModelRequest,
            trade_flow::UpdateLearningModelRequest,
            trade_flow::LearningOfflineEvaluation,
            trade_flow::LearningRolloutStatus,
            trade_flow::LearningRolloutObservation,
            trade_flow::LearningRolloutDetailResponse,
            trade_flow::CreateLearningRolloutRequest,
            trade_flow::UpdateLearningRolloutRequest,
            // Risk monitoring
            risk::RiskStatusResponse,
            risk::CircuitBreakerResponse,
            risk::CircuitBreakerConfigResponse,
            risk::RecoveryStateResponse,
            risk::StopLossStatsResponse,
            risk::RecentStopExecution,
            risk::UpdateCircuitBreakerConfigRequest,
            workspaces::UpdateArbExecutorConfigRequest,
            workspaces::ArbExecutorConfigResponse,
            // Signals
            signals::FlowFeatureResponse,
            signals::MarketMetadataResponse,
            signals::RecentSignalResponse,
            signals::StrategyPerformanceResponse,
            strategy_health::StrategyHealthResponse,
            strategy_health::StrategyHealthItemResponse,
        )
    ),
    tags(
        (name = "auth", description = "Authentication endpoints"),
        (name = "health", description = "Health check endpoints"),
        (name = "markets", description = "Market data endpoints"),
        (name = "positions", description = "Position management"),
        (name = "wallets", description = "Wallet tracking"),
        (name = "trading", description = "Order execution"),
        (name = "backtest", description = "Backtesting operations"),
        (name = "discover", description = "Wallet discovery and live trade monitoring"),
        (name = "vault", description = "Secure wallet key management"),
        (name = "recommendations", description = "Advisory wallet rotation and research recommendations"),
        (name = "users", description = "User management (admin only)"),
        (name = "admin_workspaces", description = "Workspace management (platform admin only)"),
        (name = "workspaces", description = "User workspace operations"),
        (name = "order_signing", description = "MetaMask/wallet-based order signing"),
        (name = "activity", description = "Activity feed from execution reports"),
        (name = "trade_flow", description = "Derived trade lifecycle and conversion analytics"),
        (name = "risk", description = "Risk monitoring and circuit breaker management"),
        (name = "signals", description = "Quant signal system: flow features, performance, and recent signals"),
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
        .route("/api/v1/regime/current", get(discover::get_current_regime))
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
        .route("/api/v1/invites/:token", get(workspaces::get_invite_info))
        .route(
            "/api/v1/invites/:token/accept",
            post(workspaces::accept_invite),
        )
        // WebSocket endpoints (auth handled via query param or message)
        .route("/ws/orderbook", get(websocket::ws_orderbook_handler))
        .route("/ws/positions", get(websocket::ws_positions_handler))
        .route("/ws/signals", get(websocket::ws_signals_handler))
        .route("/ws/trade-flow", get(websocket::ws_trade_flow_handler))
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
            "/api/v1/positions/summary",
            get(positions::get_positions_summary),
        )
        .route(
            "/api/v1/positions/:position_id",
            get(positions::get_position),
        )
        // Wallet endpoints (read-only)
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
        .route("/api/v1/vault/withdrawals", get(vault::list_withdrawals))
        .route(
            "/api/v1/vault/withdrawals/preflight",
            get(vault::withdrawal_preflight),
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
        .route(
            "/api/v1/backtest/schedules",
            get(backtest::list_backtest_schedules),
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
            "/api/v1/workspaces/:workspace_id/invites",
            get(workspaces::list_workspace_invites),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/invites",
            post(workspaces::create_workspace_invite),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/invites/:invite_id",
            delete(workspaces::revoke_workspace_invite),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/service-status",
            get(workspaces::get_service_status),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/status",
            get(workspaces::get_dynamic_tuner_status),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/history",
            get(workspaces::get_dynamic_tuning_history),
        )
        // Signal endpoints (read-only for all members)
        .route("/api/v1/signals/flow", get(signals::get_flow_features))
        .route(
            "/api/v1/signals/metadata",
            get(signals::get_market_metadata),
        )
        .route("/api/v1/signals/recent", get(signals::get_recent_signals))
        .route(
            "/api/v1/signals/performance",
            get(signals::get_strategy_performance),
        )
        .route(
            "/api/v1/signals/health",
            get(strategy_health::get_strategy_health),
        )
        // Activity feed (read-only for all members)
        .route("/api/v1/activity", get(activity::list_activity))
        .route(
            "/api/v1/workspaces/:workspace_id/account/summary",
            get(accounting::get_account_summary),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/account/history",
            get(accounting::get_account_history),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/account/cash-flows",
            get(accounting::list_cash_flows).post(accounting::create_cash_flow),
        )
        // Trade flow analytics (read-only for all members)
        .route(
            "/api/v1/trade-flow/summary",
            get(trade_flow::get_trade_flow_summary),
        )
        .route(
            "/api/v1/trade-flow/journeys",
            get(trade_flow::get_trade_flow_journeys),
        )
        .route(
            "/api/v1/trade-flow/markets/:market_id",
            get(trade_flow::get_market_trade_flow),
        )
        .route(
            "/api/v1/trade-flow/strategies/arb/scorecard",
            get(trade_flow::get_arb_market_scorecard),
        )
        .route(
            "/api/v1/trade-flow/strategies/arb/execution-telemetry",
            get(trade_flow::get_arb_execution_telemetry),
        )
        .route(
            "/api/v1/trade-flow/learning/overview",
            get(trade_flow::get_learning_overview),
        )
        .route(
            "/api/v1/trade-flow/learning/rollouts/:rollout_id",
            get(trade_flow::get_learning_rollout_detail),
        )
        // Risk monitoring (read-only for all members)
        .route(
            "/api/v1/workspaces/:workspace_id/risk/status",
            get(risk::get_risk_status),
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
        // Wallet-based order signing (MetaMask)
        .route("/api/v1/orders/prepare", post(order_signing::prepare_order))
        .route("/api/v1/orders/submit", post(order_signing::submit_order))
        // Position operations
        .route(
            "/api/v1/positions/:position_id/close",
            post(positions::close_position),
        )
        // Backtest operations
        .route("/api/v1/backtest", post(backtest::run_backtest))
        .route(
            "/api/v1/backtest/schedules",
            post(backtest::create_backtest_schedule),
        )
        .route(
            "/api/v1/backtest/schedules/:schedule_id",
            patch(backtest::update_backtest_schedule),
        )
        .route(
            "/api/v1/backtest/schedules/:schedule_id",
            delete(backtest::delete_backtest_schedule),
        )
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
        .route("/api/v1/vault/withdrawals", post(vault::create_withdrawal))
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
        // Circuit breaker manual controls
        .route(
            "/api/v1/workspaces/:workspace_id/risk/circuit-breaker/trip",
            post(risk::manual_trip_circuit_breaker),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/risk/circuit-breaker/reset",
            post(risk::reset_circuit_breaker),
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
        .route(
            "/api/v1/admin/learning/rollouts",
            post(trade_flow::create_learning_rollout),
        )
        .route(
            "/api/v1/admin/learning/models",
            post(trade_flow::create_learning_model),
        )
        .route(
            "/api/v1/admin/learning/models/:model_id",
            put(trade_flow::update_learning_model),
        )
        .route(
            "/api/v1/admin/learning/models/:model_id/activate",
            post(trade_flow::activate_learning_model),
        )
        .route(
            "/api/v1/admin/learning/models/:model_id/disable",
            post(trade_flow::disable_learning_model),
        )
        .route(
            "/api/v1/admin/learning/models/:model_id/retire",
            post(trade_flow::retire_learning_model),
        )
        .route(
            "/api/v1/admin/learning/rollouts/:rollout_id",
            put(trade_flow::update_learning_rollout),
        )
        .route(
            "/api/v1/admin/learning/rollouts/:rollout_id/pause",
            post(trade_flow::pause_learning_rollout),
        )
        .route(
            "/api/v1/admin/learning/rollouts/:rollout_id/resume",
            post(trade_flow::resume_learning_rollout),
        )
        .route(
            "/api/v1/admin/learning/rollouts/:rollout_id/complete",
            post(trade_flow::complete_learning_rollout),
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
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/opportunity-selection",
            put(workspaces::update_opportunity_selection_settings),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/risk/circuit-breaker/config",
            put(risk::update_circuit_breaker_config),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/arb-executor",
            put(workspaces::update_arb_executor_config),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/recovery/preview",
            post(recovery::preview_recovery),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/recovery/run",
            post(recovery::run_recovery),
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
    }
}
