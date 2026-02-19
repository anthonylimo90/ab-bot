//! Redis signal forwarder - bridges Redis pub/sub to WebSocket broadcasts.
//!
//! Subscribes to Redis channels from arb-monitor and other services,
//! then forwards signals to the API server's WebSocket broadcast channels.

use chrono::Utc;
use futures_util::StreamExt;
use polymarket_core::types::ArbOpportunity;
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::websocket::{OrderbookUpdate, SignalType, SignalUpdate};

/// Redis channel names for pub/sub.
pub mod channels {
    pub const ARB_ENTRY: &str = "arb:entry";
    pub const ARB_EXIT: &str = "arb:exit";
    pub const ARB_PRICES: &str = "arb:prices";
    pub const ARB_ALERTS: &str = "arb:alerts";
    pub const COPY_SIGNALS: &str = "copy:signals";
    pub const ORDERBOOK_UPDATES: &str = "orderbook:updates";
}

/// Configuration for the Redis forwarder.
#[derive(Debug, Clone)]
pub struct RedisForwarderConfig {
    /// Redis connection URL.
    pub redis_url: String,
    /// Whether to subscribe to arbitrage signals.
    pub subscribe_arb: bool,
    /// Whether to subscribe to copy trade signals.
    pub subscribe_copy: bool,
    /// Whether to subscribe to orderbook updates.
    pub subscribe_orderbook: bool,
    /// Reconnection delay in seconds.
    pub reconnect_delay_secs: u64,
}

impl Default for RedisForwarderConfig {
    fn default() -> Self {
        Self {
            redis_url: "redis://127.0.0.1:6379".to_string(),
            subscribe_arb: true,
            subscribe_copy: true,
            subscribe_orderbook: true,
            reconnect_delay_secs: 5,
        }
    }
}

impl RedisForwarderConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            redis_url: std::env::var("REDIS_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
            subscribe_arb: std::env::var("REDIS_SUBSCRIBE_ARB")
                .map(|v| v == "true")
                .unwrap_or(true),
            subscribe_copy: std::env::var("REDIS_SUBSCRIBE_COPY")
                .map(|v| v == "true")
                .unwrap_or(true),
            subscribe_orderbook: std::env::var("REDIS_SUBSCRIBE_ORDERBOOK")
                .map(|v| v == "true")
                .unwrap_or(true),
            reconnect_delay_secs: std::env::var("REDIS_RECONNECT_DELAY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
        }
    }
}

/// Redis signal forwarder that bridges Redis pub/sub to WebSocket broadcasts.
pub struct RedisForwarder {
    config: RedisForwarderConfig,
    signal_tx: broadcast::Sender<SignalUpdate>,
    orderbook_tx: broadcast::Sender<OrderbookUpdate>,
    arb_entry_tx: broadcast::Sender<ArbOpportunity>,
}

impl RedisForwarder {
    /// Create a new Redis forwarder.
    pub fn new(
        config: RedisForwarderConfig,
        signal_tx: broadcast::Sender<SignalUpdate>,
        orderbook_tx: broadcast::Sender<OrderbookUpdate>,
        arb_entry_tx: broadcast::Sender<ArbOpportunity>,
    ) -> Self {
        Self {
            config,
            signal_tx,
            orderbook_tx,
            arb_entry_tx,
        }
    }

    /// Start the forwarder - runs until cancelled.
    pub async fn run(&self) -> anyhow::Result<()> {
        loop {
            match self.run_inner().await {
                Ok(_) => {
                    info!("Redis forwarder connection closed normally");
                }
                Err(e) => {
                    error!(error = %e, "Redis forwarder error, reconnecting...");
                }
            }

            // Wait before reconnecting
            tokio::time::sleep(tokio::time::Duration::from_secs(
                self.config.reconnect_delay_secs,
            ))
            .await;
        }
    }

