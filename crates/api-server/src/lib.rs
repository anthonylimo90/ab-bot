//! API Server
//!
//! REST and WebSocket API for the Polymarket trading platform.
//!
//! # Features
//!
//! - **REST API**: CRUD operations for positions, wallets, strategies
//! - **WebSocket**: Real-time orderbook updates, position changes, signals
//! - **OpenAPI**: Auto-generated Swagger documentation
//! - **Authentication**: JWT and API key support
//!
//! # Example
//!
//! ```ignore
//! use api_server::{ApiServer, ServerConfig};
//!
//! let config = ServerConfig::default();
//! let server = ApiServer::new(config, pool).await?;
//! server.run().await?;
//! ```

pub mod arb_executor;
pub mod crypto;
pub mod dynamic_tuner;
pub mod email;
pub mod error;
pub mod exit_handler;
pub mod flow_feature_calculator;
pub mod gamma_syncer;
pub mod handlers;
pub mod metrics_calculator;
pub mod middleware;
pub mod quant_signal_executor;
pub mod redis_forwarder;
pub mod routes;
pub mod runtime_sync;
pub mod schema;
pub mod signals;
pub mod state;
pub mod strategy_pnl_calculator;
pub mod wallet_harvester;
pub mod websocket;

pub use arb_executor::{spawn_arb_auto_executor, ArbExecutorConfig};
pub use dynamic_tuner::{spawn_dynamic_config_subscriber, DynamicTuner};
pub use error::ApiError;
pub use exit_handler::{spawn_exit_handler, ExitHandlerConfig};
pub use flow_feature_calculator::{spawn_flow_feature_calculator, FlowFeatureConfig};
pub use gamma_syncer::{spawn_gamma_syncer, GammaSyncerConfig};
pub use metrics_calculator::{MetricsCalculator, MetricsCalculatorConfig};
pub use quant_signal_executor::{spawn_quant_signal_executor, QuantSignalExecutorConfig};
pub use redis_forwarder::{spawn_redis_forwarder, RedisForwarderConfig};
pub use routes::create_router;
pub use signals::{
    spawn_cross_market_signal_generator, spawn_flow_signal_generator,
    spawn_mean_reversion_signal_generator, spawn_resolution_signal_generator,
};
pub use state::AppState;
pub use strategy_pnl_calculator::{spawn_strategy_pnl_calculator, StrategyPnlConfig};
pub use wallet_harvester::{spawn_wallet_harvester, WalletHarvesterConfig};

use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use sqlx::PgPool;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, Semaphore};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::{info, Level};

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Host to bind to.
    pub host: String,
    /// Port to listen on.
    pub port: u16,
    /// Enable CORS for all origins (development only).
    pub cors_permissive: bool,
    /// JWT secret for authentication.
    pub jwt_secret: String,
    /// WebSocket channel capacity.
    pub ws_channel_capacity: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
            cors_permissive: true,
            jwt_secret: "development-secret-change-in-production".to_string(),
            ws_channel_capacity: 1000,
        }
    }
}

impl ServerConfig {
    /// Create from environment variables.
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            // Check PORT first (Railway), then API_PORT, then default to 3000
            port: std::env::var("PORT")
                .or_else(|_| std::env::var("API_PORT"))
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            cors_permissive: std::env::var("CORS_PERMISSIVE")
                .map(|v| v == "true")
                .unwrap_or(true),
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| "development-secret-change-in-production".to_string()),
            ws_channel_capacity: std::env::var("WS_CHANNEL_CAPACITY")
                .ok()
                .and_then(|c| c.parse().ok())
                .unwrap_or(1000),
        }
    }

    /// Get the socket address.
    pub fn socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("Invalid socket address")
    }
}

/// The API server.
pub struct ApiServer {
    config: ServerConfig,
    state: AppState,
}

