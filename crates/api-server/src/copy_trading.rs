//! Copy trading monitor - bridges wallet trade detection to copy trader execution.
//!
//! This module monitors tracked wallets for trades and forwards them to the copy trader
//! for execution, while publishing signals to WebSocket clients.

use chrono::Utc;
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};

use polymarket_core::types::OrderSide;
use trading_engine::copy_trader::{CopyTrader, DetectedTrade};
use wallet_tracker::trade_monitor::{TradeDirection, TradeMonitor, WalletTrade};

use crate::websocket::{SignalType, SignalUpdate};

/// Configuration for the copy trading monitor.
#[derive(Debug, Clone)]
pub struct CopyTradingConfig {
    /// Minimum trade value to trigger copy.
    pub min_trade_value: Decimal,
    /// Maximum latency in seconds before skipping a trade.
    pub max_latency_secs: i64,
    /// Whether copy trading is enabled.
    pub enabled: bool,
}

impl Default for CopyTradingConfig {
    fn default() -> Self {
        Self {
            min_trade_value: Decimal::new(10, 0), // $10 minimum
            max_latency_secs: 60,                  // 1 minute max latency
            enabled: true,
        }
    }
}

impl CopyTradingConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            min_trade_value: std::env::var("COPY_MIN_TRADE_VALUE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(10, 0)),
            max_latency_secs: std::env::var("COPY_MAX_LATENCY_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
            enabled: std::env::var("COPY_TRADING_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
        }
    }
}

/// Copy trading monitor that bridges TradeMonitor to CopyTrader.
pub struct CopyTradingMonitor {
    config: CopyTradingConfig,
    trade_monitor: Arc<TradeMonitor>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    signal_tx: broadcast::Sender<SignalUpdate>,
}

impl CopyTradingMonitor {
    /// Create a new copy trading monitor.
    pub fn new(
        config: CopyTradingConfig,
        trade_monitor: Arc<TradeMonitor>,
        copy_trader: Arc<RwLock<CopyTrader>>,
        signal_tx: broadcast::Sender<SignalUpdate>,
    ) -> Self {
        Self {
            config,
            trade_monitor,
            copy_trader,
            signal_tx,
        }
    }

    /// Start the monitoring loop - runs until cancelled.
    pub async fn run(&self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Copy trading monitor is disabled");
            return Ok(());
        }

        info!("Starting copy trading monitor");

        let mut trade_rx = self.trade_monitor.subscribe();

        loop {
            match trade_rx.recv().await {
                Ok(wallet_trade) => {
                    if let Err(e) = self.process_trade(wallet_trade).await {
                        error!(error = %e, "Failed to process detected trade");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "Copy trading monitor lagged, skipped messages");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("Trade monitor channel closed, stopping copy trading monitor");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn process_trade(&self, trade: WalletTrade) -> anyhow::Result<()> {
        // Check minimum trade value
        if trade.value < self.config.min_trade_value {
            debug!(
                wallet = %trade.wallet_address,
                value = %trade.value,
                min = %self.config.min_trade_value,
                "Trade below minimum value, skipping"
            );
            return Ok(());
        }

        // Check latency
        let now = Utc::now();
        let latency = now.signed_duration_since(trade.timestamp).num_seconds();
        if latency > self.config.max_latency_secs {
            warn!(
                wallet = %trade.wallet_address,
                latency = latency,
                max = self.config.max_latency_secs,
                "Trade too old, skipping"
            );
            return Ok(());
        }

        // Convert WalletTrade to DetectedTrade
        let detected = DetectedTrade {
            wallet_address: trade.wallet_address.clone(),
            market_id: trade.market_id.clone(),
            outcome_id: trade.token_id.clone(),
            side: match trade.direction {
                TradeDirection::Buy => OrderSide::Buy,
                TradeDirection::Sell => OrderSide::Sell,
            },
            price: trade.price,
            quantity: trade.quantity,
            timestamp: trade.timestamp,
            tx_hash: trade.tx_hash.clone(),
        };

        // Publish signal before attempting copy (for UI notification)
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::CopyTrade,
            market_id: trade.market_id.clone(),
            outcome_id: trade.token_id.clone(),
            action: match trade.direction {
                TradeDirection::Buy => "buy".to_string(),
                TradeDirection::Sell => "sell".to_string(),
            },
            confidence: 1.0,
            timestamp: now,
            metadata: serde_json::json!({
                "wallet_address": trade.wallet_address,
                "price": trade.price.to_string(),
                "quantity": trade.quantity.to_string(),
                "value": trade.value.to_string(),
                "tx_hash": trade.tx_hash,
                "latency_secs": latency,
            }),
        };

        // Send signal to WebSocket clients
        let _ = self.signal_tx.send(signal);

        // Process the trade through CopyTrader
        let copy_trader = self.copy_trader.read().await;
        match copy_trader.process_detected_trade(&detected).await {
            Ok(Some(report)) => {
                info!(
                    wallet = %trade.wallet_address,
                    market = %trade.market_id,
                    direction = ?trade.direction,
                    copied_quantity = %report.filled_quantity,
                    "Successfully copied trade"
                );

                // Publish success signal
                let success_signal = SignalUpdate {
                    signal_id: uuid::Uuid::new_v4(),
                    signal_type: SignalType::CopyTrade,
                    market_id: trade.market_id,
                    outcome_id: trade.token_id,
                    action: "copied".to_string(),
                    confidence: 1.0,
                    timestamp: Utc::now(),
                    metadata: serde_json::json!({
                        "wallet_address": trade.wallet_address,
                        "copied_quantity": report.filled_quantity.to_string(),
                        "execution_price": report.average_price.to_string(),
                        "order_id": report.order_id.to_string(),
                    }),
                };
                let _ = self.signal_tx.send(success_signal);
            }
            Ok(None) => {
                debug!(
                    wallet = %trade.wallet_address,
                    "Trade not copied (wallet disabled or not tracked)"
                );
            }
            Err(e) => {
                error!(
                    wallet = %trade.wallet_address,
                    error = %e,
                    "Failed to copy trade"
                );
            }
        }

        Ok(())
    }
}

/// Spawn the copy trading monitor as a background task.
pub fn spawn_copy_trading_monitor(
    config: CopyTradingConfig,
    trade_monitor: Arc<TradeMonitor>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    signal_tx: broadcast::Sender<SignalUpdate>,
) {
    let monitor = CopyTradingMonitor::new(config, trade_monitor, copy_trader, signal_tx);

    tokio::spawn(async move {
        if let Err(e) = monitor.run().await {
            error!(error = %e, "Copy trading monitor failed");
        }
    });

    info!("Copy trading monitor spawned as background task");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = CopyTradingConfig::default();
        assert_eq!(config.min_trade_value, Decimal::new(10, 0));
        assert_eq!(config.max_latency_secs, 60);
        assert!(config.enabled);
    }

    #[test]
    fn test_trade_direction_conversion() {
        let buy: OrderSide = match TradeDirection::Buy {
            TradeDirection::Buy => OrderSide::Buy,
            TradeDirection::Sell => OrderSide::Sell,
        };
        assert!(matches!(buy, OrderSide::Buy));

        let sell: OrderSide = match TradeDirection::Sell {
            TradeDirection::Buy => OrderSide::Buy,
            TradeDirection::Sell => OrderSide::Sell,
        };
        assert!(matches!(sell, OrderSide::Sell));
    }
}
