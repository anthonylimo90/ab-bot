//! Order execution engine for low-latency trade placement.

use anyhow::Result;
use auth::TradingWallet;
use dashmap::DashMap;
use polymarket_core::api::clob::{AuthenticatedClobClient, BalanceAllowanceResponse, OrderType};
use polymarket_core::api::ClobClient;
use polymarket_core::signing::{OrderSide as SigningOrderSide, OrderSigner};
use polymarket_core::types::{
    ExecutionReport, LimitOrder, MarketOrder, OrderBook, OrderSide, OrderStatus,
};
use rust_decimal::Decimal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const SUBMISSION_UNKNOWN: &str = "submission_requires_reconciliation";
/// The order was not sent because another live order for the wallet is
/// unresolved; unlike [`SUBMISSION_UNKNOWN`] this is a definitive non-submission.
const SUBMISSION_FENCE_BUSY: &str = "live_submission_fence_busy";
/// Multiple of the configured timeout allowed for a whole live operation.
const LIVE_SAFETY_TIMEOUT_FACTOR: u64 = 3;

#[path = "executor/submission.rs"]
mod submission;

const BALANCE_ALLOWANCE_RETRY_MARKER: &str = "refreshable_balance_allowance";

/// Metrics for order execution performance.
#[derive(Debug, Default)]
pub struct ExecutionMetrics {
    pub orders_submitted: u64,
    pub orders_filled: u64,
    pub orders_rejected: u64,
    pub total_volume: Decimal,
    pub total_fees: Decimal,
    pub avg_latency_us: u64,
}

/// Configuration for the order executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Default slippage tolerance for market orders.
    pub default_slippage: Decimal,
    /// Maximum order size.
    pub max_order_size: Decimal,
    /// Minimum notional depth at best price level required to execute.
    pub min_book_depth: Decimal,
    /// Fee rate for trades.
    pub fee_rate: Decimal,
    /// Whether to actually execute orders (false = paper trading).
    pub live_trading: bool,
    /// API key for authenticated trading.
    pub api_key: Option<String>,
    /// Timeout for order execution in milliseconds.
    pub timeout_ms: u64,
    /// Maximum number of retry attempts for transient failures.
    pub max_retries: u32,
    /// Base delay between retries in milliseconds (exponential backoff).
    pub retry_base_delay_ms: u64,
    /// Maximum delay between retries in milliseconds.
    pub retry_max_delay_ms: u64,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_slippage: Decimal::new(1, 2), // 1%
            max_order_size: Decimal::new(10000, 0),
            min_book_depth: Decimal::new(100, 0),
            fee_rate: Decimal::new(2, 2), // 2%
            live_trading: false,
            api_key: None,
            timeout_ms: 30000,        // 30 seconds default timeout
            max_retries: 3,           // 3 retry attempts
            retry_base_delay_ms: 100, // 100ms initial delay
            retry_max_delay_ms: 5000, // 5 second max delay
        }
    }
}

/// Execution price of a market order.
struct MarketExecutionPrice {
    /// Worst price the order may fill at; signed as the FOK limit.
    limit: Decimal,
    /// Expected average fill price across the levels it consumes.
    average: Decimal,
}

/// Result of a retry operation.
#[derive(Debug, Clone)]
pub enum RetryOutcome<T> {
    /// Operation succeeded.
    Success(T),
    /// Operation failed after all retries.
    Failed { last_error: String, attempts: u32 },
    /// Operation timed out.
    Timeout { elapsed_ms: u64 },
}

/// Determines if an error should trigger a retry.
fn is_retryable_error(error: &str) -> bool {
    let retryable_patterns = [
        "timeout",
        "connection",
        "network",
        "temporarily unavailable",
        "rate limit",
        "503",
        "502",
        "504",
        "etimedout",
        "econnreset",
        "econnrefused",
    ];

    let error_lower = error.to_lowercase();
    error_lower.contains(BALANCE_ALLOWANCE_RETRY_MARKER)
        || retryable_patterns.iter().any(|p| error_lower.contains(p))
}

fn is_balance_allowance_error(error: &str) -> bool {
    let error_lower = error.to_lowercase();
    error_lower.contains("not enough balance / allowance")
        || error_lower.contains("insufficient balance")
        || error_lower.contains("insufficient allowance")
        || (error_lower.contains("balance") && error_lower.contains("allowance"))
}

fn parse_decimal_value(raw: &str) -> Option<Decimal> {
    raw.trim().parse::<Decimal>().ok()
}

/// Low-latency order executor for Polymarket CLOB.
pub struct OrderExecutor {
    clob_client: Arc<ClobClient>,
    config: ExecutorConfig,
    database: Option<sqlx::PgPool>,
    /// Pending orders awaiting confirmation.
    pending_orders: DashMap<Uuid, OrderStatus>,
    /// Channel for execution reports.
    report_tx: broadcast::Sender<ExecutionReport>,
    metrics: std::sync::RwLock<ExecutionMetrics>,
    /// Authenticated client for live trading (swappable at runtime).
    auth_client: Arc<RwLock<Option<AuthenticatedClobClient>>>,
    /// Runtime-toggleable live mode flag (overrides config.live_trading).
    live_override: AtomicBool,
    /// Set when the process shuts down: no new trading operation starts.
    draining: Arc<AtomicBool>,
    /// Trading operations started and not yet finished.
    operations_in_flight: Arc<AtomicUsize>,
    /// Live submissions between their journal entry and recorded outcome.
    submissions_in_flight: AtomicUsize,
}

/// Admission of one unit of trading work that must not be cut in half by a
/// shutdown: both legs of an arb, a position's exit sales, a manual order.
/// Dropping it ends the operation.
#[must_use = "the operation ends when the guard is dropped"]
pub struct TradingOperation(Arc<AtomicUsize>);

impl Drop for TradingOperation {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl OrderExecutor {
    /// Create a new order executor (paper trading mode).
    pub fn new(clob_client: Arc<ClobClient>, config: ExecutorConfig) -> Self {
        let (report_tx, _) = broadcast::channel(1000);
        let live = config.live_trading;
        Self {
            clob_client,
            config,
            database: None,
            pending_orders: DashMap::new(),
            report_tx,
            metrics: std::sync::RwLock::new(ExecutionMetrics::default()),
            auth_client: Arc::new(RwLock::new(None)),
            live_override: AtomicBool::new(live),
            draining: Arc::new(AtomicBool::new(false)),
            operations_in_flight: Arc::new(AtomicUsize::new(0)),
            submissions_in_flight: AtomicUsize::new(0),
        }
    }

    /// Create a new order executor for live trading with wallet authentication.
    ///
    /// # Arguments
    ///
    /// * `clob_client` - Base CLOB client for market data
    /// * `wallet` - Trading wallet with private key for signing
    /// * `config` - Executor configuration (should have `live_trading: true`)
    ///
    /// # Example
    ///
    /// ```ignore
    /// use auth::TradingWallet;
    ///
    /// let wallet = TradingWallet::from_env()?;
    /// let config = ExecutorConfig {
    ///     live_trading: true,
    ///     ..Default::default()
    /// };
    /// let executor = OrderExecutor::new_with_wallet(clob_client, wallet, config);
    /// ```
    pub fn new_with_wallet(
        clob_client: Arc<ClobClient>,
        wallet: TradingWallet,
        config: ExecutorConfig,
    ) -> Self {
        let (report_tx, _) = broadcast::channel(1000);

        let auth_client = Self::build_auth_client(wallet, clob_client.as_ref().clone());

        info!(
            address = %auth_client.address(),
            live_trading = %config.live_trading,
            "Created order executor with wallet authentication"
        );

        let live = config.live_trading;
        Self {
            clob_client,
            config,
            database: None,
            pending_orders: DashMap::new(),
            report_tx,
            metrics: std::sync::RwLock::new(ExecutionMetrics::default()),
            auth_client: Arc::new(RwLock::new(Some(auth_client))),
            live_override: AtomicBool::new(live),
            draining: Arc::new(AtomicBool::new(false)),
            operations_in_flight: Arc::new(AtomicUsize::new(0)),
            submissions_in_flight: AtomicUsize::new(0),
        }
    }

    /// Required for live submission so intent identity survives process failure.
    /// Start a trading operation, or `None` once shutdown has begun: callers
    /// must then not start the work (nothing has been sent for it yet).
    pub fn begin_operation(&self) -> Option<TradingOperation> {
        // Counted before the check, so a drain either refuses this operation
        // or waits for it to finish.
        self.operations_in_flight.fetch_add(1, Ordering::SeqCst);
        let operation = TradingOperation(self.operations_in_flight.clone());
        (!self.draining.load(Ordering::SeqCst)).then_some(operation)
    }

    /// Count an operation a drain must wait for but never refuse (one that
    /// only takes risk away, such as cancelling an order): unlike
    /// [`Self::begin_operation`], it is counted after shutdown began too.
    pub fn track_operation(&self) -> TradingOperation {
        self.operations_in_flight.fetch_add(1, Ordering::SeqCst);
        TradingOperation(self.operations_in_flight.clone())
    }

    /// Whether shutdown has begun.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Refuse new trading operations from now on; started ones continue.
    pub fn begin_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }

    /// [`Self::begin_draining`], then wait up to `timeout` for started
    /// operations and live submissions to finish. `true` when none is left.
    pub async fn drain(&self, timeout: std::time::Duration) -> bool {
        self.begin_draining();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.operations_in_flight.load(Ordering::SeqCst) == 0
                && self.submissions_in_flight.load(Ordering::SeqCst) == 0
            {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub fn with_database(mut self, pool: sqlx::PgPool) -> Self {
        self.database = Some(pool);
        self
    }

    fn build_auth_client(wallet: TradingWallet, client: ClobClient) -> AuthenticatedClobClient {
        let signer = OrderSigner::new(wallet.into_signer());
        AuthenticatedClobClient::new(client, signer)
    }

    /// Initialize the authenticated client by deriving API credentials.
    ///
    /// This must be called before executing live orders.
    pub async fn initialize_live_trading(&self) -> Result<()> {
        let mut slot = self.auth_client.write().await;
        let client = slot.as_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "No authenticated client - call reload_wallet() or use new_with_wallet() first"
            )
        })?;
        client
            .create_or_derive_api_key()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to derive API credentials: {}", e))?;

