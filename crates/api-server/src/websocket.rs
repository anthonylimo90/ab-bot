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
use tracing::{info, warn};
use utoipa::ToSchema;

use crate::state::AppState;
use crate::trade_events::TradeEventUpdate;

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
    Alert,
    QuantSignal,
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
    /// Canonical trade lifecycle event.
    TradeFlow(Box<TradeEventUpdate>),
    /// Subscription confirmation.
    Subscribed {
        channel: String,
    },
    /// Unsubscription confirmation.
    Unsubscribed {
        channel: String,
    },
    /// Error message.
    Error {
        code: String,
        message: String,
    },
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
    Subscribe {
        channel: String,
        filters: Option<serde_json::Value>,
    },
    /// Unsubscribe from a channel.
    #[serde(rename = "unsubscribe")]
    Unsubscribe { channel: String },
    /// Ping for keepalive.
    #[serde(rename = "ping")]
    Ping,
}

// Browser clients authenticate in the first frame, keeping tokens out of URLs
// and access logs. No private receiver is subscribed until this succeeds.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum AuthenticationFrame {
    Authenticate { token: String },
}

async fn authorize_stream_token(
    pool: &sqlx::PgPool,
    jwt: &auth::jwt::JwtAuth,
    token: &str,
) -> crate::error::ApiResult<()> {
    crate::workspace_scope::authorize_workspace_session(pool, jwt, token).await
}

async fn authenticate_socket(socket: &mut WebSocket, state: &SocketState) -> Option<String> {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv()).await;
    if let Ok(Some(Ok(Message::Text(text)))) = frame {
        if let Ok(AuthenticationFrame::Authenticate { token }) = serde_json::from_str(&text) {
            if authorize_stream_token(&state.pool, &state.jwt_auth, &token)
                .await
                .is_ok()
            {
                return Some(token);
            }
        }
    }
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: 1008,
            reason: "Workspace authentication required".into(),
        })))
        .await;
    None
}

/// Largest inbound frame/message accepted on any socket. Clients only ever send
/// small JSON control frames (authenticate, ping, subscribe), so anything bigger
/// is refused instead of being buffered.
const MAX_INBOUND_BYTES: usize = 64 * 1024;

/// How the per-connection authorization verdict is cached.
///
/// Re-checking the database for every broadcast event of every connected client
/// is a join per event per client, and a transient database error used to close
/// every socket at once (a reconnect storm). Instead the verdict is cached and
/// refreshed at most every `verdict_ttl`.
#[derive(Clone, Copy, Debug)]
struct AuthPolicy {
    /// How long a positive verdict is trusted before an event triggers a re-check.
    verdict_ttl: std::time::Duration,
    /// How long after the last positive verdict a connection survives while the
    /// database cannot be reached (errors that are not a definitive "no").
    transient_grace: std::time::Duration,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            verdict_ttl: std::time::Duration::from_secs(15),
            transient_grace: std::time::Duration::from_secs(60),
        }
    }
}

/// Transport state kept separate from the full app for protocol-level tests.
#[derive(Clone)]
struct SocketState {
    pool: sqlx::PgPool,
    jwt_auth: Arc<auth::jwt::JwtAuth>,
    orderbook: broadcast::Sender<OrderbookUpdate>,
    positions: broadcast::Sender<PositionUpdate>,
    signals: broadcast::Sender<SignalUpdate>,
    trade_flow: broadcast::Sender<TradeEventUpdate>,
    auth_interval: std::time::Duration,
    auth_policy: AuthPolicy,
}

