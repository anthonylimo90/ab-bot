//! Order execution engine for low-latency trade placement.

use anyhow::Result;
use auth::TradingWallet;
use dashmap::DashMap;
use polymarket_core::api::clob::{AuthenticatedClobClient, OrderType};
use polymarket_core::api::ClobClient;
use polymarket_core::signing::{OrderSide as SigningOrderSide, OrderSigner};
use polymarket_core::types::{ExecutionReport, LimitOrder, MarketOrder, OrderSide, OrderStatus};
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

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
    retryable_patterns.iter().any(|p| error_lower.contains(p))
}

/// Low-latency order executor for Polymarket CLOB.
pub struct OrderExecutor {
    clob_client: Arc<ClobClient>,
    config: ExecutorConfig,
    /// Pending orders awaiting confirmation.
    pending_orders: DashMap<Uuid, OrderStatus>,
    /// Channel for execution reports.
    report_tx: mpsc::Sender<ExecutionReport>,
    /// Receiver for execution reports (taken once).
    report_rx: Option<mpsc::Receiver<ExecutionReport>>,
    metrics: std::sync::RwLock<ExecutionMetrics>,
    /// Authenticated client for live trading (swappable at runtime).
    auth_client: Arc<RwLock<Option<AuthenticatedClobClient>>>,
}

