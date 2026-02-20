//! Application state shared across handlers.

use anyhow::Context;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use auth::jwt::{JwtAuth, JwtConfig};
use auth::key_vault::KeyVault;
use auth::rbac::RbacManager;
use auth::{AuditLogger, AuditStorage, PostgresAuditStorage, TradingWallet};
use polymarket_core::api::{ClobClient, PolygonClient};
use polymarket_core::types::ArbOpportunity;
use risk_manager::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use trading_engine::copy_trader::CopyTrader;
use trading_engine::executor::ExecutorConfig;
use trading_engine::OrderExecutor;
use wallet_tracker::discovery::WalletDiscovery;
use wallet_tracker::trade_monitor::TradeMonitor;
use wallet_tracker::MarketRegime;

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
    /// Encryption key for sensitive DB fields (alchemy keys, etc.).
    pub encryption_key: String,
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
    /// Wallet discovery service for querying profitable wallets from DB.
    pub wallet_discovery: Arc<WalletDiscovery>,
    /// Polygon RPC client for on-chain queries (balance, etc.).
    pub polygon_client: Option<PolygonClient>,
    /// Trade monitor for copy trading (None if copy trading disabled).
    pub trade_monitor: Option<Arc<TradeMonitor>>,
    /// Copy trader (None if copy trading disabled).
    pub copy_trader: Option<Arc<RwLock<CopyTrader>>>,
    /// Current detected market regime, updated hourly by MetricsCalculator.
    pub current_regime: Arc<RwLock<MarketRegime>>,
    /// Shared Redis connection for dynamic config pub/sub (None if Redis unavailable).
    pub redis_conn: Option<redis::aio::ConnectionManager>,
    /// Set of active (non-resolved) CLOB market IDs, refreshed by OutcomeTokenCache.
    /// Used by CopyTradingMonitor to skip resolved markets before hitting the CLOB.
    pub active_clob_markets: Arc<RwLock<HashSet<String>>>,
    /// Shared copy-trade stop-loss config for runtime hot-swap (None if copy trading disabled).
    pub copy_stop_loss_config: Option<Arc<RwLock<crate::copy_trade_stop_loss::CopyStopLossConfig>>>,
    /// Shared arb executor config for runtime hot-swap (None if arb executor disabled).
    pub arb_executor_config: Option<Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>>,
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
        // Resolve encryption key for sensitive DB fields
        let encryption_key = std::env::var("ENCRYPTION_KEY").unwrap_or_else(|_| jwt_secret.clone());

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
        let live_trading_env = std::env::var("LIVE_TRADING")
            .map(|v| v == "true")
            .unwrap_or(false);
        let live_trading_workspace = match crate::runtime_sync::any_workspace_live_enabled(&pool)
            .await
        {
            Ok(enabled) => enabled,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to read workspace live_trading_enabled flags; falling back to LIVE_TRADING env"
                );
                false
            }
        };
        let live_trading = live_trading_env || live_trading_workspace;
        if live_trading_workspace && !live_trading_env {
            tracing::info!(
                "Enabling live executor because at least one workspace has live_trading_enabled=true"
            );
        }
        let executor_config = ExecutorConfig {
            live_trading,
            min_book_depth: std::env::var("EXECUTOR_MIN_BOOK_DEPTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(rust_decimal::Decimal::new(100, 0)),
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
                        match TradingWallet::from_private_key(&key_hex) {
                            Ok(wallet) => match order_executor.reload_wallet(wallet).await {
                                Ok(loaded_address) => {
                                    tracing::info!(wallet = %loaded_address, "Live trading executor initialized from vault");
                                }
                                Err(e) => {
                                    tracing::error!(
                                        error = %e,
                                        "Failed to derive API credentials from vault wallet. Server will start without live trading."
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!(error = %e, "Failed to build trading wallet from vault key bytes");
                            }
                        }
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
                match TradingWallet::from_env() {
                    Ok(wallet) => match order_executor.reload_wallet(wallet).await {
                        Ok(loaded_address) => {
                            tracing::info!(wallet = %loaded_address, "Live trading executor initialized from WALLET_PRIVATE_KEY");
                        }
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                "Failed to derive Polymarket API credentials. \
                                 Server will start without live trading. \
                                 Use the /api/trading/wallet/reload endpoint to retry."
                            );
                        }
                    },
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "LIVE_TRADING=true but no valid wallet found. \
                             Server will start without live trading."
                        );
                    }
                }
            }

            // Ensure on-chain Polymarket approvals are set for the trading wallet.
            // This is a one-time operation per wallet; existing approvals are detected and skipped.
            if order_executor.is_live_ready().await {
                let rpc_url = std::env::var("POLYGON_RPC_URL")
                    .or_else(|_| {
                        std::env::var("ALCHEMY_API_KEY")
                            .map(|k| format!("https://polygon-mainnet.g.alchemy.com/v2/{}", k))
                    })
                    .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

                // Build a signer from the same key the executor used
                let approval_wallet = TradingWallet::from_env().ok();

                if let Some(wallet) = approval_wallet {
                    let signer = wallet.signer();
                    match polymarket_core::api::approvals::ensure_polymarket_approvals(
                        signer, &rpc_url,
                    )
                    .await
                    {
                        Ok(count) => {
                            if count > 0 {
                                tracing::info!(
                                    approvals_sent = count,
                                    "Set Polymarket contract approvals, refreshing CLOB cache"
                                );
                                // Tell the CLOB server to re-read the on-chain allowance state
                                if let Err(e) = order_executor.refresh_clob_allowance_cache().await
                                {
                                    tracing::warn!(
                                        error = %e,
                                        "Failed to refresh CLOB allowance cache after approvals"
                                    );
                                }
                            } else {
                                tracing::info!("All Polymarket approvals already set");
                                // Still refresh the CLOB cache in case it's stale
                                let _ = order_executor.refresh_clob_allowance_cache().await;
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                wallet = %signer.address(),
                                "Failed to set Polymarket approvals. \
                                 If the error mentions insufficient POL for gas, \
                                 send 0.1 POL to the wallet address above on Polygon."
                            );
                        }
                    }
                }
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

        // Apply DB overrides (workspace-level CB config takes priority over env vars)
        #[derive(sqlx::FromRow)]
        struct CbOverrideRow {
            cb_max_daily_loss: Option<rust_decimal::Decimal>,
            cb_max_drawdown_pct: Option<rust_decimal::Decimal>,
            cb_max_consecutive_losses: Option<i32>,
            cb_cooldown_minutes: Option<i32>,
            cb_enabled: Option<bool>,
        }
        match sqlx::query_as::<_, CbOverrideRow>(
            r#"
            SELECT cb_max_daily_loss, cb_max_drawdown_pct, cb_max_consecutive_losses,
                   cb_cooldown_minutes, cb_enabled
            FROM workspaces
            WHERE cb_max_daily_loss IS NOT NULL
               OR cb_max_drawdown_pct IS NOT NULL
               OR cb_max_consecutive_losses IS NOT NULL
               OR cb_cooldown_minutes IS NOT NULL
               OR cb_enabled IS NOT NULL
            LIMIT 1
            "#,
        )
        .fetch_optional(&pool)
        .await
        {
            Ok(Some(row)) => {
                if let Some(v) = row.cb_max_daily_loss {
                    circuit_breaker_config.max_daily_loss = v;
                }
                if let Some(v) = row.cb_max_drawdown_pct {
                    circuit_breaker_config.max_drawdown_pct = v;
                }
                if let Some(v) = row.cb_max_consecutive_losses {
                    circuit_breaker_config.max_consecutive_losses = v as u32;
                }
                if let Some(v) = row.cb_cooldown_minutes {
                    circuit_breaker_config.cooldown_minutes = v as i64;
                }
                if let Some(v) = row.cb_enabled {
                    circuit_breaker_config.enabled = v;
                }
                tracing::info!("Loaded circuit breaker config overrides from workspace DB");
            }
            Ok(None) => {
                tracing::debug!("No workspace CB overrides found, using env/default values");
            }
            Err(e) => {
                tracing::warn!("Failed to load CB overrides from DB, using env/defaults: {e}");
            }
        }

        let circuit_breaker = Arc::new(CircuitBreaker::new(circuit_breaker_config));

        // Create Polygon client for on-chain queries (balance, etc.)
        let polygon_client = build_polygon_client_for_discovery();

        // Create wallet discovery service (works without Polygon — DB queries only)
        let wallet_discovery = match build_polygon_client_for_discovery() {
            Some(pc) => Arc::new(WalletDiscovery::new(pc, pool.clone())),
            None => Arc::new(WalletDiscovery::from_pool(pool.clone())),
        };

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

        // Create shared Redis connection for dynamic config pub/sub
        let redis_conn = {
            let redis_url = std::env::var("DYNAMIC_TUNER_REDIS_URL")
                .or_else(|_| std::env::var("REDIS_URL"))
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
            match redis::Client::open(redis_url.as_str()) {
                Ok(client) => match redis::aio::ConnectionManager::new(client).await {
                    Ok(conn) => {
                        tracing::info!("Shared Redis connection established for dynamic config");
                        Some(conn)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to connect to Redis; dynamic config updates via Redis will be unavailable");
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to open Redis client; dynamic config updates via Redis will be unavailable");
                    None
                }
            }
        };

        Ok(Self {
            pool,
            jwt_secret,
            encryption_key,
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
            wallet_discovery,
            polygon_client,
            trade_monitor: None,
            copy_trader: None,
            current_regime: Arc::new(RwLock::new(MarketRegime::Uncertain)),
            redis_conn,
            active_clob_markets: Arc::new(RwLock::new(HashSet::new())),
            copy_stop_loss_config: None,
            arb_executor_config: None,
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
    #[allow(clippy::result_large_err)]
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
        self.order_executor
            .reload_wallet(wallet)
            .await
            .with_context(|| format!("Failed to activate trading wallet {}", address))
    }
}

/// Extension trait for Arc<AppState>.
impl AppState {
    /// Create an Arc-wrapped state.
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}

fn build_polygon_client_for_discovery() -> Option<PolygonClient> {
    if let Ok(rpc_url) = std::env::var("POLYGON_RPC_URL") {
        return Some(PolygonClient::new(rpc_url));
    }
    if let Ok(alchemy_api_key) = std::env::var("ALCHEMY_API_KEY") {
        return Some(PolygonClient::with_alchemy(&alchemy_api_key));
    }
    None
}