impl From<&AppState> for SocketState {
    fn from(state: &AppState) -> Self {
        Self {
            pool: state.pool.clone(),
            jwt_auth: state.jwt_auth.clone(),
            orderbook: state.orderbook_tx.clone(),
            positions: state.position_tx.clone(),
            signals: state.signal_tx.clone(),
            trade_flow: state.trade_event_tx.clone(),
            auth_interval: std::time::Duration::from_secs(15),
            auth_policy: AuthPolicy::default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Channel {
    Orderbook,
    Positions,
    Signals,
    TradeFlow,
    All,
}
impl Channel {
    fn name(self) -> &'static str {
        match self {
            Self::Orderbook => "orderbook",
            Self::Positions => "positions",
            Self::Signals => "signals",
            Self::TradeFlow => "trade-flow",
            Self::All => "all",
        }
    }
    fn private(self) -> bool {
        !matches!(self, Self::Orderbook)
    }
}

/// Apply the inbound size limits that every socket gets.
fn limit_inbound(ws: WebSocketUpgrade) -> WebSocketUpgrade {
    ws.max_message_size(MAX_INBOUND_BYTES)
        .max_frame_size(MAX_INBOUND_BYTES)
}

fn upgrade(ws: WebSocketUpgrade, state: Arc<AppState>, channel: Channel) -> Response {
    let state = SocketState::from(state.as_ref());
    limit_inbound(ws).on_upgrade(move |socket| handle_socket(socket, state, channel))
}

pub async fn ws_orderbook_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    upgrade(ws, state, Channel::Orderbook)
}
pub async fn ws_positions_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    upgrade(ws, state, Channel::Positions)
}
pub async fn ws_signals_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    upgrade(ws, state, Channel::Signals)
}
pub async fn ws_trade_flow_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    upgrade(ws, state, Channel::TradeFlow)
}
pub async fn ws_all_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    upgrade(ws, state, Channel::All)
}

struct Subscriptions {
    orderbook: Option<broadcast::Receiver<OrderbookUpdate>>,
    positions: Option<broadcast::Receiver<PositionUpdate>>,
    signals: Option<broadcast::Receiver<SignalUpdate>>,
    trade_flow: Option<broadcast::Receiver<TradeEventUpdate>>,
}
impl Subscriptions {
    fn new(state: &SocketState, channel: Channel) -> Self {
        Self {
            orderbook: matches!(channel, Channel::Orderbook | Channel::All)
                .then(|| state.orderbook.subscribe()),
            positions: matches!(channel, Channel::Positions | Channel::All)
                .then(|| state.positions.subscribe()),
            signals: matches!(channel, Channel::Signals | Channel::All)
                .then(|| state.signals.subscribe()),
            trade_flow: matches!(channel, Channel::TradeFlow | Channel::All)
                .then(|| state.trade_flow.subscribe()),
        }
    }
    async fn next(&mut self) -> Result<WsMessage, (&'static str, broadcast::error::RecvError)> {
        tokio::select! {
            event=receive(&mut self.orderbook)=>event.map(WsMessage::Orderbook).map_err(|e|("orderbook",e)),
            event=receive(&mut self.positions)=>event.map(WsMessage::Position).map_err(|e|("positions",e)),
            event=receive(&mut self.signals)=>event.map(WsMessage::Signal).map_err(|e|("signals",e)),
            event=receive(&mut self.trade_flow)=>event.map(|e|WsMessage::TradeFlow(Box::new(e))).map_err(|e|("trade-flow",e)),
        }
    }
}
async fn receive<T: Clone>(
    rx: &mut Option<broadcast::Receiver<T>>,
) -> Result<T, broadcast::error::RecvError> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

type SocketSender = futures_util::stream::SplitSink<WebSocket, Message>;
async fn send(sender: &mut SocketSender, message: WsMessage) -> bool {
    match serde_json::to_string(&message) {
        Ok(json) => sender.send(Message::Text(json)).await.is_ok(),
        Err(_) => false,
    }
}
/// Outcome of one authorization re-check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// Session and canonical-workspace membership are still valid.
    Allowed,
    /// A definitive "no": revoked, role changed, no longer a member.
    Denied,
    /// The database could not answer (pool timeout, connection loss, ...).
    Unavailable,
}

impl Check {
    fn from_result(result: crate::error::ApiResult<()>) -> Self {
        match result {
            Ok(()) => Self::Allowed,
            Err(crate::error::ApiError::Database(error)) => {
                warn!(error = %error, "WebSocket authorization check could not reach the database");
                Self::Unavailable
            }
            Err(_) => Self::Denied,
        }
    }
}

/// Why a connection must be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Definitive negative verdict: close immediately with 1008 (policy).
    Revoked,
    /// The database has been unreachable for longer than the grace period.
    AuthorizationUnavailable,
}

/// Cached authorization verdict for one private connection.
struct StreamAuth {
    token: String,
    policy: AuthPolicy,
    /// When the verdict was last positive.
    verified_at: tokio::time::Instant,
    /// When the database was last asked (answer or not); paces retries while it is down.
    checked_at: tokio::time::Instant,
}

impl StreamAuth {
    /// Start from the successful authorization performed at connect time.
    fn new(token: String, policy: AuthPolicy, now: tokio::time::Instant) -> Self {
        Self {
            token,
            policy,
            verified_at: now,
            checked_at: now,
        }
    }

