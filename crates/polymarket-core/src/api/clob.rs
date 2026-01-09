//! Polymarket CLOB API client.

use crate::types::{Market, OrderBook, Outcome, PriceLevel};
use crate::{Error, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Polymarket CLOB API client for order book data.
pub struct ClobClient {
    base_url: String,
    ws_url: String,
    http_client: reqwest::Client,
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

    /// Fetch list of active markets.
    pub async fn get_markets(&self) -> Result<Vec<Market>> {
        let url = format!("{}/markets", self.base_url);
        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(Error::Api {
                message: format!("Failed to fetch markets: {}", response.status()),
                status: Some(response.status().as_u16()),
            });
        }

        let markets: Vec<ClobMarket> = response.json().await?;
        Ok(markets.into_iter().map(Into::into).collect())
    }

    /// Fetch order book for a specific token.
    pub async fn get_order_book(&self, token_id: &str) -> Result<OrderBook> {
        let url = format!("{}/book?token_id={}", self.base_url, token_id);
        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(Error::Api {
                message: format!("Failed to fetch order book: {}", response.status()),
                status: Some(response.status().as_u16()),
            });
        }

        let book: ClobOrderBook = response.json().await?;
        Ok(book.into())
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
                error!("WebSocket error: {}", e);
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
            info!("Subscribed to market: {}", market_id);
        }

        // Process incoming messages
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<WsMessage>(&text) {
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
                    }
                }
                Ok(Message::Ping(data)) => {
                    write.send(Message::Pong(data)).await?;
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
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
struct ClobMarket {
    condition_id: String,
    question: String,
    description: Option<String>,
    tokens: Vec<ClobToken>,
    volume: String,
    liquidity: String,
    end_date: Option<String>,
    closed: bool,
    resolved: bool,
}

#[derive(Debug, Deserialize)]
struct ClobToken {
    token_id: String,
    outcome: String,
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
            volume: m.volume.parse().unwrap_or_default(),
            liquidity: m.liquidity.parse().unwrap_or_default(),
            end_date: m.end_date.and_then(|s| s.parse().ok()),
            resolved: m.resolved,
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
