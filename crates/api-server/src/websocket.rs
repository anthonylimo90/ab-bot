//! WebSocket handlers for real-time updates.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use utoipa::ToSchema;

use crate::state::AppState;

/// Orderbook update message.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OrderbookUpdate {
    /// Market identifier.
    pub market_id: String,
    /// Update timestamp.
    pub timestamp: DateTime<Utc>,
    /// Yes outcome best bid.
    pub yes_bid: Decimal,
    /// Yes outcome best ask.
    pub yes_ask: Decimal,
    /// No outcome best bid.
    pub no_bid: Decimal,
    /// No outcome best ask.
    pub no_ask: Decimal,
    /// Arbitrage spread (if any).
    pub arb_spread: Option<Decimal>,
}

/// Position update message.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PositionUpdate {
    /// Position identifier.
    pub position_id: uuid::Uuid,
    /// Market identifier.
    pub market_id: String,
    /// Update type.
    pub update_type: PositionUpdateType,
    /// Current quantity.
    pub quantity: Decimal,
    /// Current price.
    pub current_price: Decimal,
    /// Unrealized P&L.
    pub unrealized_pnl: Decimal,
    /// Update timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Type of position update.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PositionUpdateType {
    Opened,
    Updated,
    Closed,
    PriceChanged,
}

/// Trading signal update message.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SignalUpdate {
    /// Signal identifier.
    pub signal_id: uuid::Uuid,
    /// Signal type.
    pub signal_type: SignalType,
    /// Market identifier.
    pub market_id: String,
    /// Outcome identifier.
    pub outcome_id: String,
    /// Suggested action.
    pub action: String,
    /// Signal confidence (0.0-1.0).
    pub confidence: f64,
    /// Signal timestamp.
    pub timestamp: DateTime<Utc>,
    /// Additional data.
    pub metadata: serde_json::Value,
}

/// Type of trading signal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Arbitrage,
    CopyTrade,
    StopLoss,
    TakeProfit,
    Alert,
}

/// WebSocket message wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsMessage {
    /// Orderbook update.
    Orderbook(OrderbookUpdate),
    /// Position update.
    Position(PositionUpdate),
    /// Trading signal.
    Signal(SignalUpdate),
    /// Subscription confirmation.
    Subscribed { channel: String },
    /// Unsubscription confirmation.
    Unsubscribed { channel: String },
    /// Error message.
    Error { code: String, message: String },
    /// Ping/pong for keepalive.
    Ping,
    Pong,
}

/// Client subscription request.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action")]
pub enum WsRequest {
    /// Subscribe to a channel.
    #[serde(rename = "subscribe")]
    Subscribe { channel: String, filters: Option<serde_json::Value> },
    /// Unsubscribe from a channel.
    #[serde(rename = "unsubscribe")]
    Unsubscribe { channel: String },
    /// Ping for keepalive.
    #[serde(rename = "ping")]
    Ping,
}

/// WebSocket upgrade handler for orderbook updates.
pub async fn ws_orderbook_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_orderbook_socket(socket, state))
}

/// WebSocket upgrade handler for position updates.
pub async fn ws_positions_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_positions_socket(socket, state))
}

/// WebSocket upgrade handler for trading signals.
pub async fn ws_signals_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_signals_socket(socket, state))
}

/// WebSocket upgrade handler for all updates (multiplexed).
pub async fn ws_all_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_all_socket(socket, state))
}