impl ApiServer {
    /// Create a new API server.
    pub async fn new(config: ServerConfig, pool: PgPool) -> anyhow::Result<Self> {
        // Create broadcast channels for WebSocket and automation
        let (orderbook_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (position_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (signal_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (arb_entry_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (quant_signal_tx, _) = broadcast::channel(config.ws_channel_capacity);

        // Create app state
        let state = AppState::new(
            pool,
            config.jwt_secret.clone(),
            orderbook_tx,
            position_tx,
            signal_tx,
            arb_entry_tx,
            quant_signal_tx,
        )
        .await?;

        Ok(Self { config, state })
    }

    /// Run the server.
    pub async fn run(mut self) -> anyhow::Result<()> {
        // ── Pre-create arb executor config Arc (must happen before Arc-wrapping state) ──
        let mut arb_config_pre = ArbExecutorConfig::from_env();
        if !arb_config_pre.enabled {
            match crate::runtime_sync::any_workspace_arb_enabled(&self.state.pool).await {
                Ok(true) => {
                    arb_config_pre.enabled = true;
                    info!(
                        "Enabling arb auto-executor because at least one workspace has arb_auto_execute=true"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to read workspace arb_auto_execute flags; falling back to env-only arb config"
                    );
                }
            }
        }
        let arb_config_arc = Arc::new(RwLock::new(arb_config_pre));
        self.state.arb_executor_config = Some(arb_config_arc.clone());

        // Pre-create exit handler config Arc (must happen before Arc-wrapping state)
        let mut exit_config_pre = ExitHandlerConfig::from_env();
        if !exit_config_pre.enabled {
            match crate::runtime_sync::any_workspace_exit_handler_enabled(&self.state.pool).await {
                Ok(true) => {
                    exit_config_pre.enabled = true;
                    info!(
                        "Enabling exit handler because at least one workspace has exit_handler_enabled=true"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to read workspace exit_handler_enabled flags; falling back to env-only exit config"
                    );
                }
            }
        }
        let exit_config_arc = Arc::new(RwLock::new(exit_config_pre));
        self.state.exit_handler_config = Some(exit_config_arc.clone());

        // ── Wrap state in Arc and build router ──
        let state = Arc::new(self.state);

        let router = create_router(state.clone());
        let router = router
            .layer(
                TraceLayer::new_for_http()
                    .on_request(|request: &Request<_>, _span: &tracing::Span| {
                        tracing::info!(
                            method = %request.method(),
                            uri = %request.uri(),
                            "Incoming request"
                        );
                    })
                    .on_response(DefaultOnResponse::new().level(Level::DEBUG))
                    .on_failure(
                        |error: tower_http::classify::ServerErrorsFailureClass,
                         latency: std::time::Duration,
                         _span: &tracing::Span| {
                            tracing::error!(
                                error = %error,
                                latency_ms = latency.as_millis(),
                                "Request failed"
                            );
                        },
                    ),
            )
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024)) // 2 MB
            .layer(if self.config.cors_permissive {
                CorsLayer::permissive()
            } else {
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any)
            });

        // ── Spawn background tasks ──

        // Shared semaphore to coordinate heavy background DB tasks.
        // 3 permits ensures at most 3 heavy batch operations run simultaneously,
        // leaving ~17 connections available for HTTP handlers + lighter tasks.
        let db_semaphore = Arc::new(Semaphore::new(3));
        info!("DB backpressure semaphore created (3 permits)");

        // Spawn Redis forwarder for signal bridging
        let redis_config = RedisForwarderConfig::from_env();
        spawn_redis_forwarder(
            redis_config,
            state.signal_tx.clone(),
            state.orderbook_tx.clone(),
            state.arb_entry_tx.clone(),
        );

        // Shared dedup set for arb executor + exit handler
        let arb_dedup = Arc::new(RwLock::new(HashSet::new()));

        // Spawn arb auto-executor unconditionally (per-signal guard checks enabled)
        spawn_arb_auto_executor(
            arb_config_arc.clone(),
            state.subscribe_arb_entry(),
            state.signal_tx.clone(),
            state.order_executor.clone(),
            state.circuit_breaker.clone(),
            state.clob_client.clone(),
            state.pool.clone(),
            arb_dedup.clone(),
            state.active_clob_markets.clone(),
            state.arb_executor_heartbeat.clone(),
        );

        // Spawn exit handler unconditionally (per-tick guard checks enabled)
        spawn_exit_handler(
            state
                .exit_handler_config
                .clone()
                .unwrap_or_else(|| Arc::new(RwLock::new(ExitHandlerConfig::from_env()))),
            state.order_executor.clone(),
            state.circuit_breaker.clone(),
            state.clob_client.clone(),
            state.signal_tx.clone(),
            state.pool.clone(),
            arb_dedup.clone(),
            state.exit_handler_heartbeat.clone(),
        );

        // Subscribe local runtime to dynamic updates (arb executor config knobs)
        let redis_url = std::env::var("DYNAMIC_CONFIG_REDIS_URL")
            .or_else(|_| std::env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        spawn_dynamic_config_subscriber(
            redis_url,
            state.pool.clone(),
            state.arb_executor_config.clone(),
        );

        // Spawn dynamic tuner (adaptive runtime configuration) after subscriber
        // so startup sync publications are not missed by local runtime.
        let tuner = Arc::new(DynamicTuner::new(
            state.pool.clone(),
            state.current_regime.clone(),
            state.circuit_breaker.clone(),
        ));
        tokio::spawn(tuner.start());

        // Spawn wallet harvester (discovers wallets from CLOB trades)
        let harvester_config = WalletHarvesterConfig::from_env();
        spawn_wallet_harvester(
            harvester_config,
            state.clob_client.clone(),
            state.pool.clone(),
            db_semaphore.clone(),
        );

        // Spawn Gamma API market metadata syncer (hourly, populates market_metadata)
        let gamma_config = GammaSyncerConfig::from_env();
        spawn_gamma_syncer(gamma_config, state.pool.clone(), db_semaphore.clone());

        // Spawn flow feature calculator (5 min, aggregates wallet_trades → market_flow_features)
        let flow_config = FlowFeatureConfig::from_env();
        spawn_flow_feature_calculator(flow_config, state.pool.clone(), db_semaphore.clone());

        // ── Quant signal system: executor + generators ──
        // Executor receives QuantSignal from broadcast channel and evaluates/executes.
        let quant_config = Arc::new(RwLock::new(QuantSignalExecutorConfig::from_env()));
        spawn_quant_signal_executor(
            quant_config,
            state.quant_signal_tx.subscribe(),
            state.signal_tx.clone(),
            state.order_executor.clone(),
            state.circuit_breaker.clone(),
            state.clob_client.clone(),
            state.pool.clone(),
            state.active_clob_markets.clone(),
            state.quant_executor_heartbeat.clone(),
        );

        // Signal generators — each polls feature tables and emits QuantSignal
        use signals::{
            cross_market_signal::CrossMarketSignalConfig, flow_signal::FlowSignalConfig,
            mean_reversion_signal::MeanReversionSignalConfig,
            resolution_signal::ResolutionSignalConfig,
        };

        let flow_signal_config = FlowSignalConfig::from_env();
        spawn_flow_signal_generator(
            flow_signal_config,
            state.pool.clone(),
            state.quant_signal_tx.clone(),
        );

        let resolution_config = ResolutionSignalConfig::from_env();
        spawn_resolution_signal_generator(
            resolution_config,
            state.pool.clone(),
            state.quant_signal_tx.clone(),
        );

        let mean_rev_config = MeanReversionSignalConfig::from_env();
        spawn_mean_reversion_signal_generator(
            mean_rev_config,
            state.pool.clone(),
            state.quant_signal_tx.clone(),
        );

        let cross_market_config = CrossMarketSignalConfig::from_env();
        spawn_cross_market_signal_generator(
            cross_market_config,
            state.pool.clone(),
            state.quant_signal_tx.clone(),
        );

        // Spawn strategy P&L calculator (6h, computes per-strategy performance snapshots)
        let pnl_config = StrategyPnlConfig::from_env();
        spawn_strategy_pnl_calculator(pnl_config, state.pool.clone(), db_semaphore.clone());

        // Spawn metrics calculator (populates wallet_success_metrics + market regime)
        let metrics_config = MetricsCalculatorConfig::from_env();
        if metrics_config.enabled {
            let calculator = Arc::new(
                MetricsCalculator::with_regime(
                    state.pool.clone(),
                    metrics_config.clone(),
                    state.current_regime.clone(),
                )
                .with_db_semaphore(db_semaphore.clone()),
            );
            tokio::spawn(calculator.run());
            info!(
                interval_secs = metrics_config.interval_secs,
                batch_size = metrics_config.batch_size,
                "Metrics calculator background job spawned (with regime detection)"
            );
        }

        let addr = self.config.socket_addr();
        info!(address = %addr, "Starting API server");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;

        Ok(())
    }
}
