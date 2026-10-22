//! Polymarket CLOB API client.
//!
//! This module provides both read-only and authenticated access to the
//! Polymarket CLOB API for order book data and order management.

use crate::signing::{OrderData, OrderSigner, SignedOrder};
use crate::types::{Market, OrderBook, Outcome, PriceLevel};
use crate::{Error, Result};
use alloy_primitives::U256;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use rust_decimal::RoundingStrategy;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration as StdDuration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

const COLLATERAL_DECIMALS: u32 = 6;
const MARKET_ORDER_SIZE_SCALE: u32 = 2;

#[derive(Debug, Clone, Default)]
pub struct WebSocketRuntimeStatsSnapshot {
    pub subscribed_assets: usize,
    pub subscriptions_started_total: u64,
    pub text_messages_received_total: u64,
    pub orderbook_updates_emitted_total: u64,
    pub parse_misses_total: u64,
    pub snapshot_messages_total: u64,
    pub price_change_messages_total: u64,
    pub invalid_operation_messages_total: u64,
    pub ping_messages_received_total: u64,
    pub pong_messages_received_total: u64,
    pub last_message_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_orderbook_update_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_parse_miss_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_parse_miss_kind: Option<String>,
    pub last_message_kind: Option<String>,
}

static WS_RUNTIME_STATS: LazyLock<Mutex<WebSocketRuntimeStatsSnapshot>> =
    LazyLock::new(|| Mutex::new(WebSocketRuntimeStatsSnapshot::default()));

pub fn websocket_runtime_stats_snapshot() -> WebSocketRuntimeStatsSnapshot {
    WS_RUNTIME_STATS
        .lock()
        .map(|stats| stats.clone())
        .unwrap_or_default()
}

fn update_ws_runtime_stats(apply: impl FnOnce(&mut WebSocketRuntimeStatsSnapshot)) {
    if let Ok(mut stats) = WS_RUNTIME_STATS.lock() {
        apply(&mut stats);
    }
}

#[derive(Debug, Clone, Copy)]
enum WsParseKind {
    ControlPing,
    ControlPong,
    InvalidOperation,
    InvalidJson,
    Snapshot,
    PriceChange,
    EmptyPriceChange,
    UnsupportedArray,
    UnsupportedObject,
    UnsupportedValue,
}

impl WsParseKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ControlPing => "control_ping",
            Self::ControlPong => "control_pong",
            Self::InvalidOperation => "invalid_operation",
            Self::InvalidJson => "invalid_json",
            Self::Snapshot => "snapshot",
            Self::PriceChange => "price_change",
            Self::EmptyPriceChange => "empty_price_change",
            Self::UnsupportedArray => "unsupported_array",
            Self::UnsupportedObject => "unsupported_object",
            Self::UnsupportedValue => "unsupported_value",
        }
    }

    fn is_parse_miss(self) -> bool {
        matches!(
            self,
            Self::InvalidOperation
                | Self::InvalidJson
                | Self::EmptyPriceChange
                | Self::UnsupportedArray
                | Self::UnsupportedObject
                | Self::UnsupportedValue
        )
    }
}

struct ParsedWsMessage {
    updates: Vec<OrderBookUpdate>,
    kind: WsParseKind,
}

#[cfg(test)]
#[path = "clob/cancellation_tests.rs"]
mod cancellation_tests;

#[cfg(test)]
#[path = "clob/book_tests.rs"]
mod book_tests;

/// Polymarket CLOB API client for order book data.
#[derive(Clone)]
pub struct ClobClient {
    base_url: String,
    ws_url: String,
    /// HTTP client for API requests.
    pub http_client: reqwest::Client,
}

impl ClobClient {
    /// Default CLOB API base URL.
    pub const DEFAULT_BASE_URL: &'static str = "https://clob.polymarket.com";
    /// Default WebSocket URL.
    pub const DEFAULT_WS_URL: &'static str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