async fn handle_orderbook_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut orderbook_rx = state.subscribe_orderbook();

    info!("WebSocket client connected to orderbook channel");

    // Send subscription confirmation
    let msg = WsMessage::Subscribed { channel: "orderbook".to_string() };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = sender.send(Message::Text(json)).await;
    }

    loop {
        tokio::select! {
            // Handle incoming messages from client
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(request) = serde_json::from_str::<WsRequest>(&text) {
                            match request {
                                WsRequest::Ping => {
                                    let pong = WsMessage::Pong;
                                    if let Ok(json) = serde_json::to_string(&pong) {
                                        let _ = sender.send(Message::Text(json)).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        debug!("WebSocket client disconnected from orderbook");
                        break;
                    }
                    Err(e) => {
                        warn!(error = %e, "WebSocket error");
                        break;
                    }
                    _ => {}
                }
            }
            // Handle orderbook updates
            Ok(update) = orderbook_rx.recv() => {
                let msg = WsMessage::Orderbook(update);
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket client disconnected from orderbook channel");
}

async fn handle_positions_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut position_rx = state.subscribe_positions();

    info!("WebSocket client connected to positions channel");

    let msg = WsMessage::Subscribed { channel: "positions".to_string() };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = sender.send(Message::Text(json)).await;
    }

    loop {
        tokio::select! {
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(WsRequest::Ping) = serde_json::from_str(&text) {
                            let pong = WsMessage::Pong;
                            if let Ok(json) = serde_json::to_string(&pong) {
                                let _ = sender.send(Message::Text(json)).await;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
            Ok(update) = position_rx.recv() => {
                let msg = WsMessage::Position(update);
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket client disconnected from positions channel");
}

async fn handle_signals_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut signal_rx = state.subscribe_signals();

    info!("WebSocket client connected to signals channel");

    let msg = WsMessage::Subscribed { channel: "signals".to_string() };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = sender.send(Message::Text(json)).await;
    }

    loop {
        tokio::select! {
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(WsRequest::Ping) = serde_json::from_str(&text) {
                            let pong = WsMessage::Pong;
                            if let Ok(json) = serde_json::to_string(&pong) {
                                let _ = sender.send(Message::Text(json)).await;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
            Ok(update) = signal_rx.recv() => {
                let msg = WsMessage::Signal(update);
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket client disconnected from signals channel");
}

async fn handle_all_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let mut orderbook_rx = state.subscribe_orderbook();
    let mut position_rx = state.subscribe_positions();
    let mut signal_rx = state.subscribe_signals();

    info!("WebSocket client connected to all channels");

    let msg = WsMessage::Subscribed { channel: "all".to_string() };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = sender.send(Message::Text(json)).await;
    }

    loop {
        tokio::select! {
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(WsRequest::Ping) = serde_json::from_str(&text) {
                            let pong = WsMessage::Pong;
                            if let Ok(json) = serde_json::to_string(&pong) {
                                let _ = sender.send(Message::Text(json)).await;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
            Ok(update) = orderbook_rx.recv() => {
                let msg = WsMessage::Orderbook(update);
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
            Ok(update) = position_rx.recv() => {
                let msg = WsMessage::Position(update);
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
            Ok(update) = signal_rx.recv() => {
                let msg = WsMessage::Signal(update);
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }

    info!("WebSocket client disconnected from all channels");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_message_serialization() {
        let update = OrderbookUpdate {
            market_id: "market1".to_string(),
            timestamp: Utc::now(),
            yes_bid: Decimal::new(48, 2),
            yes_ask: Decimal::new(50, 2),
            no_bid: Decimal::new(48, 2),
            no_ask: Decimal::new(50, 2),
            arb_spread: Some(Decimal::new(2, 2)),
        };

        let msg = WsMessage::Orderbook(update);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Orderbook"));
        assert!(json.contains("market1"));
    }

    #[test]
    fn test_ws_request_deserialization() {
        let json = r#"{"action": "subscribe", "channel": "orderbook"}"#;
        let request: WsRequest = serde_json::from_str(json).unwrap();

        match request {
            WsRequest::Subscribe { channel, .. } => {
                assert_eq!(channel, "orderbook");
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_position_update() {
        let update = PositionUpdate {
            position_id: uuid::Uuid::new_v4(),
            market_id: "market1".to_string(),
            update_type: PositionUpdateType::Opened,
            quantity: Decimal::new(100, 0),
            current_price: Decimal::new(50, 2),
            unrealized_pnl: Decimal::ZERO,
            timestamp: Utc::now(),
        };

        let msg = WsMessage::Position(update);
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Position"));
        assert!(json.contains("opened"));
    }
}