    /// Whether an event may be delivered on the cached verdict alone.
    fn is_fresh(&self, now: tokio::time::Instant) -> bool {
        now.saturating_duration_since(self.checked_at) < self.policy.verdict_ttl
    }

    /// Fold a re-check into the cache; `Some` means close the connection.
    fn apply(&mut self, now: tokio::time::Instant, check: Check) -> Option<Stop> {
        self.checked_at = now;
        match check {
            Check::Allowed => {
                self.verified_at = now;
                None
            }
            Check::Denied => Some(Stop::Revoked),
            // Keep serving on the last positive verdict, but only for a bounded
            // time: the grace is measured from the last *positive* answer.
            Check::Unavailable => (now.saturating_duration_since(self.verified_at)
                > self.policy.transient_grace)
                .then_some(Stop::AuthorizationUnavailable),
        }
    }

    async fn recheck(&mut self, state: &SocketState) -> Option<Stop> {
        let check = Check::from_result(
            authorize_stream_token(&state.pool, &state.jwt_auth, &self.token).await,
        );
        self.apply(tokio::time::Instant::now(), check)
    }

    /// Before delivering an event: use the cached verdict while it is fresh.
    async fn on_event(&mut self, state: &SocketState) -> Option<Stop> {
        if self.is_fresh(tokio::time::Instant::now()) {
            return None;
        }
        self.recheck(state).await
    }

    /// On the idle timer: always re-check, so revocation is noticed without traffic.
    async fn on_timer(&mut self, state: &SocketState) -> Option<Stop> {
        self.recheck(state).await
    }
}