        info!("Live trading initialized - API credentials derived");
        Ok(())
    }

    /// Check if live trading is initialized and ready.
    pub async fn is_live_ready(&self) -> bool {
        self.is_live() && self.has_live_signer().await
    }

    /// Whether the trading wallet's signer and API credentials are loaded,
    /// whatever the live switch says now.
    pub async fn has_live_signer(&self) -> bool {
        let slot = self.auth_client.read().await;
        slot.as_ref().map(|c| c.has_credentials()).unwrap_or(false)
    }

    /// Get the trading wallet address (if available).
    pub async fn wallet_address(&self) -> Option<String> {
        let slot = self.auth_client.read().await;
        slot.as_ref().map(|c| c.address())
    }

    /// Hot-reload the live trading wallet signer and API credentials.
    /// Loaded whatever the live switch says: the exits of live positions
    /// need it while live trading is off (new live entries need the switch).
    pub async fn reload_wallet(&self, wallet: TradingWallet) -> Result<String> {
        let mut auth_client = Self::build_auth_client(wallet, self.clob_client.as_ref().clone());
        let address = auth_client.address();
        auth_client
            .create_or_derive_api_key()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to derive API credentials: {}", e))?;

        let mut slot = self.auth_client.write().await;
        *slot = Some(auth_client);
        info!(address = %address, "Live trading wallet reloaded");
        Ok(address)
    }

    /// Look up, and cache, which exchange each token's orders are signed for,
    /// so orders on them never wait on (or fail at) that lookup. An error
    /// when a lookup fails. Nothing to do outside live trading.
    pub async fn resolve_exchanges(&self, token_ids: &[&str]) -> Result<()> {
        let slot = self.auth_client.read().await;
        let Some(client) = slot.as_ref() else {
            return Ok(());
        };
        for token_id in token_ids {
            client.is_neg_risk(token_id).await.map_err(|error| {
                anyhow::anyhow!("Could not tell which exchange token {token_id} trades on: {error}")
            })?;
        }
        Ok(())
    }

    /// Tell the CLOB to re-read on-chain balance/allowance state.
    ///
    /// Call this after setting on-chain approvals so the CLOB picks up the new values.
    pub async fn refresh_clob_allowance_cache(&self) -> anyhow::Result<()> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No authenticated client"))?;
        Self::refresh_clob_allowance_cache_for_client(client).await;
        Ok(())
    }

    /// Query the authenticated CLOB client's balance/allowance snapshot.
    pub async fn get_live_balance_allowance(
        &self,
        asset_type: &str,
        token_id: Option<&str>,
    ) -> Result<BalanceAllowanceResponse> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No authenticated client"))?;
        Ok(client.get_balance_allowance(token_id, asset_type).await?)
    }

    /// Optional telemetry stream. Execution results are returned directly to the
    /// caller; absent or lagging observers must never block order bookkeeping.
    pub fn subscribe_reports(&self) -> broadcast::Receiver<ExecutionReport> {
        self.report_tx.subscribe()
    }

    /// Execute a market order with timeout and retry logic. Tests only: it
    /// names no wallet or position, so production orders go through the
    /// reserved and per-position entry points.
    #[cfg(test)]
    pub async fn execute_market_order(&self, order: MarketOrder) -> Result<ExecutionReport> {
        self.execute_market_order_bound(order, self.is_live(), None)
            .await
    }

    pub async fn validate_position_execution(
        &self,
        position: &polymarket_core::types::Position,
    ) -> Result<()> {
        self.validate_execution(&position.execution_mode, position.wallet_address.as_deref())
            .await
    }

    /// Whether an order in `mode` (for `wallet`) can be carried out here. An
    /// order trades in the mode its position or reservation took: switching
    /// live trading off stops new live entries (they choose their mode from
    /// the switch), never the exits, second legs or recovery sales of live
    /// positions already open, and switching it on does not strand paper
    /// positions. A live order needs this process's signer, credentials
    /// loaded, for the same wallet.
    async fn validate_execution(&self, mode: &str, wallet: Option<&str>) -> Result<()> {
        match mode {
            "paper" => Ok(()),
            "live"
                if self.has_live_signer().await
                    && wallet.is_some()
                    && self
                        .wallet_address()
                        .await
                        .as_deref()
                        .map(str::to_lowercase)
                        == wallet.map(str::to_lowercase) =>
            {
                Ok(())
            }
            _ => anyhow::bail!("Position execution mode or wallet does not match the executor"),
        }
    }

    pub async fn execute_reserved_market_order(
        &self,
        order: MarketOrder,
        mode: &str,
        wallet: Option<&str>,
    ) -> Result<ExecutionReport> {
        self.validate_execution(mode, wallet).await?;
        self.execute_market_order_bound(order, mode == "live", wallet)
            .await
    }

    pub async fn execute_reserved_limit_order(
        &self,
        order: LimitOrder,
        mode: &str,
        wallet: Option<&str>,
    ) -> Result<ExecutionReport> {
        self.validate_execution(mode, wallet).await?;
        self.execute_limit_order_bound(order, mode == "live", wallet)
            .await
    }

    pub async fn execute_market_order_for_position(
        &self,
        order: MarketOrder,
        position: &polymarket_core::types::Position,
        leg: &str,
    ) -> Result<ExecutionReport> {
        self.validate_position_execution(position).await?;
        let id = order.id;
        if position.execution_mode == "live" {
            let pool = self
                .database
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Durable storage required"))?;
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT id FROM positions WHERE id=$1 AND version=$2 FOR UPDATE")
                .bind(position.id)
                .bind(position.version)
                .fetch_one(&mut *tx)
                .await?;
            let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM position_order_links WHERE position_id=$1 AND NOT settled)")
                .bind(position.id).fetch_one(&mut *tx).await?;
            if pending {
                anyhow::bail!("{SUBMISSION_UNKNOWN}: position has an unresolved order");
            }
            // Switching live trading off stops new live entries, whenever
            // the entry chose its mode: a buy goes out only for a position
            // whose entry has already sent one (the second leg of a pair),
            // never as an entry's first order. Sales always go out.
            if order.side == OrderSide::Buy && !self.is_live() {
                // A buy the exchange refused, or that was never sent (no
                // journaled submission), started nothing.
                let started: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM position_order_links l \
                     JOIN execution_intents i ON i.id = l.id \
                     WHERE l.position_id=$1 AND l.side='buy' AND i.status <> 'rejected')",
                )
                .bind(position.id)
                .fetch_one(&mut *tx)
                .await?;
                if !started {
                    anyhow::bail!("Live trading is switched off; no new live entry is started");
                }
            }
            sqlx::query("INSERT INTO position_order_links(id,position_id,leg,side,requested_quantity) VALUES($1,$2,$3,$4,$5)")
                .bind(id).bind(position.id).bind(leg).bind(if order.side == OrderSide::Buy { "buy" } else { "sell" }).bind(order.quantity)
                .execute(&mut *tx).await?;
            tx.commit().await?;
        }
        let result = self
            .execute_market_order_bound(
                order,
                position.execution_mode == "live",
                position.wallet_address.as_deref(),
            )
            .await;
        if let Some(pool) = &self.database {
            if result
                .as_ref()
                .is_ok_and(|report| report.status == OrderStatus::Rejected)
            {
                sqlx::query("UPDATE position_order_links SET settled=TRUE WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM execution_intents WHERE id=$1 AND status<>'rejected')")
                    .bind(id).execute(pool).await?;
            }
        }
        result
    }

    async fn execute_market_order_bound(
        &self,
        order: MarketOrder,
        live: bool,
        wallet: Option<&str>,
    ) -> Result<ExecutionReport> {
        let start = std::time::Instant::now();

        // Validate order
        if order.quantity > self.config.max_order_size {
            let report = ExecutionReport::rejected(
                order.id,
                order.market_id.clone(),
                order.outcome_id.clone(),
                order.side,
                format!(
                    "Order size {} exceeds maximum {}",
                    order.quantity, self.config.max_order_size
                ),
            );
            self.send_report(report.clone());
            return Ok(report);
        }

        self.pending_orders.insert(order.id, OrderStatus::Pending);
        info!(
            order_id = %order.id,
            market = %order.market_id,
            side = ?order.side,
            quantity = %order.quantity,
            "Executing market order"
        );

        // Execute with retry logic
        let report = self
            .execute_with_retry(
                || async {
                    if live {
                        self.execute_live_market_order(&order, wallet).await
                    } else {
                        self.simulate_market_order(&order).await
                    }
                },
                &order.id,
                &order.market_id,
                &order.outcome_id,
                order.side,
                live,
            )
            .await;

        // Update metrics
        {
            let mut metrics = self.metrics.write().unwrap();
            metrics.orders_submitted += 1;
            if report.is_success() {
                metrics.orders_filled += 1;
                metrics.total_volume += report.total_value();
                metrics.total_fees += report.fees_paid;
            } else {
                metrics.orders_rejected += 1;
            }
            let latency_us = start.elapsed().as_micros() as u64;
            metrics.avg_latency_us = (metrics.avg_latency_us * (metrics.orders_submitted - 1)
                + latency_us)
                / metrics.orders_submitted;
        }

        if matches!(
            report.status,
            OrderStatus::Filled
                | OrderStatus::Rejected
                | OrderStatus::Cancelled
                | OrderStatus::Expired
        ) {
            self.pending_orders.remove(&order.id);
        }
        self.send_report(report.clone());

        debug!(
            order_id = %order.id,
            status = ?report.status,
            latency_us = %start.elapsed().as_micros(),
            "Order execution complete"
        );

        Ok(report)
    }

    /// Execute an operation with timeout and exponential backoff retry.
    async fn execute_with_retry<F, Fut>(
        &self,
        operation: F,
        order_id: &Uuid,
        market_id: &str,
        outcome_id: &str,
        side: OrderSide,
        live: bool,
    ) -> ExecutionReport
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<ExecutionReport>>,
    {
        // A live operation prepares under its own deadline and then posts once,
        // bounded by the HTTP client timeout, so it records its own outcome.
        // The outer timeout is only a safety net: dropping a journaled POST
        // mid-flight would leave an intent the exchange may never have seen.
        let timeout = std::time::Duration::from_millis(if live {
            self.config
                .timeout_ms
                .saturating_mul(LIVE_SAFETY_TIMEOUT_FACTOR)
        } else {
            self.config.timeout_ms
        });
        let mut attempts = 0u32;
        let mut last_error = String::new();

        // Live operations cross a non-idempotent exchange boundary. Never rerun
        // the operation after a timeout or transport failure.
        let max_retries = if live { 0 } else { self.config.max_retries };
        while attempts <= max_retries {
            attempts += 1;

            // Execute with timeout
            let result = tokio::time::timeout(timeout, operation()).await;

            match result {
                Ok(Ok(report)) => {
                    if attempts > 1 {
                        info!(
                            order_id = %order_id,
                            attempts = attempts,
                            "Order succeeded after retry"
                        );
                    }
                    return report;
                }
                Ok(Err(e)) => {
                    last_error = e.to_string();

                    if last_error.contains(SUBMISSION_UNKNOWN) {
                        return ExecutionReport::pending(
                            *order_id,
                            market_id.into(),
                            outcome_id.into(),
                            side,
                            last_error,
                        );
                    }
                    // Check if this is a retryable error
                    if !is_retryable_error(&last_error) {
                        warn!(
                            order_id = %order_id,
                            error = %last_error,
                            "Non-retryable error, failing immediately"
                        );
                        return ExecutionReport::rejected(
                            *order_id,
                            market_id.to_string(),
                            outcome_id.to_string(),
                            side,
                            last_error,
                        );
                    }

                    if attempts <= max_retries {
                        // Calculate exponential backoff delay
                        let delay_ms = std::cmp::min(
                            self.config.retry_base_delay_ms * (2_u64.pow(attempts - 1)),
                            self.config.retry_max_delay_ms,
                        );

                        warn!(
                            order_id = %order_id,
                            attempt = attempts,
                            max_attempts = self.config.max_retries + 1,
                            error = %last_error,
                            retry_delay_ms = delay_ms,
                            "Retryable error, will retry"
                        );

                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
                Err(_) => {
                    if live {
                        return ExecutionReport::pending(
                            *order_id,
                            market_id.into(),
                            outcome_id.into(),
                            side,
                            format!("{SUBMISSION_UNKNOWN}: live operation timed out"),
                        );
                    }
                    // Timeout
                    last_error = format!("Operation timed out after {}ms", self.config.timeout_ms);

                    if attempts <= max_retries {
                        let delay_ms = std::cmp::min(
                            self.config.retry_base_delay_ms * (2_u64.pow(attempts - 1)),
                            self.config.retry_max_delay_ms,
                        );

                        warn!(
                            order_id = %order_id,
                            attempt = attempts,
                            max_attempts = self.config.max_retries + 1,
                            timeout_ms = self.config.timeout_ms,
                            retry_delay_ms = delay_ms,
                            "Operation timed out, will retry"
                        );

                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        // All retries exhausted
        error!(
            order_id = %order_id,
            attempts = attempts,
            last_error = %last_error,
            "Order failed after all retry attempts"
        );

        ExecutionReport::rejected(
            *order_id,
            market_id.to_string(),
            outcome_id.to_string(),
            side,
            format!("Failed after {} attempts: {}", attempts, last_error),
        )
    }

    /// Execute a limit order with timeout and retry logic. Tests only, as
    /// `execute_market_order`.
    #[cfg(test)]
    pub async fn execute_limit_order(&self, order: LimitOrder) -> Result<ExecutionReport> {
        self.execute_limit_order_bound(order, self.is_live(), None)
            .await
    }

    async fn execute_limit_order_bound(
        &self,
        order: LimitOrder,
        live: bool,
        wallet: Option<&str>,
    ) -> Result<ExecutionReport> {
        // Validate order
        if order.quantity > self.config.max_order_size {
            let report = ExecutionReport::rejected(
                order.id,
                order.market_id.clone(),
                order.outcome_id.clone(),
                order.side,
                format!(
                    "Order size {} exceeds maximum {}",
                    order.quantity, self.config.max_order_size
                ),
            );
            self.send_report(report.clone());
            return Ok(report);
        }

        self.pending_orders.insert(order.id, OrderStatus::Pending);
        info!(
            order_id = %order.id,
            market = %order.market_id,
            side = ?order.side,
            price = %order.price,
            quantity = %order.quantity,
            "Placing limit order"
        );

        // Execute with retry logic
        let report = self
            .execute_with_retry(
                || async {
                    if live {
                        self.execute_live_limit_order(&order, wallet).await
                    } else {
                        self.simulate_limit_order(&order).await
                    }
                },
                &order.id,
                &order.market_id,
                &order.outcome_id,
                order.side,
                live,
            )
            .await;

        if matches!(
            report.status,
            OrderStatus::Filled
                | OrderStatus::Rejected
                | OrderStatus::Cancelled
                | OrderStatus::Expired
        ) {
            self.pending_orders.remove(&order.id);
        }
        self.send_report(report.clone());
        Ok(report)
    }

    /// Cancel through the authenticated exchange. Local bookkeeping is only
    /// updated after the exchange acknowledges this exact order ID.
    pub async fn cancel_exchange_order(&self, exchange_order_id: &str) -> Result<()> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Live client unavailable"))?;
        Ok(client.cancel_order(exchange_order_id).await?)
    }

    pub async fn cancel_order(&self, order_id: Uuid) -> Result<bool> {
        if self.is_live() {
            anyhow::bail!("Live cancellation requires a persisted exchange order ID");
        }
        Ok(self.pending_orders.remove(&order_id).is_some())
    }

    /// Get current execution metrics.
    pub fn metrics(&self) -> ExecutionMetrics {
        self.metrics.read().unwrap().clone()
    }

    /// Check if executor is in live trading mode (reads runtime override).
    pub fn is_live(&self) -> bool {
        self.live_override.load(Ordering::Relaxed)
    }

    /// Toggle live trading mode at runtime.
    pub fn set_live_mode(&self, enabled: bool) {
        self.live_override.store(enabled, Ordering::Relaxed);
    }

    /// Get a reference to the underlying CLOB client.
    pub fn clob_client(&self) -> &ClobClient {
        &self.clob_client
    }

    /// Default slippage budget attached to market orders created by higher layers.
    pub fn default_slippage(&self) -> Decimal {
        self.config.default_slippage
    }

    /// Minimum notional a market buy needs at the best ask to execute.
    pub fn min_book_depth(&self) -> Decimal {
        self.config.min_book_depth
    }

    /// Verify that the live wallet has enough collateral balance and allowance
    /// for a multi-leg buy before any leg is submitted.
    pub async fn ensure_live_buying_power(&self, required_collateral: Decimal) -> Result<()> {
        if required_collateral <= Decimal::ZERO || !self.is_live_ready().await {
            return Ok(());
        }

        let client_guard = self.auth_client.read().await;
        let Some(client) = client_guard.as_ref() else {
            return Err(anyhow::anyhow!(
                "Live trading wallet is not initialized for collateral preflight"
            ));
        };

        let withheld = self.collateral_withheld(&client.address()).await?
            + self.collateral_resting_buys(&client.address()).await?;
        Self::ensure_live_order_capacity(
            client,
            Uuid::new_v4(),
            OrderSide::Buy,
            "collateral",
            required_collateral + withheld,
        )
        .await
    }

    /// USDC the wallet's resting live buys (manual orders) hold at the
    /// exchange, which takes it from what a new order may spend. Checked for
    /// a pair before its first leg: a second leg refused for want of cash
    /// would leave the first unhedged. A single order is checked by the
    /// exchange itself.
    async fn collateral_resting_buys(&self, wallet: &str) -> Result<Decimal> {
        let Some(pool) = &self.database else {
            return Ok(Decimal::ZERO);
        };
        // Each at its amount (a market buy without a price at $1 a share),
        // plus the most taker fee it can carry, as withdrawals hold them.
        let resting: Vec<(Decimal, Option<Decimal>)> = sqlx::query_as(
            "SELECT quantity - COALESCE(filled_quantity, 0), price \
             FROM orders \
             WHERE execution_mode = 'live' AND side = 'buy' \
               AND status IN ('pending', 'open', 'partially_filled') \
               AND (wallet_address IS NULL OR LOWER(wallet_address) = LOWER($1))",
        )
        .bind(wallet)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("Could not read resting buys for the pair preflight: {e}"))?;
        Ok(resting
            .into_iter()
            .map(|(remaining, price)| open_buy_cost(remaining, price))
            .sum())
    }

    /// The withdrawals from the live wallet still being settled, by id: a
    /// pair compares them before and after its entry is recorded (from then
    /// on new withdrawals wait for it), to check its cash again when one was
    /// reserved in between.
    pub async fn withdrawals_in_flight(&self) -> Result<Vec<Uuid>> {
        let Some(pool) = &self.database else {
            return Ok(Vec::new());
        };
        let Some(wallet) = self.wallet_address().await else {
            return Ok(Vec::new());
        };
        sqlx::query_scalar(
            "SELECT id FROM wallet_withdrawals \
             WHERE LOWER(wallet_address) = LOWER($1) \
               AND status IN ('pending', 'submitting', 'submitted')",
        )
        .bind(wallet)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("Could not read withdrawals in flight: {e}"))
    }

    /// USDC that withdrawals from `wallet` still being settled may take: the
    /// exchange balance does not know about them until they are mined.
    async fn collateral_withheld(&self, wallet: &str) -> Result<Decimal> {
        let Some(pool) = &self.database else {
            return Ok(Decimal::ZERO);
        };
        let withheld: Decimal = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount), 0) FROM wallet_withdrawals \
             WHERE LOWER(wallet_address) = LOWER($1) \
               AND status IN ('pending', 'submitting', 'submitted')",
        )
        .bind(wallet)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            anyhow::anyhow!("Could not read withdrawals in flight for the buy preflight: {e}")
        })?;
        Ok(withheld)
    }

    // Private methods

    async fn refresh_clob_allowance_cache_for_client(client: &AuthenticatedClobClient) {
        if let Err(error) = client.update_balance_allowance("COLLATERAL").await {
            warn!(error = %error, "Failed to refresh CLOB collateral allowance cache");
        }
    }

    async fn get_balance_allowance_snapshot(
        client: &AuthenticatedClobClient,
        asset_type: &str,
        token_id: Option<&str>,
    ) -> Option<BalanceAllowanceResponse> {
        match client.get_balance_allowance(token_id, asset_type).await {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                warn!(
                    error = %error,
                    asset_type,
                    token_id,
                    "Failed to query CLOB balance/allowance snapshot"
                );
                None
            }
        }
    }

    async fn ensure_live_order_capacity(
        client: &AuthenticatedClobClient,
        order_id: Uuid,
        side: OrderSide,
        token_id: &str,
        required_amount: Decimal,
    ) -> Result<()> {
        if required_amount <= Decimal::ZERO {
            return Ok(());
        }

        let (asset_type, balance_token_id, asset_label) = match side {
            OrderSide::Buy => ("COLLATERAL", None, "collateral"),
            OrderSide::Sell => ("CONDITIONAL", Some(token_id), "conditional"),
        };

        let mut snapshot = match Self::get_balance_allowance_snapshot(
            client,
            asset_type,
            balance_token_id,
        )
        .await
        {
            Some(snapshot) => snapshot,
            None => anyhow::bail!("CLOB balance preflight unavailable"),
        };

        let mut balance = snapshot.balance_in_units();
        let mut allowance = snapshot.allowance_in_units();
        anyhow::ensure!(balance.is_some(), "Invalid CLOB balance in preflight");

        let balance_short = balance.is_some_and(|value| value < required_amount);
        // A blank allowance means the exchange's cache is empty: refresh it
        // before relying on it, as for one that is too small.
        let allowance_short = allowance.is_none_or(|value| value < required_amount);

        if balance_short || allowance_short {
            warn!(
                order_id = %order_id,
                asset_type,
                token_id,
                balance = %snapshot.balance,
                allowance = %snapshot.allowance,
                required_amount = %required_amount,
                "CLOB balance/allowance preflight indicates insufficient capacity; refreshing before submit"
            );

            if let Err(error) = client
                .update_balance_allowance_for_token(asset_type, balance_token_id)
                .await
            {
                warn!(
                    error = %error,
                    order_id = %order_id,
                    asset_type,
                    "Failed to refresh CLOB balance/allowance before live submit"
                );
            }

            snapshot =
                match Self::get_balance_allowance_snapshot(client, asset_type, balance_token_id)
                    .await
                {
                    Some(snapshot) => snapshot,
                    None => anyhow::bail!("CLOB balance preflight unavailable after refresh"),
                };
            balance = snapshot.balance_in_units();
            allowance = snapshot.allowance_in_units();
            anyhow::ensure!(balance.is_some(), "Invalid CLOB balance after refresh");
            // A buy is not sent on an allowance still unknown: the exchange
            // could refuse it after an earlier leg of a pair filled. (A sell
            // the exchange refuses costs nothing, and exits must not wait on
            // an allowance the exchange may not report.)
            anyhow::ensure!(
                allowance.is_some() || side == OrderSide::Sell,
                "CLOB collateral allowance unknown after refresh"
            );
        }

        if let Some(value) = balance {
            if value < required_amount {
                return Err(anyhow::anyhow!(
                    "Insufficient {} balance for live order: available {}, required {}",
                    asset_label,
                    value,
                    required_amount
                ));
            }
        }

        if let Some(value) = allowance {
            if value < required_amount {
                return Err(anyhow::anyhow!(
                    "Insufficient {} allowance for live order: available {}, required {}",
                    asset_label,
                    value,
                    required_amount
                ));
            }
        }

        Ok(())
    }

    fn reject_market_order(
        &self,
        order: &MarketOrder,
        error: impl Into<String>,
    ) -> ExecutionReport {
        ExecutionReport::rejected(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            error.into(),
        )
    }

    /// Deadline for preparing a live order (book, balance and signing data).
    fn preparation_deadline(&self) -> tokio::time::Instant {
        tokio::time::Instant::now() + std::time::Duration::from_millis(self.config.timeout_ms)
    }

    /// Runs one preparation step before `deadline`. Expiry is a plain error:
    /// preparation precedes the journaled POST, so nothing has been sent.
    async fn prepare<T>(
        deadline: tokio::time::Instant,
        step: impl std::future::Future<Output = T>,
    ) -> Result<T> {
        tokio::time::timeout_at(deadline, step).await.map_err(|_| {
            anyhow::anyhow!("Live order preparation exceeded its deadline; nothing was sent")
        })
    }

    /// Prices a market order: the limit to sign and the expected average fill.
    #[allow(clippy::result_large_err)] // ExecutionReport is a rich domain value, not a simple error
    fn price_market_order(
        &self,
        order: &MarketOrder,
        book: &OrderBook,
    ) -> std::result::Result<MarketExecutionPrice, ExecutionReport> {
        if order.side == OrderSide::Sell {
            return self.price_market_sell(order, book);
        }
        let (price, available_size) = self.best_level_for_order(order, book)?;
        if let Some(report) = self.validate_market_order_at_best_level(order, price, available_size)
        {
            return Err(report);
        }
        Ok(MarketExecutionPrice {
            limit: price,
            average: price,
        })
    }

    /// Exits must be able to leave thin books, so entry depth gates do not
    /// apply to sells. A sell walks the bids down to the price that covers its
    /// whole quantity, staying within the slippage tolerance of the best bid
    /// (and of the expected price, when given), and signs that floor as its
    /// FOK limit; resting bids at or above it fill at their own prices.
    #[allow(clippy::result_large_err)] // ExecutionReport is a rich domain value, not a simple error
    fn price_market_sell(
        &self,
        order: &MarketOrder,
        book: &OrderBook,
    ) -> std::result::Result<MarketExecutionPrice, ExecutionReport> {
        let mut bids: Vec<&polymarket_core::types::PriceLevel> = book.bids.iter().collect();
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        let Some(best) = bids.first().map(|level| level.price) else {
            return Err(self.reject_market_order(order, "No bids available"));
        };
        if order.quantity <= Decimal::ZERO {
            return Err(self.reject_market_order(order, "Sell quantity must be positive"));
        }

        let tolerance = order
            .max_slippage
            .unwrap_or(self.config.default_slippage)
            .max(Decimal::ZERO);
        let mut floor = best * (Decimal::ONE - tolerance);
        if order.expected_price > Decimal::ZERO {
            floor = floor.max(order.expected_price * (Decimal::ONE - tolerance));
        }

        let mut remaining = order.quantity;
        let mut proceeds = Decimal::ZERO;
        let mut limit = best;
        for level in bids {
            if level.price < floor || remaining.is_zero() {
                break;
            }
            let take = remaining.min(level.size);
            proceeds += take * level.price;
            remaining -= take;
            limit = level.price;
        }
        if remaining > Decimal::ZERO {
            return Err(self.reject_market_order(
                order,
                format!(
                    "Insufficient bid liquidity: {} of {} shares available within {} of best bid {}",
                    order.quantity - remaining,
                    order.quantity,
                    tolerance,
                    best
                ),
            ));
        }

        Ok(MarketExecutionPrice {
            limit,
            average: proceeds / order.quantity,
        })
    }

    #[allow(clippy::result_large_err)] // ExecutionReport is a rich domain value, not a simple error
    fn best_level_for_order(
        &self,
        order: &MarketOrder,
        book: &OrderBook,
    ) -> std::result::Result<(Decimal, Decimal), ExecutionReport> {
        match order.side {
            OrderSide::Buy => book
                .best_ask_level()
                .map(|ask| (ask.price, ask.size))
                .ok_or_else(|| self.reject_market_order(order, "No asks available")),
            OrderSide::Sell => book
                .best_bid_level()
                .map(|bid| (bid.price, bid.size))
                .ok_or_else(|| self.reject_market_order(order, "No bids available")),
        }
    }

    fn validate_market_order_at_best_level(
        &self,
        order: &MarketOrder,
        price: Decimal,
        available_size: Decimal,
    ) -> Option<ExecutionReport> {
        let min_depth = self.config.min_book_depth;
        let notional_at_best = available_size * price;
        if notional_at_best < min_depth {
            return Some(self.reject_market_order(
                order,
                format!(
                    "Insufficient liquidity: ${:.2} at best level (min ${:.0})",
                    notional_at_best, min_depth
                ),
            ));
        }

        if order.quantity > available_size {
            return Some(self.reject_market_order(
                order,
                format!(
                    "Order size {} exceeds available {} at best price",
                    order.quantity, available_size
                ),
            ));
        }

        if let Some(max_slippage) = order.max_slippage {
            let slippage = match order.side {
                OrderSide::Buy => {
                    if order.expected_price > Decimal::ZERO {
                        (price - order.expected_price) / order.expected_price
                    } else {
                        Decimal::ZERO
                    }
                }
                OrderSide::Sell => {
                    if order.expected_price > Decimal::ZERO {
                        (order.expected_price - price) / order.expected_price
                    } else {
                        Decimal::ZERO
                    }
                }
            };

            if slippage > max_slippage {
                return Some(self.reject_market_order(
                    order,
                    format!(
                        "Slippage {:.4} exceeds max {:.4} (expected={}, actual={})",
                        slippage, max_slippage, order.expected_price, price
                    ),
                ));
            }
        }

        None
    }

    async fn execute_live_market_order(
        &self,
        order: &MarketOrder,
        wallet: Option<&str>,
    ) -> Result<ExecutionReport> {
        // Check if we have an authenticated client
        let client_guard = self.auth_client.read().await;
        let client = match client_guard.as_ref() {
            Some(client) => client,
            None => {
                warn!("No authenticated client for live trading");
                return Ok(ExecutionReport::rejected(
                    order.id,
                    order.market_id.clone(),
                    order.outcome_id.clone(),
                    order.side,
                    "Live trading wallet is not initialized".to_string(),
                ));
            }
        };

        if wallet.is_some_and(|address| !address.eq_ignore_ascii_case(&client.address())) {
            anyhow::bail!("Position wallet changed before submission");
        }

        // Everything before the journaled POST runs under this deadline: when it
        // expires, nothing has been sent and the order is plainly rejected.
        let deadline = self.preparation_deadline();
        let book =
            Self::prepare(deadline, self.clob_client.get_order_book(&order.outcome_id)).await??;
        let price = match self.price_market_order(order, &book) {
            Ok(execution) => execution.limit,
            Err(report) => return Ok(report),
        };

        // Convert order side to signing order side
        let signing_side = match order.side {
            OrderSide::Buy => SigningOrderSide::Buy,
            OrderSide::Sell => SigningOrderSide::Sell,
        };

        if !client.has_credentials() {
            return Err(anyhow::anyhow!(
                "Live trading not initialized - call initialize_live_trading() first"
            ));
        }

        // What the order spends (USDC for a buy, shares for a sell): this is
        // what gets signed.
        let market_amount = match order.side {
            OrderSide::Buy => order.quantity * price,
            OrderSide::Sell => order.quantity,
        };
        // A buy must also leave room for the taker fee charged on top of its
        // amount and for withdrawals that may still take USDC from the
        // wallet; that reserve is checked, never spent.
        let required_amount = match order.side {
            OrderSide::Buy => {
                market_amount
                    + taker_fee_allowance(order.quantity, price)
                    + Self::prepare(deadline, self.collateral_withheld(&client.address())).await??
            }
            OrderSide::Sell => market_amount,
        };

        Self::prepare(
            deadline,
            Self::ensure_live_order_capacity(
                client,
                order.id,
                order.side,
                &order.outcome_id,
                required_amount,
            ),
        )
        .await??;

        // Create signed order (FOK for market orders, expiration=0)
        let signed_order = Self::prepare(
            deadline,
            client.create_market_order(
                &order.outcome_id,
                signing_side,
                price,
                market_amount,
                OrderType::Fok,
            ),
        )
        .await?
        .map_err(|e| anyhow::anyhow!("Failed to create market order: {}", e))?;

        // Post the order (FOK for market orders)
        let response = match self
            .submit_live_once(
                client,
                order.id,
                &order.market_id,
                signed_order,
                OrderType::Fok,
                if order.completes_pair {
                    submission::PAIR_FENCE_WAIT
                        .min(std::time::Duration::from_millis(self.config.timeout_ms))
                        .max(submission::FENCE_WAIT)
                } else {
                    submission::FENCE_WAIT
                },
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let error_text = error.to_string();
                if is_balance_allowance_error(&error_text) {
                    warn!(
                        order_id = %order.id,
                        error = %error_text,
                        "Live market order rejected due to balance/allowance; refreshing CLOB cache before retry"
                    );
                    Self::refresh_clob_allowance_cache_for_client(client).await;
                    return Err(anyhow::anyhow!(
                        "{}: Failed to post order: {}",
                        BALANCE_ALLOWANCE_RETRY_MARKER,
                        error_text
                    ));
                }

                return Err(anyhow::anyhow!("Failed to post order: {}", error_text));
            }
        };

        info!(
            order_id = %order.id,
            clob_order_id = %response.order_id,
            status = %response.status,
            "Live market order submitted"
        );

        if response.refused() {
            return Ok(ExecutionReport::rejected(
                order.id,
                order.market_id.clone(),
                order.outcome_id.clone(),
                order.side,
                format!(
                    "FOK order refused: {}",
                    response.error_msg.as_deref().unwrap_or_default()
                ),
            ));
        }
        let report = self.report_from_exchange_response(
            order.id,
            &order.market_id,
            &order.outcome_id,
            order.side,
            order.quantity,
            (order.side == OrderSide::Buy).then_some(market_amount),
            true,
            &response,
        );
        if report.status == OrderStatus::Pending {
            self.mark_fok_acceptance_unresolved(order.id).await?;
        }
        Ok(report)
    }

    /// A fill-or-kill order the exchange accepted without its fill amounts
    /// has filled or been killed, which only its record can tell: its
    /// submission stays unresolved (fencing the wallet) until reconciled,
    /// rather than resting as an acknowledged order that may never be found.
    async fn mark_fok_acceptance_unresolved(&self, order_id: Uuid) -> Result<()> {
        if let Some(pool) = &self.database {
            // Only the acknowledgement just stored: never a submission the
            // reconciler has settled since.
            sqlx::query(
                "UPDATE execution_intents SET status='unknown' WHERE id=$1 AND status='acknowledged'",
            )
            .bind(order_id)
                .execute(pool)
                .await
                .map_err(|error| {
                    anyhow::anyhow!(
                        "{SUBMISSION_UNKNOWN}: failed to persist unresolved acceptance: {error}"
                    )
                })?;
        }
        Ok(())
    }

    async fn execute_live_limit_order(
        &self,
        order: &LimitOrder,
        wallet: Option<&str>,
    ) -> Result<ExecutionReport> {
        // Check if we have an authenticated client
        let client_guard = self.auth_client.read().await;
        let client = match client_guard.as_ref() {
            Some(client) => client,
            None => {
                warn!("No authenticated client for live trading");
                return Ok(ExecutionReport::rejected(
                    order.id,
                    order.market_id.clone(),
                    order.outcome_id.clone(),
                    order.side,
                    "Live trading wallet is not initialized".to_string(),
                ));
            }
        };

        if wallet.is_some_and(|address| !address.eq_ignore_ascii_case(&client.address())) {
            anyhow::bail!("Reserved order wallet changed before submission");
        }
        // Convert order side to signing order side
        let signing_side = match order.side {
            OrderSide::Buy => SigningOrderSide::Buy,
            OrderSide::Sell => SigningOrderSide::Sell,
        };

        if !client.has_credentials() {
            return Err(anyhow::anyhow!(
                "Live trading not initialized - call initialize_live_trading() first"
            ));
        }

        let deadline = self.preparation_deadline();
        let required_amount = match order.side {
            OrderSide::Buy => {
                order.quantity * order.price
                    + taker_fee_allowance(order.quantity, order.price)
                    + Self::prepare(deadline, self.collateral_withheld(&client.address())).await??
            }
            OrderSide::Sell => order.quantity,
        };

        Self::prepare(
            deadline,
            Self::ensure_live_order_capacity(
                client,
                order.id,
                order.side,
                &order.outcome_id,
                required_amount,
            ),
        )
        .await??;

        let kind = if order.order_type == polymarket_core::types::OrderType::FOK {
            OrderType::Fok
        } else {
            OrderType::Gtc
        };
        // GTC/FOK signatures use expiration=0.
        let signed_order = Self::prepare(
            deadline,
            client.create_order(
                &order.outcome_id,
                signing_side,
                order.price,
                order.quantity,
                kind,
            ),
        )
        .await?
        .map_err(|e| anyhow::anyhow!("Failed to create order: {}", e))?;

        // Post the order (GTC for limit orders)
        let response = match self
            .submit_live_once(
                client,
                order.id,
                &order.market_id,
                signed_order,
                kind,
                submission::FENCE_WAIT,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let error_text = error.to_string();
                if is_balance_allowance_error(&error_text) {
                    warn!(
                        order_id = %order.id,
                        error = %error_text,
                        "Live limit order rejected due to balance/allowance; refreshing CLOB cache before retry"
                    );
                    Self::refresh_clob_allowance_cache_for_client(client).await;
                    return Err(anyhow::anyhow!(
                        "{}: Failed to post order: {}",
                        BALANCE_ALLOWANCE_RETRY_MARKER,
                        error_text
                    ));
                }

                return Err(anyhow::anyhow!("Failed to post order: {}", error_text));
            }
        };

        info!(
            order_id = %order.id,
            clob_order_id = %response.order_id,
            status = %response.status,
            price = %order.price,
            quantity = %order.quantity,
            "Live limit order submitted"
        );

        let report = self.report_from_exchange_response(
            order.id,
            &order.market_id,
            &order.outcome_id,
            order.side,
            order.quantity,
            (order.side == OrderSide::Buy).then(|| order.quantity * order.price),
            kind == OrderType::Fok,
            &response,
        );
        if kind == OrderType::Fok && report.status == OrderStatus::Pending {
            self.mark_fok_acceptance_unresolved(order.id).await?;
        }
        Ok(report)
    }

    async fn simulate_market_order(&self, order: &MarketOrder) -> Result<ExecutionReport> {
        // Paper trading simulation - fetch real orderbook prices
        let book = self.clob_client.get_order_book(&order.outcome_id).await?;
        let price = match self.price_market_order(order, &book) {
            Ok(execution) => execution.average,
            Err(report) => return Ok(report),
        };

        let fees = order.quantity * price * self.config.fee_rate;

        info!(
            order_id = %order.id,
            price = %price,
            filled = %order.quantity,
            fees = %fees,
            "[PAPER] Simulated market order fill"
        );

        Ok(ExecutionReport::filled(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            order.quantity,
            price,
            fees,
        ))
    }

    async fn simulate_limit_order(&self, order: &LimitOrder) -> Result<ExecutionReport> {
        // Paper trading has no matching engine: a limit order fills only
        // when the book can fill all of it now at its limit or better, at
        // the book's own prices. One that would rest is refused rather than
        // booked at a price no one offered.
        let book = self.clob_client.get_order_book(&order.outcome_id).await?;
        let mut levels: Vec<&polymarket_core::types::PriceLevel> = match order.side {
            OrderSide::Buy => book
                .asks
                .iter()
                .filter(|level| level.price <= order.price)
                .collect(),
            OrderSide::Sell => book
                .bids
                .iter()
                .filter(|level| level.price >= order.price)
                .collect(),
        };
        match order.side {
            OrderSide::Buy => levels.sort_by_key(|level| level.price),
            OrderSide::Sell => levels.sort_by_key(|level| std::cmp::Reverse(level.price)),
        }
        let mut remaining = order.quantity;
        let mut value = Decimal::ZERO;
        for level in levels {
            if remaining <= Decimal::ZERO {
                break;
            }
            let take = remaining.min(level.size);
            value += take * level.price;
            remaining -= take;
        }
        if order.quantity <= Decimal::ZERO || remaining > Decimal::ZERO {
            let report = ExecutionReport::rejected(
                order.id,
                order.market_id.clone(),
                order.outcome_id.clone(),
                order.side,
                "Paper trading fills a limit order only when the book can fill all of it now \
                 at the limit or better; resting orders are not simulated"
                    .to_string(),
            );
            return Ok(report);
        }
        let price = value / order.quantity;
        let fees = value * self.config.fee_rate;

        info!(
            order_id = %order.id,
            limit = %order.price,
            price = %price,
            quantity = %order.quantity,
            fees = %fees,
            "[PAPER] Simulated limit order fill"
        );

        Ok(ExecutionReport::filled(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            order.quantity,
            price,
            fees,
        ))
    }

    pub async fn confirmed_exchange_fill(
        &self,
        maker: &str,
        snapshot: &polymarket_core::api::clob::ExchangeOrder,
    ) -> Result<(Decimal, Decimal)> {
        let guard = self.auth_client.read().await;
        let client = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No authenticated client"))?;
        if !maker.eq_ignore_ascii_case(&client.address()) {
            anyhow::bail!("Intent wallet mismatch");
        }
        Ok(client.confirmed_order_fill(snapshot).await?)
    }

    pub async fn get_exchange_order(
        &self,
        id: &str,
    ) -> Result<polymarket_core::api::clob::ExchangeOrder> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Live client unavailable"))?;
        Ok(client.get_order(id).await?)
    }

    /// The exchange's record of an order, or `None` when it holds none.
    pub async fn find_exchange_order(
        &self,
        id: &str,
    ) -> Result<Option<polymarket_core::api::clob::ExchangeOrder>> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Live client unavailable"))?;
        Ok(client.find_order(id).await?)
    }

    /// Whether `maker`'s trades on `asset_id` since `after` (unix seconds)
    /// involve the exchange order `order_id`.
    pub async fn exchange_order_has_trades(
        &self,
        maker: &str,
        order_id: &str,
        asset_id: &str,
        after: i64,
    ) -> Result<bool> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Live client unavailable"))?;
        if !maker.eq_ignore_ascii_case(&client.address()) {
            anyhow::bail!("Intent wallet mismatch");
        }
        Ok(client.order_has_trades(order_id, asset_id, after).await?)
    }

    /// The report of an order the exchange answered. A fill-or-kill order
    /// matched with its amounts filled in full, though its signed amount
    /// (rounded to what the exchange accepts) may buy slightly fewer shares
    /// than `quantity`. `max_spend` is the most USDC a buy signed for (its
    /// quantity at its limit price): an answer that spent more is not read
    /// as a fill (the order stays pending for the reconciler, which books
    /// settled trades), so amounts read in the wrong units cannot over-book.
    #[allow(clippy::too_many_arguments)]
    fn report_from_exchange_response(
        &self,
        id: Uuid,
        market: &str,
        outcome: &str,
        side: OrderSide,
        quantity: Decimal,
        max_spend: Option<Decimal>,
        fill_or_kill: bool,
        response: &polymarket_core::api::clob::PostOrderResponse,
    ) -> ExecutionReport {
        let mut report = ExecutionReport::pending(
            id,
            market.into(),
            outcome.into(),
            side,
            "Awaiting confirmed exchange fills".into(),
        )
        .with_exchange_id(response.order_id.clone());
        report.requested_quantity = quantity;
        if response.refused() {
            report.status = OrderStatus::Rejected;
            return report;
        }
        // "live" and "delayed" are acceptance states, never proof of a fill.
        if response.status.eq_ignore_ascii_case("matched") {
            if let (Some(maker), Some(taker)) = (
                response
                    .making_amount
                    .as_deref()
                    .and_then(parse_decimal_value),
                response
                    .taking_amount
                    .as_deref()
                    .and_then(parse_decimal_value),
            ) {
                let (filled, value) = match side {
                    OrderSide::Buy => (taker, maker),
                    OrderSide::Sell => (maker, taker),
                };
                // A buy can receive more shares than it asked for: the
                // exchange passes a better price on to the taker as shares.
                // A sell never parts with more than it signed.
                if filled > Decimal::ZERO
                    && (side == OrderSide::Buy || filled <= quantity)
                    && value > Decimal::ZERO
                    && value <= filled
                    // The cent the signed amount is rounded by.
                    && max_spend.is_none_or(|cap| value <= cap + Decimal::new(1, 2))
                {
                    report.status = if filled >= quantity || fill_or_kill {
                        OrderStatus::Filled
                    } else {
                        OrderStatus::PartiallyFilled
                    };
                    report.filled_quantity = filled;
                    report.average_price = value / filled;
                    report.fees_paid = value * self.config.fee_rate;
                    report.error_message = None;
                }
            }
        }
        report.transaction_hash = response.settlement_tx_hash();
        report
    }

    fn send_report(&self, report: ExecutionReport) {
        // Lossy telemetry only; callers receive the authoritative return value.
        let _ = self.report_tx.send(report);
    }
}

