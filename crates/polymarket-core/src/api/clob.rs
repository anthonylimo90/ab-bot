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
        Self {
            base_url: base_url.unwrap_or_else(|| Self::DEFAULT_BASE_URL.to_string()),
            ws_url: ws_url.unwrap_or_else(|| Self::DEFAULT_WS_URL.to_string()),
            http_client: reqwest::Client::new(),
        }
    }

    /// Maximum retry attempts for API calls.
    const MAX_RETRIES: u32 = 3;

    /// Execute an HTTP GET with retry and exponential backoff.
    async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response> {
        let mut last_error = None;

        for attempt in 0..Self::MAX_RETRIES {
            match self.http_client.get(url).send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) if response.status().is_server_error() => {
                    let status = response.status();
                    warn!(
                        attempt = attempt + 1,
                        status = %status,
                        url = url,
                        "Retryable API error, backing off"
                    );
                    last_error = Some(Error::Api {
                        message: format!("Server error: {}", status),
                        status: Some(status.as_u16()),
                    });
                }
                Ok(response) => {
                    // Client error (4xx) — don't retry
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
    pub async fn get_markets(&self) -> Result<Vec<Market>> {
        let mut all_markets = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let url = match &cursor {
                Some(c) => format!("{}/markets?next_cursor={}", self.base_url, c),
                None => format!("{}/markets", self.base_url),
            };

            let response = self.get_with_retry(&url).await?;

            let page: MarketsResponse = response.json().await?;
            all_markets.extend(page.data.into_iter().map(Into::into));

            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => break,
            }

            // Limit to avoid infinite loops (max ~5000 markets)
            if all_markets.len() > 5000 {
                break;
            }
        }

        Ok(all_markets)
    }

    /// Fetch order book for a specific token.
    pub async fn get_order_book(&self, token_id: &str) -> Result<OrderBook> {
        let url = format!("{}/book?token_id={}", self.base_url, token_id);
        let response = self.get_with_retry(&url).await?;

        let book: ClobOrderBook = response.json().await?;
        Ok(book.into())
    }

    /// Fetch recent trades from the CLOB API.
    ///
    /// This is a public endpoint (no auth needed) that returns recent trades
    /// with maker/taker addresses — the primary source for wallet discovery.
    pub async fn get_recent_trades(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Vec<ClobTrade>> {
        let mut url = format!("{}/trades?limit={}", self.base_url, limit);
        if let Some(c) = cursor {
            url.push_str(&format!("&next_cursor={}", c));
        }

        let response = self.get_with_retry(&url).await?;
        let text = response.text().await?;

        // The CLOB API may return trades as a top-level array or wrapped in { data: [...] }
        if let Ok(trades) = serde_json::from_str::<Vec<ClobTrade>>(&text) {
            return Ok(trades);
        }

        if let Ok(wrapper) = serde_json::from_str::<TradesResponse>(&text) {
            if let Some(trades) = wrapper.data {
                return Ok(trades);
            }
        }

        // If neither format works, return empty rather than failing
        warn!("Could not parse CLOB trades response, returning empty");
        Ok(Vec::new())
    }

    /// Subscribe to real-time order book updates via WebSocket.
    /// Returns a channel receiver that yields order book updates.
    pub async fn subscribe_orderbook(
        &self,
        market_ids: Vec<String>,
    ) -> Result<mpsc::Receiver<OrderBookUpdate>> {
        let (tx, rx) = mpsc::channel(1000);
        let ws_url = self.ws_url.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::ws_loop(ws_url, market_ids, tx).await {
                warn!("WebSocket connection ended: {}", e);
            }
        });

        Ok(rx)
    }

    async fn ws_loop(
        ws_url: String,
        market_ids: Vec<String>,
        tx: mpsc::Sender<OrderBookUpdate>,
    ) -> Result<()> {
        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut write, mut read) = ws_stream.split();

        // Subscribe to markets
        for market_id in &market_ids {
            let subscribe_msg = serde_json::json!({
                "type": "subscribe",
                "market": market_id,
                "channel": "book"
            });
            write.send(Message::Text(subscribe_msg.to_string())).await?;
            debug!("Subscribed to market: {}", market_id);
        }
        info!("Subscribed to {} markets", market_ids.len());

        // Process incoming messages
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => match serde_json::from_str::<WsMessage>(&text) {
                    Ok(ws_msg) => {
                        if let Some(update) = ws_msg.into_update() {
                            if tx.send(update).await.is_err() {
                                warn!("Receiver dropped, closing WebSocket");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to parse WebSocket message: {} - {}", e, text);
                    }
                },
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await?;
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    warn!("WebSocket receive error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }
}

/// A trade from the CLOB API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClobTrade {
    /// Unique trade ID.
    pub id: String,
    /// Taker order ID.
    #[serde(default)]
    pub taker_order_id: String,
    /// Market condition ID.
    #[serde(default)]
    pub market: String,
    /// Asset/token ID.
    pub asset_id: String,
    /// Trade side (BUY/SELL).
    pub side: String,
    /// Trade size.
    pub size: String,
    /// Trade price.
    pub price: String,
    /// Maker address.
    pub maker_address: String,
    /// Trader address (taker).
    #[serde(default)]
    pub trader_address: Option<String>,
    /// Timestamp string.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Status of the trade.
    #[serde(default)]
    pub status: Option<String>,
    /// Match time (Unix timestamp).
    #[serde(default)]
    pub match_time: Option<String>,
}

/// Response wrapper for paginated trades.
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
#[serde(tag = "type", rename_all = "snake_case")]
enum WsMessage {
    Book {
        market: String,
        asset_id: String,
        bids: Vec<ClobPriceLevel>,
        asks: Vec<ClobPriceLevel>,
        timestamp: String,
    },
    #[serde(other)]
    Other,
}

impl WsMessage {
    fn into_update(self) -> Option<OrderBookUpdate> {
        match self {
            WsMessage::Book {
                market,
                asset_id,
                bids,
                asks,
                timestamp,
            } => Some(OrderBookUpdate {
                market_id: market,
                asset_id,
                timestamp: timestamp.parse().unwrap_or_else(|_| chrono::Utc::now()),
                bids: bids
                    .into_iter()
                    .map(|l| PriceLevel {
                        price: l.price.parse().unwrap_or_default(),
                        size: l.size.parse().unwrap_or_default(),
                    })
                    .collect(),
                asks: asks
                    .into_iter()
                    .map(|l| PriceLevel {
                        price: l.price.parse().unwrap_or_default(),
                        size: l.size.parse().unwrap_or_default(),
                    })
                    .collect(),
            }),
            WsMessage::Other => None,
        }
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
}

/// Response from posting an order.
#[derive(Debug, Clone, Deserialize)]
pub struct PostOrderResponse {
    /// Order ID assigned by the CLOB.
    #[serde(rename = "orderID")]
    pub order_id: String,
    /// Status of the order.
    pub status: String,
    /// Transaction hash if applicable.
    #[serde(rename = "transactionHash")]
    pub transaction_hash: Option<String>,
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
    /// Order signer for EIP-712 signing.
    signer: OrderSigner,
    /// API credentials for L2 authentication (optional until derived).
    credentials: Option<ApiCredentials>,
}

impl AuthenticatedClobClient {
    /// Create a new authenticated client.
    pub fn new(client: ClobClient, signer: OrderSigner) -> Self {
        Self {
            client,
            signer,
            credentials: None,
        }
    }

    /// Create with pre-existing API credentials.
    pub fn with_credentials(
        client: ClobClient,
        signer: OrderSigner,
        credentials: ApiCredentials,
    ) -> Self {
        Self {
            client,
            signer,
            credentials: Some(credentials),
        }
    }

    /// Get the wallet address.
    pub fn address(&self) -> String {
        format!("{:?}", self.signer.address())
    }

    /// Derive API credentials from wallet signature (L1 authentication).
    ///
    /// This authenticates with Polymarket using your wallet signature
    /// and returns API credentials for subsequent requests.
    pub async fn derive_api_key(&mut self) -> Result<ApiCredentials> {
        let timestamp = current_timestamp();

        // Sign the auth message with timestamp
        let signature = self
            .signer
            .sign_auth_message_with_timestamp(timestamp)
            .await
            .map_err(|e| Error::Signing {
                message: format!("Failed to sign auth message: {}", e),
            })?;

        let url = format!("{}/auth/derive-api-key", self.client.base_url);

        let body = serde_json::json!({
            "address": self.address(),
            "signature": signature,
            "timestamp": timestamp,
            "nonce": 0
        });

        let response = self
            .client
            .http_client
            .post(&url)
            .json(&body)
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

    /// Set API credentials directly (if already have them).
    pub fn set_credentials(&mut self, credentials: ApiCredentials) {
        self.credentials = Some(credentials);
    }

    /// Check if credentials are available.
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some()
    }

    /// Create and sign an order.
    pub async fn create_order(
        &self,
        token_id: &str,
        side: crate::signing::OrderSide,
        price: Decimal,
        size: Decimal,
        expiration_secs: u64,
    ) -> Result<SignedOrder> {
        let order = self
            .signer
            .order_builder()
            .token_id_str(token_id)
            .side(side)
            .price(price)
            .size(size)
            .expires_in(expiration_secs)
            .build()
            .ok_or_else(|| Error::Order {
                message: "Failed to build order - missing required fields".to_string(),
            })?;

        let signed = self
            .signer
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
        };

        let body = serde_json::to_string(&request)?;

        // Sign the request with L2 HMAC
        let signature = sign_l2_request(credentials, method, path, &timestamp, Some(&body))?;

        let response = self
            .client
            .http_client
            .post(&url)
            .header("POLY-ADDRESS", self.address())
            .header("POLY-SIGNATURE", signature)
            .header("POLY-TIMESTAMP", &timestamp)
            .header("POLY-API-KEY", &credentials.api_key)
            .header("POLY-PASSPHRASE", &credentials.api_passphrase)
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
            .header("POLY-ADDRESS", self.address())
            .header("POLY-SIGNATURE", signature)
            .header("POLY-TIMESTAMP", &timestamp)
            .header("POLY-API-KEY", &credentials.api_key)
            .header("POLY-PASSPHRASE", &credentials.api_passphrase)
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

        let path = match market {
            Some(m) => format!("/orders?market={}", m),
            None => "/orders".to_string(),
        };
        let url = format!("{}{}", self.client.base_url, path);
        let timestamp = current_timestamp().to_string();
        let method = "GET";

        let signature = sign_l2_request(credentials, method, &path, &timestamp, None)?;

        let response = self
            .client
            .http_client
            .get(&url)
            .header("POLY-ADDRESS", self.address())
            .header("POLY-SIGNATURE", signature)
            .header("POLY-TIMESTAMP", &timestamp)
            .header("POLY-API-KEY", &credentials.api_key)
            .header("POLY-PASSPHRASE", &credentials.api_passphrase)
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
            .header("POLY-ADDRESS", self.address())
            .header("POLY-SIGNATURE", signature)
            .header("POLY-TIMESTAMP", &timestamp)
            .header("POLY-API-KEY", &credentials.api_key)
            .header("POLY-PASSPHRASE", &credentials.api_passphrase)
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

    // Decode the secret (it's base64 encoded)
    let secret_bytes = base64::engine::general_purpose::STANDARD
        .decode(&credentials.api_secret)
        .map_err(|e| Error::Signing {
            message: format!("Invalid API secret encoding: {}", e),
        })?;

    // Create HMAC-SHA256
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret_bytes).map_err(|e| Error::Signing {
        message: format!("Failed to create HMAC: {}", e),
    })?;

    mac.update(message.as_bytes());
    let result = mac.finalize();

    // Return as base64
    Ok(base64::engine::general_purpose::STANDARD.encode(result.into_bytes()))
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
                3600,                // 1 hour
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

        // Should be valid base64
        assert!(base64::engine::general_purpose::STANDARD
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

        assert!(base64::engine::general_purpose::STANDARD
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
