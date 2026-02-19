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
pub mod auto_optimizer;
pub mod copy_trade_stop_loss;
pub mod copy_trading;
pub mod crypto;
pub mod dynamic_tuner;
pub mod email;
pub mod error;
pub mod exit_handler;
pub mod handlers;
pub mod metrics_calculator;
pub mod middleware;
pub mod redis_forwarder;
pub mod routes;
pub mod runtime_sync;
pub mod schema;
pub mod state;
pub mod wallet_harvester;
pub mod websocket;

pub use arb_executor::{spawn_arb_auto_executor, ArbExecutorConfig};
pub use auto_optimizer::AutoOptimizer;
pub use copy_trade_stop_loss::{spawn_copy_stop_loss_monitor, CopyStopLossConfig};
pub use copy_trading::{spawn_copy_trading_monitor, CopyTradingConfig};
pub use dynamic_tuner::{spawn_dynamic_config_subscriber, DynamicTuner};
pub use error::ApiError;
pub use exit_handler::{spawn_exit_handler, ExitHandlerConfig};
pub use metrics_calculator::{MetricsCalculator, MetricsCalculatorConfig};
pub use redis_forwarder::{spawn_redis_forwarder, RedisForwarderConfig};
pub use routes::create_router;
pub use runtime_sync::reconcile_copy_runtime;
pub use state::AppState;
pub use wallet_harvester::{spawn_wallet_harvester, WalletHarvesterConfig};

