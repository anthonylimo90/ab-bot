//! Application state shared across handlers.

use anyhow::Context;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;

use auth::jwt::{JwtAuth, JwtConfig};
use auth::key_vault::KeyVault;
use auth::rbac::RbacManager;
use auth::{AuditLogger, AuditStorage, PostgresAuditStorage, TradingWallet};
use polymarket_core::api::ClobClient;
use polymarket_core::types::ArbOpportunity;
use risk_manager::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use trading_engine::executor::ExecutorConfig;
use trading_engine::OrderExecutor;

use crate::auto_optimizer::AutomationEvent;
use crate::email::{EmailClient, EmailConfig};
use crate::websocket::{OrderbookUpdate, PositionUpdate, SignalUpdate};

async fn resolve_startup_wallet_address(pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
    if let Ok(address) = std::env::var("TRADING_WALLET_ADDRESS") {
        return Ok(Some(address.to_lowercase()));
    }

    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT address
        FROM user_wallets
        WHERE is_primary = true
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(address,)| address.to_lowercase()))
}

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: PgPool,
    /// JWT secret for token validation.
    pub jwt_secret: String,
    /// JWT authentication handler.
    pub jwt_auth: Arc<JwtAuth>,
    /// RBAC manager for fine-grained permission checking.
    pub rbac: Arc<RbacManager>,
    /// Key vault for secure wallet key storage.
    pub key_vault: Arc<KeyVault>,
    /// Audit logger for security and compliance logging.
    pub audit_logger: Arc<AuditLogger>,
    /// Email client for sending transactional emails.
    pub email_client: Option<Arc<EmailClient>>,
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
    /// Broadcast channel for automation events (circuit breaker trips, etc.).
    pub automation_tx: broadcast::Sender<AutomationEvent>,
    /// Broadcast channel for arb entry signals (feeds ArbAutoExecutor).
    pub arb_entry_tx: broadcast::Sender<ArbOpportunity>,
}

