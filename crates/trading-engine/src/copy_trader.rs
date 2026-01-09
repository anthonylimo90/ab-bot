//! Copy trading system for mirroring successful wallet strategies.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use polymarket_core::types::{ExecutionReport, MarketOrder, OrderSide};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::OrderExecutor;

/// Configuration for a tracked wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedWallet {
    pub id: Uuid,
    pub address: String,
    pub alias: Option<String>,
    /// Percentage of capital allocated to this wallet's strategy (0-100).
    pub allocation_pct: Decimal,
    /// Delay in milliseconds before copying a trade.
    pub copy_delay_ms: u64,
    /// Maximum position size to copy per trade.
    pub max_position_size: Decimal,
    /// Whether to actively copy this wallet's trades.
    pub enabled: bool,
    /// Timestamp when tracking started.
    pub added_at: DateTime<Utc>,
    /// Last trade copied from this wallet.
    pub last_copied_trade: Option<DateTime<Utc>>,
    /// Total value copied from this wallet.
    pub total_copied_value: Decimal,
    /// P&L from copied trades.
    pub total_pnl: Decimal,
}

impl TrackedWallet {
    pub fn new(address: String, allocation_pct: Decimal) -> Self {
        Self {
            id: Uuid::new_v4(),
            address,
            alias: None,
            allocation_pct,
            copy_delay_ms: 0,
            max_position_size: Decimal::new(1000, 0),
            enabled: true,
            added_at: Utc::now(),
            last_copied_trade: None,
            total_copied_value: Decimal::ZERO,
            total_pnl: Decimal::ZERO,
        }
    }

    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.alias = Some(alias.into());
        self
    }

    pub fn with_delay(mut self, delay_ms: u64) -> Self {
        self.copy_delay_ms = delay_ms;
        self
    }

    pub fn with_max_size(mut self, max_size: Decimal) -> Self {
        self.max_position_size = max_size;
        self
    }
}

/// Strategy for allocating capital across tracked wallets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllocationStrategy {
    /// Split equally among all enabled tracked wallets.
    EqualWeight,
    /// Weight by configured allocation percentages.
    ConfiguredWeight,
    /// Weight by historical ROI performance.
    PerformanceWeighted,
    /// Weight by risk-adjusted returns (Sharpe-like).
    RiskAdjusted,
}

/// A trade detected from a tracked wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedTrade {
    pub wallet_address: String,
    pub market_id: String,
    pub outcome_id: String,
    pub side: OrderSide,
    pub price: Decimal,
    pub quantity: Decimal,
    pub timestamp: DateTime<Utc>,
    pub tx_hash: String,
}

/// Copy trading engine that mirrors trades from successful wallets.
pub struct CopyTrader {
    /// Wallets being tracked, keyed by address.
    tracked_wallets: DashMap<String, TrackedWallet>,
    /// Order executor for placing copy trades.
    executor: Arc<OrderExecutor>,
    /// Total capital available for copy trading.
    total_capital: Decimal,
    /// Allocation strategy.
    allocation_strategy: AllocationStrategy,
    /// Channel for detected trades to process.
    trade_rx: Option<mpsc::Receiver<DetectedTrade>>,
    /// Channel sender for incoming trades.
    trade_tx: mpsc::Sender<DetectedTrade>,
    /// Whether copy trading is active.
    active: bool,
}

impl CopyTrader {
    /// Create a new copy trader.
    pub fn new(executor: Arc<OrderExecutor>, total_capital: Decimal) -> Self {
        let (trade_tx, trade_rx) = mpsc::channel(1000);
        Self {
            tracked_wallets: DashMap::new(),
            executor,
            total_capital,
            allocation_strategy: AllocationStrategy::ConfiguredWeight,
            trade_rx: Some(trade_rx),
            trade_tx,
            active: true,
        }
    }

    /// Set the allocation strategy.
    pub fn with_strategy(mut self, strategy: AllocationStrategy) -> Self {
        self.allocation_strategy = strategy;
        self
    }

    /// Add a wallet to track.
    pub fn add_tracked_wallet(&self, wallet: TrackedWallet) {
        info!(
            address = %wallet.address,
            alias = ?wallet.alias,
            allocation = %wallet.allocation_pct,
            "Adding tracked wallet"
        );
        self.tracked_wallets.insert(wallet.address.to_lowercase(), wallet);
    }

    /// Remove a wallet from tracking.
    pub fn remove_tracked_wallet(&self, address: &str) -> Option<TrackedWallet> {
        self.tracked_wallets.remove(&address.to_lowercase()).map(|(_, w)| w)
    }

    /// Get a tracked wallet by address.
    pub fn get_tracked_wallet(&self, address: &str) -> Option<TrackedWallet> {
        self.tracked_wallets.get(&address.to_lowercase()).map(|w| w.clone())
    }

