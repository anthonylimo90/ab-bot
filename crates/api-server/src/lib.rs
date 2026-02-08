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
pub mod copy_trading;
pub mod email;
pub mod error;
pub mod exit_handler;
pub mod handlers;
pub mod middleware;
pub mod redis_forwarder;
pub mod routes;
pub mod state;
pub mod websocket;

pub use arb_executor::{spawn_arb_auto_executor, ArbExecutorConfig};
pub use auto_optimizer::AutoOptimizer;
pub use copy_trading::{spawn_copy_trading_monitor, CopyTradingConfig};
pub use error::ApiError;
pub use exit_handler::{spawn_exit_handler, ExitHandlerConfig};
pub use redis_forwarder::{spawn_redis_forwarder, RedisForwarderConfig};
pub use routes::create_router;
pub use state::AppState;

use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use axum::Router;
use polymarket_core::api::PolygonClient;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::{info, Level};
use trading_engine::copy_trader::{CopyTrader, TrackedWallet};
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
    router: Router,
    state: Arc<AppState>,
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

        // Create app state
        let state = Arc::new(
            AppState::new(
                pool,
                config.jwt_secret.clone(),
                orderbook_tx,
                position_tx,
                signal_tx,
                automation_tx,
                arb_entry_tx,
            )
            .await?,
        );

        // Build router
        let router = create_router(state.clone());

        // Add middleware - use minimal tracing to avoid log spam
        // Only log errors (not every request) to stay under Railway's 500 logs/sec limit
        let router = router
            .layer(
                TraceLayer::new_for_http()
                    // Log request starts for debugging (temporarily at ERROR level to see them)
                    .on_request(|request: &Request<_>, _span: &tracing::Span| {
                        tracing::info!(
                            method = %request.method(),
                            uri = %request.uri(),
                            "Incoming request"
                        );
                    })
                    // Only log responses at DEBUG level
                    .on_response(DefaultOnResponse::new().level(Level::DEBUG))
                    // Log failures at ERROR level with request details
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
            .layer(if config.cors_permissive {
                CorsLayer::permissive()
            } else {
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any)
            });

        Ok(Self {
            config,
            router,
            state,
        })
    }

    /// Get a reference to the app state.
    pub fn state(&self) -> Arc<AppState> {
        self.state.clone()
    }

    /// Run the server.
    pub async fn run(self) -> anyhow::Result<()> {
        // Spawn Redis forwarder for signal bridging
        let redis_config = RedisForwarderConfig::from_env();
        spawn_redis_forwarder(
            redis_config,
            self.state.signal_tx.clone(),
            self.state.orderbook_tx.clone(),
            self.state.arb_entry_tx.clone(),
        );

        // Shared dedup set for arb executor + exit handler
        let arb_dedup = Arc::new(RwLock::new(HashSet::new()));

        // Spawn arb auto-executor if enabled
        let arb_config = ArbExecutorConfig::from_env();
        if arb_config.enabled {
            spawn_arb_auto_executor(
                arb_config,
                self.state.subscribe_arb_entry(),
                self.state.signal_tx.clone(),
                self.state.order_executor.clone(),
                self.state.circuit_breaker.clone(),
                self.state.clob_client.clone(),
                self.state.pool.clone(),
                arb_dedup.clone(),
            );
        }

        // Spawn exit handler if enabled
        let exit_config = ExitHandlerConfig::from_env();
        if exit_config.enabled {
            spawn_exit_handler(
                exit_config,
                self.state.order_executor.clone(),
                self.state.circuit_breaker.clone(),
                self.state.clob_client.clone(),
                self.state.signal_tx.clone(),
                self.state.pool.clone(),
                arb_dedup.clone(),
            );
        }

        // Spawn auto-optimizer background service
        let optimizer = Arc::new(AutoOptimizer::new(self.state.pool.clone()));
        tokio::spawn(optimizer.start(None));

        // Spawn copy trading monitor + wallet trade monitor if enabled
        let copy_config = CopyTradingConfig::from_env();
        if copy_config.enabled {
            #[derive(sqlx::FromRow)]
            struct TrackedWalletRow {
                address: String,
                label: Option<String>,
                allocation_pct: Decimal,
                copy_delay_ms: i32,
                max_position_size: Option<Decimal>,
            }

            let tracked_wallets: Vec<TrackedWalletRow> = sqlx::query_as(
                r#"
                SELECT address, label, allocation_pct, copy_delay_ms, max_position_size
                FROM tracked_wallets
                WHERE copy_enabled = TRUE
                ORDER BY success_score DESC NULLS LAST, added_at ASC
                "#,
            )
            .fetch_all(&self.state.pool)
            .await?;

            if tracked_wallets.is_empty() {
                tracing::warn!(
                    "Copy trading is enabled but no tracked wallets have copy_enabled=true"
                );
            } else if let Some(polygon_client) = build_polygon_client() {
                let trade_monitor =
                    Arc::new(TradeMonitor::new(polygon_client, MonitorConfig::default()));
                let total_capital = std::env::var("COPY_TOTAL_CAPITAL")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(Decimal::new(10000, 0));
                let copy_trader = CopyTrader::new(self.state.order_executor.clone(), total_capital);

                for row in tracked_wallets {
                    trade_monitor.add_wallet(&row.address).await;

                    let mut wallet = TrackedWallet::new(row.address.clone(), row.allocation_pct)
                        .with_delay(row.copy_delay_ms.max(0) as u64);
                    if let Some(label) = row.label {
                        wallet = wallet.with_alias(label);
                    }
                    if let Some(max_size) = row.max_position_size {
                        wallet = wallet.with_max_size(max_size);
                    }
                    copy_trader.add_tracked_wallet(wallet);
                }

                trade_monitor.start().await?;
                spawn_copy_trading_monitor(
                    copy_config,
                    trade_monitor,
                    Arc::new(RwLock::new(copy_trader)),
                    self.state.signal_tx.clone(),
                );
                tracing::info!("Copy trading monitor stack initialized");
            } else {
                tracing::warn!(
                    "Copy trading is enabled but POLYGON_RPC_URL / ALCHEMY_API_KEY is not set; wallet trade monitoring is disabled"
                );
            }
        }

        let addr = self.config.socket_addr();
        info!(address = %addr, "Starting API server");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, self.router).await?;

        Ok(())
    }

    /// Get the router for testing.
    pub fn router(&self) -> Router {
        self.router.clone()
    }
}

fn build_polygon_client() -> Option<Arc<PolygonClient>> {
    if let Ok(rpc_url) = std::env::var("POLYGON_RPC_URL") {
        return Some(Arc::new(PolygonClient::new(rpc_url)));
    }
    if let Ok(alchemy_api_key) = std::env::var("ALCHEMY_API_KEY") {
        return Some(Arc::new(PolygonClient::with_alchemy(&alchemy_api_key)));
    }
    None
}
