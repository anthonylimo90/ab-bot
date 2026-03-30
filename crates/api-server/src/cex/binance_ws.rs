//! Binance WebSocket client for real-time BTC/ETH aggTrade feeds.

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use super::price_tracker::{CexPriceTick, CexSymbol};

/// Configuration for the Binance WebSocket client.
#[derive(Debug, Clone)]
pub struct BinanceWsConfig {
    /// Combined stream URL. Default: wss://stream.binance.com:9443
    pub base_url: String,
    /// Streams to subscribe to.
    pub streams: Vec<String>,
    /// Initial reconnect delay in milliseconds.
    pub reconnect_delay_ms: u64,
    /// Maximum reconnect delay in milliseconds.
    pub max_reconnect_delay_ms: u64,
}

impl BinanceWsConfig {
    pub fn from_env() -> Self {
        Self {
            base_url: std::env::var("BINANCE_WS_URL")
                .unwrap_or_else(|_| "wss://stream.binance.com:9443".to_string()),
            streams: vec![
                "btcusdt@aggTrade".to_string(),
                "ethusdt@aggTrade".to_string(),
            ],
            reconnect_delay_ms: 1000,
            max_reconnect_delay_ms: 30000,
        }
    }

    fn build_url(&self) -> String {
        let streams = self.streams.join("/");
        format!("{}/stream?streams={}", self.base_url, streams)
    }
}

/// Raw Binance aggTrade event (we only parse the fields we need).
#[derive(Debug, Deserialize)]
struct BinanceStreamWrapper {
    stream: String,
    data: AggTradeData,
}

#[derive(Debug, Deserialize)]
struct AggTradeData {
    /// Price as string (Binance sends numbers as strings).
    p: String,
    /// Aggregate trade ID.
    #[serde(rename = "a")]
    _agg_trade_id: u64,
}

fn stream_to_symbol(stream: &str) -> Option<CexSymbol> {
    if stream.starts_with("btcusdt") {
        Some(CexSymbol::BtcUsdt)
    } else if stream.starts_with("ethusdt") {
        Some(CexSymbol::EthUsdt)
    } else {
        None
    }
}

/// Spawn a Binance WebSocket client that sends price ticks on the channel.
pub fn spawn_binance_ws_client(
    config: BinanceWsConfig,
    price_tx: mpsc::Sender<CexPriceTick>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reconnect_delay = time::Duration::from_millis(config.reconnect_delay_ms);
        let max_delay = time::Duration::from_millis(config.max_reconnect_delay_ms);

        loop {
            let url = config.build_url();
            info!(url = %url, "Connecting to Binance WebSocket");

            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    info!("Binance WebSocket connected");
                    reconnect_delay = time::Duration::from_millis(config.reconnect_delay_ms);

                    let (mut _write, mut read) = ws_stream.split();

                    loop {
                        match tokio::time::timeout(time::Duration::from_secs(30), read.next()).await
                        {
                            Ok(Some(Ok(Message::Text(text)))) => {
                                match serde_json::from_str::<BinanceStreamWrapper>(&text) {
                                    Ok(wrapper) => {
                                        let Some(symbol) = stream_to_symbol(&wrapper.stream) else {
                                            continue;
                                        };
                                        let Ok(price) = wrapper.data.p.parse::<f64>() else {
                                            continue;
                                        };
                                        let tick = CexPriceTick {
                                            symbol,
                                            price,
                                            received_at: time::Instant::now(),
                                        };
                                        if price_tx.try_send(tick).is_err() {
                                            // Channel full — drop oldest tick (receiver is slow)
                                            debug!("Binance price channel full, dropping tick");
                                        }
                                    }
                                    Err(e) => {
                                        debug!(error = %e, "Failed to parse Binance message");
                                    }
                                }
                            }
                            Ok(Some(Ok(Message::Ping(data)))) => {
                                if let Err(e) = _write.send(Message::Pong(data)).await {
                                    warn!(error = %e, "Failed to send pong");
                                    break;
                                }
                            }
                            Ok(Some(Ok(Message::Close(_)))) => {
                                info!("Binance WebSocket closed by server");
                                break;
                            }
                            Ok(Some(Err(e))) => {
                                warn!(error = %e, "Binance WebSocket error");
                                break;
                            }
                            Ok(None) => {
                                info!("Binance WebSocket stream ended");
                                break;
                            }
                            Err(_) => {
                                warn!("Binance WebSocket read timeout (30s)");
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to connect to Binance WebSocket");
                }
            }

            warn!(
                delay_ms = reconnect_delay.as_millis(),
                "Reconnecting to Binance WebSocket"
            );
            tokio::time::sleep(reconnect_delay).await;
            reconnect_delay = (reconnect_delay * 2).min(max_delay);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_to_symbol() {
        assert_eq!(
            stream_to_symbol("btcusdt@aggTrade"),
            Some(CexSymbol::BtcUsdt)
        );
        assert_eq!(
            stream_to_symbol("ethusdt@aggTrade"),
            Some(CexSymbol::EthUsdt)
        );
        assert_eq!(stream_to_symbol("unknown@aggTrade"), None);
    }

    #[test]
    fn test_build_url() {
        let config = BinanceWsConfig {
            base_url: "wss://stream.binance.com:9443".to_string(),
            streams: vec![
                "btcusdt@aggTrade".to_string(),
                "ethusdt@aggTrade".to_string(),
            ],
            reconnect_delay_ms: 1000,
            max_reconnect_delay_ms: 30000,
        };
        assert_eq!(
            config.build_url(),
            "wss://stream.binance.com:9443/stream?streams=btcusdt@aggTrade/ethusdt@aggTrade"
        );
    }
}