    /// Get all tracked wallets.
    pub fn list_tracked_wallets(&self) -> Vec<TrackedWallet> {
        self.tracked_wallets.iter().map(|e| e.value().clone()).collect()
    }

    /// Get only enabled tracked wallets.
    pub fn enabled_wallets(&self) -> Vec<TrackedWallet> {
        self.tracked_wallets
            .iter()
            .filter(|e| e.value().enabled)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Update total capital available for copy trading.
    pub fn update_capital(&mut self, capital: Decimal) {
        self.total_capital = capital;
    }

    /// Enable or disable a tracked wallet.
    pub fn set_wallet_enabled(&self, address: &str, enabled: bool) -> bool {
        if let Some(mut wallet) = self.tracked_wallets.get_mut(&address.to_lowercase()) {
            wallet.enabled = enabled;
            info!(address = %address, enabled = %enabled, "Updated wallet status");
            true
        } else {
            false
        }
    }

    /// Get the trade sender for submitting detected trades.
    pub fn trade_sender(&self) -> mpsc::Sender<DetectedTrade> {
        self.trade_tx.clone()
    }

    /// Take the trade receiver (can only be called once).
    pub fn take_trade_receiver(&mut self) -> Option<mpsc::Receiver<DetectedTrade>> {
        self.trade_rx.take()
    }

    /// Process a detected trade and generate copy order.
    pub async fn process_detected_trade(&self, trade: &DetectedTrade) -> Result<Option<ExecutionReport>> {
        if !self.active {
            debug!("Copy trading is paused, skipping trade");
            return Ok(None);
        }

        let wallet = match self.tracked_wallets.get(&trade.wallet_address.to_lowercase()) {
            Some(w) if w.enabled => w.clone(),
            Some(_) => {
                debug!(
                    wallet = %trade.wallet_address,
                    "Wallet is disabled, skipping trade"
                );
                return Ok(None);
            }
            None => {
                debug!(
                    wallet = %trade.wallet_address,
                    "Wallet not tracked, skipping trade"
                );
                return Ok(None);
            }
        };

        // Apply copy delay if configured
        if wallet.copy_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(wallet.copy_delay_ms)).await;
        }

        // Calculate position size based on allocation
        let allocated_capital = self.calculate_allocated_capital(&wallet);
        let copy_quantity = self.calculate_copy_quantity(&trade, &wallet, allocated_capital);

        if copy_quantity <= Decimal::ZERO {
            warn!(
                wallet = %trade.wallet_address,
                "Calculated copy quantity is zero, skipping"
            );
            return Ok(None);
        }

        info!(
            wallet = %trade.wallet_address,
            market = %trade.market_id,
            side = ?trade.side,
            original_qty = %trade.quantity,
            copy_qty = %copy_quantity,
            "Copying trade"
        );

        // Create and execute the copy order
        let order = MarketOrder::new(
            trade.market_id.clone(),
            trade.outcome_id.clone(),
            trade.side,
            copy_quantity,
        );

        let report = self.executor.execute_market_order(order).await?;

        // Update wallet stats
        if report.is_success() {
            if let Some(mut wallet) = self.tracked_wallets.get_mut(&trade.wallet_address.to_lowercase()) {
                wallet.last_copied_trade = Some(Utc::now());
                wallet.total_copied_value += report.total_value();
            }
        }