impl AppState {
    /// Create a new application state.
    pub async fn new(
        pool: PgPool,
        jwt_secret: String,
        orderbook_tx: broadcast::Sender<OrderbookUpdate>,
        position_tx: broadcast::Sender<PositionUpdate>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        automation_tx: broadcast::Sender<AutomationEvent>,
        arb_entry_tx: broadcast::Sender<ArbOpportunity>,
    ) -> anyhow::Result<Self> {
        // Create JWT auth handler
        let jwt_config = JwtConfig {
            secret: jwt_secret.clone(),
            expiry_hours: 24,
            issuer: Some("ab-bot-api".to_string()),
            audience: None,
        };
        let jwt_auth = Arc::new(JwtAuth::new(jwt_config));

        // Create RBAC manager with default roles
        let rbac = Arc::new(RbacManager::new());

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

        // Create audit logger with PostgreSQL storage
        let audit_storage: Arc<dyn AuditStorage> =
            Arc::new(PostgresAuditStorage::new(pool.clone()));
        let audit_logger = Arc::new(AuditLogger::new(audit_storage));

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

        if live_trading {
            let startup_wallet = resolve_startup_wallet_address(&pool)
                .await
                .context("Failed resolving startup trading wallet address")?;

            if let Some(address) = startup_wallet {
                match key_vault.get_wallet_key(&address).await {
                    Ok(Some(key_bytes)) => {
                        let key_hex = format!("0x{}", hex::encode(key_bytes));
                        let wallet = TradingWallet::from_private_key(&key_hex)
                            .context("Failed to build trading wallet from vault key bytes")?;
                        let loaded_address = order_executor.reload_wallet(wallet).await.context(
                            "Failed to initialize live trading executor from vault wallet",
                        )?;
                        tracing::info!(wallet = %loaded_address, "Live trading executor initialized from vault");
                    }
                    Ok(None) => {
                        tracing::warn!(
                            address = %address,
                            "Startup trading wallet was selected but key not found in vault"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            address = %address,
                            error = %e,
                            "Failed loading startup trading wallet from vault"
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "No primary wallet found in vault metadata for live trading startup"
                );
            }

            if !order_executor.is_live_ready().await {
                tracing::warn!(
                    "Falling back to WALLET_PRIVATE_KEY for live trading wallet initialization"
                );
                let wallet = TradingWallet::from_env().context(
                    "LIVE_TRADING=true but no valid wallet found. Connect a primary vault wallet or set WALLET_PRIVATE_KEY",
                )?;
                let loaded_address = order_executor.reload_wallet(wallet).await.context(
                    "Failed to initialize live trading executor from WALLET_PRIVATE_KEY",
                )?;
                tracing::info!(wallet = %loaded_address, "Live trading executor initialized from WALLET_PRIVATE_KEY");
            }
        }

        // Create circuit breaker for risk management
        let mut circuit_breaker_config = CircuitBreakerConfig::default();
        if let Ok(v) = std::env::var("CB_MAX_DAILY_LOSS") {
            if let Ok(d) = v.parse::<rust_decimal::Decimal>() {
                circuit_breaker_config.max_daily_loss = d;
            }
        }
        if let Ok(v) = std::env::var("CB_MAX_DRAWDOWN_PCT") {
            if let Ok(d) = v.parse::<rust_decimal::Decimal>() {
                circuit_breaker_config.max_drawdown_pct = d;
            }
        }
        if let Ok(v) = std::env::var("CB_MAX_CONSECUTIVE_LOSSES") {
            if let Ok(n) = v.parse::<u32>() {
                circuit_breaker_config.max_consecutive_losses = n;
            }
        }
        if let Ok(v) = std::env::var("CB_COOLDOWN_MINUTES") {
            if let Ok(n) = v.parse::<i64>() {
                circuit_breaker_config.cooldown_minutes = n;
            }
        }
        let circuit_breaker = Arc::new(CircuitBreaker::new(circuit_breaker_config));

        // Create email client if configured
        let email_client = EmailConfig::from_env().and_then(|config| {
            match EmailClient::new(config) {
                Ok(client) => {
                    tracing::info!("Email client initialized successfully");
                    Some(Arc::new(client))
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize email client: {}. Password reset emails will not be sent.", e);
                    None
                }
            }
        });

        Ok(Self {
            pool,
            jwt_secret,
            jwt_auth,
            rbac,
            key_vault,
            audit_logger,
            email_client,
            clob_client,
            order_executor,
            circuit_breaker,
            orderbook_tx,
            position_tx,
            signal_tx,
            automation_tx,
            arb_entry_tx,
        })
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

    /// Subscribe to automation events.
    pub fn subscribe_automation(&self) -> broadcast::Receiver<AutomationEvent> {
        self.automation_tx.subscribe()
    }

    /// Publish an automation event.
    pub fn publish_automation(
        &self,
        event: AutomationEvent,
    ) -> Result<usize, broadcast::error::SendError<AutomationEvent>> {
        self.automation_tx.send(event)
    }

    /// Subscribe to arb entry signals.
    pub fn subscribe_arb_entry(&self) -> broadcast::Receiver<ArbOpportunity> {
        self.arb_entry_tx.subscribe()
    }

    /// Publish an arb entry signal.
    pub fn publish_arb_entry(
        &self,
        arb: ArbOpportunity,
    ) -> Result<usize, broadcast::error::SendError<ArbOpportunity>> {
        self.arb_entry_tx.send(arb)
    }

    /// Activate a vault wallet for live trading without restarting the server.
    pub async fn activate_trading_wallet(&self, address: &str) -> anyhow::Result<String> {
        let key_bytes = self
            .key_vault
            .get_wallet_key(address)
            .await
            .context("Failed to load wallet key from vault")?
            .ok_or_else(|| anyhow::anyhow!("Wallet key not found in vault for {}", address))?;
        let key_hex = format!("0x{}", hex::encode(key_bytes));
        let wallet = TradingWallet::from_private_key(&key_hex)
            .context("Failed to parse private key from vault payload")?;
        self.order_executor.reload_wallet(wallet).await
    }
}

/// Extension trait for Arc<AppState>.
impl AppState {
    /// Create an Arc-wrapped state.
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}
