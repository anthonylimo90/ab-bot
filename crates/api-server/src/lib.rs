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

pub mod copy_trading;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod redis_forwarder;
pub mod routes;
pub mod state;
pub mod websocket;

pub use copy_trading::{spawn_copy_trading_monitor, CopyTradingConfig};
pub use error::ApiError;
pub use redis_forwarder::{spawn_redis_forwarder, RedisForwarderConfig};
pub use routes::create_router;
pub use state::AppState;

use axum::Router;
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

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
            port: std::env::var("API_PORT")
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
        // Create broadcast channels for WebSocket
        let (orderbook_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (position_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (signal_tx, _) = broadcast::channel(config.ws_channel_capacity);

        // Create app state
        let state = Arc::new(AppState::new(
            pool,
            config.jwt_secret.clone(),
            orderbook_tx,
            position_tx,
            signal_tx,
        ));

        // Build router
        let router = create_router(state.clone());

        // Add middleware
        let router = router
            .layer(TraceLayer::new_for_http())
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
        );

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
