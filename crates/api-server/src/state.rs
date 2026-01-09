//! Application state shared across handlers.

use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::websocket::{OrderbookUpdate, PositionUpdate, SignalUpdate};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: PgPool,
    /// JWT secret for token validation.
    pub jwt_secret: String,
    /// Broadcast channel for orderbook updates.
    pub orderbook_tx: broadcast::Sender<OrderbookUpdate>,
    /// Broadcast channel for position updates.
    pub position_tx: broadcast::Sender<PositionUpdate>,
    /// Broadcast channel for trading signals.
    pub signal_tx: broadcast::Sender<SignalUpdate>,
}

impl AppState {
    /// Create a new application state.
    pub fn new(
        pool: PgPool,
        jwt_secret: String,
        orderbook_tx: broadcast::Sender<OrderbookUpdate>,
        position_tx: broadcast::Sender<PositionUpdate>,
        signal_tx: broadcast::Sender<SignalUpdate>,
    ) -> Self {
        Self {
            pool,
            jwt_secret,
            orderbook_tx,
            position_tx,
            signal_tx,
        }
    }

    /// Subscribe to orderbook updates.
    pub fn subscribe_orderbook(&self) -> broadcast::Receiver<OrderbookUpdate> {
        self.orderbook_tx.subscribe()
    }

    /// Subscribe to position updates.
    pub fn subscribe_positions(&self) -> broadcast::Receiver<PositionUpdate> {
        self.position_tx.subscribe()
    }

    /// Subscribe to signal updates.
    pub fn subscribe_signals(&self) -> broadcast::Receiver<SignalUpdate> {
        self.signal_tx.subscribe()
    }

    /// Publish an orderbook update.
    pub fn publish_orderbook(&self, update: OrderbookUpdate) -> Result<usize, broadcast::error::SendError<OrderbookUpdate>> {
        self.orderbook_tx.send(update)
    }

    /// Publish a position update.
    pub fn publish_position(&self, update: PositionUpdate) -> Result<usize, broadcast::error::SendError<PositionUpdate>> {
        self.position_tx.send(update)
    }

    /// Publish a signal update.
    pub fn publish_signal(&self, update: SignalUpdate) -> Result<usize, broadcast::error::SendError<SignalUpdate>> {
        self.signal_tx.send(update)
    }
}

/// Extension trait for Arc<AppState>.
impl AppState {
    /// Create an Arc-wrapped state.
    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}
