//! Real-time trade monitoring for tracked wallets.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use polymarket_core::api::PolygonClient;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

/// A trade detected from a monitored wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTrade {
    /// Wallet address that made the trade.
    pub wallet_address: String,
    /// Transaction hash.
    pub tx_hash: String,
    /// Block number.
    pub block_number: u64,
    /// Timestamp of the trade.
    pub timestamp: DateTime<Utc>,
    /// Market/asset identifier.
    pub market_id: String,
    /// Token/outcome ID.
    pub token_id: String,
    /// Trade direction.
    pub direction: TradeDirection,
    /// Price per share.
    pub price: Decimal,
    /// Quantity of shares.
    pub quantity: Decimal,
    /// Total value of the trade.
    pub value: Decimal,
    /// Whether this trade has been processed for copy trading.
    pub processed: bool,
}

impl WalletTrade {
    /// Check if this is a significant trade worth copying.
    pub fn is_significant(&self, min_value: Decimal) -> bool {
        self.value >= min_value
    }

    /// Get the trade age in seconds.
    pub fn age_seconds(&self) -> i64 {
        (Utc::now() - self.timestamp).num_seconds()
    }
}

/// Direction of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeDirection {
    Buy,
    Sell,
}

/// Configuration for trade monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// Polling interval in seconds.
    pub poll_interval_secs: u64,
    /// Minimum trade value to track.
    pub min_trade_value: Decimal,
    /// Maximum trade age to process (in seconds).
    pub max_trade_age_secs: u64,
    /// Maximum number of trades to keep in history.
    pub max_history_size: usize,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 10,
            min_trade_value: Decimal::new(100, 0),
            max_trade_age_secs: 300, // 5 minutes
            max_history_size: 10000,
        }
    }
}

/// Real-time trade monitor for tracked wallets.
pub struct TradeMonitor {
    polygon_client: Arc<PolygonClient>,
    config: MonitorConfig,
    /// Wallets being monitored.
    monitored_wallets: Arc<RwLock<HashSet<String>>>,
    /// Recent trades by wallet.
    recent_trades: DashMap<String, Vec<WalletTrade>>,
    /// Last processed block per wallet.
    last_block: DashMap<String, u64>,
    /// Channel for new trade notifications.
    trade_tx: broadcast::Sender<WalletTrade>,
    /// Whether monitoring is active.
    active: Arc<RwLock<bool>>,
}

impl TradeMonitor {
    /// Create a new trade monitor.
    pub fn new(polygon_client: Arc<PolygonClient>, config: MonitorConfig) -> Self {
        let (trade_tx, _) = broadcast::channel(1000);
        Self {
            polygon_client,
            config,
            monitored_wallets: Arc::new(RwLock::new(HashSet::new())),
            recent_trades: DashMap::new(),
            last_block: DashMap::new(),
            trade_tx,
            active: Arc::new(RwLock::new(false)),
        }
    }