    pub fn new(base_url: Option<String>, ws_url: Option<String>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(30))
            .connect_timeout(StdDuration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            base_url: base_url.unwrap_or_else(|| Self::DEFAULT_BASE_URL.to_string()),
            ws_url: ws_url.unwrap_or_else(|| Self::DEFAULT_WS_URL.to_string()),
            http_client,
        }
    }

    /// Submit an already signed order using credentials derived for its maker.
    /// No private key or bot trading identity is used for external wallet orders.
    pub async fn post_signed_order(
        &self,
        address: &str,
        credentials: &ApiCredentials,
        signed_order: SignedOrder,
        order_type: OrderType,
    ) -> Result<PostOrderResponse> {
        if !signed_order.maker.eq_ignore_ascii_case(address)
            || !signed_order.signer.eq_ignore_ascii_case(address)
        {
            return Err(Error::Auth {
                message: "Signed order maker must match credential owner".into(),
            });
        }
        let url = format!("{}/order", self.base_url);
        let timestamp = current_timestamp().to_string();
        let method = "POST";
        let path = "/order";

        let request = PostOrderRequest {
            order: signed_order,
            order_type,
            owner: credentials.api_key.clone(),
            defer_exec: false,
            post_only: false,
        };

        let body = serde_json::to_string(&request)?;

        debug!(payload = %body, "POST /order request body");

        // Sign the request with L2 HMAC
        let signature = sign_l2_request(credentials, method, path, &timestamp, Some(&body))?;

        let response = self
            .http_client
            .post(&url)
            .header("POLY_ADDRESS", address)
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", &timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: format!("Failed to post order: {} - {}", status, text),
                status: Some(status),
            });
        }

        let result: PostOrderResponse = response.json().await?;
        info!(order_id = %result.order_id, "Order posted successfully");

        Ok(result)
    }

    /// Exchange L1 authentication using a wallet-provided ClobAuth signature.
    pub async fn derive_external_api_key(
        &self,
        address: &str,
        timestamp: u64,
        nonce: u64,
        signature: &str,
    ) -> Result<ApiCredentials> {
        let mut response = self
            .http_client
            .get(format!("{}/auth/derive-api-key", self.base_url))
            .header("POLY_ADDRESS", address)
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_NONCE", nonce.to_string())
            .send()
            .await?;
        if matches!(response.status().as_u16(), 400 | 404) {
            response = self
                .http_client
                .post(format!("{}/auth/api-key", self.base_url))
                .header("POLY_ADDRESS", address)
                .header("POLY_SIGNATURE", signature)
                .header("POLY_TIMESTAMP", timestamp.to_string())
                .header("POLY_NONCE", nonce.to_string())
                .send()
                .await?;
        }
        if !response.status().is_success() {
            return Err(Error::Auth {
                message: "Unable to derive wallet-specific exchange credentials".into(),
            });
        }
        let derived: DeriveApiKeyResponse = response.json().await?;
        Ok(ApiCredentials::new(
            derived.api_key,
            derived.secret,
            derived.passphrase,
        ))
    }

    /// The taker fee terms the exchange charges in the market of
    /// `condition_id`: the rate `r` and exponent `e` of its CLOB market info
    /// (`fd`), free when it reports none. Terms that cannot be priced are an
    /// error, not free.
    pub async fn get_fee_schedule(&self, condition_id: &str) -> Result<crate::types::FeeSchedule> {
        let info: serde_json::Value = self
            .get_with_retry(&format!("{}/clob-markets/{condition_id}", self.base_url))
            .await?
            .json()
            .await?;
        fee_schedule_from_market_info(&info)
    }

    /// Whether `token_id` trades on the neg-risk exchange, which decides the
    /// contract its orders are signed for.
    pub async fn get_neg_risk(&self, token_id: &str) -> Result<bool> {
        let neg: serde_json::Value = self
            .get_with_retry(&format!("{}/neg-risk?token_id={token_id}", self.base_url))
            .await?
            .json()
            .await?;
        neg.get("neg_risk")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| Error::Order {
                message: "Missing exchange contract metadata".into(),
            })
    }

    /// The smallest price step `token_id` trades at: the exchange refuses an
    /// order whose price is not a whole number of steps.
    pub async fn get_tick_size(&self, token_id: &str) -> Result<Decimal> {
        let answer: serde_json::Value = self
            .get_with_retry(&format!("{}/tick-size?token_id={token_id}", self.base_url))
            .await?
            .json()
            .await?;
        tick_size_from(&answer).ok_or_else(|| Error::Order {
            message: "Missing market tick size".into(),
        })
    }

    /// Maximum retry attempts for API calls.
    const MAX_RETRIES: u32 = 3;

    /// Execute an HTTP GET with retry and exponential backoff.
    ///
    /// Retries on 5xx server errors and 429 rate-limit responses (with a longer
    /// backoff for 429). All other 4xx errors fail immediately.
    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response> {
        let mut last_error = None;

        for attempt in 0..Self::MAX_RETRIES {
            match self.http_client.get(url).send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response)
                    if response.status().as_u16() == 429 || response.status().is_server_error() =>
                {
                    let status = response.status();
                    let is_rate_limited = status.as_u16() == 429;
                    warn!(
                        attempt = attempt + 1,
                        status = %status,
                        url = url,
                        rate_limited = is_rate_limited,
                        "Retryable API error, backing off"
                    );
                    last_error = Some(Error::Api {
                        message: format!(
                            "{}: {}",
                            if is_rate_limited {
                                "Rate limited"
                            } else {
                                "Server error"
                            },
                            status
                        ),
                        status: Some(status.as_u16()),
                    });

                    // Use longer backoff for 429 to respect rate limits
                    if attempt + 1 < Self::MAX_RETRIES {
                        let backoff = if is_rate_limited {
                            // 2s, 4s, 8s for rate limits
                            StdDuration::from_millis(2000 * 2u64.pow(attempt))
                        } else {
                            // 500ms, 1s, 2s for server errors
                            StdDuration::from_millis(500 * 2u64.pow(attempt))
                        };
                        tokio::time::sleep(backoff).await;
                    }
                    continue;
                }
                Ok(response) => {
                    // Client error (4xx except 429) — don't retry
                    return Err(Error::Api {
                        message: format!("API error: {}", response.status()),
                        status: Some(response.status().as_u16()),
                    });
                }
                Err(e) => {
                    warn!(
                        attempt = attempt + 1,
                        error = %e,
                        url = url,
                        "HTTP request failed, backing off"
                    );
                    last_error = Some(Error::Http(Box::new(e)));
                }
            }

            if attempt + 1 < Self::MAX_RETRIES {
                let backoff = StdDuration::from_millis(500 * 2u64.pow(attempt));
                tokio::time::sleep(backoff).await;
            }
        }

        Err(last_error.unwrap_or(Error::Api {
            message: "Max retries exceeded".to_string(),
            status: None,
        }))
    }

    /// Fetch list of active markets.
    ///
    /// Requests only active (non-closed) markets from the CLOB API to reduce
    /// page count and ensure all active markets are included regardless of
    /// total market count.
    pub async fn get_markets(&self) -> Result<Vec<Market>> {
        let mut all_markets = Vec::new();
        let mut cursor: Option<String> = None;
        // Safety valve: configurable via CLOB_MARKET_LIMIT env var (default 200,000).
        let limit: usize = std::env::var("CLOB_MARKET_LIMIT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(200_000);
        let mut pages = 0u32;

        loop {
            let url = match &cursor {
                Some(c) => format!("{}/markets?active=true&next_cursor={}", self.base_url, c),
                None => format!("{}/markets?active=true", self.base_url),
            };

            let response = self.get_with_retry(&url).await?;

            let page: MarketsResponse = response.json().await?;
            all_markets.extend(page.data.into_iter().map(Into::into));
            pages += 1;

            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => break,
            }

            if all_markets.len() > limit {
                warn!(
                    count = all_markets.len(),
                    limit, pages, "Market pagination safety limit reached"
                );
                break;
            }
        }

        info!(
            total = all_markets.len(),
            pages, "Fetched all active markets from CLOB"
        );

        Ok(all_markets)
    }

    /// Fetch a single market by condition ID.
    ///
    /// Uses `GET /markets/{condition_id}` which returns a single `ClobMarket`
    /// instead of paginating through the full market list.
    pub async fn get_market_by_id(&self, condition_id: &str) -> Result<Market> {
        let url = format!("{}/markets/{}", self.base_url, condition_id);
        let response = self.get_with_retry(&url).await?;
        let clob_market: ClobMarket = response.json().await?;
        Ok(clob_market.into())
    }

    /// Fetch order book for a specific token.
    pub async fn get_order_book(&self, token_id: &str) -> Result<OrderBook> {
        let url = format!("{}/book?token_id={}", self.base_url, token_id);
        let response = self.get_with_retry(&url).await?;

        let book: ClobOrderBook = response.json().await?;
        Ok(book.into())
    }

    /// Fetch recent trades from the Data API.
    ///
    /// This is a public endpoint (no auth needed) that returns recent trades
    /// with wallet addresses — the primary source for wallet discovery.
    ///
    /// Note: Switched from CLOB API to Data API because CLOB /trades endpoint
    /// now requires authentication.
    /// The public Data API no longer offers reliable offset pagination for this endpoint,
    /// so callers should treat this as a latest-trades snapshot.
    pub async fn get_recent_trades(&self, limit: u32) -> Result<Vec<ClobTrade>> {
        // Use Data API instead of CLOB API - it's public and doesn't require auth
        let data_api_url = "https://data-api.polymarket.com";
        let url = format!("{}/trades?limit={}", data_api_url, limit);

        let response = self.get_with_retry(&url).await?;
        let text = response.text().await?;

        // Data API returns trades as a top-level array
        match serde_json::from_str::<Vec<ClobTrade>>(&text) {
            Ok(trades) => Ok(trades),
            Err(e) => {
                let preview = if text.len() > 500 {
                    &text[..500]
                } else {
                    &text
                };
                warn!(
                    error = %e,
                    response_preview = %preview,
                    "Could not parse Data API trades response"
                );
                Err(Error::Api {
                    message: format!("Data API response parse error: {}", e),
                    status: None,
                })
            }
        }
    }

    /// Fetch recent activity for a specific wallet from the Data API.
    ///
    /// Calls `https://data-api.polymarket.com/activity?user=ADDRESS&limit=N`
    /// and returns only entries of type `TRADE`, mapped to `ClobTrade`.
    pub async fn get_wallet_activity(
        &self,
        wallet_address: &str,
        limit: u32,
    ) -> Result<Vec<ClobTrade>> {
        let data_api_url = "https://data-api.polymarket.com";
        let url = format!(
            "{}/activity?user={}&limit={}&type=TRADE",
            data_api_url, wallet_address, limit
        );

        let response = self.get_with_retry(&url).await?;
        let text = response.text().await?;

        match serde_json::from_str::<Vec<ActivityEntry>>(&text) {
            Ok(entries) => {
                let trades: Vec<ClobTrade> = entries
                    .into_iter()
                    .filter_map(|e| e.into_clob_trade())
                    .collect();
                debug!(
                    wallet = %wallet_address,
                    trade_count = trades.len(),
                    "Fetched wallet activity"
                );
                Ok(trades)
            }
            Err(e) => {
                let preview = if text.len() > 500 {
                    &text[..500]
                } else {
                    &text
                };
                warn!(
                    error = %e,
                    wallet = %wallet_address,
                    response_preview = %preview,
                    "Could not parse wallet activity response"
                );
                Err(Error::Api {
                    message: format!("Wallet activity parse error: {}", e),
                    status: None,
                })
            }
        }
    }

    /// Fetch full trade history for a wallet using offset-based pagination.
    ///
    /// Used by the discovery/enrichment path (not the real-time monitor, which only
    /// needs recent trades). Fetches up to `max_pages` pages of `limit` trades each.
    pub async fn get_wallet_activity_paginated(
        &self,
        wallet_address: &str,
        limit: u32,
        max_pages: usize,
    ) -> Result<Vec<ClobTrade>> {
        let data_api_url = "https://data-api.polymarket.com";
        let mut all_trades = Vec::new();
        let mut offset: u64 = 0;

        for page in 0..max_pages {
            let url = format!(
                "{}/activity?user={}&limit={}&type=TRADE&offset={}",
                data_api_url, wallet_address, limit, offset
            );

            let response = self.get_with_retry(&url).await?;
            let text = response.text().await?;

            match serde_json::from_str::<Vec<ActivityEntry>>(&text) {
                Ok(entries) => {
                    let trades: Vec<ClobTrade> = entries
                        .into_iter()
                        .filter_map(|e| e.into_clob_trade())
                        .collect();
                    let count = trades.len();
                    all_trades.extend(trades);

                    if count < limit as usize {
                        // Last page
                        break;
                    }
                    offset += limit as u64;
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        wallet = %wallet_address,
                        page,
                        "Could not parse wallet activity page"
                    );
                    break;
                }
            }
        }

        debug!(
            wallet = %wallet_address,
            total_trades = all_trades.len(),
            "Fetched paginated wallet activity"
        );
        Ok(all_trades)
    }

    /// Subscribe to real-time order book updates via WebSocket.
    ///
    /// Expects token IDs (`asset_id`) for the market channel subscription.
    /// Returns a channel receiver that yields normalized order book updates and
    /// automatically reconnects with exponential backoff on disconnection.
    pub async fn subscribe_orderbook(
        &self,
        asset_ids: Vec<String>,
    ) -> Result<mpsc::Receiver<OrderBookUpdate>> {
        let (tx, rx) = mpsc::channel(1000);
        let ws_url = self.ws_url.clone();

        tokio::spawn(async move {
            Self::ws_loop_with_reconnect(ws_url, asset_ids, tx).await;
        });

        Ok(rx)
    }

    /// WebSocket loop with automatic reconnection and exponential backoff.
    async fn ws_loop_with_reconnect(
        ws_url: String,
        asset_ids: Vec<String>,
        tx: mpsc::Sender<OrderBookUpdate>,
    ) {
        let mut attempt = 0u32;
        let max_backoff_secs = 60u64;
        let base_delay_secs = 1u64;

        // A session that stayed up this long was healthy: the next reconnect
        // starts from the base delay instead of the accumulated backoff.
        const HEALTHY_SESSION: StdDuration = StdDuration::from_secs(60);

        loop {
            let session_started = std::time::Instant::now();
            match Self::ws_loop(&ws_url, &asset_ids, &tx).await {
                Ok(()) => {
                    info!("WebSocket connection closed cleanly");
                }
                Err(e) => {
                    warn!(attempt = attempt + 1, error = %e, "WebSocket connection failed");
                }
            }
            if session_started.elapsed() >= HEALTHY_SESSION {
                attempt = 0;
            }

            // Check if the receiver has been dropped (no one listening)
            if tx.is_closed() {
                info!("WebSocket receiver dropped, stopping reconnection");
                return;
            }

            // Exponential backoff: 1s, 2s, 4s, 8s, ... up to max_backoff_secs
            let delay_secs = std::cmp::min(
                base_delay_secs.saturating_mul(2u64.saturating_pow(attempt)),
                max_backoff_secs,
            );
            warn!(
                delay_secs = delay_secs,
                attempt = attempt + 1,
                "Reconnecting WebSocket in {}s",
                delay_secs
            );
            tokio::time::sleep(StdDuration::from_secs(delay_secs)).await;

            attempt = attempt.saturating_add(1);
        }
    }

    async fn ws_loop(
        ws_url: &str,
        asset_ids: &[String],
        tx: &mpsc::Sender<OrderBookUpdate>,
    ) -> Result<()> {
        if asset_ids.is_empty() {
            return Err(Error::Config {
                message: "Cannot subscribe to orderbook stream with empty asset list".to_string(),
            });
        }

        let (ws_stream, _) = connect_async(ws_url).await?;
        let (mut write, mut read) = ws_stream.split();
        let read_timeout_secs = std::env::var("CLOB_WS_READ_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120_u64);
        let ping_interval_secs = std::env::var("CLOB_WS_PING_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10_u64);
        let mut ping_tick = tokio::time::interval(StdDuration::from_secs(ping_interval_secs));
        // After an overrun, wait a full period instead of catching up.
        ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping_tick.tick().await;
        let mut order_books_by_asset: HashMap<String, OrderBook> = HashMap::new();

        // Subscribe to market channel by token IDs.
        let subscribe_msg = serde_json::json!({
            "type": "market",
            "assets_ids": asset_ids,
            "custom_feature_enabled": false
        });
        write.send(Message::Text(subscribe_msg.to_string())).await?;
        update_ws_runtime_stats(|stats| {
            stats.subscribed_assets = asset_ids.len();
            stats.subscriptions_started_total = stats.subscriptions_started_total.saturating_add(1);
            stats.last_message_kind = Some("subscription_started".to_string());
        });
        info!("Subscribed to {} assets via WebSocket", asset_ids.len());

        // Use a persistent deadline that only resets when data is actually received.
        // This prevents the ping ticker from inadvertently resetting the read timeout
        // every 10s (which made the 120s timeout effectively infinite when the server
        // stopped sending data).
        let read_deadline = tokio::time::sleep(StdDuration::from_secs(read_timeout_secs));
        tokio::pin!(read_deadline);

        // Process incoming messages
        loop {
            tokio::select! {
                _ = ping_tick.tick() => {
                    write.send(Message::Text("PING".to_string())).await?;
                }
                _ = &mut read_deadline => {
                    // Read timeout actually fired — no data for read_timeout_secs
                    warn!(
                        timeout_secs = read_timeout_secs,
                        "WebSocket read timed out without messages"
                    );
                    return Err(Error::Api {
                        message: format!(
                            "WebSocket read timed out after {}s without messages",
                            read_timeout_secs
                        ),
                        status: None,
                    });
                }
                msg = read.next() => {
                    // Reset deadline on any received frame
                    read_deadline.as_mut().reset(tokio::time::Instant::now() + StdDuration::from_secs(read_timeout_secs));

                    let msg = match msg {
                        Some(msg) => msg,
                        None => {
                            warn!("WebSocket stream ended");
                            return Ok(());
                        }
                    };

                    match msg {
                        Ok(Message::Text(text)) => {
                            let parsed = parse_ws_message(&text, &mut order_books_by_asset);
                            let now = chrono::Utc::now();
                            update_ws_runtime_stats(|stats| {
                                stats.text_messages_received_total =
                                    stats.text_messages_received_total.saturating_add(1);
                                stats.last_message_at = Some(now);
                                stats.last_message_kind =
                                    Some(parsed.kind.as_str().to_string());
                                match parsed.kind {
                                    WsParseKind::Snapshot => {
                                        stats.snapshot_messages_total =
                                            stats.snapshot_messages_total.saturating_add(1);
                                    }
                                    WsParseKind::PriceChange => {
                                        stats.price_change_messages_total =
                                            stats.price_change_messages_total.saturating_add(1);
                                    }
                                    WsParseKind::InvalidOperation => {
                                        stats.invalid_operation_messages_total =
                                            stats.invalid_operation_messages_total.saturating_add(1);
                                    }
                                    WsParseKind::ControlPing => {
                                        stats.ping_messages_received_total =
                                            stats.ping_messages_received_total.saturating_add(1);
                                    }
                                    WsParseKind::ControlPong => {
                                        stats.pong_messages_received_total =
                                            stats.pong_messages_received_total.saturating_add(1);
                                    }
                                    _ => {}
                                }
                                if parsed.kind.is_parse_miss() {
                                    stats.parse_misses_total =
                                        stats.parse_misses_total.saturating_add(1);
                                    stats.last_parse_miss_at = Some(now);
                                    stats.last_parse_miss_kind =
                                        Some(parsed.kind.as_str().to_string());
                                }
                                if !parsed.updates.is_empty() {
                                    stats.orderbook_updates_emitted_total = stats
                                        .orderbook_updates_emitted_total
                                        .saturating_add(parsed.updates.len() as u64);
                                    stats.last_orderbook_update_at = Some(now);
                                }
                            });

                            for update in parsed.updates {
                                if tx.send(update).await.is_err() {
                                    warn!("Receiver dropped, closing WebSocket");
                                    return Ok(());
                                }
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            let now = chrono::Utc::now();
                            update_ws_runtime_stats(|stats| {
                                stats.ping_messages_received_total =
                                    stats.ping_messages_received_total.saturating_add(1);
                                stats.last_message_at = Some(now);
                                stats.last_message_kind = Some("ping_frame".to_string());
                            });
                            write.send(Message::Pong(data)).await?;
                        }
                        Ok(Message::Pong(_)) => {
                            let now = chrono::Utc::now();
                            update_ws_runtime_stats(|stats| {
                                stats.pong_messages_received_total =
                                    stats.pong_messages_received_total.saturating_add(1);
                                stats.last_message_at = Some(now);
                                stats.last_message_kind = Some("pong_frame".to_string());
                            });
                            debug!("Received websocket pong");
                        }
                        Ok(Message::Close(_)) => {
                            update_ws_runtime_stats(|stats| {
                                stats.last_message_at = Some(chrono::Utc::now());
                                stats.last_message_kind = Some("close_frame".to_string());
                            });
                            info!("WebSocket closed by server");
                            return Ok(());
                        }
                        Err(e) => {
                            warn!("WebSocket receive error: {}", e);
                            return Err(e.into());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// A trade from the Data API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobTrade {
    /// Transaction hash (unique identifier).
    #[serde(alias = "transactionHash", alias = "id")]
    pub transaction_hash: String,
    /// Wallet address (proxy wallet in Data API).
    #[serde(alias = "proxyWallet", alias = "maker_address")]
    pub wallet_address: String,
    /// Trade side (BUY/SELL).
    pub side: String,
    /// Asset/token ID.
    #[serde(alias = "asset")]
    pub asset_id: String,
    /// Market condition ID.
    #[serde(alias = "conditionId", default)]
    pub condition_id: Option<String>,
    /// Trade size (quantity).
    pub size: f64,
    /// Trade price.
    pub price: f64,
    /// Unix timestamp.
    pub timestamp: i64,
    /// Market title.
    #[serde(default)]
    pub title: Option<String>,
    /// Market slug.
    #[serde(default)]
    pub slug: Option<String>,
    /// Outcome name.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// A single entry from the Data API `/activity` endpoint.
///
/// Field names differ from the `/trades` endpoint (e.g. `usdcSize` instead of
/// `size`, plus a `type` discriminator).
#[derive(Debug, Deserialize)]
struct ActivityEntry {
    /// Transaction hash.
    #[serde(alias = "transactionHash", default)]
    transaction_hash: Option<String>,
    /// Unique identifier (fallback when transaction_hash is absent).
    #[serde(default)]
    id: Option<String>,
    /// Proxy wallet address.
    #[serde(alias = "proxyWallet", default)]
    proxy_wallet: Option<String>,
    /// Trade side (BUY / SELL).
    #[serde(default)]
    side: Option<String>,
    /// Asset / token ID.
    #[serde(default)]
    asset: Option<String>,
    /// Condition (market) ID.
    #[serde(alias = "conditionId", default)]
    condition_id: Option<String>,
    /// USDC size of the trade.
    #[serde(alias = "usdcSize", default)]
    usdc_size: Option<f64>,
    /// Quantity / shares.
    #[serde(default)]
    size: Option<f64>,
    /// Trade price.
    #[serde(default)]
    price: Option<f64>,
    /// Unix timestamp (seconds).
    #[serde(default)]
    timestamp: Option<i64>,
    /// ISO-8601 timestamp (fallback).
    #[serde(alias = "createdAt", default)]
    created_at: Option<String>,
    /// Activity type — we only care about "TRADE".
    #[serde(alias = "type", default)]
    activity_type: Option<String>,
    /// Market title.
    #[serde(default)]
    title: Option<String>,
    /// Market slug.
    #[serde(default)]
    slug: Option<String>,
    /// Outcome name.
    #[serde(default)]
    outcome: Option<String>,
}

impl ActivityEntry {
    /// Convert to a `ClobTrade`, returning `None` for non-TRADE entries or
    /// entries missing required fields.
    fn into_clob_trade(self) -> Option<ClobTrade> {
        // Only process TRADE activity types
        let activity_type = self.activity_type.unwrap_or_default();
        if !activity_type.eq_ignore_ascii_case("TRADE") {
            return None;
        }

        let transaction_hash = self
            .transaction_hash
            .or(self.id)
            .filter(|s| !s.is_empty())?;
        let wallet_address = self.proxy_wallet.filter(|s| !s.is_empty())?;
        let side = self.side.filter(|s| !s.is_empty())?;
        let asset_id = self.asset.filter(|s| !s.is_empty())?;
        let price = self.price.unwrap_or(0.0);

        // usdcSize is a dollar amount — divide by price to get share quantity.
        // size is already in shares — use directly.
        let computed_size = if let Some(usdc) = self.usdc_size {
            if price > 0.0 {
                usdc / price
            } else {
                0.0
            }
        } else {
            self.size.unwrap_or(0.0)
        };

        // Parse timestamp: prefer unix seconds, fall back to ISO-8601
        let timestamp = self.timestamp.unwrap_or_else(|| {
            self.created_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp())
                .unwrap_or(0)
        });

        Some(ClobTrade {
            transaction_hash,
            wallet_address,
            side,
            asset_id,
            condition_id: self.condition_id,
            size: computed_size,
            price,
            timestamp,
            title: self.title,
            slug: self.slug,
            outcome: self.outcome,
        })
    }
}

/// Order book update from WebSocket.
#[derive(Debug, Clone)]
pub struct OrderBookUpdate {
    pub market_id: String,
    pub asset_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

// Internal API response types

#[derive(Debug, Deserialize)]
struct MarketsResponse {
    data: Vec<ClobMarket>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClobMarket {
    condition_id: String,
    question: String,
    description: Option<String>,
    tokens: Vec<ClobToken>,
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    liquidity: Option<String>,
    #[serde(alias = "end_date_iso")]
    end_date: Option<String>,
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default, alias = "accepting_orders")]
    accepting_orders: bool,
    #[serde(default, alias = "enable_order_book")]
    enable_order_book: bool,
    #[serde(default)]
    active: bool,
}

#[derive(Debug, Deserialize)]
struct ClobToken {
    token_id: String,
    outcome: String,
    #[serde(default)]
    price: Option<f64>,
    #[serde(default)]
    winner: Option<bool>,
}

impl From<ClobMarket> for Market {
    fn from(m: ClobMarket) -> Self {
        Market {
            id: m.condition_id,
            question: m.question,
            description: m.description,
            outcomes: m
                .tokens
                .into_iter()
                .map(|t| Outcome {
                    id: t.token_id.clone(),
                    name: t.outcome,
                    token_id: t.token_id,
                    price: t.price.and_then(Decimal::from_f64_retain),
                    winner: t.winner,
                })
                .collect(),
            volume: m.volume.and_then(|v| v.parse().ok()).unwrap_or_default(),
            liquidity: m.liquidity.and_then(|v| v.parse().ok()).unwrap_or_default(),
            end_date: m.end_date.and_then(|s| s.parse().ok()),
            // CLOB `active=true` still returns many closed historical markets.
            // Treat explicit `closed` / `archived` as authoritative for liveness.
            resolved: m.closed
                || m.archived
                || (!m.accepting_orders && !m.enable_order_book && !m.active),
            resolution: None,
            category: None,
            tags: Vec::new(),
            fees_enabled: false,
            fee_type: None,
            fee_schedule: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ClobOrderBook {
    market: String,
    asset_id: String,
    bids: Vec<ClobPriceLevel>,
    asks: Vec<ClobPriceLevel>,
    timestamp: String,
}

#[derive(Debug, Deserialize)]
struct ClobPriceLevel {
    price: String,
    size: String,
}

impl From<ClobOrderBook> for OrderBook {
    fn from(b: ClobOrderBook) -> Self {
        OrderBook {
            market_id: b.market,
            outcome_id: b.asset_id,
            timestamp: b.timestamp.parse().unwrap_or_else(|_| chrono::Utc::now()),
            bids: parse_book_levels(b.bids, true),
            asks: parse_book_levels(b.asks, false),
        }
    }
}

/// Converts exchange levels into a validated, best-first side of the book.
/// The exchange lists levels worst-first, and a malformed level must never
/// default to a zero price that would then sort as the best ask.
fn parse_book_levels(levels: Vec<ClobPriceLevel>, bids: bool) -> Vec<PriceLevel> {
    let mut parsed: Vec<PriceLevel> = levels
        .into_iter()
        .filter_map(|l| PriceLevel::parse_exchange(&l.price, &l.size))
        .collect();
    if bids {
        crate::types::sort_bids_best_first(&mut parsed);
    } else {
        crate::types::sort_asks_best_first(&mut parsed);
    }
    parsed
}

#[derive(Debug, Deserialize)]
struct WsBook {
    market: String,
    asset_id: String,
    #[serde(default, alias = "buys")]
    bids: Vec<ClobPriceLevel>,
    #[serde(default, alias = "sells")]
    asks: Vec<ClobPriceLevel>,
    timestamp: String,
}

#[derive(Debug, Deserialize)]
struct WsPriceChangeEvent {
    market: String,
    #[serde(default)]
    price_changes: Vec<WsPriceChange>,
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsPriceChange {
    asset_id: String,
    price: String,
    size: String,
    side: String,
}

fn parse_ws_message(text: &str, book_state: &mut HashMap<String, OrderBook>) -> ParsedWsMessage {
    let trimmed = text.trim();

    if trimmed.eq_ignore_ascii_case("PONG") || trimmed.eq_ignore_ascii_case("PING") {
        return ParsedWsMessage {
            updates: Vec::new(),
            kind: if trimmed.eq_ignore_ascii_case("PING") {
                WsParseKind::ControlPing
            } else {
                WsParseKind::ControlPong
            },
        };
    }
    if trimmed.eq_ignore_ascii_case("INVALID OPERATION") {
        warn!("Received INVALID OPERATION from CLOB websocket");
        return ParsedWsMessage {
            updates: Vec::new(),
            kind: WsParseKind::InvalidOperation,
        };
    }

    let value = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => value,
        Err(e) => {
            debug!(
                "Failed to parse websocket JSON message: {} - {}",
                e, trimmed
            );
            return ParsedWsMessage {
                updates: Vec::new(),
                kind: WsParseKind::InvalidJson,
            };
        }
    };

    match value {
        serde_json::Value::Array(items) => {
            let updates: Vec<OrderBookUpdate> = items
                .into_iter()
                .filter_map(parse_ws_book_from_value)
                .inspect(|update| {
                    book_state.insert(update.asset_id.clone(), update_to_orderbook(update));
                })
                .collect();
            let kind = if updates.is_empty() {
                WsParseKind::UnsupportedArray
            } else {
                WsParseKind::Snapshot
            };
            ParsedWsMessage { updates, kind }
        }
        serde_json::Value::Object(_) => {
            if let Some(update) = parse_ws_book_from_value(value.clone()) {
                book_state.insert(update.asset_id.clone(), update_to_orderbook(&update));
                return ParsedWsMessage {
                    updates: vec![update],
                    kind: WsParseKind::Snapshot,
                };
            }

            if let Ok(event) = serde_json::from_value::<WsPriceChangeEvent>(value) {
                let updates = apply_price_changes(event, book_state);
                let kind = if updates.is_empty() {
                    WsParseKind::EmptyPriceChange
                } else {
                    WsParseKind::PriceChange
                };
                return ParsedWsMessage { updates, kind };
            }

            ParsedWsMessage {
                updates: Vec::new(),
                kind: WsParseKind::UnsupportedObject,
            }
        }
        _ => ParsedWsMessage {
            updates: Vec::new(),
            kind: WsParseKind::UnsupportedValue,
        },
    }
}

fn parse_ws_book_from_value(value: serde_json::Value) -> Option<OrderBookUpdate> {
    let ws_book = serde_json::from_value::<WsBook>(value).ok()?;

    Some(OrderBookUpdate {
        market_id: ws_book.market,
        asset_id: ws_book.asset_id,
        timestamp: parse_ws_timestamp(&ws_book.timestamp),
        bids: parse_book_levels(ws_book.bids, true),
        asks: parse_book_levels(ws_book.asks, false),
    })
}

fn apply_price_changes(
    event: WsPriceChangeEvent,
    book_state: &mut HashMap<String, OrderBook>,
) -> Vec<OrderBookUpdate> {
    let timestamp = event
        .timestamp
        .as_deref()
        .map(parse_ws_timestamp)
        .unwrap_or_else(chrono::Utc::now);

    let mut updates = Vec::new();
    for change in event.price_changes {
        let price = match change.price.parse::<Decimal>() {
            Ok(price) => price,
            Err(_) => continue,
        };
        let size = match change.size.parse::<Decimal>() {
            Ok(size) => size,
            Err(_) => continue,
        };

        // A delta without its snapshot would publish a partial book whose
        // first level need not be the real best price; wait for the snapshot.
        let Some(mut book) = book_state.remove(&change.asset_id) else {
            debug!(asset_id = %change.asset_id, "Ignoring price change before book snapshot");
            continue;
        };

        book.market_id = event.market.clone();
        book.timestamp = timestamp;

        if change.side.eq_ignore_ascii_case("BUY") {
            upsert_level(&mut book.bids, price, size, true);
        } else if change.side.eq_ignore_ascii_case("SELL") {
            upsert_level(&mut book.asks, price, size, false);
        } else {
            book_state.insert(change.asset_id, book);
            continue;
        }

        let update = OrderBookUpdate {
            market_id: book.market_id.clone(),
            asset_id: book.outcome_id.clone(),
            timestamp: book.timestamp,
            bids: book.bids.clone(),
            asks: book.asks.clone(),
        };
        book_state.insert(change.asset_id, book);
        updates.push(update);
    }

    updates
}

fn parse_ws_timestamp(raw: &str) -> chrono::DateTime<chrono::Utc> {
    if let Ok(ms) = raw.parse::<i64>() {
        if let Some(ts) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms) {
            return ts;
        }
    }
    raw.parse().unwrap_or_else(|_| chrono::Utc::now())
}

fn update_to_orderbook(update: &OrderBookUpdate) -> OrderBook {
    OrderBook {
        market_id: update.market_id.clone(),
        outcome_id: update.asset_id.clone(),
        timestamp: update.timestamp,
        bids: update.bids.clone(),
        asks: update.asks.clone(),
    }
}

fn upsert_level(levels: &mut Vec<PriceLevel>, price: Decimal, size: Decimal, descending: bool) {
    if let Some(idx) = levels.iter().position(|l| l.price == price) {
        if size <= Decimal::ZERO {
            levels.remove(idx);
        } else {
            levels[idx].size = size;
        }
    } else if size > Decimal::ZERO && price > Decimal::ZERO && price < Decimal::ONE {
        levels.push(PriceLevel { price, size });
    }

    if descending {
        crate::types::sort_bids_best_first(levels);
    } else {
        crate::types::sort_asks_best_first(levels);
    }
}

// ============================================================================
// Authenticated CLOB Client
// ============================================================================

/// API credentials for authenticated CLOB requests.
#[derive(Clone)]
pub struct ApiCredentials {
    /// API key (derived from wallet).
    pub api_key: String,
    /// API secret for HMAC signing.
    pub api_secret: String,
    /// Passphrase for additional security.
    pub api_passphrase: String,
}

impl std::fmt::Debug for ApiCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiCredentials")
            .field("api_key", &"[REDACTED]")
            .field("api_secret", &"[REDACTED]")
            .field("api_passphrase", &"[REDACTED]")
            .finish()
    }
}

impl ApiCredentials {
    /// Create new API credentials.
    pub fn new(api_key: String, api_secret: String, api_passphrase: String) -> Self {
        Self {
            api_key,
            api_secret,
            api_passphrase,
        }
    }

    /// Load from environment variables.
    #[allow(clippy::result_large_err)]
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("POLY_API_KEY").map_err(|_| Error::Config {
            message: "POLY_API_KEY environment variable not set".to_string(),
        })?;
        let api_secret = std::env::var("POLY_API_SECRET").map_err(|_| Error::Config {
            message: "POLY_API_SECRET environment variable not set".to_string(),
        })?;
        let api_passphrase = std::env::var("POLY_API_PASSPHRASE").map_err(|_| Error::Config {
            message: "POLY_API_PASSPHRASE environment variable not set".to_string(),
        })?;

        Ok(Self {
            api_key,
            api_secret,
            api_passphrase,
        })
    }
}

/// Order type for submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderType {
    /// Good-till-cancelled limit order.
    #[default]
    Gtc,
    /// Fill-or-kill market order.
    Fok,
    /// Good-till-date limit order.
    Gtd,
}

/// Request body for posting an order, as the official V2 clients send it.
#[derive(Debug, Clone, Serialize)]
pub struct PostOrderRequest {
    pub order: SignedOrder,
    /// API key of the order owner.
    pub owner: String,
    #[serde(rename = "orderType")]
    pub order_type: OrderType,
    /// Whether matching may be deferred (it is not: the response reports fills).
    #[serde(rename = "deferExec")]
    pub defer_exec: bool,
    /// Whether the order must rest on the book rather than take liquidity.
    #[serde(rename = "postOnly")]
    pub post_only: bool,
}

/// Response from the balance-allowance endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceAllowanceResponse {
    /// Balance in base units (as string).
    #[serde(default)]
    pub balance: String,
    /// Allowance in base units (as string), as V1 reported it.
    #[serde(default)]
    pub allowance: String,
    /// Allowance per spender contract in base units, as V2 reports it.
    #[serde(default)]
    pub allowances: HashMap<String, String>,
}

impl BalanceAllowanceResponse {
    /// CLOB collateral and conditional tokens both use six decimal places.
    pub fn balance_in_units(&self) -> Option<Decimal> {
        base_units_to_decimal(&self.balance)
    }

    /// The allowance an order can count on: the least any spender has
    /// (every one of them is approved for trading), unknown when any is
    /// unreadable. An unlimited approval (2^256 − 1 base units) is larger
    /// than a `Decimal` holds; it reads as `Decimal::MAX`, not as unknown.
    pub fn allowance_in_units(&self) -> Option<Decimal> {
        let units = |raw: &str| {
            let raw = raw.trim();
            base_units_to_decimal(raw).or_else(|| {
                (!raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit())).then_some(Decimal::MAX)
            })
        };
        if self.allowances.is_empty() {
            return units(&self.allowance);
        }
        self.allowances
            .values()
            .map(|raw| units(raw))
            .try_fold(Decimal::MAX, |least, value| Some(least.min(value?)))
    }
}

#[cfg(test)]
mod balance_unit_tests {
    use super::*;

    #[test]
    fn balances_are_six_decimal_base_units_and_invalid_is_not_zero() {
        let response = BalanceAllowanceResponse {
            balance: "1250000".into(),
            allowance: "2000000".into(),
            allowances: HashMap::new(),
        };
        assert_eq!(response.balance_in_units(), Some(Decimal::new(125, 2)));
        assert_eq!(response.allowance_in_units(), Some(Decimal::from(2)));
        assert_eq!(base_units_to_decimal("0"), Some(Decimal::ZERO));
        for invalid in ["", "unavailable", "-1", "1.5"] {
            assert_eq!(base_units_to_decimal(invalid), None);
        }
        let allowance = |allowance: &str| BalanceAllowanceResponse {
            balance: "0".into(),
            allowance: allowance.into(),
            allowances: HashMap::new(),
        };
        // An unlimited approval is known, not unreadable: no refresh per order.
        let unlimited =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(
            allowance(unlimited).allowance_in_units(),
            Some(Decimal::MAX)
        );
        // A blank allowance is unknown: the preflight refreshes it.
        assert_eq!(allowance("").allowance_in_units(), None);
        assert_eq!(allowance("1.5").allowance_in_units(), None);

        // V2 reports an allowance per spender: an order counts on the least.
        let v2: BalanceAllowanceResponse = serde_json::from_value(serde_json::json!({
            "balance": "5000000",
            "allowances": {
                "0xE111180000d2663C0091e4f400237545B87B996B": unlimited,
                "0xe2222d279d744050d28e00520010520000310F59": "3000000",
            }
        }))
        .unwrap();
        assert_eq!(v2.allowance_in_units(), Some(Decimal::from(3)));
        let unreadable: BalanceAllowanceResponse = serde_json::from_value(serde_json::json!({
            "balance": "0",
            "allowances": {"0xa": unlimited, "0xb": "n/a"}
        }))
        .unwrap();
        assert_eq!(unreadable.allowance_in_units(), None);
    }
}

fn base_units_to_decimal(raw: &str) -> Option<Decimal> {
    let value: Decimal = raw.trim().parse().ok()?;
    (value >= Decimal::ZERO && value.fract().is_zero()).then(|| value / Decimal::from(1_000_000))
}

/// Response from posting an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostOrderResponse {
    /// Order ID assigned by the CLOB (empty when it took no order).
    #[serde(default, rename = "orderID", deserialize_with = "null_as_default")]
    pub order_id: String,
    /// Status of the order: "matched", "live" (resting) or "delayed" (in a
    /// matching delay). Anything else proves nothing: the order is
    /// reconciled from the exchange.
    #[serde(default, deserialize_with = "null_as_default")]
    pub status: String,
    #[serde(default, rename = "makingAmount")]
    pub making_amount: Option<String>,
    #[serde(default, rename = "takingAmount")]
    pub taking_amount: Option<String>,
    /// Transaction hash if applicable (V1).
    #[serde(default, rename = "transactionHash")]
    pub transaction_hash: Option<String>,
    /// Settlement transaction hashes of the order's fills, best effort (V2).
    #[serde(
        default,
        rename = "transactionsHashes",
        skip_serializing_if = "Option::is_none"
    )]
    pub transactions_hashes: Option<Vec<String>>,
    /// Whether the exchange took the order (V2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// Why the exchange did not take it (V2).
    #[serde(default, rename = "errorMsg", skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
}

impl PostOrderResponse {
    /// A settlement transaction of the order's fills, when one is known.
    pub fn settlement_tx_hash(&self) -> Option<String> {
        self.transaction_hash
            .clone()
            .filter(|hash| !hash.is_empty())
            .or_else(|| self.transactions_hashes.as_ref()?.first().cloned())
    }

    /// Whether the exchange took no order: it said so (`success: false`),
    /// named none, and gave no status saying one may have been placed. Only
    /// this answer proves nothing can fill.
    pub fn refused(&self) -> bool {
        self.success == Some(false)
            && self.order_id.is_empty()
            && !matches!(
                self.status.to_lowercase().as_str(),
                "matched" | "live" | "delayed" | "unmatched"
            )
    }

    /// Whether the answer leaves open if an order exists: it is neither a
    /// refusal nor an acceptance (`success` not false, and the order
    /// matched, resting or in a matching delay). A missing status and one
    /// that contradicts `success` are read from the exchange instead, and so
    /// is `unmatched` ("marketable but failed to delay. Placement still
    /// succeeded", Polymarket says of limit orders): where it rests, and how
    /// much of it filled, is read from the exchange's record.
    pub fn unclear(&self) -> bool {
        let accepted = self.success != Some(false)
            && matches!(
                self.status.to_lowercase().as_str(),
                "matched" | "live" | "delayed"
            );
        !accepted && !self.refused()
    }
}

/// The `minimum_tick_size` of a tick-size answer, sent as a number or a
/// string: a step strictly between 0 and 1.
pub fn tick_size_from(answer: &serde_json::Value) -> Option<Decimal> {
    let tick = match answer.get("minimum_tick_size")? {
        serde_json::Value::String(text) => text.parse::<Decimal>().ok()?,
        serde_json::Value::Number(number) => number.to_string().parse::<Decimal>().ok()?,
        _ => return None,
    };
    (tick > Decimal::ZERO && tick < Decimal::ONE).then(|| tick.normalize())
}

/// Whether `price` is a whole number of `tick` steps, from one step to one
/// step short of a dollar.
pub fn price_on_tick(price: Decimal, tick: Decimal) -> bool {
    tick > Decimal::ZERO
        && price >= tick
        && price <= Decimal::ONE - tick
        && (price % tick).is_zero()
}

/// A string field the exchange may send as `null`: read as empty.
fn null_as_default<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// Confirmed exchange snapshot. Limit price is not an average execution price.
#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeOrder {
    #[serde(default)]
    pub associate_trades: Vec<String>,
    pub id: String,
    pub status: String,
    pub original_size: String,
    pub size_matched: String,
}

