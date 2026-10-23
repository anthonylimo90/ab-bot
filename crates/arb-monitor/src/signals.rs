//! Signal publishing for arbitrage alerts.

use anyhow::Result;
use chrono::{DateTime, Utc};
use polymarket_core::config::AlertsConfig;
use polymarket_core::types::ArbOpportunity;
use redis::AsyncCommands;
use tracing::{debug, warn};

/// Redis channels for pub/sub.
pub mod channels {
    pub const ENTRY: &str = "arb:entry";
    pub const RUNTIME_STATS: &str = "arb:runtime:stats";
    pub const RUNTIME_STATS_LATEST: &str = "arb:runtime:stats:latest";
    pub const DYNAMIC_CONFIG_UPDATES: &str = "dynamic:config:update";
}

/// Lifetime of the latest runtime stats; they are republished every minute.
const RUNTIME_STATS_TTL_SECS: u64 = 300;

/// Publishes arbitrage signals to Redis and external alerting services.
pub struct SignalPublisher {
    redis: redis::aio::ConnectionManager,
    alerts: Alerter,
}

/// Alerts being sent at once; more are dropped rather than queued behind a
/// slow webhook.
const MAX_ALERTS_IN_FLIGHT: usize = 4;

/// Sends alerts to Telegram and Discord in the background, so a slow or
/// hung webhook never holds up the order book loop that raises them.
struct Alerter {
    config: AlertsConfig,
    http_client: reqwest::Client,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
}