    async fn run_inner(&self) -> anyhow::Result<()> {
        let client = redis::Client::open(self.config.redis_url.as_str())?;
        let conn = client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();

        info!("Connected to Redis for signal forwarding");

        // Subscribe to configured channels
        if self.config.subscribe_arb {
            pubsub.subscribe(channels::ARB_ENTRY).await?;
            pubsub.subscribe(channels::ARB_EXIT).await?;
            pubsub.subscribe(channels::ARB_ALERTS).await?;
            debug!("Subscribed to arbitrage channels");
        }

        if self.config.subscribe_copy {
            pubsub.subscribe(channels::COPY_SIGNALS).await?;
            debug!("Subscribed to copy trading channel");
        }

        if self.config.subscribe_orderbook {
            pubsub.subscribe(channels::ORDERBOOK_UPDATES).await?;
            debug!("Subscribed to orderbook updates channel");
        }

        info!("Redis forwarder listening for signals");

        // Process messages
        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let channel: String = msg.get_channel_name().to_string();
            let payload: String = match msg.get_payload() {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "Failed to get message payload");
                    continue;
                }
            };

            debug!(channel = %channel, "Received Redis message");

            if let Err(e) = self.process_message(&channel, &payload).await {
                warn!(channel = %channel, error = %e, "Failed to process message");
            }
        }

        Ok(())
    }

    async fn process_message(&self, channel: &str, payload: &str) -> anyhow::Result<()> {
        match channel {
            channels::ARB_ENTRY => {
                self.handle_arb_entry(payload).await?;
            }
            channels::ARB_EXIT => {
                self.handle_arb_exit(payload).await?;
            }
            channels::ARB_ALERTS => {
                self.handle_arb_alert(payload).await?;
            }
            channels::COPY_SIGNALS => {
                self.handle_copy_signal(payload).await?;
            }
            channels::ORDERBOOK_UPDATES => {
                self.handle_orderbook_update(payload).await?;
            }
            _ => {
                debug!(channel = %channel, "Unknown channel");
            }
        }

        Ok(())
    }

    async fn handle_arb_entry(&self, payload: &str) -> anyhow::Result<()> {
        let arb: ArbOpportunity = serde_json::from_str(payload)?;

        // Forward to arb auto-executor before WebSocket processing
        let receivers = self.arb_entry_tx.send(arb.clone()).unwrap_or(0);
        info!(
            market_id = %arb.market_id,
            net_profit = %arb.net_profit,
            receivers,
            "Forwarded arb entry signal from Redis to executor"
        );

        // Clone values needed for logging before moving
        let market_id_log = arb.market_id.clone();
        let net_profit_log = arb.net_profit;

        // Convert to SignalUpdate
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: arb.market_id.clone(),
            outcome_id: "both".to_string(), // Arb involves both outcomes
            action: "entry".to_string(),
            confidence: arb.net_profit.to_string().parse().unwrap_or(0.0),
            timestamp: arb.timestamp,
            metadata: serde_json::json!({
                "yes_ask": arb.yes_ask.to_string(),
                "no_ask": arb.no_ask.to_string(),
                "total_cost": arb.total_cost.to_string(),
                "gross_profit": arb.gross_profit.to_string(),
                "net_profit": arb.net_profit.to_string(),
            }),
        };

        // Also send an orderbook update with the arb spread
        let orderbook_update = OrderbookUpdate {
            market_id: arb.market_id,
            timestamp: arb.timestamp,
            yes_bid: Decimal::ZERO, // Entry signal doesn't have bid info
            yes_ask: arb.yes_ask,
            no_bid: Decimal::ZERO,
            no_ask: arb.no_ask,
            arb_spread: Some(arb.net_profit),
        };

        // Forward to WebSocket broadcast channels
        let _ = self.signal_tx.send(signal);
        let _ = self.orderbook_tx.send(orderbook_update);

        info!(
            market_id = %market_id_log,
            net_profit = %net_profit_log,
            "Forwarded arbitrage entry signal"
        );

        Ok(())
    }

    async fn handle_arb_exit(&self, payload: &str) -> anyhow::Result<()> {
        #[derive(serde::Deserialize)]
        struct ArbExit {
            market_id: String,
            position_id: String,
            profit: String,
            timestamp: String,
        }

        let exit: ArbExit = serde_json::from_str(payload)?;

        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: exit.market_id.clone(),
            outcome_id: exit.position_id.clone(),
            action: "exit".to_string(),
            confidence: 1.0,
            timestamp: chrono::DateTime::parse_from_rfc3339(&exit.timestamp)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            metadata: serde_json::json!({
                "profit": exit.profit,
                "position_id": exit.position_id,
            }),
        };

        let _ = self.signal_tx.send(signal);

        info!(
            market_id = %exit.market_id,
            profit = %exit.profit,
            "Forwarded arbitrage exit signal"
        );

        Ok(())
    }

    async fn handle_arb_alert(&self, payload: &str) -> anyhow::Result<()> {
        // Generic alert - just forward as an Alert signal type
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Alert,
            market_id: "".to_string(),
            outcome_id: "".to_string(),
            action: "alert".to_string(),
            confidence: 1.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "message": payload,
            }),
        };

        let _ = self.signal_tx.send(signal);

        debug!("Forwarded alert signal");

        Ok(())
    }

    async fn handle_copy_signal(&self, payload: &str) -> anyhow::Result<()> {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct CopySignal {
            wallet_address: String,
            market_id: String,
            outcome_id: String,
            action: String,
            quantity: String,
            price: String,
            #[serde(default)]
            timestamp: Option<String>,
        }

        let copy: CopySignal = serde_json::from_str(payload)?;

        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::CopyTrade,
            market_id: copy.market_id.clone(),
            outcome_id: copy.outcome_id,
            action: copy.action,
            confidence: 1.0,
            timestamp: copy
                .timestamp
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            metadata: serde_json::json!({
                "wallet_address": copy.wallet_address,
                "quantity": copy.quantity,
                "price": copy.price,
            }),
        };

        let _ = self.signal_tx.send(signal);

        info!(
            market_id = %copy.market_id,
            wallet = %copy.wallet_address,
            "Forwarded copy trade signal"
        );

        Ok(())
    }

    async fn handle_orderbook_update(&self, payload: &str) -> anyhow::Result<()> {
        #[derive(serde::Deserialize)]
        struct RawOrderbookUpdate {
            market_id: String,
            yes_bid: String,
            yes_ask: String,
            no_bid: String,
            no_ask: String,
            #[serde(default)]
            arb_spread: Option<String>,
            #[serde(default)]
            timestamp: Option<String>,
        }

        let raw: RawOrderbookUpdate = serde_json::from_str(payload)?;

        let update = OrderbookUpdate {
            market_id: raw.market_id,
            timestamp: raw
                .timestamp
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok())
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(Utc::now),
            yes_bid: raw.yes_bid.parse().unwrap_or(Decimal::ZERO),
            yes_ask: raw.yes_ask.parse().unwrap_or(Decimal::ZERO),
            no_bid: raw.no_bid.parse().unwrap_or(Decimal::ZERO),
            no_ask: raw.no_ask.parse().unwrap_or(Decimal::ZERO),
            arb_spread: raw.arb_spread.and_then(|s| s.parse().ok()),
        };

        let _ = self.orderbook_tx.send(update);

        debug!("Forwarded orderbook update");

        Ok(())
    }
}