impl ExchangeOrder {
    /// The order's status, lowercase and without the `ORDER_STATUS_` prefix
    /// some answers carry: `live`, `matched`, `canceled`,
    /// `canceled_market_resolved` or `invalid`.
    pub fn state(&self) -> String {
        let status = self.status.to_ascii_lowercase();
        match status.strip_prefix("order_status_") {
            Some(state) => state.to_string(),
            None => status,
        }
    }

    /// Whether the exchange will match no more of the order: cancelled (by
    /// its owner, or with its unfilled rest when the market resolved),
    /// invalid or expired. A fully matched order is told by its sizes.
    pub fn is_closed(&self) -> bool {
        matches!(
            self.state().as_str(),
            "canceled"
                | "cancelled"
                | "canceled_market_resolved"
                | "invalid"
                | "expired"
                | "rejected"
        )
    }
}

/// Open order information.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenOrder {
    pub id: String,
    pub asset_id: String,
    pub market: String,
    pub side: String,
    pub price: String,
    pub size: String,
    pub status: String,
    pub created_at: Option<String>,
}

/// Response from deriving API credentials.
#[derive(Debug, Clone, Deserialize)]
pub struct DeriveApiKeyResponse {
    #[serde(rename = "apiKey")]
    pub api_key: String,
    pub secret: String,
    pub passphrase: String,
}