impl Alerter {
    fn new(config: AlertsConfig) -> Result<Self> {
        Ok(Self {
            config,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()?,
            slots: std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_ALERTS_IN_FLIGHT)),
        })
    }

    /// Start sending `message` to every configured service; returns at once.
    fn send(&self, message: String) {
        let telegram = self
            .config
            .telegram_bot_token
            .clone()
            .zip(self.config.telegram_chat_id.clone());
        let discord = self.config.discord_webhook_url.clone();
        if telegram.is_none() && discord.is_none() {
            return;
        }
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            warn!("Dropping an alert: earlier alerts are still being sent");
            return;
        };
        let http = self.http_client.clone();
        tokio::spawn(async move {
            let _slot = slot;
            if let Some((token, chat_id)) = telegram {
                if let Err(e) = send_telegram(&http, &token, &chat_id, &message).await {
                    warn!("Failed to send Telegram alert: {}", e);
                }
            }
            if let Some(webhook_url) = discord {
                if let Err(e) = send_discord(&http, &webhook_url, &message).await {
                    warn!("Failed to send Discord alert: {}", e);
                }
            }
        });
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct RuntimeMarketInsight {
    pub market_id: String,
    pub tier: String,
    pub total_score: f64,
    pub baseline_score: f64,
    pub opportunity_score: f64,
    pub hit_rate_score: f64,
    pub freshness_score: f64,
    pub sticky_score: f64,
    pub novelty_score: Option<f64>,
    pub rotation_score: Option<f64>,
    pub upside_score: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct RuntimeStats {
    pub updates_per_minute: f64,
    pub stalls_last_minute: f64,
    pub resets_last_minute: f64,
    #[serde(default)]
    pub ws_text_messages_per_minute: f64,
    #[serde(default)]
    pub ws_orderbook_updates_per_minute: f64,
    #[serde(default)]
    pub ws_parse_misses_per_minute: f64,
    #[serde(default)]
    pub ws_snapshot_messages_per_minute: f64,
    #[serde(default)]
    pub ws_price_change_messages_per_minute: f64,
    pub monitored_markets: f64,
    #[serde(default)]
    pub monitored_assets: f64,
    #[serde(default)]
    pub evaluated_books_per_minute: f64,
    #[serde(default)]
    pub profitable_books_per_minute: f64,
    #[serde(default)]
    pub eligible_profitable_books_per_minute: f64,
    #[serde(default)]
    pub filtered_by_selection_per_minute: f64,
    #[serde(default)]
    pub filtered_by_profit_per_minute: f64,
    #[serde(default)]
    pub filtered_by_depth_per_minute: f64,
    #[serde(default)]
    pub filtered_by_cooldown_per_minute: f64,
    #[serde(default)]
    pub entry_signals_per_minute: f64,
    #[serde(default)]
    pub near_miss_under_1bp_per_minute: f64,
    #[serde(default)]
    pub near_miss_under_5bps_per_minute: f64,
    #[serde(default)]
    pub near_miss_under_25bps_per_minute: f64,
    #[serde(default)]
    pub near_miss_under_50bps_per_minute: f64,
    #[serde(default)]
    pub gross_positive_but_net_negative_per_minute: f64,
    #[serde(default)]
    pub best_gross_profit_bps_per_minute: f64,
    #[serde(default)]
    pub best_net_profit_bps_per_minute: f64,
    #[serde(default)]
    pub best_eligible_gross_profit_bps_per_minute: f64,
    #[serde(default)]
    pub best_eligible_net_profit_bps_per_minute: f64,
    #[serde(default)]
    pub best_fee_drag_bps_per_minute: f64,
    #[serde(default)]
    pub closest_threshold_gap_bps_per_minute: Option<f64>,
    #[serde(default)]
    pub selection_refreshes_applied_per_minute: f64,
    #[serde(default)]
    pub selection_refreshes_suppressed_per_minute: f64,
    #[serde(default)]
    pub last_selection_market_delta: f64,
    #[serde(default)]
    pub last_selection_asset_delta: f64,
    #[serde(default)]
    pub core_markets: f64,
    #[serde(default)]
    pub exploration_markets: f64,
    #[serde(default)]
    pub last_rerank_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_resubscribe_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ws_last_message_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ws_last_orderbook_update_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ws_last_parse_miss_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ws_last_parse_miss_kind: Option<String>,
    #[serde(default)]
    pub ws_last_message_kind: Option<String>,
    #[serde(default)]
    pub selected_markets: Vec<RuntimeMarketInsight>,
    /// Minimum net profit per $1 pair a signal must clear.
    #[serde(default)]
    pub min_profit_threshold: Option<f64>,
    /// Depth in USD each side's book must hold at the entry price.
    #[serde(default)]
    pub min_book_depth_usd: Option<f64>,
    /// Seconds before the same market can signal again.
    #[serde(default)]
    pub signal_cooldown_secs: Option<i64>,
}

impl SignalPublisher {
    /// Create a new signal publisher.
    pub async fn new(redis_client: redis::Client, alerts_config: AlertsConfig) -> Result<Self> {
        let redis = redis::aio::ConnectionManager::new(redis_client).await?;
        Ok(Self {
            redis,
            alerts: Alerter::new(alerts_config)?,
        })
    }

    /// Publish an entry signal for an arbitrage opportunity.
    pub async fn publish_entry_signal(
        &mut self,
        arb: &ArbOpportunity,
        observed_at: DateTime<Utc>,
    ) -> Result<()> {
        let mut entry_signal = arb.clone();
        // Freshness in the executor should reflect when we decided to trade,
        // not the upstream order book event time carried by the last leg update.
        entry_signal.timestamp = observed_at;
        let payload = serde_json::to_string(&entry_signal)?;

        // Publish to Redis
        let _: () = self.redis.publish(channels::ENTRY, &payload).await?;
        debug!(
            "Published entry signal to Redis: {}",
            entry_signal.market_id
        );

        // Send external alerts if configured
        self.alerts.send(format!(
            "ARB ENTRY: {} | Cost: {:.4} | Profit: {:.4}",
            entry_signal.market_id, entry_signal.total_cost, entry_signal.net_profit
        ));

        Ok(())
    }

    /// Publish runtime health stats so the tuner can react to stream quality.
    pub async fn publish_runtime_stats(&mut self, stats: &RuntimeStats) -> Result<()> {
        let payload = serde_json::to_string(stats)?;
        // Expires if the monitor stops, so the tuner never acts on old stats.
        let _: () = self
            .redis
            .set_ex(
                channels::RUNTIME_STATS_LATEST,
                &payload,
                RUNTIME_STATS_TTL_SECS,
            )
            .await?;
        let _: () = self.redis.publish(channels::RUNTIME_STATS, payload).await?;
        Ok(())
    }
}

async fn send_telegram(
    http: &reqwest::Client,
    token: &str,
    chat_id: &str,
    message: &str,
) -> Result<()> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    // The URL carries the bot token: errors are reported without it.
    http.post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": message,
            "parse_mode": "HTML"
        }))
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| anyhow::anyhow!("Telegram request failed: {}", error.without_url()))?;
    debug!("Sent Telegram alert");
    Ok(())
}

async fn send_discord(http: &reqwest::Client, webhook_url: &str, message: &str) -> Result<()> {
    // A webhook URL is a credential too.
    http.post(webhook_url)
        .json(&serde_json::json!({
            "content": message
        }))
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| anyhow::anyhow!("Discord request failed: {}", error.without_url()))?;
    debug!("Sent Discord alert");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn a_slow_webhook_never_holds_up_the_caller() {
        // A webhook that answers after two seconds and counts deliveries.
        let delivered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let counter = delivered.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let counter = counter.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buffer = [0u8; 4096];
                    let _ = socket.read(&mut buffer).await;
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let _ = socket
                        .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        let alerter = Alerter::new(AlertsConfig {
            telegram_bot_token: None,
            telegram_chat_id: None,
            discord_webhook_url: Some(url),
        })
        .unwrap();

        let started = std::time::Instant::now();
        for n in 0..MAX_ALERTS_IN_FLIGHT + 2 {
            alerter.send(format!("alert {n}"));
        }
        assert!(started.elapsed() < std::time::Duration::from_millis(500));

        // The ones that fit were delivered; the overflow was dropped.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_eq!(
            delivered.load(std::sync::atomic::Ordering::SeqCst),
            MAX_ALERTS_IN_FLIGHT
        );
    }
}