/// Spawn the Redis forwarder as a background task.
///
/// This function starts the forwarder in a tokio task and returns immediately.
/// The forwarder will automatically reconnect on connection failures.
pub fn spawn_redis_forwarder(
    config: RedisForwarderConfig,
    signal_tx: broadcast::Sender<SignalUpdate>,
    orderbook_tx: broadcast::Sender<OrderbookUpdate>,
    arb_entry_tx: broadcast::Sender<ArbOpportunity>,
) {
    let forwarder = RedisForwarder::new(config, signal_tx, orderbook_tx, arb_entry_tx);

    tokio::spawn(async move {
        if let Err(e) = forwarder.run().await {
            error!(error = %e, "Redis forwarder failed");
        }
    });

    info!("Redis forwarder spawned as background task");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = RedisForwarderConfig::default();
        assert_eq!(config.redis_url, "redis://127.0.0.1:6379");
        assert!(config.subscribe_arb);
        assert!(config.subscribe_copy);
        assert!(config.subscribe_orderbook);
        assert_eq!(config.reconnect_delay_secs, 5);
    }

    #[test]
    fn test_arb_entry_parsing() {
        let json = r#"{
            "market_id": "test-market",
            "timestamp": "2024-01-01T00:00:00Z",
            "yes_ask": "0.48",
            "no_ask": "0.48",
            "total_cost": "0.96",
            "gross_profit": "0.04",
            "net_profit": "0.02"
        }"#;

        let arb: ArbOpportunity = serde_json::from_str(json).unwrap();
        assert_eq!(arb.market_id, "test-market");
    }

    #[test]
    fn test_copy_signal_parsing() {
        let json = r#"{
            "wallet_address": "0x123",
            "market_id": "test-market",
            "outcome_id": "yes",
            "action": "buy",
            "quantity": "100",
            "price": "0.50"
        }"#;

        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct CopySignal {
            wallet_address: String,
            market_id: String,
            outcome_id: String,
            action: String,
            quantity: String,
            price: String,
        }

        let copy: CopySignal = serde_json::from_str(json).unwrap();
        assert_eq!(copy.wallet_address, "0x123");
        assert_eq!(copy.market_id, "test-market");
    }
}