/// Authenticated CLOB client for order management.
///
/// Wraps `ClobClient` with signing capabilities for authenticated requests.
pub struct AuthenticatedClobClient {
    /// Base read-only client.
    pub client: ClobClient,
    /// Order signer for standard CTF Exchange markets.
    signer: OrderSigner,
    /// Order signer for neg-risk CTF Exchange markets.
    neg_risk_signer: OrderSigner,
    /// API credentials for L2 authentication (optional until derived).
    credentials: Option<ApiCredentials>,
    /// Cache for is_neg_risk lookups (token_id -> bool). Rarely changes per token.
    neg_risk_cache: Mutex<HashMap<String, bool>>,
}

impl AuthenticatedClobClient {
    /// Create a new authenticated client.
    pub fn new(client: ClobClient, signer: OrderSigner) -> Self {
        let neg_risk_signer = signer.to_neg_risk();
        Self {
            client,
            signer,
            neg_risk_signer,
            credentials: None,
            neg_risk_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Create with pre-existing API credentials.
    pub fn with_credentials(
        client: ClobClient,
        signer: OrderSigner,
        credentials: ApiCredentials,
    ) -> Self {
        let neg_risk_signer = signer.to_neg_risk();
        Self {
            client,
            signer,
            neg_risk_signer,
            credentials: Some(credentials),
            neg_risk_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Get the wallet address (EIP-55 checksummed).
    pub fn address(&self) -> String {
        format!("{}", self.signer.address())
    }

    /// Derive API credentials from wallet signature (L1 authentication).
    ///
    /// This authenticates with Polymarket using EIP-712 signed ClobAuth
    /// and returns API credentials for subsequent requests.
    pub async fn derive_api_key(&mut self) -> Result<ApiCredentials> {
        let timestamp = current_timestamp();
        let nonce: u64 = 0;

        // Sign the CLOB auth message using EIP-712 typed data
        let signature = self
            .signer
            .sign_clob_auth_message(timestamp, nonce)
            .await
            .map_err(|e| Error::Signing {
                message: format!("Failed to sign CLOB auth message: {}", e),
            })?;

        let url = format!("{}/auth/derive-api-key", self.client.base_url);

        let response = self
            .client
            .http_client
            .get(&url)
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", &signature)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_NONCE", nonce.to_string())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: format!("Failed to derive API key: {} - {}", status, text),
                status: Some(status),
            });
        }

        let derive_response: DeriveApiKeyResponse = response.json().await?;

        let credentials = ApiCredentials::new(
            derive_response.api_key,
            derive_response.secret,
            derive_response.passphrase,
        );

        self.credentials = Some(credentials.clone());
        info!("Successfully derived API credentials");

        Ok(credentials)
    }