/// The most taker fee a buy of `quantity` shares at `price` can be charged
/// in USDC on top of its amount: the highest published rate (fee-free markets
/// are charged none), as the executor does not know each market's terms.
/// Every live buy keeps this much room; a multi-leg preflight must too.
pub fn taker_fee_allowance(quantity: Decimal, price: Decimal) -> Decimal {
    quantity * polymarket_core::types::FeeSchedule::CONSERVATIVE.fee_per_share(price)
}

/// The most USDC a resting buy of `remaining` shares may still spend: its
/// amount plus the most taker fee it can carry. A market buy without a
/// price is held at a dollar a share, which also covers its fee
/// (p + 0.07·p(1 − p) ≤ 1).
pub fn open_buy_cost(remaining: Decimal, price: Option<Decimal>) -> Decimal {
    let remaining = remaining.max(Decimal::ZERO);
    match price {
        Some(price) => remaining * price + taker_fee_allowance(remaining, price),
        None => remaining,
    }
}

impl Clone for ExecutionMetrics {
    fn clone(&self) -> Self {
        Self {
            orders_submitted: self.orders_submitted,
            orders_filled: self.orders_filled,
            orders_rejected: self.orders_rejected,
            total_volume: self.total_volume,
            total_fees: self.total_fees,
            avg_latency_us: self.avg_latency_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use polymarket_core::api::ClobClient;

    fn book(bids: &[(&str, &str)], asks: &[(&str, &str)]) -> OrderBook {
        let level = |(price, size): &(&str, &str)| polymarket_core::types::PriceLevel {
            price: price.parse().unwrap(),
            size: size.parse().unwrap(),
        };
        OrderBook {
            market_id: "m".into(),
            outcome_id: "t".into(),
            timestamp: chrono::Utc::now(),
            bids: bids.iter().map(level).collect(),
            asks: asks.iter().map(level).collect(),
        }
    }

    fn executor() -> OrderExecutor {
        OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig::default(),
        )
    }

    /// A CLOB serving one book: asks 0.50 x 1000, bids 0.49 x 1000.
    async fn paper_book_client() -> Arc<ClobClient> {
        use axum::{routing::get, Json, Router};
        let app = Router::new().route(
            "/book",
            get(|| async {
                Json(serde_json::json!({
                    "market": "m", "asset_id": "1",
                    "bids": [{"price": "0.49", "size": "1000"}],
                    "asks": [{"price": "0.50", "size": "1000"}],
                    "timestamp": "2026-01-01T00:00:00Z"
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Arc::new(ClobClient::new(Some(url), None))
    }

    #[tokio::test]
    async fn a_blank_allowance_is_refreshed_before_an_order_relies_on_it() {
        use axum::{extract::State, routing::get, Json, Router};
        use std::sync::atomic::AtomicUsize;

        // The exchange's cache answers a blank allowance until refreshed.
        let refreshes = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/balance-allowance",
                get(|State(refreshes): State<Arc<AtomicUsize>>| async move {
                    let allowance = if refreshes.load(Ordering::SeqCst) == 0 {
                        ""
                    } else {
                        "5000000"
                    };
                    Json(serde_json::json!({"balance": "5000000", "allowance": allowance}))
                }),
            )
            .route(
                "/balance-allowance/update",
                get(|State(refreshes): State<Arc<AtomicUsize>>| async move {
                    refreshes.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({}))
                }),
            )
            .with_state(refreshes.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let wallet = TradingWallet::from_private_key(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let client = AuthenticatedClobClient::with_credentials(
            ClobClient::new(Some(url), None),
            OrderSigner::new(wallet.into_signer()),
            polymarket_core::api::clob::ApiCredentials {
                api_key: "key".into(),
                api_secret: "c2VjcmV0".into(),
                api_passphrase: "pass".into(),
            },
        );
        OrderExecutor::ensure_live_order_capacity(
            &client,
            Uuid::new_v4(),
            OrderSide::Buy,
            "token",
            Decimal::ONE,
        )
        .await
        .unwrap();
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    }

    #[sqlx::test(migrations = false)]
    async fn a_market_buy_spends_its_own_amount_and_only_checks_the_withdrawal_reserve(
        pool: sqlx::PgPool,
    ) {
        use axum::{
            extract::State,
            routing::{get, post},
            Json, Router,
        };
        use sqlx::Executor;

        type Posted = Arc<std::sync::Mutex<Option<serde_json::Value>>>;
        // The exchange: a book with depth at 0.50, a wallet holding 50 USDC,
        // and an order endpoint that records what was posted.
        let posted: Posted = Arc::default();
        let app = Router::new()
            .route(
                "/book",
                get(|| async {
                    Json(serde_json::json!({
                        "market": "m", "asset_id": "1",
                        "bids": [{"price": "0.49", "size": "1000"}],
                        "asks": [{"price": "0.50", "size": "1000"}],
                        "timestamp": "2026-01-01T00:00:00Z"
                    }))
                }),
            )
            .route(
                "/balance-allowance",
                get(|| async {
                    Json(serde_json::json!({"balance": "50000000", "allowance": "50000000"}))
                }),
            )
            .route(
                "/balance-allowance/update",
                get(|| async { Json(serde_json::json!({})) }),
            )
            .route(
                "/neg-risk",
                get(|| async { Json(serde_json::json!({"neg_risk": false})) }),
            )
            .route(
                "/order",
                post(
                    |State(posted): State<Posted>, Json(body): Json<serde_json::Value>| async move {
                        *posted.lock().unwrap() = Some(body);
                        // A fill-or-kill order that cannot fill is killed.
                        (
                            axum::http::StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": "order couldn't be fully filled. FOK orders are fully filled or killed."
                            })),
                        )
                    },
                ),
            )
            .with_state(posted.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let wallet = TradingWallet::from_private_key(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let client = AuthenticatedClobClient::with_credentials(
            ClobClient::new(Some(url.clone()), None),
            OrderSigner::new(wallet.into_signer()),
            polymarket_core::api::clob::ApiCredentials {
                api_key: "key".into(),
                api_secret: "c2VjcmV0".into(),
                api_passphrase: "pass".into(),
            },
        );
        let address = client.address();
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(Some(url), None)),
            ExecutorConfig {
                live_trading: true,
                ..ExecutorConfig::default()
            },
        )
        .with_database(pool.clone());
        *executor.auth_client.write().await = Some(client);
        pool.execute(include_str!(
            "../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute(
            "CREATE TABLE position_order_links (id UUID, settled BOOLEAN); \
             CREATE TABLE orders (id UUID, status TEXT); \
             CREATE TABLE wallet_withdrawals (wallet_address TEXT, amount NUMERIC, status TEXT)",
        )
        .await
        .unwrap();

        // 10 shares at 0.50 spend 5 USDC.
        let buy = || MarketOrder::new("m".into(), "1".into(), OrderSide::Buy, Decimal::from(10));
        let maker_amount = || {
            posted
                .lock()
                .unwrap()
                .take()
                .map(|body| body["order"]["makerAmount"].as_str().unwrap().to_string())
        };
        let _ = executor.execute_live_market_order(&buy(), None).await;
        assert_eq!(maker_amount().as_deref(), Some("5000000"));

        // A 40 USDC withdrawal in flight: still 5 USDC spent (45 of 50 held).
        sqlx::query("INSERT INTO wallet_withdrawals VALUES ($1, 40, 'submitted')")
            .bind(&address)
            .execute(&pool)
            .await
            .unwrap();
        let _ = executor.execute_live_market_order(&buy(), None).await;
        assert_eq!(maker_amount().as_deref(), Some("5000000"));

        // 46 USDC in flight leaves less than 5: refused, nothing posted.
        sqlx::query("UPDATE wallet_withdrawals SET amount = 46")
            .execute(&pool)
            .await
            .unwrap();
        let refused = executor.execute_live_market_order(&buy(), None).await;
        assert!(refused.is_err() || !refused.unwrap().is_success());
        assert_eq!(maker_amount(), None);

        // 44.9 in flight leaves 5.1: enough for the 5 USDC amount but not for
        // the taker fee it may pay on top (up to 10 × 0.07 × 0.25 = 0.175).
        sqlx::query("UPDATE wallet_withdrawals SET amount = 44.9")
            .execute(&pool)
            .await
            .unwrap();
        let refused = executor.execute_live_market_order(&buy(), None).await;
        assert!(refused.is_err() || !refused.unwrap().is_success());
        assert_eq!(maker_amount(), None);
        // 44.8 leaves 5.2: amount and fee fit, and the amount signed is still 5.
        sqlx::query("UPDATE wallet_withdrawals SET amount = 44.8")
            .execute(&pool)
            .await
            .unwrap();
        let _ = executor.execute_live_market_order(&buy(), None).await;
        assert_eq!(maker_amount().as_deref(), Some("5000000"));

        // A pair's preflight also holds back what resting buys reserve: 44.8
        // in flight and 5 for the pair fit in 50; a resting 10 @ 0.50 does not.
        pool.execute(
            "ALTER TABLE orders ADD COLUMN execution_mode TEXT, ADD COLUMN side TEXT, \
             ADD COLUMN quantity NUMERIC, ADD COLUMN filled_quantity NUMERIC, \
             ADD COLUMN price NUMERIC, ADD COLUMN wallet_address TEXT",
        )
        .await
        .unwrap();
        assert!(executor
            .ensure_live_buying_power(Decimal::from(5))
            .await
            .is_ok());
        sqlx::query(
            "INSERT INTO orders VALUES (gen_random_uuid(), 'open', 'live', 'buy', 10, 0, 0.50, $1)",
        )
        .bind(&address)
        .execute(&pool)
        .await
        .unwrap();
        assert!(executor
            .ensure_live_buying_power(Decimal::from(5))
            .await
            .is_err());
    }

    #[sqlx::test(migrations = false)]
    async fn a_fill_or_kill_limit_order_accepted_without_amounts_stays_unresolved(
        pool: sqlx::PgPool,
    ) {
        use axum::{
            routing::{get, post},
            Json, Router,
        };
        use sqlx::Executor;

        // The exchange accepts every order as "delayed", without amounts.
        let app = Router::new()
            .route(
                "/balance-allowance",
                get(|| async {
                    Json(serde_json::json!({"balance": "50000000", "allowance": "50000000"}))
                }),
            )
            .route(
                "/balance-allowance/update",
                get(|| async { Json(serde_json::json!({})) }),
            )
            .route(
                "/neg-risk",
                get(|| async { Json(serde_json::json!({"neg_risk": false})) }),
            )
            .route(
                "/order",
                post(|Json(body): Json<serde_json::Value>| async move {
                    let hash = body["order"]["hash"].clone();
                    Json(serde_json::json!({
                        "success": true, "orderID": hash, "status": "delayed"
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let wallet = TradingWallet::from_private_key(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        let client = AuthenticatedClobClient::with_credentials(
            ClobClient::new(Some(url.clone()), None),
            OrderSigner::new(wallet.into_signer()),
            polymarket_core::api::clob::ApiCredentials {
                api_key: "key".into(),
                api_secret: "c2VjcmV0".into(),
                api_passphrase: "pass".into(),
            },
        );
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(Some(url), None)),
            ExecutorConfig {
                live_trading: true,
                ..ExecutorConfig::default()
            },
        )
        .with_database(pool.clone());
        *executor.auth_client.write().await = Some(client);
        pool.execute(include_str!(
            "../../../migrations/069_execution_intents.sql"
        ))
        .await
        .unwrap();
        pool.execute(
            "CREATE TABLE position_order_links (id UUID, settled BOOLEAN); \
             CREATE TABLE orders (id UUID, status TEXT); \
             CREATE TABLE wallet_withdrawals (wallet_address TEXT, amount NUMERIC, status TEXT)",
        )
        .await
        .unwrap();

        let status_of = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>("SELECT status FROM execution_intents WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let order = || {
            LimitOrder::new(
                "m".into(),
                "1".into(),
                OrderSide::Buy,
                Decimal::new(5, 1),
                Decimal::TEN,
            )
        };
        // Filled or killed, only its record can tell: it keeps fencing.
        let fok = order().fok();
        let report = executor.execute_live_limit_order(&fok, None).await.unwrap();
        assert_eq!(report.status, OrderStatus::Pending);
        assert_eq!(status_of(fok.id).await, "unknown");
        sqlx::query("UPDATE execution_intents SET status='reconciled'")
            .execute(&pool)
            .await
            .unwrap();
        // A resting order is acknowledged and followed as such.
        let gtc = order().gtc();
        let report = executor.execute_live_limit_order(&gtc, None).await.unwrap();
        assert_eq!(report.status, OrderStatus::Pending);
        assert_eq!(status_of(gtc.id).await, "acknowledged");
    }

    #[sqlx::test(migrations = false)]
    async fn buys_keep_back_usdc_that_unsettled_withdrawals_may_take(pool: sqlx::PgPool) {
        use sqlx::Executor;
        // Without a database there is nothing to keep back.
        assert_eq!(
            executor().collateral_withheld("0xabc").await.unwrap(),
            Decimal::ZERO
        );
        let executor = executor().with_database(pool.clone());
        // Fail closed: an unreadable journal is not "nothing in flight".
        assert!(executor.collateral_withheld("0xabc").await.is_err());

        pool.execute(
            "CREATE TABLE wallet_withdrawals (wallet_address TEXT, amount NUMERIC, status TEXT); \
             INSERT INTO wallet_withdrawals VALUES \
                ('0xABC', 5, 'pending'), ('0xabc', 7, 'submitting'), ('0xabc', 11, 'submitted'), \
                ('0xabc', 13, 'confirmed'), ('0xabc', 17, 'failed'), ('0xdef', 19, 'submitted')",
        )
        .await
        .unwrap();
        assert_eq!(
            executor.collateral_withheld("0xAbC").await.unwrap(),
            Decimal::from(23)
        );
    }

    fn sell(quantity: &str) -> MarketOrder {
        MarketOrder::new(
            "m".into(),
            "t".into(),
            OrderSide::Sell,
            quantity.parse().unwrap(),
        )
    }

    #[test]
    fn market_sell_walks_bids_to_cover_its_quantity() {
        let executor = executor();
        // Worst-first, small best level, no $100 best-level notional.
        let book = book(&[("0.40", "1000"), ("0.495", "50"), ("0.50", "60")], &[]);

        let priced = executor
            .price_market_order(&sell("100"), &book)
            .ok()
            .unwrap();
        assert_eq!(priced.limit, Decimal::new(495, 3));
        assert_eq!(priced.average, Decimal::new(498, 3));
    }

    #[test]
    fn market_sell_rejects_liquidity_beyond_slippage() {
        let executor = executor();
        let book = book(&[("0.40", "1000"), ("0.50", "60")], &[]);
        assert!(executor.price_market_order(&sell("100"), &book).is_err());

        // A wider tolerance reaches the deeper level.
        let wide = sell("100").with_slippage(Decimal::new(25, 2));
        let priced = executor.price_market_order(&wide, &book).ok().unwrap();
        assert_eq!(priced.limit, Decimal::new(40, 2));

        // The expected price bounds the floor too.
        let anchored = wide.with_expected_price(Decimal::new(60, 2));
        assert!(executor.price_market_order(&anchored, &book).is_err());
    }

    #[test]
    fn market_buy_keeps_best_level_depth_gates() {
        let executor = executor();
        let thin = book(&[], &[("0.99", "1000"), ("0.50", "10")]);
        let buy = MarketOrder::new("m".into(), "t".into(), OrderSide::Buy, Decimal::from(5));
        assert!(executor.price_market_order(&buy, &thin).is_err());

        let deep = book(&[], &[("0.99", "1000"), ("0.50", "1000")]);
        let priced = executor.price_market_order(&buy, &deep).ok().unwrap();
        assert_eq!(priced.limit, Decimal::new(50, 2));
    }

    #[test]
    fn accepted_limit_orders_need_confirmed_amounts_before_reporting_fills() {
        use polymarket_core::api::clob::PostOrderResponse;
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig::default(),
        );
        for status in ["live", "delayed", "matched"] {
            let response: PostOrderResponse = serde_json::from_value(
                serde_json::json!({"orderID":"exchange-id","status":status}),
            )
            .unwrap();
            let report = executor.report_from_exchange_response(
                Uuid::new_v4(),
                "market",
                "token",
                OrderSide::Buy,
                Decimal::TEN,
                None,
                false,
                &response,
            );
            assert_eq!(report.status, OrderStatus::Pending);
            assert_eq!(report.filled_quantity, Decimal::ZERO);
            assert_eq!(report.fees_paid, Decimal::ZERO);
            assert_eq!(report.exchange_order_id.as_deref(), Some("exchange-id"));
        }
        for (side, maker, taker, expected) in [
            (OrderSide::Buy, "2", "4", OrderStatus::PartiallyFilled),
            (OrderSide::Buy, "5", "10", OrderStatus::Filled),
            (OrderSide::Sell, "10", "5", OrderStatus::Filled),
        ] {
            let response: PostOrderResponse = serde_json::from_value(serde_json::json!({"orderID":"exchange-id","status":"matched","makingAmount":maker,"takingAmount":taker})).unwrap();
            let report = executor.report_from_exchange_response(
                Uuid::new_v4(),
                "market",
                "token",
                side,
                Decimal::TEN,
                None,
                false,
                &response,
            );
            assert_eq!(report.status, expected);
            assert_eq!(report.average_price, Decimal::new(5, 1));
        }
        // A fill-or-kill buy whose cent-rounded amount bought 9.99 of 10
        // shares filled in full; the same answer to a resting order did not.
        let response: PostOrderResponse = serde_json::from_value(serde_json::json!({"orderID":"exchange-id","status":"matched","makingAmount":"4.995","takingAmount":"9.99"})).unwrap();
        for (fill_or_kill, expected) in [
            (true, OrderStatus::Filled),
            (false, OrderStatus::PartiallyFilled),
        ] {
            let report = executor.report_from_exchange_response(
                Uuid::new_v4(),
                "market",
                "token",
                OrderSide::Buy,
                Decimal::TEN,
                None,
                fill_or_kill,
                &response,
            );
            assert_eq!(report.status, expected);
        }
        // A buy given a better price receives more shares than it asked for:
        // filled, with every share it received. A sell never fills beyond
        // what it signed.
        for (side, maker, taker, expected) in [
            (OrderSide::Buy, "5", "10.2", OrderStatus::Filled),
            (OrderSide::Sell, "10.2", "5", OrderStatus::Pending),
        ] {
            let response: PostOrderResponse = serde_json::from_value(serde_json::json!({"orderID":"exchange-id","status":"matched","makingAmount":maker,"takingAmount":taker})).unwrap();
            let report = executor.report_from_exchange_response(
                Uuid::new_v4(),
                "market",
                "token",
                side,
                Decimal::TEN,
                None,
                false,
                &response,
            );
            assert_eq!(report.status, expected);
            if expected == OrderStatus::Filled {
                assert_eq!(report.filled_quantity, "10.2".parse::<Decimal>().unwrap());
            }
        }
        // A buy signed for 5 USDC: an answer within it (a cent of rounding
        // included) fills; one spending more, as amounts read in the wrong
        // units would, stays pending.
        for (maker, taker, expected) in [
            ("5.01", "10", OrderStatus::Filled),
            ("5000000", "10000000", OrderStatus::Pending),
        ] {
            let response: PostOrderResponse = serde_json::from_value(serde_json::json!({"orderID":"exchange-id","status":"matched","makingAmount":maker,"takingAmount":taker})).unwrap();
            let report = executor.report_from_exchange_response(
                Uuid::new_v4(),
                "market",
                "token",
                OrderSide::Buy,
                Decimal::TEN,
                Some(Decimal::new(5, 0)),
                true,
                &response,
            );
            assert_eq!(report.status, expected);
        }
    }

    #[tokio::test]
    async fn fence_busy_is_a_definitive_rejection() {
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig {
                live_trading: true,
                ..ExecutorConfig::default()
            },
        );
        let report = executor
            .execute_with_retry(
                || async {
                    Err(anyhow::anyhow!(
                        "{SUBMISSION_FENCE_BUSY}: another live order is still being posted; nothing was sent"
                    ))
                },
                &Uuid::new_v4(),
                "market",
                "token",
                OrderSide::Sell,
                true,
            )
            .await;
        assert_eq!(report.status, OrderStatus::Rejected);
    }

    #[tokio::test]
    async fn live_ambiguity_is_pending_and_never_retried() {
        use std::sync::atomic::AtomicUsize;
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig {
                live_trading: true,
                timeout_ms: 5,
                ..ExecutorConfig::default()
            },
        );
        let attempts = AtomicUsize::new(0);
        let report = executor
            .execute_with_retry(
                || async {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!(
                        "{SUBMISSION_UNKNOWN}: network lost response"
                    ))
                },
                &Uuid::new_v4(),
                "market",
                "token",
                OrderSide::Buy,
                true,
            )
            .await;
        assert_eq!(report.status, OrderStatus::Pending);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let report = executor
            .execute_with_retry(
                std::future::pending,
                &Uuid::new_v4(),
                "market",
                "token",
                OrderSide::Buy,
                true,
            )
            .await;
        assert_eq!(report.status, OrderStatus::Pending);
    }

    #[tokio::test]
    async fn report_observers_never_block_execution_beyond_capacity() {
        // Orders over the size limit are reported without a request.
        let executor = OrderExecutor::new(
            Arc::new(ClobClient::new(None, None)),
            ExecutorConfig {
                max_order_size: Decimal::ONE,
                ..Default::default()
            },
        );
        for observe in [false, true] {
            let _undrained = observe.then(|| executor.subscribe_reports());
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                for _ in 0..2500 {
                    let report = executor
                        .execute_market_order(MarketOrder::new(
                            "market".into(),
                            "token".into(),
                            OrderSide::Buy,
                            Decimal::TWO,
                        ))
                        .await
                        .unwrap();
                    assert_eq!(report.status, OrderStatus::Rejected);
                }
            })
            .await
            .expect("report observers must not stall orders");
        }
    }

    #[tokio::test]
    async fn test_order_size_validation() {
        let clob_client = Arc::new(ClobClient::new(None, None));
        let config = ExecutorConfig {
            max_order_size: Decimal::new(100, 0),
            live_trading: false,
            ..Default::default()
        };
        let executor = OrderExecutor::new(clob_client, config);

        let order = MarketOrder::new(
            "market".to_string(),
            "token".to_string(),
            OrderSide::Buy,
            Decimal::new(1000, 0), // Exceeds max
        );

        let report = executor.execute_market_order(order).await.unwrap();
        assert_eq!(report.status, OrderStatus::Rejected);
        assert!(report.error_message.is_some());
    }

    #[test]
    fn test_is_retryable_error() {
        // Retryable errors
        assert!(is_retryable_error("connection refused"));
        assert!(is_retryable_error("Network timeout"));
        assert!(is_retryable_error("temporarily unavailable"));
        assert!(is_retryable_error("rate limit exceeded"));
        assert!(is_retryable_error("503 Service Unavailable"));
        assert!(is_retryable_error("502 Bad Gateway"));
        assert!(is_retryable_error("ETIMEDOUT"));
        assert!(is_retryable_error("ECONNRESET"));
        assert!(is_retryable_error(
            "refreshable_balance_allowance: Failed to post order: not enough balance / allowance"
        ));

        // Non-retryable errors
        assert!(!is_retryable_error("invalid order"));
        assert!(!is_retryable_error("insufficient funds"));
        assert!(!is_retryable_error("market closed"));
        assert!(!is_retryable_error("unauthorized"));
    }

    #[test]
    fn test_is_balance_allowance_error() {
        assert!(is_balance_allowance_error("not enough balance / allowance"));
        assert!(is_balance_allowance_error("insufficient allowance"));
        assert!(is_balance_allowance_error(
            "Balance and allowance are stale"
        ));
        assert!(!is_balance_allowance_error("invalid order"));
    }

    #[test]
    fn test_parse_decimal_value() {
        assert_eq!(parse_decimal_value("12.34"), Some(Decimal::new(1234, 2)));
        assert_eq!(parse_decimal_value(" 0 "), Some(Decimal::ZERO));
        assert_eq!(parse_decimal_value(""), None);
        assert_eq!(parse_decimal_value("abc"), None);
    }

    #[test]
    fn test_executor_config_defaults() {
        let config = ExecutorConfig::default();

        assert_eq!(config.timeout_ms, 30000);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_base_delay_ms, 100);
        assert_eq!(config.retry_max_delay_ms, 5000);
        assert!(!config.live_trading);
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        let config = ExecutorConfig {
            retry_base_delay_ms: 100,
            retry_max_delay_ms: 5000,
            ..Default::default()
        };

        // First retry: 100 * 2^0 = 100ms
        let delay1 = std::cmp::min(
            config.retry_base_delay_ms * (2_u64.pow(0)),
            config.retry_max_delay_ms,
        );
        assert_eq!(delay1, 100);

        // Second retry: 100 * 2^1 = 200ms
        let delay2 = std::cmp::min(
            config.retry_base_delay_ms * (2_u64.pow(1)),
            config.retry_max_delay_ms,
        );
        assert_eq!(delay2, 200);

        // Third retry: 100 * 2^2 = 400ms
        let delay3 = std::cmp::min(
            config.retry_base_delay_ms * (2_u64.pow(2)),
            config.retry_max_delay_ms,
        );
        assert_eq!(delay3, 400);

        // Large attempt should cap at max
        let delay_max = std::cmp::min(
            config.retry_base_delay_ms * (2_u64.pow(10)), // Would be 102400
            config.retry_max_delay_ms,
        );
        assert_eq!(delay_max, 5000);
    }

    #[test]
    fn test_backoff_caps_at_max_delay() {
        let config = ExecutorConfig {
            retry_base_delay_ms: 100,
            retry_max_delay_ms: 5000,
            ..Default::default()
        };

        // Very high attempt numbers should all cap at max_delay
        for attempt in [20, 30, 50, 63] {
            let delay = std::cmp::min(
                config
                    .retry_base_delay_ms
                    .saturating_mul(2_u64.saturating_pow(attempt)),
                config.retry_max_delay_ms,
            );
            assert_eq!(delay, 5000, "Attempt {} should cap at max_delay", attempt);
        }
    }

    #[test]
    fn test_zero_quantity_passes_size_validation() {
        // Zero quantity should pass the max_order_size check (0 <= max)
        let max_order_size = Decimal::new(100, 0);
        let zero_qty = Decimal::ZERO;
        assert!(
            zero_qty <= max_order_size,
            "Zero qty should pass size validation"
        );

        // Negative quantity should also pass the numeric check
        let neg_qty = Decimal::new(-1, 0);
        assert!(neg_qty <= max_order_size);
    }

    #[test]
    fn test_is_retryable_error_case_insensitive() {
        // All patterns should be matched case-insensitively
        assert!(is_retryable_error("TIMEOUT"));
        assert!(is_retryable_error("Connection Refused"));
        assert!(is_retryable_error("NETWORK ERROR"));
        assert!(is_retryable_error("Temporarily Unavailable"));
        assert!(is_retryable_error("Rate Limit Exceeded"));
        assert!(is_retryable_error("ECONNREFUSED"));

        // Non-retryable remain non-retryable
        assert!(!is_retryable_error("INVALID ORDER"));
        assert!(!is_retryable_error("Insufficient Funds"));
    }

    #[tokio::test]
    async fn an_order_trades_in_its_positions_mode_whatever_the_switch() {
        let clob_client = paper_book_client().await;
        let config = ExecutorConfig {
            live_trading: true,
            ..Default::default()
        };
        let executor = OrderExecutor::new(clob_client, config);
        let order = || {
            LimitOrder::new(
                "market".to_string(),
                "token".to_string(),
                OrderSide::Sell,
                Decimal::new(49, 2),
                Decimal::new(10, 0),
            )
        };
        // A paper position still exits (simulated) once live trading is on.
        let report = executor
            .execute_reserved_limit_order(order(), "paper", None)
            .await
            .unwrap();
        assert_eq!(report.status, OrderStatus::Filled);
        // A live order needs the signer, whatever the switch says.
        assert!(executor
            .execute_reserved_limit_order(order(), "live", Some("0xabc"))
            .await
            .is_err());
        executor.set_live_mode(false);
        assert!(executor
            .execute_reserved_limit_order(order(), "live", Some("0xabc"))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_paper_trading_limit_order_simulates() {
        let config = ExecutorConfig {
            live_trading: false,
            fee_rate: Decimal::new(2, 2), // 2%
            ..Default::default()
        };
        let executor = OrderExecutor::new(paper_book_client().await, config);
        let limit = |side, price: i64, quantity: i64| {
            LimitOrder::new(
                "market".to_string(),
                "token".to_string(),
                side,
                Decimal::new(price, 2),
                Decimal::new(quantity, 0),
            )
        };

        // A buy that crosses the 0.50 ask fills there, not at its limit.
        let report = executor
            .execute_limit_order(limit(OrderSide::Buy, 60, 10))
            .await
            .unwrap();
        assert_eq!(report.status, OrderStatus::Filled);
        assert_eq!(report.average_price, Decimal::new(50, 2));
        assert_eq!(report.filled_quantity, Decimal::new(10, 0));
        // Fees: 10 * 0.50 * 0.02 = 0.10
        assert_eq!(report.fees_paid, Decimal::new(10, 2));

        // A sell at or under the 0.49 bid fills at the bid.
        let report = executor
            .execute_limit_order(limit(OrderSide::Sell, 40, 10))
            .await
            .unwrap();
        assert_eq!(report.status, OrderStatus::Filled);
        assert_eq!(report.average_price, Decimal::new(49, 2));

        // Orders that would rest, or that the book cannot fill whole, are
        // refused rather than filled at a price no one offered.
        for order in [
            limit(OrderSide::Buy, 40, 10),
            limit(OrderSide::Sell, 99, 10),
            limit(OrderSide::Buy, 50, 2000),
        ] {
            let report = executor.execute_limit_order(order).await.unwrap();
            assert_eq!(report.status, OrderStatus::Rejected);
        }
    }
}