use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use sqlx::PgPool;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::AtomicI64;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::{info, warn, Level};
use trading_engine::copy_trader::CopyTrader;
use wallet_tracker::trade_monitor::{MonitorConfig, TradeMonitor};

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
        let (automation_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (arb_entry_tx, _) = broadcast::channel(config.ws_channel_capacity);

        // Create app state (not yet Arc-wrapped so copy trading fields can be set)
        let state = AppState::new(
            pool,
            config.jwt_secret.clone(),
            orderbook_tx,
            position_tx,
            signal_tx,
            automation_tx,
            arb_entry_tx,
        )
        .await?;

        Ok(Self { config, state })
    }

    /// Run the server.
    pub async fn run(mut self) -> anyhow::Result<()> {
        // ── Copy trading setup (must happen before Arc-wrapping state) ──
        let mut copy_config = CopyTradingConfig::from_env();
        if !copy_config.enabled {
            match crate::runtime_sync::any_workspace_copy_enabled(&self.state.pool).await {
                Ok(true) => {
                    copy_config.enabled = true;
                    info!(
                        "Enabling copy trading runtime because at least one workspace has copy_trading_enabled=true"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed reading workspace copy_trading_enabled flags; falling back to env-only copy config"
                    );
                }
            }
        }
        let mut copy_monitor_args: Option<(
            CopyTradingConfig,
            Arc<TradeMonitor>,
            Arc<RwLock<CopyTrader>>,
            Arc<AtomicI64>,
        )> = None;

        if copy_config.enabled {
            let trade_monitor = Arc::new(TradeMonitor::new(
                self.state.clob_client.clone(),
                MonitorConfig::from_env(),
            ));
            let total_capital = std::env::var("COPY_TOTAL_CAPITAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(rust_decimal::Decimal::new(10000, 0));
            let copy_trader = CopyTrader::new(self.state.order_executor.clone(), total_capital)
                .with_policy(trading_engine::CopyTradingPolicy::from_env());
            let copy_trader = Arc::new(RwLock::new(copy_trader));
            let copy_latency_atomic = Arc::new(AtomicI64::new(copy_config.max_latency_secs));

            if let Err(e) = crate::dynamic_tuner::sync_dynamic_config_snapshot_to_copy_trader(
                &self.state.pool,
                &copy_trader,
                Some(&copy_latency_atomic),
            )
            .await
            {
                warn!(
                    error = %e,
                    "Failed to apply startup dynamic config snapshot; falling back to env policy"
                );
            }

            // Store in AppState so allocation handlers can sync at runtime
            self.state.trade_monitor = Some(trade_monitor.clone());
            self.state.copy_trader = Some(copy_trader.clone());

            match crate::runtime_sync::reconcile_copy_runtime(
                &self.state.pool,
                Some(&trade_monitor),
                Some(&copy_trader),
            )
            .await
            {
                Ok(stats) => {
                    if stats.desired_wallets == 0 {
                        warn!("Copy trading runtime is enabled but no active wallets are eligible");
                    }
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to reconcile copy runtime on startup; copy monitor may start empty"
                    );
                }
            }

            copy_monitor_args =
                Some((copy_config, trade_monitor, copy_trader, copy_latency_atomic));
        }

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

        // Spawn arb auto-executor if enabled
        let mut arb_config = ArbExecutorConfig::from_env();
        if !arb_config.enabled {
            match crate::runtime_sync::any_workspace_arb_enabled(&state.pool).await {
                Ok(true) => {
                    arb_config.enabled = true;
                    info!(
                        "Enabling arb auto-executor because at least one workspace has arb_auto_execute=true"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to read workspace arb_auto_execute flags; falling back to env-only arb config"
                    );
                }
            }
        }
        if arb_config.enabled {
            spawn_arb_auto_executor(
                arb_config,
                state.subscribe_arb_entry(),
                state.signal_tx.clone(),
                state.order_executor.clone(),
                state.circuit_breaker.clone(),
                state.clob_client.clone(),
                state.pool.clone(),
                arb_dedup.clone(),
            );
        }

        // Spawn exit handler if enabled
        let exit_config = ExitHandlerConfig::from_env();
        if exit_config.enabled {
            spawn_exit_handler(
                exit_config,
                state.order_executor.clone(),
                state.circuit_breaker.clone(),
                state.clob_client.clone(),
                state.signal_tx.clone(),
                state.pool.clone(),
                arb_dedup.clone(),
            );
        }

        // Spawn auto-optimizer background service
        let optimizer = Arc::new(
            AutoOptimizer::new(state.pool.clone())
                .with_runtime_handles(state.trade_monitor.clone(), state.copy_trader.clone()),
        );
        tokio::spawn(optimizer.start(None));

        // Extract the latency atomic (if copy trading is active) so the
        // dynamic config subscriber can write to it at runtime.
        let copy_latency_atomic: Option<Arc<AtomicI64>> = copy_monitor_args
            .as_ref()
            .map(|(_, _, _, atomic)| atomic.clone());

        // Subscribe local runtime to dynamic updates (copy-trader knobs)
        let redis_url = std::env::var("DYNAMIC_CONFIG_REDIS_URL")
            .or_else(|_| std::env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        spawn_dynamic_config_subscriber(
            redis_url,
            state.copy_trader.clone(),
            state.pool.clone(),
            copy_latency_atomic,
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
        );

        // Spawn metrics calculator (populates wallet_success_metrics + market regime)
        let metrics_config = MetricsCalculatorConfig::from_env();
        if metrics_config.enabled {
            let calculator = Arc::new(MetricsCalculator::with_regime(
                state.pool.clone(),
                metrics_config.clone(),
                state.current_regime.clone(),
            ));
            tokio::spawn(calculator.run());
            info!(
                interval_secs = metrics_config.interval_secs,
                batch_size = metrics_config.batch_size,
                "Metrics calculator background job spawned (with regime detection)"
            );
        }

        // Start copy trading monitor (objects were created above, before Arc wrap)
        if let Some((copy_config, trade_monitor, copy_trader, latency_atomic)) = copy_monitor_args {
            trade_monitor.start().await?;
            spawn_copy_trading_monitor(
                copy_config,
                trade_monitor.clone(),
                copy_trader.clone(),
                state.circuit_breaker.clone(),
                state.signal_tx.clone(),
                state.pool.clone(),
                latency_atomic,
            );

            // Spawn copy-trade stop-loss / mirror-exit monitor
            let stop_loss_config = CopyStopLossConfig::from_env();
            spawn_copy_stop_loss_monitor(
                stop_loss_config,
                state.pool.clone(),
                state.order_executor.clone(),
                state.circuit_breaker.clone(),
                state.clob_client.clone(),
                copy_trader,
                Some(trade_monitor),
                state.signal_tx.clone(),
            );

            tracing::info!(
                "Copy trading monitor stack initialized (with stop-loss + mirror exits)"
            );
        }

        let addr = self.config.socket_addr();
        info!(address = %addr, "Starting API server");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;

        Ok(())
    }
}