    /// Create new API credentials via POST /auth/api-key (L1 authentication).
    pub async fn create_api_key(&mut self) -> Result<ApiCredentials> {
        let timestamp = current_timestamp();
        let nonce: u64 = 0;

        let signature = self
            .signer
            .sign_clob_auth_message(timestamp, nonce)
            .await
            .map_err(|e| Error::Signing {
                message: format!("Failed to sign CLOB auth message: {}", e),
            })?;

        let url = format!("{}/auth/api-key", self.client.base_url);

        let response = self
            .client
            .http_client
            .post(&url)
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", &signature)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_NONCE", nonce.to_string())
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: format!("Failed to create API key: {} - {}", status, text),
                status: Some(status),
            });
        }

        let derive_response: DeriveApiKeyResponse = response.json().await?;

        let credentials = ApiCredentials::new(
            derive_response.api_key,
            derive_response.secret,
            derive_response.passphrase,
        );

        self.credentials = Some(credentials.clone());
        info!("Successfully created API credentials");

        Ok(credentials)
    }

    /// Create or derive API credentials (tries create first, then derive).
    pub async fn create_or_derive_api_key(&mut self) -> Result<ApiCredentials> {
        match self.create_api_key().await {
            Ok(creds) => Ok(creds),
            Err(create_err) => {
                warn!("create_api_key failed, trying derive: {}", create_err);
                self.derive_api_key().await
            }
        }
    }

    /// Set API credentials directly (if already have them).
    pub fn set_credentials(&mut self, credentials: ApiCredentials) {
        self.credentials = Some(credentials);
    }

    /// Check if credentials are available.
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some()
    }

    /// Whether a token trades on the neg-risk exchange, which decides the
    /// contract its orders are signed for. A successful lookup is cached; a
    /// failed one is an error, never a guess: an order signed for the wrong
    /// exchange is refused.
    pub async fn is_neg_risk(&self, token_id: &str) -> Result<bool> {
        if let Some(&cached) = self.neg_risk_cache.lock().unwrap().get(token_id) {
            return Ok(cached);
        }
        let neg_risk = self.client.get_neg_risk(token_id).await?;
        self.neg_risk_cache
            .lock()
            .unwrap()
            .insert(token_id.to_string(), neg_risk);
        debug!(token_id, neg_risk, "Queried and cached neg-risk status");
        Ok(neg_risk)
    }

    /// Query the CLOB's view of balance and allowance for the authenticated user.
    ///
    /// This checks what the CLOB server sees (on-chain) for the maker address.
    /// Useful for diagnosing "not enough balance / allowance" errors.
    pub async fn get_balance_allowance(
        &self,
        token_id: Option<&str>,
        asset_type: &str,
    ) -> Result<BalanceAllowanceResponse> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set".to_string(),
        })?;

        let mut url = format!(
            "{}/balance-allowance?asset_type={}&signature_type=0",
            self.client.base_url, asset_type
        );
        if let Some(tid) = token_id {
            url.push_str(&format!("&token_id={}", tid));
        }

        let timestamp = current_timestamp().to_string();
        let method = "GET";
        // L2 HMAC signs only the path (no query string) per the official SDK
        let path = "/balance-allowance";
        let signature = sign_l2_request(credentials, method, path, &timestamp, None)?;

        let response = self
            .client
            .http_client
            .get(&url)
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", &timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: format!("Failed to get balance-allowance: {} - {}", status, text),
                status: Some(status),
            });
        }

        let result: BalanceAllowanceResponse = response.json().await?;
        Ok(result)
    }

    /// Tell the CLOB server to re-read on-chain balance/allowance state.
    ///
    /// Must be called after setting on-chain approvals so the CLOB picks up the
    /// new allowance values. `signature_type` is 0 for EOA wallets. Conditional
    /// asset refreshes require a token id.
    pub async fn update_balance_allowance_for_token(
        &self,
        asset_type: &str,
        token_id: Option<&str>,
    ) -> Result<()> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set".to_string(),
        })?;

        let mut url = format!(
            "{}/balance-allowance/update?asset_type={}&signature_type=0",
            self.client.base_url, asset_type
        );
        if let Some(tid) = token_id {
            url.push_str(&format!("&token_id={}", tid));
        }

        let timestamp = current_timestamp().to_string();
        let method = "GET";
        let path = "/balance-allowance/update";
        let signature = sign_l2_request(credentials, method, path, &timestamp, None)?;

        let response = self
            .client
            .http_client
            .get(&url)
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", &signature)
            .header("POLY_TIMESTAMP", &timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            warn!(
                status,
                response = %text,
                asset_type,
                token_id,
                "Failed to update CLOB balance-allowance cache"
            );
        } else {
            info!(asset_type, token_id, "CLOB balance-allowance cache updated");
        }

        Ok(())
    }

    /// Refresh collateral balance/allowance state without a token-specific scope.
    pub async fn update_balance_allowance(&self, asset_type: &str) -> Result<()> {
        self.update_balance_allowance_for_token(asset_type, None)
            .await
    }

    /// Create and sign an order.
    ///
    /// For GTC/FOK orders, expiration is forced to 0 per the CLOB API contract.
    /// Only GTD orders use a real expiration timestamp.
    pub async fn create_order(
        &self,
        token_id: &str,
        side: crate::signing::OrderSide,
        price: Decimal,
        size: Decimal,
        order_type: OrderType,
    ) -> Result<SignedOrder> {
        // Determine which exchange contract to use for signing
        let neg_risk = self.is_neg_risk(token_id).await?;
        let signer = if neg_risk {
            &self.neg_risk_signer
        } else {
            &self.signer
        };

        info!(
            token_id = token_id,
            side = ?side,
            price = %price,
            size = %size,
            neg_risk = neg_risk,
            order_type = ?order_type,
            "Building order"
        );

        let mut builder = signer
            .order_builder()
            .token_id_str(token_id)
            .side(side)
            .price(price)
            .size(size);

        // Only GTD orders have a real expiration; GTC/FOK must be 0
        builder = match order_type {
            OrderType::Gtd => builder.expires_in(3600), // 1 hour default for GTD
            _ => builder.expires_at(0),
        };

        let order = builder.build().ok_or_else(|| Error::Order {
            message: format!(
                "Failed to build order - missing or invalid fields (token id {token_id})"
            ),
        })?;

        info!(
            maker_amount = %order.maker_amount,
            taker_amount = %order.taker_amount,
            salt = %order.salt,
            "Order built with amounts"
        );

        let signed = signer
            .sign_order(&order)
            .await
            .map_err(|e| Error::Signing {
                message: format!("Failed to sign order: {}", e),
            })?;

        Ok(signed)
    }

    /// Create and sign a market order using Polymarket's amount-based semantics.
    ///
    /// For BUY orders, `amount` is the USDC notional to spend.
    /// For SELL orders, `amount` is the token quantity to sell.
    pub async fn create_market_order(
        &self,
        token_id: &str,
        side: crate::signing::OrderSide,
        price: Decimal,
        amount: Decimal,
        order_type: OrderType,
    ) -> Result<SignedOrder> {
        if amount <= Decimal::ZERO {
            return Err(Error::Order {
                message: "Market order amount must be positive".to_string(),
            });
        }
        if price <= Decimal::ZERO {
            return Err(Error::Order {
                message: "Market order price must be positive".to_string(),
            });
        }

        let neg_risk = self.is_neg_risk(token_id).await?;
        let signer = if neg_risk {
            &self.neg_risk_signer
        } else {
            &self.signer
        };

        let (maker_amount, taker_amount) = calculate_market_order_amounts(side, amount, price)?;
        let token_id = U256::from_str_radix(token_id, 10).map_err(|_| Error::Order {
            message: format!("Invalid token id: {token_id}"),
        })?;

        info!(
            token_id = %token_id,
            side = ?side,
            price = %price,
            amount = %amount,
            neg_risk = neg_risk,
            order_type = ?order_type,
            "Building market order"
        );

        let expiration = match order_type {
            OrderType::Gtd => current_timestamp() + 3600,
            _ => 0,
        };
        let order = OrderData::new(
            signer.address(),
            token_id,
            side,
            maker_amount,
            taker_amount,
            expiration,
        );

        info!(
            maker_amount = %order.maker_amount,
            taker_amount = %order.taker_amount,
            salt = %order.salt,
            "Market order built with amounts"
        );

        signer.sign_order(&order).await.map_err(|e| Error::Signing {
            message: format!("Failed to sign order: {e}"),
        })
    }

    /// Post a signed order to the CLOB.
    pub async fn post_order(
        &self,
        signed_order: SignedOrder,
        order_type: OrderType,
    ) -> Result<PostOrderResponse> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set - call derive_api_key() first".to_string(),
        })?;

        self.client
            .post_signed_order(&self.address(), credentials, signed_order, order_type)
            .await
    }

    /// Read the authenticated exchange order, including cumulative confirmed fills.
    pub async fn get_order(&self, order_id: &str) -> Result<ExchangeOrder> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set".into(),
        })?;
        let path = format!("/data/order/{order_id}");
        let timestamp = current_timestamp().to_string();
        let signature = sign_l2_request(credentials, "GET", &path, &timestamp, None)?;
        let response = self
            .client
            .http_client
            .get(format!("{}{}", self.client.base_url, path))
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::Api {
                message: "Failed to reconcile exchange order".into(),
                status: Some(response.status().as_u16()),
            });
        }
        Ok(response.json().await?)
    }

    /// Like [`Self::get_order`], but `Ok(None)` when the exchange holds no
    /// such order (HTTP 404 or an empty body). Any other failure is an error,
    /// never evidence of absence.
    pub async fn find_order(&self, order_id: &str) -> Result<Option<ExchangeOrder>> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set".into(),
        })?;
        let path = format!("/data/order/{order_id}");
        let timestamp = current_timestamp().to_string();
        let signature = sign_l2_request(credentials, "GET", &path, &timestamp, None)?;
        let response = self
            .client
            .http_client
            .get(format!("{}{}", self.client.base_url, path))
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Error::Api {
                message: "Failed to reconcile exchange order".into(),
                status: Some(response.status().as_u16()),
            });
        }
        let body: serde_json::Value = response.json().await?;
        let empty = body.is_null()
            || body.as_str().is_some_and(str::is_empty)
            || body.as_object().is_some_and(serde_json::Map::is_empty);
        if empty {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(body)?))
    }

    /// Whether any of this wallet's trades on `asset_id` since `after` (unix
    /// seconds) involved `order_id`, as taker or as a matched maker order.
    /// Every page must be read: a partial listing is an error, not a "no".
    pub async fn order_has_trades(
        &self,
        order_id: &str,
        asset_id: &str,
        after: i64,
    ) -> Result<bool> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set".into(),
        })?;
        let incomplete = |message: &str| Error::Api {
            message: message.to_string(),
            status: None,
        };
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let path = "/data/trades";
            let timestamp = current_timestamp().to_string();
            let signature = sign_l2_request(credentials, "GET", path, &timestamp, None)?;
            let mut query = vec![
                ("asset_id", asset_id.to_string()),
                ("after", after.to_string()),
            ];
            if let Some(cursor) = &cursor {
                query.push(("next_cursor", cursor.clone()));
            }
            let response = self
                .client
                .http_client
                .get(format!("{}{}", self.client.base_url, path))
                .query(&query)
                .header("POLY_ADDRESS", self.address())
                .header("POLY_SIGNATURE", signature)
                .header("POLY_TIMESTAMP", timestamp)
                .header("POLY_API_KEY", &credentials.api_key)
                .header("POLY_PASSPHRASE", &credentials.api_passphrase)
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(Error::Api {
                    message: "Failed to list trades".into(),
                    status: Some(response.status().as_u16()),
                });
            }
            let page: serde_json::Value = response.json().await?;
            let rows = page
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(|| incomplete("Trade listing has no data array"))?;
            let involves_order = |trade: &serde_json::Value| {
                trade["taker_order_id"].as_str() == Some(order_id)
                    || trade["maker_orders"].as_array().is_some_and(|makers| {
                        makers
                            .iter()
                            .any(|maker| maker["order_id"].as_str() == Some(order_id))
                    })
            };
            if rows.iter().any(involves_order) {
                return Ok(true);
            }
            match page.get("next_cursor").and_then(|v| v.as_str()) {
                None | Some("") | Some("LTE=") => return Ok(false),
                Some(next) => cursor = Some(next.to_string()),
            }
        }
        Err(incomplete("Too many trade pages to rule out an execution"))
    }

    /// Reconstruct execution price from authenticated, confirmed trade records.
    /// Schema: github.com/Polymarket/clob-client/blob/main/src/types.ts (Trade).
    pub async fn confirmed_order_fill(&self, order: &ExchangeOrder) -> Result<(Decimal, Decimal)> {
        let invalid = || Error::Api {
            message: "Incomplete or inconsistent confirmed trade evidence".into(),
            status: None,
        };
        let expected = order
            .size_matched
            .parse::<Decimal>()
            .map_err(|_| invalid())?;
        if expected.is_zero() {
            return Ok((Decimal::ZERO, Decimal::ZERO));
        }
        let credentials = self.credentials.as_ref().ok_or_else(invalid)?;
        let mut seen = std::collections::HashSet::new();
        let mut quantity = Decimal::ZERO;
        let mut value = Decimal::ZERO;
        for id in &order.associate_trades {
            if !seen.insert(id) {
                return Err(invalid());
            }
            let path = "/data/trades";
            let timestamp = current_timestamp().to_string();
            let signature = sign_l2_request(credentials, "GET", path, &timestamp, None)?;
            let response = self
                .client
                .http_client
                .get(format!("{}{}", self.client.base_url, path))
                .query(&[("id", id.as_str())])
                .header("POLY_ADDRESS", self.address())
                .header("POLY_SIGNATURE", signature)
                .header("POLY_TIMESTAMP", timestamp)
                .header("POLY_API_KEY", &credentials.api_key)
                .header("POLY_PASSPHRASE", &credentials.api_passphrase)
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(invalid());
            }
            let page: serde_json::Value = response.json().await?;
            let rows = page
                .get("data")
                .and_then(|v| v.as_array())
                .ok_or_else(invalid)?;
            let trade = rows
                .iter()
                .find(|v| v["id"].as_str() == Some(id))
                .ok_or_else(invalid)?;
            if trade["status"].as_str() != Some("CONFIRMED") {
                return Err(invalid());
            }
            let mut add = |q: &serde_json::Value, p: &serde_json::Value| -> Result<()> {
                let q = q
                    .as_str()
                    .ok_or_else(invalid)?
                    .parse::<Decimal>()
                    .map_err(|_| invalid())?;
                let p = p
                    .as_str()
                    .ok_or_else(invalid)?
                    .parse::<Decimal>()
                    .map_err(|_| invalid())?;
                if q <= Decimal::ZERO || p <= Decimal::ZERO || p > Decimal::ONE {
                    return Err(invalid());
                }
                quantity += q;
                value += q * p;
                Ok(())
            };
            if trade["taker_order_id"].as_str() == Some(&order.id) {
                add(&trade["size"], &trade["price"])?;
            } else {
                let makers = trade["maker_orders"].as_array().ok_or_else(invalid)?;
                for maker in makers
                    .iter()
                    .filter(|v| v["order_id"].as_str() == Some(&order.id))
                {
                    add(&maker["matched_amount"], &maker["price"])?;
                }
            }
        }
        if quantity != expected {
            return Err(invalid());
        }
        Ok((quantity, value / quantity))
    }

    /// Cancel an order by ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set - call derive_api_key() first".to_string(),
        })?;

        let path = "/order";
        let timestamp = current_timestamp().to_string();
        let body = serde_json::to_string(&serde_json::json!({"orderID":order_id}))?;
        let signature = sign_l2_request(credentials, "DELETE", path, &timestamp, Some(&body))?;
        let response = self
            .client
            .http_client
            .delete(format!("{}{}", self.client.base_url, path))
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Api {
                message: "Exchange cancellation request failed".into(),
                status: Some(status.as_u16()),
            });
        }
        let result: serde_json::Value = response.json().await?;
        let confirmed = result
            .get("canceled")
            .and_then(|v| v.as_array())
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(order_id)));
        if !confirmed {
            return Err(Error::Order {
                message: "Exchange did not confirm cancellation; reconcile order status".into(),
            });
        }
        Ok(())
    }

    /// Get open orders for the authenticated user.
    pub async fn get_open_orders(&self, market: Option<&str>) -> Result<Vec<OpenOrder>> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set - call derive_api_key() first".to_string(),
        })?;

        let url = match market {
            Some(m) => format!("{}/orders?market={}", self.client.base_url, m),
            None => format!("{}/orders", self.client.base_url),
        };
        let timestamp = current_timestamp().to_string();
        let method = "GET";
        // L2 HMAC signs only the path (no query string) per the official SDK
        let path = "/orders";

        let signature = sign_l2_request(credentials, method, path, &timestamp, None)?;

        let response = self
            .client
            .http_client
            .get(&url)
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", &timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: format!("Failed to get open orders: {} - {}", status, text),
                status: Some(status),
            });
        }

        let orders: Vec<OpenOrder> = response.json().await?;
        Ok(orders)
    }

    /// Cancel all open orders.
    pub async fn cancel_all_orders(&self) -> Result<()> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set - call derive_api_key() first".to_string(),
        })?;

        let url = format!("{}/orders", self.client.base_url);
        let timestamp = current_timestamp().to_string();
        let method = "DELETE";
        let path = "/orders";

        let signature = sign_l2_request(credentials, method, path, &timestamp, None)?;

        let response = self
            .client
            .http_client
            .delete(&url)
            .header("POLY_ADDRESS", self.address())
            .header("POLY_SIGNATURE", signature)
            .header("POLY_TIMESTAMP", &timestamp)
            .header("POLY_API_KEY", &credentials.api_key)
            .header("POLY_PASSPHRASE", &credentials.api_passphrase)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: format!("Failed to cancel all orders: {} - {}", status, text),
                status: Some(status),
            });
        }

        info!("All orders cancelled successfully");
        Ok(())
    }
}