        Ok(Some(report))
    }

    /// Calculate capital allocated to a specific wallet.
    fn calculate_allocated_capital(&self, wallet: &TrackedWallet) -> Decimal {
        match self.allocation_strategy {
            AllocationStrategy::EqualWeight => {
                let enabled_count = self.enabled_wallets().len();
                if enabled_count == 0 {
                    Decimal::ZERO
                } else {
                    self.total_capital / Decimal::from(enabled_count)
                }
            }
            AllocationStrategy::ConfiguredWeight => {
                self.total_capital * wallet.allocation_pct / Decimal::new(100, 0)
            }
            AllocationStrategy::PerformanceWeighted => {
                // Weight by historical ROI
                let wallets = self.enabled_wallets();
                let total_pnl: Decimal = wallets.iter().map(|w| w.total_pnl.max(Decimal::ONE)).sum();
                if total_pnl <= Decimal::ZERO {
                    self.total_capital / Decimal::from(wallets.len().max(1))
                } else {
                    let wallet_weight = wallet.total_pnl.max(Decimal::ONE) / total_pnl;
                    self.total_capital * wallet_weight
                }
            }
            AllocationStrategy::RiskAdjusted => {
                // For now, same as configured weight
                // TODO: Implement Sharpe-ratio based weighting
                self.total_capital * wallet.allocation_pct / Decimal::new(100, 0)
            }
        }
    }

    /// Calculate quantity to copy based on original trade and allocation.
    fn calculate_copy_quantity(
        &self,
        trade: &DetectedTrade,
        wallet: &TrackedWallet,
        allocated_capital: Decimal,
    ) -> Decimal {
        // Calculate max affordable quantity based on allocated capital
        let max_affordable = if trade.price > Decimal::ZERO {
            allocated_capital / trade.price
        } else {
            Decimal::ZERO
        };

        // Apply limits
        trade.quantity
            .min(wallet.max_position_size)
            .min(max_affordable)
    }

    /// Pause copy trading.
    pub fn pause(&mut self) {
        self.active = false;
        info!("Copy trading paused");
    }

    /// Resume copy trading.
    pub fn resume(&mut self) {
        self.active = true;
        info!("Copy trading resumed");
    }

    /// Check if copy trading is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get summary statistics.
    pub fn stats(&self) -> CopyTradingStats {
        let wallets = self.list_tracked_wallets();
        let enabled_wallets = wallets.iter().filter(|w| w.enabled).count();
        let total_copied: Decimal = wallets.iter().map(|w| w.total_copied_value).sum();
        let total_pnl: Decimal = wallets.iter().map(|w| w.total_pnl).sum();

        CopyTradingStats {
            total_tracked_wallets: wallets.len(),
            enabled_wallets,
            total_capital: self.total_capital,
            total_copied_value: total_copied,
            total_pnl,
            is_active: self.active,
        }
    }
}

/// Summary statistics for copy trading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyTradingStats {
    pub total_tracked_wallets: usize,
    pub enabled_wallets: usize,
    pub total_capital: Decimal,
    pub total_copied_value: Decimal,
    pub total_pnl: Decimal,
    pub is_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use polymarket_core::api::ClobClient;
    use crate::executor::ExecutorConfig;

    fn create_test_executor() -> Arc<OrderExecutor> {
        let clob_client = Arc::new(ClobClient::new(None, None));
        let config = ExecutorConfig {
            live_trading: false,
            ..Default::default()
        };
        Arc::new(OrderExecutor::new(clob_client, config))
    }

    #[test]
    fn test_tracked_wallet_creation() {
        let wallet = TrackedWallet::new(
            "0x1234".to_string(),
            Decimal::new(20, 0),
        )
        .with_alias("Top Trader")
        .with_delay(100);

        assert_eq!(wallet.allocation_pct, Decimal::new(20, 0));
        assert_eq!(wallet.alias, Some("Top Trader".to_string()));
        assert_eq!(wallet.copy_delay_ms, 100);
        assert!(wallet.enabled);
    }

    #[test]
    fn test_add_and_list_wallets() {
        let executor = create_test_executor();
        let copy_trader = CopyTrader::new(executor, Decimal::new(10000, 0));

        copy_trader.add_tracked_wallet(TrackedWallet::new("0xAAA".to_string(), Decimal::new(50, 0)));
        copy_trader.add_tracked_wallet(TrackedWallet::new("0xBBB".to_string(), Decimal::new(50, 0)));

        let wallets = copy_trader.list_tracked_wallets();
        assert_eq!(wallets.len(), 2);
    }

    #[test]
    fn test_allocation_equal_weight() {
        let executor = create_test_executor();
        let copy_trader = CopyTrader::new(executor, Decimal::new(10000, 0))
            .with_strategy(AllocationStrategy::EqualWeight);

        copy_trader.add_tracked_wallet(TrackedWallet::new("0xAAA".to_string(), Decimal::new(0, 0)));
        copy_trader.add_tracked_wallet(TrackedWallet::new("0xBBB".to_string(), Decimal::new(0, 0)));

        let wallet = copy_trader.get_tracked_wallet("0xAAA").unwrap();
        let allocated = copy_trader.calculate_allocated_capital(&wallet);
        assert_eq!(allocated, Decimal::new(5000, 0)); // 10000 / 2
    }

    #[test]
    fn test_enable_disable_wallet() {
        let executor = create_test_executor();
        let copy_trader = CopyTrader::new(executor, Decimal::new(10000, 0));

        copy_trader.add_tracked_wallet(TrackedWallet::new("0xAAA".to_string(), Decimal::new(50, 0)));

        assert!(copy_trader.set_wallet_enabled("0xAAA", false));
        let wallet = copy_trader.get_tracked_wallet("0xAAA").unwrap();
        assert!(!wallet.enabled);

        assert!(copy_trader.set_wallet_enabled("0xaaa", true)); // Case insensitive
        let wallet = copy_trader.get_tracked_wallet("0xAAA").unwrap();
        assert!(wallet.enabled);
    }
}