async fn close_for(sender: &mut SocketSender, stop: Stop) {
    let (code, reason) = match stop {
        Stop::Revoked => (1008, "Workspace session no longer authorized"),
        // 1013: try again later. The client should reconnect once the service is back.
        Stop::AuthorizationUnavailable => (1013, "Authorization temporarily unavailable"),
    };
    let _ = sender
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn handle_socket(mut socket: WebSocket, state: SocketState, channel: Channel) {
    let mut stream_auth = if channel.private() {
        let Some(token) = authenticate_socket(&mut socket, &state).await else {
            return;
        };
        Some(StreamAuth::new(
            token,
            state.auth_policy,
            tokio::time::Instant::now(),
        ))
    } else {
        None
    };
    // Subscribe only after authorization succeeds, including multiplexed streams.
    let mut subscriptions = Subscriptions::new(&state, channel);
    let (mut sender, mut receiver) = socket.split();
    if !send(
        &mut sender,
        WsMessage::Subscribed {
            channel: channel.name().into(),
        },
    )
    .await
    {
        return;
    }
    let mut auth_check = tokio::time::interval_at(
        tokio::time::Instant::now() + state.auth_interval,
        state.auth_interval,
    );
    auth_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    info!(channel = channel.name(), "WebSocket client connected");
    loop {
        tokio::select! {
            _=auth_check.tick(),if stream_auth.is_some()=>{
                if let Some(stop)=match stream_auth.as_mut() {Some(a)=>a.on_timer(&state).await,None=>None} {
                    close_for(&mut sender,stop).await;break
                }
            }
            incoming=receiver.next()=>match incoming {
                Some(Ok(Message::Text(text)))=>{
                    if matches!(serde_json::from_str::<WsRequest>(&text),Ok(WsRequest::Ping))
                        && !send(&mut sender,WsMessage::Pong).await {break}
                }
                None|Some(Ok(Message::Close(_)))|Some(Err(_))=>break,
                _=>{},
            },
            event=subscriptions.next()=>{
                if let Some(stop)=match stream_auth.as_mut() {Some(a)=>a.on_event(&state).await,None=>None} {
                    close_for(&mut sender,stop).await;break
                }
                let message=match event {
                    Ok(message)=>message,
                    Err((stream,broadcast::error::RecvError::Lagged(n)))=>{
                        warn!(channel=channel.name(),stream,missed=n,"WebSocket client lagged");
                        WsMessage::Error{code:"LAGGED".into(),message:format!("Missed {n} {stream} updates")}
                    }
                    Err((_,broadcast::error::RecvError::Closed))=>break,
                };
                if !send(&mut sender,message).await {break}
            }
        }
    }
    info!(channel = channel.name(), "WebSocket client disconnected");
}

#[cfg(test)]
#[path = "websocket/socket_tests.rs"]
mod socket_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = false)]
    async fn private_stream_requires_valid_token_and_current_canonical_membership(
        pool: sqlx::PgPool,
    ) {
        use sqlx::Executor;
        pool.execute("CREATE TABLE workspaces (id UUID PRIMARY KEY, created_at TIMESTAMPTZ DEFAULT NOW(), trading_wallet_address TEXT, live_trading_enabled BOOLEAN DEFAULT true, exit_handler_enabled BOOLEAN DEFAULT false, arb_auto_execute BOOLEAN DEFAULT false); CREATE TABLE workspace_members (workspace_id UUID, user_id UUID, role TEXT)").await.unwrap();
        let workspace = uuid::Uuid::new_v4();
        let user = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces (id) VALUES ($1)")
            .bind(workspace)
            .execute(&pool)
            .await
            .unwrap();
        let jwt = auth::jwt::JwtAuth::new(auth::jwt::JwtConfig {
            secret: "stream-test-secret-with-at-least-32-bytes".into(),
            expiry_hours: 1,
            issuer: None,
            audience: None,
        });
        pool.execute("CREATE TABLE users (id UUID PRIMARY KEY, role SMALLINT DEFAULT 0, auth_version BIGINT DEFAULT 0); CREATE TABLE revoked_sessions (jti TEXT)").await.unwrap();
        sqlx::query("INSERT INTO users (id) VALUES ($1)")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        let token = jwt
            .create_token(&user.to_string(), auth::UserRole::Viewer)
            .unwrap();
        assert!(authorize_stream_token(&pool, &jwt, "invalid")
            .await
            .is_err());
        assert!(authorize_stream_token(&pool, &jwt, &token).await.is_err());
        sqlx::query("INSERT INTO workspace_members VALUES ($1, $2, 'viewer')")
            .bind(workspace)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        assert!(authorize_stream_token(&pool, &jwt, &token).await.is_ok());
        // The joined query must enforce the same persistent revocations as HTTP.
        for mutation in [
            "UPDATE users SET auth_version=1",
            "UPDATE users SET role=1",
            "UPDATE workspace_members SET role='invalid'",
        ] {
            pool.execute(mutation).await.unwrap();
            assert!(authorize_stream_token(&pool, &jwt, &token).await.is_err());
            pool.execute("UPDATE users SET auth_version=0, role=0; UPDATE workspace_members SET role='viewer'")
                .await.unwrap();
            assert!(authorize_stream_token(&pool, &jwt, &token).await.is_ok());
        }
        let claims = jwt.validate_token(&token).unwrap();
        sqlx::query("INSERT INTO revoked_sessions VALUES ($1)")
            .bind(&claims.jti)
            .execute(&pool)
            .await
            .unwrap();
        assert!(authorize_stream_token(&pool, &jwt, &token).await.is_err());
        pool.execute("DELETE FROM revoked_sessions").await.unwrap();
        let expired = auth::Claims::new(user.to_string(), auth::UserRole::Viewer, -1);
        assert!(authorize_stream_token(
            &pool,
            &jwt,
            &jwt.create_token_with_claims(&expired).unwrap()
        )
        .await
        .is_err());
        pool.execute("DELETE FROM workspace_members").await.unwrap();
        assert!(authorize_stream_token(&pool, &jwt, &token).await.is_err());
        pool.execute("DELETE FROM users").await.unwrap();
        assert!(authorize_stream_token(&pool, &jwt, &token).await.is_err());
        assert!(serde_json::from_str::<AuthenticationFrame>(r#"{"action":"ping"}"#).is_err());
    }

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

    #[test]
    fn test_trade_flow_update_serialization() {
        let update = TradeEventUpdate {
            id: uuid::Uuid::new_v4(),
            occurred_at: Utc::now(),
            strategy: "flow".to_string(),
            execution_mode: "paper".to_string(),
            source: "quant".to_string(),
            market_id: "market1".to_string(),
            position_id: None,
            signal_id: None,
            event_type: "signal_generated".to_string(),
            state_from: None,
            state_to: Some("pending".to_string()),
            reason: None,
            direction: Some("buy_yes".to_string()),
            confidence: Some(0.7),
            expected_edge: None,
            observed_edge: None,
            requested_size_usd: None,
            filled_size_usd: None,
            fill_price: None,
            realized_pnl: None,
            unrealized_pnl: None,
            metadata: serde_json::json!({}),
        };

        let msg = WsMessage::TradeFlow(Box::new(update));
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("TradeFlow"));
        assert!(json.contains("signal_generated"));
    }
}