fn market_price_scale(price: Decimal) -> u32 {
    price.normalize().scale().clamp(1, 4)
}

fn market_amount_scale(price_scale: u32) -> u32 {
    price_scale.saturating_add(2)
}

fn round_down(value: Decimal, scale: u32) -> Decimal {
    value.trunc_with_scale(scale)
}

fn round_up(value: Decimal, scale: u32) -> Decimal {
    value.round_dp_with_strategy(scale, RoundingStrategy::ToPositiveInfinity)
}

fn decimal_places(value: Decimal) -> u32 {
    value.normalize().scale()
}

fn to_fixed_u128(value: Decimal, scale: u32) -> u128 {
    value
        .trunc_with_scale(scale)
        .trunc_with_scale(COLLATERAL_DECIMALS)
        .mantissa()
        .unsigned_abs()
}

fn calculate_market_order_amounts(
    side: crate::signing::OrderSide,
    amount: Decimal,
    price: Decimal,
) -> Result<(U256, U256)> {
    if amount <= Decimal::ZERO || price <= Decimal::ZERO {
        return Err(Error::Order {
            message: "Market order amount and price must be positive".to_string(),
        });
    }

    let price_scale = market_price_scale(price);
    let amount_scale = market_amount_scale(price_scale);
    let raw_price = round_down(price, price_scale);

    let (raw_maker_amt, raw_taker_amt) = match side {
        crate::signing::OrderSide::Buy => {
            let raw_maker_amt = round_down(amount, MARKET_ORDER_SIZE_SCALE);
            if raw_maker_amt <= Decimal::ZERO {
                return Err(Error::Order {
                    message: "Market BUY amount rounds down to zero".to_string(),
                });
            }
            let mut raw_taker_amt = raw_maker_amt / raw_price;
            if decimal_places(raw_taker_amt) > amount_scale {
                raw_taker_amt = round_up(raw_taker_amt, amount_scale + 4);
                if decimal_places(raw_taker_amt) > amount_scale {
                    raw_taker_amt = round_down(raw_taker_amt, amount_scale);
                }
            }
            (raw_maker_amt, raw_taker_amt)
        }
        crate::signing::OrderSide::Sell => {
            let raw_maker_amt = round_down(amount, MARKET_ORDER_SIZE_SCALE);
            if raw_maker_amt <= Decimal::ZERO {
                return Err(Error::Order {
                    message: "Market SELL amount rounds down to zero".to_string(),
                });
            }
            let mut raw_taker_amt = raw_maker_amt * raw_price;
            if decimal_places(raw_taker_amt) > amount_scale {
                raw_taker_amt = round_up(raw_taker_amt, amount_scale + 4);
                if decimal_places(raw_taker_amt) > amount_scale {
                    raw_taker_amt = round_down(raw_taker_amt, amount_scale);
                }
            }
            (raw_maker_amt, raw_taker_amt)
        }
    };

    Ok((
        U256::from(to_fixed_u128(raw_maker_amt, MARKET_ORDER_SIZE_SCALE)),
        U256::from(to_fixed_u128(raw_taker_amt, amount_scale)),
    ))
}

