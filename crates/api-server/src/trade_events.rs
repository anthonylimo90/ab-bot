use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::broadcast;
use tracing::warn;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TradeEventUpdate {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub strategy: String,
    pub execution_mode: String,
    pub source: String,
    pub market_id: String,
    pub position_id: Option<Uuid>,
    pub signal_id: Option<Uuid>,
    pub event_type: String,
    pub state_from: Option<String>,
    pub state_to: Option<String>,
    pub reason: Option<String>,
    pub direction: Option<String>,
    pub confidence: Option<f64>,
    pub expected_edge: Option<Decimal>,
    pub observed_edge: Option<Decimal>,
    pub requested_size_usd: Option<Decimal>,
    pub filled_size_usd: Option<Decimal>,
    pub fill_price: Option<Decimal>,
    pub realized_pnl: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct NewTradeEvent {
    pub strategy: String,
    pub execution_mode: String,
    pub source: String,
    pub market_id: String,
    pub position_id: Option<Uuid>,
    pub signal_id: Option<Uuid>,
    pub event_type: String,
    pub state_from: Option<String>,
    pub state_to: Option<String>,
    pub reason: Option<String>,
    pub direction: Option<String>,
    pub confidence: Option<f64>,
    pub expected_edge: Option<Decimal>,
    pub observed_edge: Option<Decimal>,
    pub requested_size_usd: Option<Decimal>,
    pub filled_size_usd: Option<Decimal>,
    pub fill_price: Option<Decimal>,
    pub realized_pnl: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
    pub metadata: serde_json::Value,
}

impl NewTradeEvent {
    pub fn new(
        strategy: impl Into<String>,
        execution_mode: impl Into<String>,
        source: impl Into<String>,
        market_id: impl Into<String>,
        event_type: impl Into<String>,
    ) -> Self {
        Self {
            strategy: strategy.into(),
            execution_mode: execution_mode.into(),
            source: source.into(),
            market_id: market_id.into(),
            position_id: None,
            signal_id: None,
            event_type: event_type.into(),
            state_from: None,
            state_to: None,
            reason: None,
            direction: None,
            confidence: None,
            expected_edge: None,
            observed_edge: None,
            requested_size_usd: None,
            filled_size_usd: None,
            fill_price: None,
            realized_pnl: None,
            unrealized_pnl: None,
            metadata: serde_json::json!({}),
        }
    }
}

#[derive(Clone)]
pub struct TradeEventRecorder {
    pool: PgPool,
    tx: broadcast::Sender<TradeEventUpdate>,
}

impl TradeEventRecorder {
    pub fn new(pool: PgPool, tx: broadcast::Sender<TradeEventUpdate>) -> Self {
        Self { pool, tx }
    }

    pub async fn record(&self, event: NewTradeEvent) -> anyhow::Result<TradeEventUpdate> {
        let update = TradeEventUpdate {
            id: Uuid::new_v4(),
            occurred_at: Utc::now(),
            strategy: event.strategy,
            execution_mode: event.execution_mode,
            source: event.source,
            market_id: event.market_id,
            position_id: event.position_id,
            signal_id: event.signal_id,
            event_type: event.event_type,
            state_from: event.state_from,
            state_to: event.state_to,
            reason: event.reason,
            direction: event.direction,
            confidence: event.confidence,
            expected_edge: event.expected_edge,
            observed_edge: event.observed_edge,
            requested_size_usd: event.requested_size_usd,
            filled_size_usd: event.filled_size_usd,
            fill_price: event.fill_price,
            realized_pnl: event.realized_pnl,
            unrealized_pnl: event.unrealized_pnl,
            metadata: ensure_object(event.metadata),
        };

        sqlx::query(
            r#"
            INSERT INTO trade_events (
                id, occurred_at, strategy, execution_mode, source, market_id,
                position_id, signal_id, event_type, state_from, state_to, reason,
                direction, confidence, expected_edge, observed_edge, requested_size_usd,
                filled_size_usd, fill_price, realized_pnl, unrealized_pnl, metadata
            )
            VALUES (
                $1, $2, $3, $4, $5, $6,
                $7, $8, $9, $10, $11, $12,
                $13, $14, $15, $16, $17,
                $18, $19, $20, $21, $22
            )
            "#,
        )
        .bind(update.id)
        .bind(update.occurred_at)
        .bind(&update.strategy)
        .bind(&update.execution_mode)
        .bind(&update.source)
        .bind(&update.market_id)
        .bind(update.position_id)
        .bind(update.signal_id)
        .bind(&update.event_type)
        .bind(&update.state_from)
        .bind(&update.state_to)
        .bind(&update.reason)
        .bind(&update.direction)
        .bind(update.confidence)
        .bind(update.expected_edge)
        .bind(update.observed_edge)
        .bind(update.requested_size_usd)
        .bind(update.filled_size_usd)
        .bind(update.fill_price)
        .bind(update.realized_pnl)
        .bind(update.unrealized_pnl)
        .bind(&update.metadata)
        .execute(&self.pool)
        .await?;

        let _ = self.tx.send(update.clone());
        Ok(update)
    }

    pub async fn record_warn(&self, event: NewTradeEvent) {
        if let Err(error) = self.record(event).await {
            warn!(error = %error, "Failed to persist trade event");
        }
    }
}

fn ensure_object(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(_) => value,
        _ => serde_json::json!({ "value": value }),
    }
}
