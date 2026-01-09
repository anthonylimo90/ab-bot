//! Signal publishing for arbitrage alerts.

use anyhow::Result;
use futures_util::StreamExt;
use polymarket_core::config::AlertsConfig;
use polymarket_core::types::ArbOpportunity;
use redis::AsyncCommands;
use tracing::{debug, warn};

/// Redis channels for pub/sub.
pub mod channels {
    pub const ENTRY: &str = "arb:entry";
    pub const EXIT: &str = "arb:exit";
    pub const PRICES: &str = "arb:prices";
    pub const ALERTS: &str = "arb:alerts";
}

/// Publishes arbitrage signals to Redis and external alerting services.
pub struct SignalPublisher {
    redis: redis::aio::ConnectionManager,
    alerts_config: AlertsConfig,
    http_client: reqwest::Client,
}

impl SignalPublisher {
    /// Create a new signal publisher.
    pub async fn new(redis_client: redis::Client, alerts_config: AlertsConfig) -> Result<Self> {
        let redis = redis::aio::ConnectionManager::new(redis_client).await?;
        Ok(Self {
            redis,
            alerts_config,
            http_client: reqwest::Client::new(),
        })
    }

    /// Publish an entry signal for an arbitrage opportunity.
    pub async fn publish_entry_signal(&mut self, arb: &ArbOpportunity) -> Result<()> {
        let payload = serde_json::to_string(arb)?;

        // Publish to Redis
        let _: () = self.redis.publish(channels::ENTRY, &payload).await?;
        debug!("Published entry signal to Redis: {}", arb.market_id);

        // Send external alerts if configured
        self.send_alerts(&format!(
            "ARB ENTRY: {} | Cost: {:.4} | Profit: {:.4}",
            arb.market_id, arb.total_cost, arb.net_profit
        ))
        .await?;

        Ok(())
    }

    /// Publish an exit signal.
    pub async fn publish_exit_signal(
        &mut self,
        market_id: &str,
        position_id: &str,
        profit: rust_decimal::Decimal,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "market_id": market_id,
            "position_id": position_id,
            "profit": profit.to_string(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        let _: () = self
            .redis
            .publish(channels::EXIT, payload.to_string())
            .await?;

        self.send_alerts(&format!(
            "ARB EXIT: {} | Profit: {:.4}",
            market_id, profit
        ))
        .await?;

        Ok(())
    }

    /// Send alerts to configured external services.
    async fn send_alerts(&self, message: &str) -> Result<()> {
        // Telegram
        if let (Some(token), Some(chat_id)) = (
            &self.alerts_config.telegram_bot_token,
            &self.alerts_config.telegram_chat_id,
        ) {
            if let Err(e) = self.send_telegram(token, chat_id, message).await {
                warn!("Failed to send Telegram alert: {}", e);
            }
        }

        // Discord
        if let Some(webhook_url) = &self.alerts_config.discord_webhook_url {
            if let Err(e) = self.send_discord(webhook_url, message).await {
                warn!("Failed to send Discord alert: {}", e);
            }
        }

        Ok(())
    }

    async fn send_telegram(&self, token: &str, chat_id: &str, message: &str) -> Result<()> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", token);

        self.http_client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": message,
                "parse_mode": "HTML"
            }))
            .send()
            .await?;

        debug!("Sent Telegram alert");
        Ok(())
    }

    async fn send_discord(&self, webhook_url: &str, message: &str) -> Result<()> {
        self.http_client
            .post(webhook_url)
            .json(&serde_json::json!({
                "content": message
            }))
            .send()
            .await?;

        debug!("Sent Discord alert");
        Ok(())
    }
}

/// Subscribes to arbitrage signals from Redis.
pub struct SignalSubscriber {
    pubsub: redis::aio::PubSub,
}

impl SignalSubscriber {
    /// Create a new signal subscriber.
    pub async fn new(redis_client: redis::Client) -> Result<Self> {
        let conn = redis_client.get_async_connection().await?;
        let pubsub = conn.into_pubsub();
        Ok(Self { pubsub })
    }

    /// Subscribe to entry signals.
    pub async fn subscribe_entries(&mut self) -> Result<()> {
        self.pubsub.subscribe(channels::ENTRY).await?;
        Ok(())
    }

    /// Subscribe to exit signals.
    pub async fn subscribe_exits(&mut self) -> Result<()> {
        self.pubsub.subscribe(channels::EXIT).await?;
        Ok(())
    }

    /// Get the next message from subscribed channels.
    pub async fn next_message(&mut self) -> Option<redis::Msg> {
        self.pubsub.on_message().next().await
    }
}