/// The fee terms in CLOB market info (`{"fd": {"r": rate, "e": exponent}}`).
fn fee_schedule_from_market_info(info: &serde_json::Value) -> Result<crate::types::FeeSchedule> {
    let invalid = || Error::Order {
        message: format!(
            "Unpriceable exchange fee terms: {}",
            info.get("fd").unwrap_or(info)
        ),
    };
    let terms = info.get("fd").filter(|fd| !fd.is_null());
    let rate = match terms.and_then(|fd| fd.get("r")) {
        None => return Ok(crate::types::FeeSchedule::FREE),
        Some(rate) => rate.as_f64().ok_or_else(invalid)?,
    };
    if !(rate.is_finite() && rate >= 0.0) {
        return Err(invalid());
    }
    if rate == 0.0 {
        return Ok(crate::types::FeeSchedule::FREE);
    }
    let exponent = terms
        .and_then(|fd| fd.get("e"))
        .and_then(|e| e.as_f64())
        .filter(|e| e.is_finite() && *e > 0.0)
        .ok_or_else(invalid)?;
    Ok(crate::types::FeeSchedule {
        rate: Decimal::try_from(rate).map_err(|_| invalid())?,
        exponent: Decimal::try_from(exponent).map_err(|_| invalid())?,
    })
}

/// Get current Unix timestamp in seconds.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Sign a request with HMAC-SHA256 for L2 authentication.
#[allow(clippy::result_large_err)]
fn sign_l2_request(
    credentials: &ApiCredentials,
    method: &str,
    path: &str,
    timestamp: &str,
    body: Option<&str>,
) -> Result<String> {
    // Build the message: timestamp + method + path + body
    let message = match body {
        Some(b) => format!("{}{}{}{}", timestamp, method, path, b),
        None => format!("{}{}{}", timestamp, method, path),
    };

    // Decode the secret — Polymarket uses URL-safe base64 encoding for API secrets,
    // but fall back to standard base64 for compatibility.
    let secret_bytes = base64::engine::general_purpose::URL_SAFE
        .decode(&credentials.api_secret)
        .or_else(|_| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&credentials.api_secret)
        })
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&credentials.api_secret))
        .map_err(|e| Error::Signing {
            message: format!("Invalid API secret encoding: {}", e),
        })?;

    // Create HMAC-SHA256
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret_bytes).map_err(|e| Error::Signing {
        message: format!("Failed to create HMAC: {}", e),
    })?;

    mac.update(message.as_bytes());
    let result = mac.finalize();

    // Return as URL-safe base64 (matching Polymarket's Python/TypeScript clients)
    Ok(base64::engine::general_purpose::URL_SAFE.encode(result.into_bytes()))
}

impl std::fmt::Debug for AuthenticatedClobClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticatedClobClient")
            .field("address", &self.address())
            .field("has_credentials", &self.has_credentials())
            .finish()
    }
}

#[cfg(test)]
mod authenticated_tests {
    use super::*;
    use alloy_signer_local::PrivateKeySigner;
    use std::str::FromStr;

    const TEST_PRIVATE_KEY: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn test_auth_client() -> AuthenticatedClobClient {
        let client = ClobClient::new(None, None);
        let signer = PrivateKeySigner::from_str(TEST_PRIVATE_KEY).unwrap();
        let order_signer = OrderSigner::new(signer);
        AuthenticatedClobClient::new(client, order_signer)
    }

    #[test]
    fn test_authenticated_client_creation() {
        let auth_client = test_auth_client();
        assert!(!auth_client.has_credentials());
        assert!(auth_client.address().starts_with("0x"));
    }

    #[test]
    fn test_set_credentials() {
        let mut auth_client = test_auth_client();
        assert!(!auth_client.has_credentials());

        auth_client.set_credentials(ApiCredentials::new(
            "key".to_string(),
            "c2VjcmV0".to_string(), // "secret" in base64
            "pass".to_string(),
        ));

        assert!(auth_client.has_credentials());
    }

