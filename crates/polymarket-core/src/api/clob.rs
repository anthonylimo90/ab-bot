//! Polymarket CLOB API client.
//!
//! This module provides both read-only and authenticated access to the
//! Polymarket CLOB API for order book data and order management.

use crate::signing::{OrderSigner, SignedOrder};
use crate::types::{Market, OrderBook, Outcome, PriceLevel};
use crate::{Error, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration as StdDuration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

/// Polymarket CLOB API client for order book data.
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
                    last_error = Some(Error::Http(e));
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
    /// Fetch recent trades from the Data API with offset-based pagination.
    ///
    /// Returns the trades and the next offset for pagination (current offset + trade count).
    /// Pass `None` for the first page, then feed the returned offset back for subsequent pages.
    pub async fn get_recent_trades(
        &self,
        limit: u32,
        cursor: Option<u64>,
    ) -> Result<(Vec<ClobTrade>, Option<u64>)> {
        // Use Data API instead of CLOB API - it's public and doesn't require auth
        let data_api_url = "https://data-api.polymarket.com";
        let mut url = format!("{}/trades?limit={}", data_api_url, limit);
        if let Some(offset) = cursor {
            url.push_str(&format!("&offset={}", offset));
        }

        let response = self.get_with_retry(&url).await?;
        let text = response.text().await?;

        // Data API returns trades as a top-level array
        match serde_json::from_str::<Vec<ClobTrade>>(&text) {
            Ok(trades) => {
                let count = trades.len() as u64;
                let next_offset = if count >= limit as u64 {
                    // More pages likely available
                    Some(cursor.unwrap_or(0) + count)
                } else {
                    // Last page — fewer results than requested
                    None
                };
                Ok((trades, next_offset))
            }
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

        loop {
            match Self::ws_loop(&ws_url, &asset_ids, &tx).await {
                Ok(()) => {
                    info!("WebSocket connection closed cleanly");
                }
                Err(e) => {
                    warn!(attempt = attempt + 1, error = %e, "WebSocket connection failed");
                }
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
        ping_tick.tick().await;
        let mut order_books_by_asset: HashMap<String, OrderBook> = HashMap::new();

        // Subscribe to market channel by token IDs.
        let subscribe_msg = serde_json::json!({
            "type": "market",
            "assets_ids": asset_ids,
            "custom_feature_enabled": false
        });
        write.send(Message::Text(subscribe_msg.to_string())).await?;
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
                            let updates = parse_ws_updates(&text, &mut order_books_by_asset);
                            for update in updates {
                                if tx.send(update).await.is_err() {
                                    warn!("Receiver dropped, closing WebSocket");
                                    return Ok(());
                                }
                            }
                        }
                        Ok(Message::Ping(data)) => {
                            write.send(Message::Pong(data)).await?;
                        }
                        Ok(Message::Pong(_)) => {
                            debug!("Received websocket pong");
                        }
                        Ok(Message::Close(_)) => {
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

/// Response wrapper for paginated trades (legacy CLOB API format - no longer used).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct TradesResponse {
    #[serde(default)]
    data: Option<Vec<ClobTrade>>,
    /// Some endpoints return trades at top level.
    #[serde(flatten)]
    _extra: serde_json::Value,
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
    active: bool,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
                })
                .collect(),
            volume: m.volume.and_then(|v| v.parse().ok()).unwrap_or_default(),
            liquidity: m.liquidity.and_then(|v| v.parse().ok()).unwrap_or_default(),
            end_date: m.end_date.and_then(|s| s.parse().ok()),
            resolved: m.closed && !m.active,
            resolution: None,
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
            bids: b
                .bids
                .into_iter()
                .map(|l| PriceLevel {
                    price: l.price.parse().unwrap_or_default(),
                    size: l.size.parse().unwrap_or_default(),
                })
                .collect(),
            asks: b
                .asks
                .into_iter()
                .map(|l| PriceLevel {
                    price: l.price.parse().unwrap_or_default(),
                    size: l.size.parse().unwrap_or_default(),
                })
                .collect(),
        }
    }
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

fn parse_ws_updates(
    text: &str,
    book_state: &mut HashMap<String, OrderBook>,
) -> Vec<OrderBookUpdate> {
    let trimmed = text.trim();

    if trimmed.eq_ignore_ascii_case("PONG") || trimmed.eq_ignore_ascii_case("PING") {
        return Vec::new();
    }
    if trimmed.eq_ignore_ascii_case("INVALID OPERATION") {
        warn!("Received INVALID OPERATION from CLOB websocket");
        return Vec::new();
    }

    let value = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => value,
        Err(e) => {
            debug!(
                "Failed to parse websocket JSON message: {} - {}",
                e, trimmed
            );
            return Vec::new();
        }
    };

    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(parse_ws_book_from_value)
            .inspect(|update| {
                book_state.insert(update.asset_id.clone(), update_to_orderbook(update));
            })
            .collect(),
        serde_json::Value::Object(_) => {
            if let Some(update) = parse_ws_book_from_value(value.clone()) {
                book_state.insert(update.asset_id.clone(), update_to_orderbook(&update));
                return vec![update];
            }

            if let Ok(event) = serde_json::from_value::<WsPriceChangeEvent>(value) {
                return apply_price_changes(event, book_state);
            }

            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn parse_ws_book_from_value(value: serde_json::Value) -> Option<OrderBookUpdate> {
    let ws_book = serde_json::from_value::<WsBook>(value).ok()?;

    Some(OrderBookUpdate {
        market_id: ws_book.market,
        asset_id: ws_book.asset_id,
        timestamp: parse_ws_timestamp(&ws_book.timestamp),
        bids: ws_book
            .bids
            .into_iter()
            .map(|l| PriceLevel {
                price: l.price.parse().unwrap_or_default(),
                size: l.size.parse().unwrap_or_default(),
            })
            .collect(),
        asks: ws_book
            .asks
            .into_iter()
            .map(|l| PriceLevel {
                price: l.price.parse().unwrap_or_default(),
                size: l.size.parse().unwrap_or_default(),
            })
            .collect(),
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

        let mut book = book_state
            .remove(&change.asset_id)
            .unwrap_or_else(|| OrderBook {
                market_id: event.market.clone(),
                outcome_id: change.asset_id.clone(),
                timestamp,
                bids: Vec::new(),
                asks: Vec::new(),
            });

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
    } else if size > Decimal::ZERO {
        levels.push(PriceLevel { price, size });
    }

    if descending {
        levels.sort_by(|a, b| b.price.cmp(&a.price));
    } else {
        levels.sort_by(|a, b| a.price.cmp(&b.price));
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

/// Request body for posting an order.
#[derive(Debug, Clone, Serialize)]
pub struct PostOrderRequest {
    pub order: SignedOrder,
    #[serde(rename = "orderType")]
    pub order_type: OrderType,
    /// API key of the order owner.
    pub owner: String,
    /// Post-only flag (default false). Omitted when None.
    #[serde(rename = "postOnly", skip_serializing_if = "Option::is_none")]
    pub post_only: Option<bool>,
}

/// Response from the balance-allowance endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceAllowanceResponse {
    /// Balance in base units (as string).
    #[serde(default)]
    pub balance: String,
    /// Allowance in base units (as string).
    #[serde(default)]
    pub allowance: String,
}

/// Response from posting an order.
#[derive(Debug, Clone, Deserialize)]
pub struct PostOrderResponse {
    /// Order ID assigned by the CLOB.
    #[serde(rename = "orderID")]
    pub order_id: String,
    /// Status of the order (e.g. "matched", "delayed", "unmatched").
    pub status: String,
    /// Transaction hash if applicable.
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
}

impl PostOrderResponse {
    /// Check if the order was successfully matched/filled.
    /// FOK orders that fail return status "unmatched".
    pub fn is_filled(&self) -> bool {
        let s = self.status.to_lowercase();
        s == "matched" || s == "live" || s == "delayed"
    }

    /// Check if the order was explicitly not filled (FOK rejection).
    pub fn is_unfilled(&self) -> bool {
        let s = self.status.to_lowercase();
        s == "unmatched" || s == "rejected"
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
    /// Cache for fee_rate_bps lookups (token_id -> bps). Rarely changes per token.
    fee_rate_cache: Mutex<HashMap<String, u64>>,
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
            fee_rate_cache: Mutex::new(HashMap::new()),
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
            fee_rate_cache: Mutex::new(HashMap::new()),
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

    /// Query the CLOB API for whether a token uses the neg-risk exchange.
    /// Results are cached per token_id since neg-risk status rarely changes.
    async fn is_neg_risk(&self, token_id: &str) -> Result<bool> {
        // Check cache first
        if let Some(&cached) = self.neg_risk_cache.lock().unwrap().get(token_id) {
            return Ok(cached);
        }

        let url = format!("{}/neg-risk?token_id={}", self.client.base_url, token_id);
        let response = self.client.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            // Default to neg_risk=true since most Polymarket markets are neg-risk
            warn!(
                token_id = token_id,
                status = response.status().as_u16(),
                "Failed to query neg-risk status, defaulting to true"
            );
            return Ok(true);
        }

        #[derive(Deserialize)]
        struct NegRiskResponse {
            neg_risk: bool,
        }

        let result: NegRiskResponse = response.json().await.map_err(|e| {
            warn!(error = %e, "Failed to parse neg-risk response, defaulting to true");
            e
        })?;

        // Store in cache
        self.neg_risk_cache
            .lock()
            .unwrap()
            .insert(token_id.to_string(), result.neg_risk);

        debug!(
            token_id = token_id,
            neg_risk = result.neg_risk,
            "Queried and cached neg-risk status"
        );
        Ok(result.neg_risk)
    }

    /// Query the CLOB API for the taker fee rate for a token.
    /// Results are cached per token_id since fee rates rarely change.
    async fn get_fee_rate_bps(&self, token_id: &str) -> Result<u64> {
        // Check cache first
        if let Some(&cached) = self.fee_rate_cache.lock().unwrap().get(token_id) {
            return Ok(cached);
        }

        let url = format!("{}/fee-rate?token_id={}", self.client.base_url, token_id);
        let response = self.client.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            warn!(
                token_id = token_id,
                status = response.status().as_u16(),
                "Failed to query fee-rate, defaulting to 0"
            );
            return Ok(0);
        }

        #[derive(Deserialize)]
        struct FeeRateResponse {
            #[serde(alias = "base_fee", alias = "baseFee", alias = "fee")]
            fee: Option<u64>,
            #[serde(alias = "fee_rate_bps", alias = "feeRateBps")]
            fee_rate_bps: Option<u64>,
        }

        // Try to parse as structured response first, fall back to plain number
        let text = response.text().await?;
        let fee = if let Ok(resp) = serde_json::from_str::<FeeRateResponse>(&text) {
            resp.fee_rate_bps.or(resp.fee).unwrap_or(0)
        } else if let Ok(val) = text.trim().trim_matches('"').parse::<u64>() {
            val
        } else {
            warn!(token_id = token_id, response = %text, "Could not parse fee-rate response");
            0
        };

        // Store in cache
        self.fee_rate_cache
            .lock()
            .unwrap()
            .insert(token_id.to_string(), fee);

        debug!(
            token_id = token_id,
            fee_rate_bps = fee,
            "Fetched and cached fee rate"
        );
        Ok(fee)
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
    /// new allowance values. `signature_type` is 0 for EOA wallets.
    pub async fn update_balance_allowance(&self, asset_type: &str) -> Result<()> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set".to_string(),
        })?;

        let url = format!(
            "{}/balance-allowance/update?asset_type={}&signature_type=0",
            self.client.base_url, asset_type
        );

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
                "Failed to update CLOB balance-allowance cache"
            );
        } else {
            info!(asset_type, "CLOB balance-allowance cache updated");
        }

        Ok(())
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
        // Diagnostic: check CLOB's view of our balance before placing order
        match self.get_balance_allowance(None, "COLLATERAL").await {
            Ok(ba) => {
                info!(
                    balance = %ba.balance,
                    allowance = %ba.allowance,
                    maker = %self.address(),
                    "CLOB USDC balance/allowance check"
                );
            }
            Err(e) => {
                warn!(error = %e, "Failed to check CLOB balance/allowance (continuing)");
            }
        }

        // Determine which exchange contract to use for signing
        let neg_risk = self.is_neg_risk(token_id).await.unwrap_or(true);
        let signer = if neg_risk {
            &self.neg_risk_signer
        } else {
            &self.signer
        };

        // Fetch the market's required fee rate
        let fee_rate = self.get_fee_rate_bps(token_id).await.unwrap_or(0);

        info!(
            token_id = token_id,
            side = ?side,
            price = %price,
            size = %size,
            fee_rate_bps = fee_rate,
            neg_risk = neg_risk,
            order_type = ?order_type,
            "Building order"
        );

        let mut builder = signer
            .order_builder()
            .token_id_str(token_id)
            .side(side)
            .price(price)
            .size(size)
            .fee_rate_bps(fee_rate);

        // Only GTD orders have a real expiration; GTC/FOK must be 0
        builder = match order_type {
            OrderType::Gtd => builder.expires_in(3600), // 1 hour default for GTD
            _ => builder.expires_at(0),
        };

        let order = builder.build().ok_or_else(|| Error::Order {
            message: "Failed to build order - missing required fields".to_string(),
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

    /// Post a signed order to the CLOB.
    pub async fn post_order(
        &self,
        signed_order: SignedOrder,
        order_type: OrderType,
    ) -> Result<PostOrderResponse> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set - call derive_api_key() first".to_string(),
        })?;

        let url = format!("{}/order", self.client.base_url);
        let timestamp = current_timestamp().to_string();
        let method = "POST";
        let path = "/order";

        let request = PostOrderRequest {
            order: signed_order,
            order_type,
            owner: credentials.api_key.clone(),
            post_only: None,
        };

        let body = serde_json::to_string(&request)?;

        debug!(payload = %body, "POST /order request body");

        // Sign the request with L2 HMAC
        let signature = sign_l2_request(credentials, method, path, &timestamp, Some(&body))?;

        let response = self
            .client
            .http_client
            .post(&url)
            .header("POLY_ADDRESS", self.address())
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

    /// Cancel an order by ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        let credentials = self.credentials.as_ref().ok_or_else(|| Error::Auth {
            message: "API credentials not set - call derive_api_key() first".to_string(),
        })?;

        let url = format!("{}/order/{}", self.client.base_url, order_id);
        let timestamp = current_timestamp().to_string();
        let method = "DELETE";
        let path = format!("/order/{}", order_id);

        let signature = sign_l2_request(credentials, method, &path, &timestamp, None)?;

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
                message: format!("Failed to cancel order: {} - {}", status, text),
                status: Some(status),
            });
        }

        info!(order_id = %order_id, "Order cancelled successfully");
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
}
