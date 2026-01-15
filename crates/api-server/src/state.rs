//! Application state shared across handlers.

use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;

use auth::jwt::{JwtAuth, JwtConfig};
use auth::key_vault::KeyVault;
use polymarket_core::api::ClobClient;
use risk_manager::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use trading_engine::executor::ExecutorConfig;
use trading_engine::OrderExecutor;

use crate::websocket::{OrderbookUpdate, PositionUpdate, SignalUpdate};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: PgPool,
    /// JWT secret for token validation.
    pub jwt_secret: String,
    /// JWT authentication handler.
    pub jwt_auth: Arc<JwtAuth>,
    /// Key vault for secure wallet key storage.
    pub key_vault: Arc<KeyVault>,
    /// CLOB API client for Polymarket.
    pub clob_client: Arc<ClobClient>,
    /// Order execution engine.
    pub order_executor: Arc<OrderExecutor>,
    /// Circuit breaker for risk management.
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Broadcast channel for orderbook updates.
    pub orderbook_tx: broadcast::Sender<OrderbookUpdate>,
    /// Broadcast channel for position updates.
    pub position_tx: broadcast::Sender<PositionUpdate>,
    /// Broadcast channel for trading signals.
    pub signal_tx: broadcast::Sender<SignalUpdate>,
}

impl AppState {
    /// Create a new application state.
    pub fn new(
        pool: PgPool,
        jwt_secret: String,
        orderbook_tx: broadcast::Sender<OrderbookUpdate>,
        position_tx: broadcast::Sender<PositionUpdate>,
        signal_tx: broadcast::Sender<SignalUpdate>,
    ) -> Self {
        // Create JWT auth handler
        let jwt_config = JwtConfig {
            secret: jwt_secret.clone(),
            expiry_hours: 24,
            issuer: Some("ab-bot-api".to_string()),
            audience: None,
        };
        let jwt_auth = Arc::new(JwtAuth::new(jwt_config));

        // Create KeyVault from environment or with default config
        let key_vault = match KeyVault::from_env() {
            Ok(vault) => Arc::new(vault),
            Err(e) => {
                tracing::warn!(
                    "Failed to initialize KeyVault from env: {}. Using in-memory vault.",
                    e
                );
                // Fall back to in-memory vault with a default key
                let default_key = jwt_secret.as_bytes().to_vec();
                Arc::new(KeyVault::new(
                    auth::key_vault::KeyVaultProvider::Memory,
                    default_key,
                ))
            }
        };

        // Create CLOB client
        let clob_url = std::env::var("POLYMARKET_CLOB_URL").ok();
        let clob_client = Arc::new(ClobClient::new(clob_url, None));

        // Create order executor
        let live_trading = std::env::var("LIVE_TRADING")
            .map(|v| v == "true")
            .unwrap_or(false);
        let executor_config = ExecutorConfig {
            live_trading,
            ..Default::default()
        };
        let order_executor = Arc::new(OrderExecutor::new(clob_client.clone(), executor_config));

        // Create circuit breaker for risk management
        let circuit_breaker_config = CircuitBreakerConfig::default();
        let circuit_breaker = Arc::new(CircuitBreaker::new(circuit_breaker_config));

        Self {
            pool,
            jwt_secret,
            jwt_auth,
            key_vault,
            clob_client,
            order_executor,
            circuit_breaker,
            orderbook_tx,
            position_tx,
            signal_tx,
        }
    }

    /// Subscribe to orderbook updates.
    pub fn subscribe_orderbook(&self) -> broadcast::Receiver<OrderbookUpdate> {
        self.orderbook_tx.subscribe()
    }

    /// Subscribe to position updates.
    pub fn subscribe_positions(&self) -> broadcast::Receiver<PositionUpdate> {
        self.position_tx.subscribe()
    }

    /// Subscribe to signal updates.
    pub fn subscribe_signals(&self) -> broadcast::Receiver<SignalUpdate> {
        self.signal_tx.subscribe()
    }

    /// Publish an orderbook update.
    pub fn publish_orderbook(
        &self,
        update: OrderbookUpdate,
    ) -> Result<usize, broadcast::error::SendError<OrderbookUpdate>> {
        self.orderbook_tx.send(update)
    }

    /// Publish a position update.
    pub fn publish_position(
        &self,
        update: PositionUpdate,
    ) -> Result<usize, broadcast::error::SendError<PositionUpdate>> {
        self.position_tx.send(update)
    }

    /// Publish a signal update.
    pub fn publish_signal(
        &self,
        update: SignalUpdate,
    ) -> Result<usize, broadcast::error::SendError<SignalUpdate>> {
        self.signal_tx.send(update)
    }
}

/// Extension trait for Arc<AppState>.
impl AppState {
    /// Create an Arc-wrapped state.
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}