    #[tokio::test]
    async fn test_create_order() {
        let auth_client = test_auth_client();
        auth_client
            .neg_risk_cache
            .lock()
            .unwrap()
            .insert("12345".into(), false);

        let signed = auth_client
            .create_order(
                "12345",
                crate::signing::OrderSide::Buy,
                Decimal::new(50, 2), // 0.50
                Decimal::from(100),  // 100 USDC
                OrderType::Gtc,
            )
            .await
            .unwrap();

        assert!(signed.signature.starts_with("0x"));
        assert_eq!(signed.side, "BUY");
        assert_eq!(signed.expiration, "0");

        // The body the official V2 clients post.
        let body = serde_json::to_value(PostOrderRequest {
            order: signed,
            owner: "key".into(),
            order_type: OrderType::Gtc,
            defer_exec: false,
            post_only: false,
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "order": body["order"].clone(),
                "owner": "key",
                "orderType": "GTC",
                "deferExec": false,
                "postOnly": false,
            })
        );
        assert!(body["order"].get("taker").is_none());
        assert!(body["order"].get("nonce").is_none());
        assert!(body["order"].get("feeRateBps").is_none());
    }

    #[tokio::test]
    async fn orders_are_signed_for_the_v2_exchange_of_their_market() {
        use crate::signing::Eip712Domain;
        let auth_client = test_auth_client();
        for (token, neg_risk, domain) in [
            ("11", false, Eip712Domain::ctf_exchange()),
            ("22", true, Eip712Domain::neg_risk_ctf_exchange()),
        ] {
            auth_client
                .neg_risk_cache
                .lock()
                .unwrap()
                .insert(token.into(), neg_risk);
            let signed = auth_client
                .create_order(
                    token,
                    crate::signing::OrderSide::Sell,
                    Decimal::new(37, 2),
                    Decimal::from(20),
                    OrderType::Gtd,
                )
                .await
                .unwrap();
            let order = crate::signing::OrderData {
                salt: U256::from(signed.salt),
                maker: signed.maker.parse().unwrap(),
                signer: signed.signer.parse().unwrap(),
                token_id: U256::from_str_radix(&signed.token_id, 10).unwrap(),
                maker_amount: U256::from_str_radix(&signed.maker_amount, 10).unwrap(),
                taker_amount: U256::from_str_radix(&signed.taker_amount, 10).unwrap(),
                side: 1,
                signature_type: signed.signature_type,
                timestamp: U256::from_str_radix(&signed.timestamp, 10).unwrap(),
                metadata: signed.metadata.parse().unwrap(),
                builder: signed.builder.parse().unwrap(),
                expiration: U256::ZERO,
            };
            let digest = alloy_primitives::keccak256(
                [
                    &[0x19u8, 0x01][..],
                    domain.separator().as_slice(),
                    order.struct_hash().as_slice(),
                ]
                .concat(),
            );
            assert_eq!(signed.order_hash, Some(format!("{digest:#x}")));
            let signature: alloy_primitives::PrimitiveSignature = signed.signature.parse().unwrap();
            assert_eq!(
                signature.recover_address_from_prehash(&digest).unwrap(),
                auth_client.signer.address()
            );
            // A good-till-date order carries its expiry, unsigned.
            assert!(signed.expiration.parse::<u64>().unwrap() > current_timestamp());
        }
    }

    #[test]
    fn v2_order_responses_are_read() {
        let response: PostOrderResponse = serde_json::from_value(serde_json::json!({
            "success": true, "errorMsg": "", "orderID": "0xabc", "status": "matched",
            "takingAmount": "10", "makingAmount": "4.5",
            "transactionsHashes": ["0xt1", "0xt2"], "tradeIDs": ["t"]
        }))
        .unwrap();
        assert!(!response.refused() && !response.unclear());
        assert_eq!(response.settlement_tx_hash().as_deref(), Some("0xt1"));
        let answer = |json: serde_json::Value| -> PostOrderResponse {
            serde_json::from_value(json).unwrap()
        };
        // Not taken and no order named: refused, whatever the status is
        // missing or empty.
        let refused = answer(serde_json::json!({
            "success": false, "errorMsg": "not enough balance", "orderID": "", "status": ""
        }));
        assert!(refused.refused() && !refused.unclear());
        // An answer is never unreadable for a field the clients treat as
        // optional: missing, null or empty alike.
        let sparse = answer(serde_json::json!({
            "success": false, "errorMsg": "not enough balance", "transactionsHashes": null
        }));
        assert!(sparse.refused());
        assert_eq!(sparse.settlement_tx_hash(), None);
        // Neither a refusal nor an acceptance: read from the exchange.
        for unclear in [
            serde_json::json!({"success": true, "orderID": null, "status": null, "errorMsg": null}),
            serde_json::json!({"success": false, "errorMsg": "timeout", "orderID": "0xabc", "status": ""}),
            serde_json::json!({"success": false, "orderID": "", "status": "live"}),
            serde_json::json!({"success": false, "orderID": "0xabc", "status": "matched"}),
            serde_json::json!({"success": true, "orderID": "0xabc", "status": "unmatched"}),
            serde_json::json!({"success": false, "orderID": "", "status": "UNMATCHED"}),
            serde_json::json!({"orderID": "0xabc", "status": "rejected"}),
        ] {
            let unclear = answer(unclear.clone());
            assert!(unclear.unclear() && !unclear.refused(), "{unclear:?}");
        }
        let nulls = answer(serde_json::json!({"success": true, "orderID": null, "status": null}));
        assert!(nulls.order_id.is_empty() && nulls.status.is_empty());
        // Accepted: matched, resting or delayed, unless `success` says not.
        for status in ["matched", "live", "delayed", "LIVE"] {
            for success in [serde_json::json!(true), serde_json::Value::Null] {
                let accepted = answer(serde_json::json!({
                    "success": success, "orderID": "0xabc", "status": status
                }));
                assert!(
                    !accepted.refused() && !accepted.unclear(),
                    "{status} {success}"
                );
            }
        }
        // V1 responses read as before.
        let v1 = answer(serde_json::json!({
            "orderID": "0xabc", "status": "live", "transactionHash": "0xt"
        }));
        assert!(!v1.refused() && !v1.unclear());
        assert_eq!(v1.settlement_tx_hash().as_deref(), Some("0xt"));
    }

    #[test]
    fn prices_are_checked_against_the_market_tick() {
        let tick = |answer: serde_json::Value| tick_size_from(&answer);
        assert_eq!(
            tick(serde_json::json!({"minimum_tick_size": 0.01})),
            Some(Decimal::new(1, 2))
        );
        assert_eq!(
            tick(serde_json::json!({"minimum_tick_size": "0.0025"})),
            Some(Decimal::new(25, 4))
        );
        assert_eq!(tick(serde_json::json!({"minimum_tick_size": 0})), None);
        assert_eq!(tick(serde_json::json!({"minimum_tick_size": 1})), None);
        assert_eq!(tick(serde_json::json!({})), None);
        let d = |text: &str| text.parse::<Decimal>().unwrap();
        assert!(price_on_tick(d("0.55"), d("0.01")));
        assert!(!price_on_tick(d("0.555"), d("0.01")));
        assert!(price_on_tick(d("0.505"), d("0.005")));
        assert!(!price_on_tick(d("0.5025"), d("0.005")));
        assert!(price_on_tick(d("0.5025"), d("0.0025")));
        // Within one step of either end only.
        assert!(price_on_tick(d("0.01"), d("0.01")) && price_on_tick(d("0.99"), d("0.01")));
        assert!(!price_on_tick(d("0.005"), d("0.01")) && !price_on_tick(d("0.995"), d("0.01")));
        assert!(price_on_tick(d("0.1"), d("0.1")) && price_on_tick(d("0.9"), d("0.1")));
    }

    #[test]
    fn closed_exchange_orders_are_told_in_every_spelling() {
        let order = |status: &str| ExchangeOrder {
            associate_trades: Vec::new(),
            id: "0xabc".into(),
            status: status.into(),
            original_size: "10".into(),
            size_matched: "4".into(),
        };
        for closed in [
            "CANCELED",
            "canceled",
            "CANCELED_MARKET_RESOLVED",
            "ORDER_STATUS_CANCELED_MARKET_RESOLVED",
            "INVALID",
            "ORDER_STATUS_INVALID",
            "expired",
        ] {
            assert!(order(closed).is_closed(), "{closed}");
        }
        for open in [
            "LIVE",
            "ORDER_STATUS_LIVE",
            "MATCHED",
            "delayed",
            "unmatched",
            "",
        ] {
            assert!(!order(open).is_closed(), "{open}");
        }
        assert_eq!(order("ORDER_STATUS_LIVE").state(), "live");
    }

    #[test]
    fn exchange_fee_terms_are_read_from_market_info() {
        use crate::types::FeeSchedule;
        let terms = |info: serde_json::Value| fee_schedule_from_market_info(&info);
        assert_eq!(
            terms(serde_json::json!({"fd": {"r": 0.07, "e": 1, "to": true}})).unwrap(),
            FeeSchedule {
                rate: Decimal::new(7, 2),
                exponent: Decimal::ONE
            }
        );
        assert_eq!(
            terms(serde_json::json!({"fd": {"r": 0.05, "e": 2}})).unwrap(),
            FeeSchedule {
                rate: Decimal::new(5, 2),
                exponent: Decimal::TWO
            }
        );
        // No fee terms, or a zero rate: free.
        assert_eq!(
            terms(serde_json::json!({"t": []})).unwrap(),
            FeeSchedule::FREE
        );
        assert_eq!(
            terms(serde_json::json!({"fd": null})).unwrap(),
            FeeSchedule::FREE
        );
        assert_eq!(
            terms(serde_json::json!({"fd": {"r": 0, "e": 0}})).unwrap(),
            FeeSchedule::FREE
        );
        // A rate without a usable exponent, or a bad rate, is not free.
        assert!(terms(serde_json::json!({"fd": {"r": 0.07}})).is_err());
        assert!(terms(serde_json::json!({"fd": {"r": 0.07, "e": 0}})).is_err());
        assert!(terms(serde_json::json!({"fd": {"r": -1, "e": 1}})).is_err());
        assert!(terms(serde_json::json!({"fd": {"r": "x", "e": 1}})).is_err());
    }

    #[test]
    fn test_calculate_market_order_amounts_buy_uses_usdc_notional() {
        let (maker_amount, taker_amount) = calculate_market_order_amounts(
            crate::signing::OrderSide::Buy,
            Decimal::new(2500, 2), // $25.00
            Decimal::new(48, 2),   // 0.48
        )
        .unwrap();

        assert_eq!(maker_amount, U256::from(25_000_000u64));
        assert_eq!(taker_amount, U256::from(52_083_300u64));
    }

    #[test]
    fn test_calculate_market_order_amounts_sell_uses_share_size() {
        let (maker_amount, taker_amount) = calculate_market_order_amounts(
            crate::signing::OrderSide::Sell,
            Decimal::new(521, 2), // 5.21 shares
            Decimal::new(48, 2),  // 0.48
        )
        .unwrap();

        assert_eq!(maker_amount, U256::from(5_210_000u64));
        assert_eq!(taker_amount, U256::from(2_500_800u64));
    }

    #[test]
    fn test_sign_l2_request() {
        let credentials = ApiCredentials::new(
            "test-key".to_string(),
            base64::engine::general_purpose::STANDARD.encode("test-secret"),
            "test-passphrase".to_string(),
        );

        let signature = sign_l2_request(
            &credentials,
            "POST",
            "/order",
            "1700000000",
            Some(r#"{"order":"data"}"#),
        )
        .unwrap();

        // Should be valid URL-safe base64
        assert!(base64::engine::general_purpose::URL_SAFE
            .decode(&signature)
            .is_ok());
    }

    #[test]
    fn test_sign_l2_request_no_body() {
        let credentials = ApiCredentials::new(
            "test-key".to_string(),
            base64::engine::general_purpose::STANDARD.encode("test-secret"),
            "test-passphrase".to_string(),
        );

        let signature =
            sign_l2_request(&credentials, "GET", "/orders", "1700000000", None).unwrap();

        assert!(base64::engine::general_purpose::URL_SAFE
            .decode(&signature)
            .is_ok());
    }

    #[test]
    fn test_debug_does_not_expose_credentials() {
        let mut auth_client = test_auth_client();
        auth_client.set_credentials(ApiCredentials::new(
            "secret-key".to_string(),
            "c2VjcmV0".to_string(),
            "secret-pass".to_string(),
        ));

        let debug_str = format!("{:?}", auth_client);
        assert!(!debug_str.contains("secret-key"));
        assert!(!debug_str.contains("secret-pass"));
    }

    #[test]
    fn test_activity_entry_uses_id_when_transaction_hash_missing() {
        let entry = ActivityEntry {
            transaction_hash: None,
            id: Some("activity-id".to_string()),
            proxy_wallet: Some("0xabc".to_string()),
            side: Some("BUY".to_string()),
            asset: Some("asset-id".to_string()),
            condition_id: Some("condition-id".to_string()),
            usdc_size: None,
            size: Some(12.0),
            price: Some(0.45),
            timestamp: Some(1_700_000_000),
            created_at: None,
            activity_type: Some("TRADE".to_string()),
            title: Some("Title".to_string()),
            slug: Some("slug".to_string()),
            outcome: Some("Yes".to_string()),
        };

        let trade = entry.into_clob_trade().expect("trade");
        assert_eq!(trade.transaction_hash, "activity-id");
        assert_eq!(trade.wallet_address, "0xabc");
        assert_eq!(trade.condition_id.as_deref(), Some("condition-id"));
    }

    #[test]
    fn test_closed_clob_market_is_treated_as_resolved_even_if_active_true() {
        let market = ClobMarket {
            condition_id: "0xdeadbeef".to_string(),
            question: "Historical market".to_string(),
            description: None,
            tokens: vec![
                ClobToken {
                    token_id: "yes".to_string(),
                    outcome: "Yes".to_string(),
                    price: Some(0.4),
                    winner: None,
                },
                ClobToken {
                    token_id: "no".to_string(),
                    outcome: "No".to_string(),
                    price: Some(0.6),
                    winner: None,
                },
            ],
            volume: None,
            liquidity: None,
            end_date: Some("2024-01-01T00:00:00Z".to_string()),
            closed: true,
            archived: false,
            accepting_orders: false,
            enable_order_book: false,
            active: true,
        };

        let converted: Market = market.into();
        assert!(converted.resolved);
    }
}