    /// Subscribe to trade notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<WalletTrade> {
        self.trade_tx.subscribe()
    }

    /// Add a wallet to monitor.
    pub async fn add_wallet(&self, address: &str) {
        let address_lower = address.to_lowercase();
        let mut wallets = self.monitored_wallets.write().await;
        if wallets.insert(address_lower.clone()) {
            info!(address = %address_lower, "Started monitoring wallet");
        }
    }

    /// Remove a wallet from monitoring.
    pub async fn remove_wallet(&self, address: &str) -> bool {
        let address_lower = address.to_lowercase();
        let mut wallets = self.monitored_wallets.write().await;
        let removed = wallets.remove(&address_lower);
        if removed {
            self.recent_trades.remove(&address_lower);
            self.last_block.remove(&address_lower);
            info!(address = %address_lower, "Stopped monitoring wallet");
        }
        removed
    }

    /// Get all monitored wallets.
    pub async fn monitored_wallets(&self) -> Vec<String> {
        let wallets = self.monitored_wallets.read().await;
        wallets.iter().cloned().collect()
    }

    /// Check if a wallet is being monitored.
    pub async fn is_monitoring(&self, address: &str) -> bool {
        let wallets = self.monitored_wallets.read().await;
        wallets.contains(&address.to_lowercase())
    }

    /// Get recent trades for a wallet.
    pub fn get_recent_trades(&self, address: &str) -> Vec<WalletTrade> {
        self.recent_trades
            .get(&address.to_lowercase())
            .map(|trades| trades.clone())
            .unwrap_or_default()
    }

    /// Get all recent trades across all monitored wallets.
    pub fn get_all_recent_trades(&self) -> Vec<WalletTrade> {
        let mut all_trades: Vec<WalletTrade> = self
            .recent_trades
            .iter()
            .flat_map(|entry| entry.value().clone())
            .collect();

        all_trades.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_trades
    }

    /// Start the monitoring loop.
    pub async fn start(&self) -> Result<()> {
        {
            let mut active = self.active.write().await;
            if *active {
                return Ok(()); // Already running
            }
            *active = true;
        }

        info!("Starting trade monitor");

        let monitor = self.clone_for_task();
        tokio::spawn(async move {
            monitor.monitoring_loop().await;
        });

        Ok(())
    }

    /// Stop the monitoring loop.
    pub async fn stop(&self) {
        let mut active = self.active.write().await;
        *active = false;
        info!("Stopping trade monitor");
    }

    /// Check if monitoring is active.
    pub async fn is_active(&self) -> bool {
        *self.active.read().await
    }

    /// Manually poll for new trades (useful for testing).
    pub async fn poll_once(&self) -> Result<Vec<WalletTrade>> {
        let wallets = self.monitored_wallets.read().await.clone();
        let mut all_new_trades = Vec::new();

        for address in wallets {
            match self.poll_wallet(&address).await {
                Ok(trades) => {
                    all_new_trades.extend(trades);
                }
                Err(e) => {
                    warn!(address = %address, error = %e, "Failed to poll wallet");
                }
            }
        }

        Ok(all_new_trades)
    }

    // Private methods

    fn clone_for_task(&self) -> Self {
        Self {
            polygon_client: self.polygon_client.clone(),
            config: self.config.clone(),
            monitored_wallets: self.monitored_wallets.clone(),
            recent_trades: self.recent_trades.clone(),
            last_block: self.last_block.clone(),
            trade_tx: self.trade_tx.clone(),
            active: self.active.clone(),
        }
    }

    async fn monitoring_loop(&self) {
        let mut poll_interval = interval(Duration::from_secs(self.config.poll_interval_secs));

        loop {
            poll_interval.tick().await;

            if !*self.active.read().await {
                break;
            }

            let wallets = self.monitored_wallets.read().await.clone();

            for address in wallets {
                if !*self.active.read().await {
                    break;
                }

                match self.poll_wallet(&address).await {
                    Ok(trades) => {
                        for trade in trades {
                            if trade.is_significant(self.config.min_trade_value) {
                                if self.trade_tx.send(trade).is_err() {
                                    debug!("No subscribers for trade notifications");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!(address = %address, error = %e, "Failed to poll wallet");
                    }
                }

                // Small delay between wallets to avoid rate limiting
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            // Cleanup old trades
            self.cleanup_old_trades();
        }

        info!("Trade monitoring loop stopped");
    }

    async fn poll_wallet(&self, address: &str) -> Result<Vec<WalletTrade>> {
        let last_block = self.last_block.get(address).map(|b| *b);

        // Fetch recent transfers
        let transfers = self
            .polygon_client
            .get_asset_transfers(address, None, None)
            .await?;

        let mut new_trades = Vec::new();

        for transfer in transfers {
            // Skip if we've already processed this block
            let block_num: u64 = transfer.block_num.parse().unwrap_or(0);
            if let Some(last) = last_block {
                if block_num <= last {
                    continue;
                }
            }

            // Parse the transfer into a trade
            if let Some(trade) = self.parse_transfer_to_trade(address, &transfer) {
                // Check if trade is recent enough
                if trade.age_seconds() <= self.config.max_trade_age_secs as i64 {
                    new_trades.push(trade.clone());

                    // Store in recent trades
                    self.recent_trades
                        .entry(address.to_lowercase())
                        .or_default()
                        .push(trade);
                }

                // Update last block
                self.last_block.insert(address.to_lowercase(), block_num);
            }
        }

        if !new_trades.is_empty() {
            info!(
                address = %address,
                count = new_trades.len(),
                "Detected new trades from wallet"
            );
        }

        Ok(new_trades)
    }

    fn parse_transfer_to_trade(
        &self,
        wallet_address: &str,
        transfer: &polymarket_core::api::polygon::AssetTransfer,
    ) -> Option<WalletTrade> {
        let timestamp = transfer
            .metadata
            .as_ref()
            .and_then(|m| m.block_timestamp.as_ref())
            .and_then(|ts| ts.parse::<DateTime<Utc>>().ok())?;

        let value = transfer.value.map(|v| Decimal::try_from(v).ok())??;
        let quantity = value; // Simplified - would need price data

        let direction = if transfer.from.to_lowercase() == wallet_address.to_lowercase() {
            TradeDirection::Sell
        } else {
            TradeDirection::Buy
        };

        Some(WalletTrade {
            wallet_address: wallet_address.to_string(),
            tx_hash: transfer.hash.clone(),
            block_number: transfer.block_num.parse().unwrap_or(0),
            timestamp,
            market_id: transfer.asset.clone().unwrap_or_default(),
            token_id: transfer.asset.clone().unwrap_or_default(),
            direction,
            price: Decimal::ONE, // Would need price oracle
            quantity,
            value,
            processed: false,
        })
    }

    fn cleanup_old_trades(&self) {
        let max_age = chrono::Duration::seconds(self.config.max_trade_age_secs as i64 * 10);
        let cutoff = Utc::now() - max_age;

        for mut entry in self.recent_trades.iter_mut() {
            let trades = entry.value_mut();
            trades.retain(|t| t.timestamp > cutoff);

            // Also limit total size
            if trades.len() > self.config.max_history_size {
                trades.drain(0..(trades.len() - self.config.max_history_size));
            }
        }
    }
}

/// Statistics for trade monitoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorStats {
    pub monitored_wallets: usize,
    pub total_trades_tracked: usize,
    pub trades_last_hour: usize,
    pub total_volume_tracked: Decimal,
    pub is_active: bool,
}

impl TradeMonitor {
    /// Get monitoring statistics.
    pub async fn stats(&self) -> MonitorStats {
        let wallets = self.monitored_wallets.read().await;
        let hour_ago = Utc::now() - chrono::Duration::hours(1);

        let mut total_trades = 0;
        let mut recent_trades = 0;
        let mut total_volume = Decimal::ZERO;

        for entry in self.recent_trades.iter() {
            for trade in entry.value() {
                total_trades += 1;
                total_volume += trade.value;
                if trade.timestamp > hour_ago {
                    recent_trades += 1;
                }
            }
        }

        MonitorStats {
            monitored_wallets: wallets.len(),
            total_trades_tracked: total_trades,
            trades_last_hour: recent_trades,
            total_volume_tracked: total_volume,
            is_active: *self.active.read().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_trade_significance() {
        let trade = WalletTrade {
            wallet_address: "0x1234".to_string(),
            tx_hash: "0xabcd".to_string(),
            block_number: 1000,
            timestamp: Utc::now(),
            market_id: "market1".to_string(),
            token_id: "yes".to_string(),
            direction: TradeDirection::Buy,
            price: Decimal::new(50, 2),
            quantity: Decimal::new(100, 0),
            value: Decimal::new(50, 0),
            processed: false,
        };

        assert!(trade.is_significant(Decimal::new(10, 0)));
        assert!(!trade.is_significant(Decimal::new(100, 0)));
    }

    #[test]
    fn test_trade_age() {
        let recent_trade = WalletTrade {
            wallet_address: "0x1234".to_string(),
            tx_hash: "0xabcd".to_string(),
            block_number: 1000,
            timestamp: Utc::now() - chrono::Duration::seconds(30),
            market_id: "market1".to_string(),
            token_id: "yes".to_string(),
            direction: TradeDirection::Buy,
            price: Decimal::ONE,
            quantity: Decimal::ONE,
            value: Decimal::ONE,
            processed: false,
        };

        assert!(recent_trade.age_seconds() >= 30);
        assert!(recent_trade.age_seconds() < 35);
    }

    #[test]
    fn test_monitor_config_default() {
        let config = MonitorConfig::default();
        assert_eq!(config.poll_interval_secs, 10);
        assert_eq!(config.min_trade_value, Decimal::new(100, 0));
        assert_eq!(config.max_trade_age_secs, 300);
    }

    #[tokio::test]
    async fn test_add_remove_wallet() {
        let polygon_client = Arc::new(PolygonClient::new("test_key".to_string()));
        let monitor = TradeMonitor::new(polygon_client, MonitorConfig::default());

        monitor.add_wallet("0xAAAA").await;
        monitor.add_wallet("0xBBBB").await;

        let wallets = monitor.monitored_wallets().await;
        assert_eq!(wallets.len(), 2);

        assert!(monitor.is_monitoring("0xaaaa").await); // Case insensitive
        assert!(monitor.remove_wallet("0xAAAA").await);
        assert!(!monitor.is_monitoring("0xaaaa").await);

        let wallets = monitor.monitored_wallets().await;
        assert_eq!(wallets.len(), 1);
    }
}