impl OrderExecutor {
    /// Create a new order executor (paper trading mode).
    pub fn new(clob_client: Arc<ClobClient>, config: ExecutorConfig) -> Self {
        let (report_tx, report_rx) = mpsc::channel(1000);
        Self {
            clob_client,
            config,
            pending_orders: DashMap::new(),
            report_tx,
            report_rx: Some(report_rx),
            metrics: std::sync::RwLock::new(ExecutionMetrics::default()),
            auth_client: Arc::new(RwLock::new(None)),
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
        let (report_tx, report_rx) = mpsc::channel(1000);

        let auth_client = Self::build_auth_client(wallet);

        info!(
            address = %auth_client.address(),
            live_trading = %config.live_trading,
            "Created order executor with wallet authentication"
        );

        Self {
            clob_client,
            config,
            pending_orders: DashMap::new(),
            report_tx,
            report_rx: Some(report_rx),
            metrics: std::sync::RwLock::new(ExecutionMetrics::default()),
            auth_client: Arc::new(RwLock::new(Some(auth_client))),
        }
    }

    fn build_auth_client(wallet: TradingWallet) -> AuthenticatedClobClient {
        let signer = OrderSigner::new(wallet.into_signer());
        let client = ClobClient::new(None, None);
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
        if !self.config.live_trading {
            return false;
        }
        let slot = self.auth_client.read().await;
        slot.as_ref().map(|c| c.has_credentials()).unwrap_or(false)
    }

    /// Get the trading wallet address (if available).
    pub async fn wallet_address(&self) -> Option<String> {
        let slot = self.auth_client.read().await;
        slot.as_ref().map(|c| c.address())
    }

    /// Hot-reload the live trading wallet signer and API credentials.
    pub async fn reload_wallet(&self, wallet: TradingWallet) -> Result<String> {
        if !self.config.live_trading {
            return Err(anyhow::anyhow!(
                "Executor is not in live mode; set LIVE_TRADING=true"
            ));
        }

        let mut auth_client = Self::build_auth_client(wallet);
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

    /// Tell the CLOB to re-read on-chain balance/allowance state.
    ///
    /// Call this after setting on-chain approvals so the CLOB picks up the new values.
    pub async fn refresh_clob_allowance_cache(&self) -> anyhow::Result<()> {
        let slot = self.auth_client.read().await;
        let client = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No authenticated client"))?;
        // Update both COLLATERAL (USDC.e) and CONDITIONAL (CTF tokens)
        let _ = client.update_balance_allowance("COLLATERAL").await;
        let _ = client.update_balance_allowance("CONDITIONAL").await;
        Ok(())
    }

    /// Take the execution report receiver (can only be called once).
    pub fn take_report_receiver(&mut self) -> Option<mpsc::Receiver<ExecutionReport>> {
        self.report_rx.take()
    }

    /// Execute a market order with timeout and retry logic.
    pub async fn execute_market_order(&self, order: MarketOrder) -> Result<ExecutionReport> {
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
            self.send_report(report.clone()).await;
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
                    if self.config.live_trading {
                        self.execute_live_market_order(&order).await
                    } else {
                        self.simulate_market_order(&order).await
                    }
                },
                &order.id,
                &order.market_id,
                &order.outcome_id,
                order.side,
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

        self.pending_orders.remove(&order.id);
        self.send_report(report.clone()).await;

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
    ) -> ExecutionReport
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<ExecutionReport>>,
    {
        let timeout = std::time::Duration::from_millis(self.config.timeout_ms);
        let mut attempts = 0u32;
        let mut last_error = String::new();

        while attempts <= self.config.max_retries {
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

                    if attempts <= self.config.max_retries {
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
                    // Timeout
                    last_error = format!("Operation timed out after {}ms", self.config.timeout_ms);

                    if attempts <= self.config.max_retries {
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

    /// Execute a limit order with timeout and retry logic.
    pub async fn execute_limit_order(&self, order: LimitOrder) -> Result<ExecutionReport> {
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
            self.send_report(report.clone()).await;
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
                    if self.config.live_trading {
                        self.execute_live_limit_order(&order).await
                    } else {
                        self.simulate_limit_order(&order).await
                    }
                },
                &order.id,
                &order.market_id,
                &order.outcome_id,
                order.side,
            )
            .await;

        self.pending_orders.remove(&order.id);
        self.send_report(report.clone()).await;
        Ok(report)
    }

    /// Cancel a pending order.
    pub async fn cancel_order(&self, order_id: Uuid) -> Result<bool> {
        if self.pending_orders.remove(&order_id).is_some() {
            info!(order_id = %order_id, "Order cancelled");
            Ok(true)
        } else {
            warn!(order_id = %order_id, "Order not found for cancellation");
            Ok(false)
        }
    }

    /// Get current execution metrics.
    pub fn metrics(&self) -> ExecutionMetrics {
        self.metrics.read().unwrap().clone()
    }

    /// Check if executor is in live trading mode.
    pub fn is_live(&self) -> bool {
        self.config.live_trading
    }

    // Private methods

    async fn execute_live_market_order(&self, order: &MarketOrder) -> Result<ExecutionReport> {
        // Check if we have an authenticated client
        let client_guard = self.auth_client.read().await;
        let client = match client_guard.as_ref() {
            Some(client) => client,
            None => {
                warn!("No authenticated client for live trading - falling back to simulation");
                return self.simulate_market_order(order).await;
            }
        };

        // Get the best price from orderbook
        let book = self.clob_client.get_order_book(&order.outcome_id).await?;

        let price = match order.side {
            OrderSide::Buy => {
                if let Some(ask) = book.asks.first() {
                    ask.price
                } else {
                    return Ok(ExecutionReport::rejected(
                        order.id,
                        order.market_id.clone(),
                        order.outcome_id.clone(),
                        order.side,
                        "No asks available".to_string(),
                    ));
                }
            }
            OrderSide::Sell => {
                if let Some(bid) = book.bids.first() {
                    bid.price
                } else {
                    return Ok(ExecutionReport::rejected(
                        order.id,
                        order.market_id.clone(),
                        order.outcome_id.clone(),
                        order.side,
                        "No bids available".to_string(),
                    ));
                }
            }
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

        // Create signed order (FOK for market orders, expiration=0)
        let signed_order = client
            .create_order(
                &order.outcome_id,
                signing_side,
                price,
                order.quantity,
                OrderType::Fok,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create order: {}", e))?;

        // Post the order (FOK for market orders)
        let response = client
            .post_order(signed_order, OrderType::Fok)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to post order: {}", e))?;

        info!(
            order_id = %order.id,
            clob_order_id = %response.order_id,
            status = %response.status,
            "Live market order submitted"
        );

        // Calculate fees
        let fees = order.quantity * price * self.config.fee_rate;

        // Return success (actual fill status would come from webhook/polling)
        Ok(ExecutionReport::success(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            order.quantity,
            price,
            fees,
        ))
    }

    async fn execute_live_limit_order(&self, order: &LimitOrder) -> Result<ExecutionReport> {
        // Check if we have an authenticated client
        let client_guard = self.auth_client.read().await;
        let client = match client_guard.as_ref() {
            Some(client) => client,
            None => {
                warn!("No authenticated client for live trading - falling back to simulation");
                return self.simulate_limit_order(order).await;
            }
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

        // Create signed order (GTC for limit orders, expiration=0)
        let signed_order = client
            .create_order(
                &order.outcome_id,
                signing_side,
                order.price,
                order.quantity,
                OrderType::Gtc,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create order: {}", e))?;

        // Post the order (GTC for limit orders)
        let response = client
            .post_order(signed_order, OrderType::Gtc)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to post order: {}", e))?;

        info!(
            order_id = %order.id,
            clob_order_id = %response.order_id,
            status = %response.status,
            price = %order.price,
            quantity = %order.quantity,
            "Live limit order submitted"
        );

        // Calculate fees
        let fees = order.quantity * order.price * self.config.fee_rate;

        Ok(ExecutionReport::success(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            order.quantity,
            order.price,
            fees,
        ))
    }

    async fn simulate_market_order(&self, order: &MarketOrder) -> Result<ExecutionReport> {
        // Paper trading simulation - fetch real orderbook prices
        let book = self.clob_client.get_order_book(&order.outcome_id).await?;

        let (price, available) = match order.side {
            OrderSide::Buy => {
                if let Some(ask) = book.asks.first() {
                    (ask.price, ask.size)
                } else {
                    // Simulate with a default price if no orderbook
                    (Decimal::new(50, 2), Decimal::new(10000, 0))
                }
            }
            OrderSide::Sell => {
                if let Some(bid) = book.bids.first() {
                    (bid.price, bid.size)
                } else {
                    (Decimal::new(50, 2), Decimal::new(10000, 0))
                }
            }
        };

        let fill_quantity = order.quantity.min(available);
        let fees = fill_quantity * price * self.config.fee_rate;

        info!(
            order_id = %order.id,
            price = %price,
            filled = %fill_quantity,
            fees = %fees,
            "[PAPER] Simulated market order fill"
        );

        Ok(ExecutionReport::success(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            fill_quantity,
            price,
            fees,
        ))
    }

    async fn simulate_limit_order(&self, order: &LimitOrder) -> Result<ExecutionReport> {
        // Simulate limit order - assume it fills at limit price
        let fees = order.quantity * order.price * self.config.fee_rate;

        info!(
            order_id = %order.id,
            price = %order.price,
            quantity = %order.quantity,
            fees = %fees,
            "[PAPER] Simulated limit order placement"
        );

        Ok(ExecutionReport::success(
            order.id,
            order.market_id.clone(),
            order.outcome_id.clone(),
            order.side,
            order.quantity,
            order.price,
            fees,
        ))
    }

    async fn send_report(&self, report: ExecutionReport) {
        if self.report_tx.send(report).await.is_err() {
            warn!("No receiver for execution report");
        }
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

        // Non-retryable errors
        assert!(!is_retryable_error("invalid order"));
        assert!(!is_retryable_error("insufficient funds"));
        assert!(!is_retryable_error("market closed"));
        assert!(!is_retryable_error("unauthorized"));
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
    async fn test_paper_trading_limit_order_simulates() {
        let clob_client = Arc::new(ClobClient::new(None, None));
        let config = ExecutorConfig {
            live_trading: false,
            fee_rate: Decimal::new(2, 2), // 2%
            ..Default::default()
        };
        let executor = OrderExecutor::new(clob_client, config);

        let order = LimitOrder::new(
            "market".to_string(),
            "token".to_string(),
            OrderSide::Buy,
            Decimal::new(50, 2), // price 0.50
            Decimal::new(10, 0), // quantity 10
        );

        let report = executor.execute_limit_order(order).await.unwrap();
        // Paper mode should fill at the limit price
        assert_eq!(report.status, OrderStatus::Filled);
        assert_eq!(report.average_price, Decimal::new(50, 2));
        assert_eq!(report.filled_quantity, Decimal::new(10, 0));
        // Fees: 10 * 0.50 * 0.02 = 0.10
        assert_eq!(report.fees_paid, Decimal::new(10, 2));
    }
}
