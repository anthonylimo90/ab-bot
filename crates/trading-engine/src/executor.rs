//! Order execution engine for low-latency trade placement.

use anyhow::Result;
use auth::TradingWallet;
use dashmap::DashMap;
use polymarket_core::api::clob::{ApiCredentials, AuthenticatedClobClient, OrderType};
use polymarket_core::api::ClobClient;
use polymarket_core::signing::{OrderSide as SigningOrderSide, OrderSigner};
use polymarket_core::types::{
    ExecutionReport, LimitOrder, MarketOrder, OrderSide, OrderStatus,
};
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
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            default_slippage: Decimal::new(1, 2), // 1%
            max_order_size: Decimal::new(10000, 0),
            fee_rate: Decimal::new(2, 2), // 2%
            live_trading: false,
            api_key: None,
        }
    }
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
    /// Authenticated client for live trading (optional).
    auth_client: Option<Arc<RwLock<AuthenticatedClobClient>>>,
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
            auth_client: None,
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

        // Create the order signer from the wallet
        let signer = OrderSigner::new(wallet.into_signer());

        // Create authenticated client
        let client = ClobClient::new(None, None);
        let auth_client = AuthenticatedClobClient::new(client, signer);

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
            auth_client: Some(Arc::new(RwLock::new(auth_client))),
        }
    }

    /// Initialize the authenticated client by deriving API credentials.
    ///
    /// This must be called before executing live orders.
    pub async fn initialize_live_trading(&self) -> Result<()> {
        let auth_client = self.auth_client.as_ref().ok_or_else(|| {
            anyhow::anyhow!("No authenticated client - use new_with_wallet() for live trading")
        })?;

        let mut client = auth_client.write().await;
        client.derive_api_key().await.map_err(|e| {
            anyhow::anyhow!("Failed to derive API credentials: {}", e)
        })?;

        info!("Live trading initialized - API credentials derived");
        Ok(())
    }

    /// Check if live trading is initialized and ready.
    pub async fn is_live_ready(&self) -> bool {
        if !self.config.live_trading {
            return false;
        }
        if let Some(auth_client) = &self.auth_client {
            let client = auth_client.read().await;
            client.has_credentials()
        } else {
            false
        }
    }

    /// Get the trading wallet address (if available).
    pub async fn wallet_address(&self) -> Option<String> {
        if let Some(auth_client) = &self.auth_client {
            let client = auth_client.read().await;
            Some(client.address())
        } else {
            None
        }
    }

    /// Take the execution report receiver (can only be called once).
    pub fn take_report_receiver(&mut self) -> Option<mpsc::Receiver<ExecutionReport>> {
        self.report_rx.take()
    }

    /// Execute a market order.
    pub async fn execute_market_order(&self, order: MarketOrder) -> Result<ExecutionReport> {
        let start = std::time::Instant::now();

        // Validate order
        if order.quantity > self.config.max_order_size {
            let report = ExecutionReport::rejected(
                order.id,
                order.market_id.clone(),
                order.outcome_id.clone(),
                order.side,
                format!("Order size {} exceeds maximum {}", order.quantity, self.config.max_order_size),
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

        let report = if self.config.live_trading {
            self.execute_live_market_order(&order).await?
        } else {
            self.simulate_market_order(&order).await?
        };

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
            metrics.avg_latency_us =
                (metrics.avg_latency_us * (metrics.orders_submitted - 1) + latency_us)
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

    /// Execute a limit order.
    pub async fn execute_limit_order(&self, order: LimitOrder) -> Result<ExecutionReport> {
        // Validate order
        if order.quantity > self.config.max_order_size {
            let report = ExecutionReport::rejected(
                order.id,
                order.market_id.clone(),
                order.outcome_id.clone(),
                order.side,
                format!("Order size {} exceeds maximum {}", order.quantity, self.config.max_order_size),
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

        let report = if self.config.live_trading {
            self.execute_live_limit_order(&order).await?
        } else {
            self.simulate_limit_order(&order).await?
        };

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
        let auth_client = match &self.auth_client {
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

        // Create and sign the order
        let client = auth_client.read().await;

        if !client.has_credentials() {
            return Err(anyhow::anyhow!(
                "Live trading not initialized - call initialize_live_trading() first"
            ));
        }

        // Create signed order (1 hour expiration for market orders)
        let signed_order = client
            .create_order(
                &order.outcome_id,
                signing_side,
                price,
                order.quantity,
                3600, // 1 hour expiration
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
        let auth_client = match &self.auth_client {
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

        let client = auth_client.read().await;

        if !client.has_credentials() {
            return Err(anyhow::anyhow!(
                "Live trading not initialized - call initialize_live_trading() first"
            ));
        }

        // Create signed order (use configured expiration or default 24 hours)
        let expiration_secs = 24 * 3600; // 24 hours for limit orders
        let signed_order = client
            .create_order(
                &order.outcome_id,
                signing_side,
                order.price,
                order.quantity,
                expiration_secs,
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
}
