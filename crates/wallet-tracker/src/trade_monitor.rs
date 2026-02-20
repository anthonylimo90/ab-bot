//! Real-time trade monitoring for tracked wallets.
//!
//! Uses the Polymarket Data API (via `ClobClient`) to detect trades
//! from monitored wallets in near-real-time.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use polymarket_core::api::{ClobClient, ClobTrade};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tokio::time::{interval, Duration};
use tracing::{info, warn};

/// A trade detected from a monitored wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTrade {
    /// Wallet address that made the trade.
    pub wallet_address: String,
    /// Transaction hash.
    pub tx_hash: String,
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
    /// Number of activity rows fetched per wallet poll.
    pub wallet_activity_limit: u32,
    /// Maximum number of trades to keep in history.
    pub max_history_size: usize,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 15,
            min_trade_value: Decimal::new(5, 2), // $0.05
            max_trade_age_secs: 900,             // 15 minutes to tolerate upstream lag
            wallet_activity_limit: 100,
            max_history_size: 10000,
        }
    }
}

impl MonitorConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            poll_interval_secs: std::env::var("TRADE_MONITOR_POLL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(15),
            min_trade_value: std::env::var("TRADE_MONITOR_MIN_VALUE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(5, 2)), // $0.05
            max_trade_age_secs: std::env::var("TRADE_MONITOR_MAX_AGE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(900),
            wallet_activity_limit: std::env::var("TRADE_MONITOR_WALLET_LIMIT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            max_history_size: 10000,
        }
    }
}

/// Real-time trade monitor for tracked wallets.
pub struct TradeMonitor {
    clob_client: Arc<ClobClient>,
    config: MonitorConfig,
    /// Wallets being monitored (lowercased).
    monitored_wallets: Arc<RwLock<HashSet<String>>>,
    /// Recent trades by wallet.
    recent_trades: DashMap<String, Vec<WalletTrade>>,
    /// Transaction hashes already seen (dedup).
    seen_tx_hashes: Arc<RwLock<HashSet<String>>>,
    /// Channel for new trade notifications.
    trade_tx: broadcast::Sender<WalletTrade>,
    /// Whether monitoring is active.
    active: Arc<RwLock<bool>>,
}

impl TradeMonitor {
    /// Create a new trade monitor.
    pub fn new(clob_client: Arc<ClobClient>, config: MonitorConfig) -> Self {
        let (trade_tx, _) = broadcast::channel(1000);
        Self {
            clob_client,
            config,
            monitored_wallets: Arc::new(RwLock::new(HashSet::new())),
            recent_trades: DashMap::new(),
            seen_tx_hashes: Arc::new(RwLock::new(HashSet::new())),
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
        self.poll_all_wallets().await
    }

    // Private methods

    fn clone_for_task(&self) -> Self {
        Self {
            clob_client: self.clob_client.clone(),
            config: self.config.clone(),
            monitored_wallets: self.monitored_wallets.clone(),
            recent_trades: self.recent_trades.clone(),
            seen_tx_hashes: self.seen_tx_hashes.clone(),
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

            match self.poll_all_wallets().await {
                Ok(trades) => {
                    for trade in trades {
                        if trade.is_significant(self.config.min_trade_value)
                            && self.trade_tx.send(trade).is_err()
                        {
                            warn!("No subscribers for trade notifications");
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to poll Data API for trades");
                }
            }

            // Cleanup old trades and stale tx hashes
            self.cleanup_old_trades();
        }

        info!("Trade monitoring loop stopped");
    }

    /// Poll each monitored wallet individually via the Data API `/activity`
    /// endpoint and return any new trades detected since the last poll.
    async fn poll_all_wallets(&self) -> Result<Vec<WalletTrade>> {
        let wallets = self.monitored_wallets.read().await;
        if wallets.is_empty() {
            return Ok(Vec::new());
        }
        let wallet_list: Vec<String> = wallets.iter().cloned().collect();
        drop(wallets);

        // Poll all wallets concurrently
        let futures: Vec<_> = wallet_list
            .iter()
            .map(|addr| {
                self.clob_client
                    .get_wallet_activity(addr, self.config.wallet_activity_limit)
            })
            .collect();
        let results = futures_util::future::join_all(futures).await;

        let mut new_trades = Vec::new();
        let mut seen = self.seen_tx_hashes.write().await;

        for (wallet_addr, result) in wallet_list.iter().zip(results) {
            let clob_trades = match result {
                Ok(trades) => trades,
                Err(e) => {
                    warn!(
                        wallet = %wallet_addr,
                        error = %e,
                        "Failed to fetch activity for wallet"
                    );
                    continue;
                }
            };

            for ct in &clob_trades {
                // Validate the trade belongs to the wallet we requested
                if ct.wallet_address.to_lowercase() != *wallet_addr {
                    warn!(
                        expected = %wallet_addr,
                        actual = %ct.wallet_address,
                        "Activity endpoint returned trade for wrong wallet, skipping"
                    );
                    continue;
                }

                // Dedup by a composite key to avoid dropping legitimate multi-fill trades
                // that share one transaction hash.
                let dedup_key = format!(
                    "{}:{}:{}:{}:{}:{}",
                    ct.transaction_hash, ct.asset_id, ct.side, ct.timestamp, ct.price, ct.size
                );
                if seen.contains(&dedup_key) {
                    continue;
                }

                if let Some(trade) = Self::clob_trade_to_wallet_trade(ct) {
                    // Check age
                    if trade.age_seconds() > self.config.max_trade_age_secs as i64 {
                        continue;
                    }

                    seen.insert(dedup_key);

                    // Store in recent trades
                    self.recent_trades
                        .entry(wallet_addr.clone())
                        .or_default()
                        .push(trade.clone());

                    new_trades.push(trade);
                }
            }
        }

        if !new_trades.is_empty() {
            info!(
                count = new_trades.len(),
                "Detected new trades from monitored wallets"
            );
        }

        Ok(new_trades)
    }

    /// Convert a ClobTrade from the Data API into a WalletTrade.
    fn clob_trade_to_wallet_trade(ct: &ClobTrade) -> Option<WalletTrade> {
        let timestamp = DateTime::from_timestamp(ct.timestamp, 0)?;
        let price = Decimal::from_f64(ct.price).unwrap_or(Decimal::ZERO);
        let quantity = Decimal::from_f64(ct.size).unwrap_or(Decimal::ZERO);
        let value = price * quantity;

        let direction = match ct.side.to_uppercase().as_str() {
            "BUY" => TradeDirection::Buy,
            "SELL" => TradeDirection::Sell,
            _ => return None,
        };

        let market_id = ct
            .condition_id
            .clone()
            .unwrap_or_else(|| ct.asset_id.clone());

        Some(WalletTrade {
            wallet_address: ct.wallet_address.to_lowercase(),
            tx_hash: ct.transaction_hash.clone(),
            timestamp,
            market_id,
            token_id: ct.asset_id.clone(),
            direction,
            price,
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

        // Prune seen_tx_hashes if it gets too large (keep bounded)
        if let Ok(mut seen) = self.seen_tx_hashes.try_write() {
            if seen.len() > 50_000 {
                seen.clear();
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
        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(config.min_trade_value, Decimal::new(5, 2));
        assert_eq!(config.max_trade_age_secs, 900);
        assert_eq!(config.wallet_activity_limit, 100);
    }

    #[test]
    fn test_monitor_config_from_env() {
        // Without env vars set, should use defaults
        let config = MonitorConfig::from_env();
        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(config.min_trade_value, Decimal::new(5, 2));
        assert_eq!(config.max_trade_age_secs, 900);
        assert_eq!(config.wallet_activity_limit, 100);
    }

    #[tokio::test]
    async fn test_add_remove_wallet() {
        let clob_client = Arc::new(ClobClient::new(None, None));
        let monitor = TradeMonitor::new(clob_client, MonitorConfig::default());

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

    #[test]
    fn test_clob_trade_to_wallet_trade_buy() {
        let ct = ClobTrade {
            transaction_hash: "0xtx123".to_string(),
            wallet_address: "0xWALLET".to_string(),
            side: "BUY".to_string(),
            asset_id: "token_abc".to_string(),
            condition_id: Some("condition_xyz".to_string()),
            size: 100.0,
            price: 0.65,
            timestamp: Utc::now().timestamp(),
            title: None,
            slug: None,
            outcome: None,
        };

        let wt = TradeMonitor::clob_trade_to_wallet_trade(&ct).unwrap();
        assert_eq!(wt.wallet_address, "0xwallet");
        assert_eq!(wt.tx_hash, "0xtx123");
        assert_eq!(wt.market_id, "condition_xyz");
        assert_eq!(wt.token_id, "token_abc");
        assert_eq!(wt.direction, TradeDirection::Buy);
        assert_eq!(wt.price, Decimal::from_f64(0.65).unwrap());
        assert_eq!(wt.quantity, Decimal::from_f64(100.0).unwrap());
    }

    #[test]
    fn test_clob_trade_to_wallet_trade_sell() {
        let ct = ClobTrade {
            transaction_hash: "0xtx456".to_string(),
            wallet_address: "0xSELLER".to_string(),
            side: "SELL".to_string(),
            asset_id: "token_def".to_string(),
            condition_id: None,
            size: 50.0,
            price: 0.30,
            timestamp: Utc::now().timestamp(),
            title: None,
            slug: None,
            outcome: None,
        };

        let wt = TradeMonitor::clob_trade_to_wallet_trade(&ct).unwrap();
        assert_eq!(wt.direction, TradeDirection::Sell);
        // When condition_id is None, market_id falls back to asset_id
        assert_eq!(wt.market_id, "token_def");
    }

    #[test]
    fn test_clob_trade_to_wallet_trade_invalid_side() {
        let ct = ClobTrade {
            transaction_hash: "0xtx789".to_string(),
            wallet_address: "0xBAD".to_string(),
            side: "UNKNOWN".to_string(),
            asset_id: "token".to_string(),
            condition_id: None,
            size: 10.0,
            price: 0.50,
            timestamp: Utc::now().timestamp(),
            title: None,
            slug: None,
            outcome: None,
        };

        assert!(TradeMonitor::clob_trade_to_wallet_trade(&ct).is_none());
    }
}
