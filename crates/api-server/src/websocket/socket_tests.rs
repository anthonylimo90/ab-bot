use super::*;
use axum::{routing::get, Router};
use sqlx::Executor;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};
use uuid::Uuid;

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn fixture(pool: sqlx::PgPool) -> (SocketState, String, String, tokio::task::JoinHandle<()>) {
    pool.execute("CREATE TABLE workspaces(id UUID PRIMARY KEY,created_at TIMESTAMPTZ DEFAULT NOW(),trading_wallet_address TEXT,live_trading_enabled BOOLEAN DEFAULT TRUE,exit_handler_enabled BOOLEAN DEFAULT FALSE,arb_auto_execute BOOLEAN DEFAULT FALSE);
        CREATE TABLE workspace_members(workspace_id UUID,user_id UUID,role TEXT);
        CREATE TABLE users(id UUID PRIMARY KEY,role SMALLINT DEFAULT 0,auth_version BIGINT DEFAULT 0);
        CREATE TABLE revoked_sessions(jti TEXT);
        INSERT INTO workspaces(id) VALUES('00000000-0000-0000-0000-000000000001');
        INSERT INTO users(id) VALUES('00000000-0000-0000-0000-000000000002');
        INSERT INTO workspace_members VALUES('00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000002','viewer')").await.unwrap();
    let jwt_auth = Arc::new(auth::jwt::JwtAuth::new(auth::jwt::JwtConfig {
        secret: "socket-protocol-test-secret-at-least-32-bytes".into(),
        expiry_hours: 1,
        issuer: None,
        audience: None,
    }));
    let token = jwt_auth
        .create_token(
            "00000000-0000-0000-0000-000000000002",
            auth::UserRole::Viewer,
        )
        .unwrap();
    let state = SocketState {
        pool,
        jwt_auth,
        orderbook: broadcast::channel(2).0,
        positions: broadcast::channel(2).0,
        signals: broadcast::channel(2).0,
        trade_flow: broadcast::channel(2).0,
        auth_interval: Duration::from_secs(15),
        // No verdict caching by default, so these protocol tests keep checking the
        // database on every event; the caching tests use the routes below.
        auth_policy: AuthPolicy {
            verdict_ttl: Duration::ZERO,
            ..AuthPolicy::default()
        },
    };
    let mut router = Router::new();
    for (path, channel) in [
        ("/positions", Channel::Positions),
        ("/signals", Channel::Signals),
        ("/trade-flow", Channel::TradeFlow),
        ("/all", Channel::All),
        ("/orderbook", Channel::Orderbook),
    ] {
        router = router.route(
            path,
            get(
                move |ws: WebSocketUpgrade, State(state): State<SocketState>| async move {
                    ws.on_upgrade(move |socket| handle_socket(socket, state, channel))
                },
            ),
        );
    }
    router = router.route(
        "/idle",
        get(
            |ws: WebSocketUpgrade, State(mut state): State<SocketState>| async move {
                state.auth_interval = Duration::from_millis(20);
                ws.on_upgrade(move |socket| handle_socket(socket, state, Channel::Positions))
            },
        ),
    );
    // (path, auth_interval, verdict_ttl, transient_grace) for the caching tests.
    for (path, interval, ttl, grace) in [
        // Events are served from the cache for 2s; the idle timer is far away.
        ("/cached-events", 15_000, 2_000, 60_000),
        // Long cache, fast idle timer: revocation must still be noticed promptly.
        ("/cached-timer", 60, 30_000, 60_000),
        // No cache, short grace: for database-outage behaviour.
        ("/outage", 100, 0, 1_000),
    ] {
        router = router.route(
            path,
            get(
                move |ws: WebSocketUpgrade, State(mut state): State<SocketState>| async move {
                    state.auth_interval = Duration::from_millis(interval);
                    state.auth_policy = AuthPolicy {
                        verdict_ttl: Duration::from_millis(ttl),
                        transient_grace: Duration::from_millis(grace),
                    };
                    ws.on_upgrade(move |socket| handle_socket(socket, state, Channel::Positions))
                },
            ),
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let app = router.with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (state, token, url, server)
}

async fn read(client: &mut Client) -> ClientMessage {
    tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}
async fn authenticated(url: &str, path: &str, token: &str) -> Client {
    let (mut client, _) = connect_async(format!("{url}/{path}")).await.unwrap();
    client
        .send(ClientMessage::Text(
            serde_json::json!({"action":"authenticate","token":token}).to_string(),
        ))
        .await
        .unwrap();
    let ack: WsMessage =
        serde_json::from_str(&read(&mut client).await.into_text().unwrap()).unwrap();
    assert!(matches!(ack, WsMessage::Subscribed { .. }));
    client
}
fn publish(state: &SocketState, channel: Channel) -> &'static str {
    match channel {
        Channel::Orderbook => {
            let _ = state.orderbook.send(OrderbookUpdate {
                market_id: "market".into(),
                timestamp: Utc::now(),
                yes_bid: Decimal::ONE,
                yes_ask: Decimal::ONE,
                no_bid: Decimal::ZERO,
                no_ask: Decimal::ONE,
                arb_spread: None,
            });
            "Orderbook"
        }
        Channel::Positions | Channel::All => {
            let _ = state.positions.send(PositionUpdate {
                position_id: Uuid::new_v4(),
                market_id: "market".into(),
                update_type: PositionUpdateType::Updated,
                quantity: Decimal::ONE,
                current_price: Decimal::ONE,
                unrealized_pnl: Decimal::ZERO,
                timestamp: Utc::now(),
            });
            "Position"
        }
        Channel::Signals => {
            let _ = state.signals.send(SignalUpdate {
                signal_id: Uuid::new_v4(),
                signal_type: SignalType::Alert,
                market_id: "market".into(),
                outcome_id: "yes".into(),
                action: "update".into(),
                confidence: 1.0,
                timestamp: Utc::now(),
                metadata: serde_json::json!({}),
            });
            "Signal"
        }
        Channel::TradeFlow => {
            let event=serde_json::from_value(serde_json::json!({"id":Uuid::new_v4(),"occurred_at":Utc::now(),"strategy":"arb","execution_mode":"paper","source":"arb","market_id":"market","event_type":"updated","metadata":{}})).unwrap();
            let _ = state.trade_flow.send(event);
            "TradeFlow"
        }
    }
}
fn assert_policy_close(message: ClientMessage) {
    let ClientMessage::Close(Some(frame)) = message else {
        panic!("Expected policy close")
    };
    assert_eq!(u16::from(frame.code), 1008);
}

#[sqlx::test(migrations = false)]
async fn every_private_channel_authenticates_before_subscribing_and_revokes_before_data(
    pool: sqlx::PgPool,
) {
    let (state, token, url, server) = fixture(pool).await;
    for channel in [
        Channel::Positions,
        Channel::Signals,
        Channel::TradeFlow,
        Channel::All,
    ] {
        let (mut client, _) = connect_async(format!("{url}/{}", channel.name()))
            .await
            .unwrap();
        assert_eq!(
            state.positions.receiver_count()
                + state.signals.receiver_count()
                + state.trade_flow.receiver_count()
                + state.orderbook.receiver_count(),
            0
        );
        publish(&state, channel);
        client
            .send(ClientMessage::Text(
                serde_json::json!({"action":"authenticate","token":token}).to_string(),
            ))
            .await
            .unwrap();
        let ack: WsMessage =
            serde_json::from_str(&read(&mut client).await.into_text().unwrap()).unwrap();
        assert!(matches!(ack, WsMessage::Subscribed { .. }));
        client
            .send(ClientMessage::Text("{\"action\":\"ping\"}".into()))
            .await
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<WsMessage>(&read(&mut client).await.into_text().unwrap())
                .unwrap(),
            WsMessage::Pong
        ));
        let kind = publish(&state, channel);
        let event: serde_json::Value =
            serde_json::from_str(&read(&mut client).await.into_text().unwrap()).unwrap();
        assert_eq!(event["type"], kind);
        state
            .pool
            .execute("DELETE FROM workspace_members")
            .await
            .unwrap();
        publish(&state, channel);
        assert_policy_close(read(&mut client).await);
        state.pool.execute("INSERT INTO workspace_members VALUES('00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000002','viewer')").await.unwrap();
    }
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn invalid_auth_idle_revocation_and_public_eof_use_common_transport(pool: sqlx::PgPool) {
    let (state, token, url, server) = fixture(pool).await;
    let (mut invalid, _) = connect_async(format!("{url}/all")).await.unwrap();
    invalid
        .send(ClientMessage::Text(
            "{\"action\":\"authenticate\",\"token\":\"bad\"}".into(),
        ))
        .await
        .unwrap();
    assert_policy_close(read(&mut invalid).await);
    let mut idle = authenticated(&url, "idle", &token).await;
    state
        .pool
        .execute("UPDATE users SET auth_version=1")
        .await
        .unwrap();
    assert_policy_close(read(&mut idle).await); // no publication required
    state.pool.execute("DROP TABLE workspace_members; DROP TABLE users; DROP TABLE revoked_sessions; DROP TABLE workspaces").await.unwrap();
    let (mut public, _) = connect_async(format!("{url}/orderbook")).await.unwrap();
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut public).await.into_text().unwrap()).unwrap(),
        WsMessage::Subscribed { .. }
    ));
    publish(&state, Channel::Orderbook);
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut public).await.into_text().unwrap()).unwrap(),
        WsMessage::Orderbook(_)
    ));
    drop(public);
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.orderbook.receiver_count() > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn lag_is_reported_on_the_wire_and_stream_resumes(pool: sqlx::PgPool) {
    let (state, token, url, server) = fixture(pool).await;
    let mut client = authenticated(&url, "positions", &token).await;
    let mut lock = state.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE users IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    for _ in 0..100 {
        publish(&state, Channel::Positions);
    }
    lock.rollback().await.unwrap();
    let mut lagged = false;
    for _ in 0..4 {
        let message: WsMessage =
            serde_json::from_str(&read(&mut client).await.into_text().unwrap()).unwrap();
        if matches!(message,WsMessage::Error{ref code,..} if code=="LAGGED") {
            lagged = true;
            break;
        }
    }
    assert!(lagged);
    publish(&state, Channel::Positions);
    let message: WsMessage =
        serde_json::from_str(&read(&mut client).await.into_text().unwrap()).unwrap();
    assert!(matches!(message, WsMessage::Position(_)));
    server.abort();
}

/// The connection is gone (close frame, error or EOF) rather than serving a frame.
async fn ended(client: &mut Client) -> bool {
    match tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .unwrap()
    {
        None | Some(Err(_)) | Some(Ok(ClientMessage::Close(_))) => true,
        Some(Ok(other)) => panic!("frame was served: {other:?}"),
    }
}

const MEMBERSHIP: &str = "INSERT INTO workspace_members VALUES('00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000002','viewer')";

#[sqlx::test(migrations = false)]
async fn events_are_served_from_the_cached_verdict_until_it_expires(pool: sqlx::PgPool) {
    let (state, token, url, server) = fixture(pool).await;
    let mut client = authenticated(&url, "cached-events", &token).await;

    // While the verdict is fresh no event touches the database: with the users
    // table locked, any per-event authorization query would hang.
    let mut lock = state.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE users IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    publish(&state, Channel::Positions);
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut client).await.into_text().unwrap()).unwrap(),
        WsMessage::Position(_)
    ));
    lock.rollback().await.unwrap();

    // Revoked, but still inside the cache window: the stale verdict is used...
    state
        .pool
        .execute("DELETE FROM workspace_members")
        .await
        .unwrap();
    publish(&state, Channel::Positions);
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut client).await.into_text().unwrap()).unwrap(),
        WsMessage::Position(_)
    ));

    // ...and the first event after the cache expires re-checks and closes (1008).
    tokio::time::sleep(Duration::from_millis(2_200)).await;
    publish(&state, Channel::Positions);
    assert_policy_close(read(&mut client).await);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn revocation_closes_within_the_refresh_window_even_when_idle_and_cached(pool: sqlx::PgPool) {
    let (state, token, url, server) = fixture(pool).await;
    // 30s cache but a 60ms idle timer: the timer always re-checks.
    let mut client = authenticated(&url, "cached-timer", &token).await;
    state
        .pool
        .execute("DELETE FROM workspace_members")
        .await
        .unwrap();
    assert_policy_close(read(&mut client).await); // no event published
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_database_outage_is_survived_for_the_grace_period_then_the_socket_closes(
    pool: sqlx::PgPool,
) {
    let (state, token, url, server) = fixture(pool).await;
    let mut client = authenticated(&url, "outage", &token).await;
    // Every authorization query now fails with a database (not an authorization) error.
    state.pool.close().await;
    publish(&state, Channel::Positions);
    // Inside the 1s grace the last positive verdict keeps the stream alive.
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut client).await.into_text().unwrap()).unwrap(),
        WsMessage::Position(_)
    ));
    // Beyond it the socket is closed (1013 try-again-later, not a policy violation),
    // with no event needed: the idle timer notices.
    match read(&mut client).await {
        ClientMessage::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 1013),
        other => panic!("expected close, got {other:?}"),
    }
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_definitive_denial_is_not_softened_by_the_outage_grace(pool: sqlx::PgPool) {
    let (state, token, url, server) = fixture(pool).await;
    let mut client = authenticated(&url, "outage", &token).await;
    // The database answers, and the answer is "no": closed at once with 1008.
    state
        .pool
        .execute("DELETE FROM workspace_members")
        .await
        .unwrap();
    publish(&state, Channel::Positions);
    assert_policy_close(read(&mut client).await);
    state.pool.execute(MEMBERSHIP).await.unwrap();
    server.abort();
}

#[test]
fn verdict_cache_state_machine() {
    use tokio::time::Instant;
    let at = |base: Instant, secs: u64| base + Duration::from_secs(secs);
    let t0 = Instant::now();
    let policy = AuthPolicy::default();
    assert_eq!(policy.verdict_ttl, Duration::from_secs(15));
    assert_eq!(policy.transient_grace, Duration::from_secs(60));

    let mut auth = StreamAuth::new("token".into(), policy, t0);
    assert!(auth.is_fresh(at(t0, 14)));
    assert!(!auth.is_fresh(at(t0, 15)));

    // A definitive "no" closes immediately, even on a fresh cache.
    assert_eq!(auth.apply(at(t0, 1), Check::Denied), Some(Stop::Revoked));

    // Database errors: keep serving for at most 60s after the last positive verdict,
    // and retries are paced by the ttl rather than made for every event.
    let mut auth = StreamAuth::new("token".into(), policy, t0);
    assert_eq!(auth.apply(at(t0, 20), Check::Unavailable), None);
    assert!(auth.is_fresh(at(t0, 21)));
    assert!(!auth.is_fresh(at(t0, 35)));
    assert_eq!(auth.apply(at(t0, 60), Check::Unavailable), None);
    assert_eq!(
        auth.apply(at(t0, 61), Check::Unavailable),
        Some(Stop::AuthorizationUnavailable)
    );

    // A successful check resets the grace window.
    let mut auth = StreamAuth::new("token".into(), policy, t0);
    assert_eq!(auth.apply(at(t0, 50), Check::Unavailable), None);
    assert_eq!(auth.apply(at(t0, 55), Check::Allowed), None);
    assert_eq!(auth.apply(at(t0, 100), Check::Unavailable), None);
    assert_eq!(
        auth.apply(at(t0, 116), Check::Unavailable),
        Some(Stop::AuthorizationUnavailable)
    );
}

#[test]
fn only_database_errors_count_as_transient() {
    use crate::error::ApiError;
    assert_eq!(Check::from_result(Ok(())), Check::Allowed);
    assert_eq!(
        Check::from_result(Err(ApiError::Database(sqlx::Error::PoolClosed))),
        Check::Unavailable
    );
    assert_eq!(
        Check::from_result(Err(ApiError::Database(sqlx::Error::PoolTimedOut))),
        Check::Unavailable
    );
    for denied in [
        ApiError::Unauthorized("revoked".into()),
        ApiError::Forbidden("not a member".into()),
    ] {
        assert_eq!(Check::from_result(Err(denied)), Check::Denied);
    }
}

#[sqlx::test(migrations = false)]
async fn oversized_inbound_frames_are_refused_on_public_and_private_sockets(pool: sqlx::PgPool) {
    let state = AppState::for_tests(pool);
    let app = Router::new()
        .route("/ws/orderbook", get(ws_orderbook_handler))
        .route("/ws/positions", get(ws_positions_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let padded_ping =
        |bytes: usize| serde_json::json!({"action": "ping", "pad": "x".repeat(bytes)}).to_string();

    // Public stream: a 32 KiB frame is fine, a 100 KiB one ends the connection.
    let (mut public, _) = connect_async(format!("{url}/ws/orderbook")).await.unwrap();
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut public).await.into_text().unwrap()).unwrap(),
        WsMessage::Subscribed { .. }
    ));
    public
        .send(ClientMessage::Text(padded_ping(32 * 1024)))
        .await
        .unwrap();
    assert!(matches!(
        serde_json::from_str::<WsMessage>(&read(&mut public).await.into_text().unwrap()).unwrap(),
        WsMessage::Pong
    ));
    public
        .send(ClientMessage::Text(padded_ping(100 * 1024)))
        .await
        .unwrap();
    assert!(ended(&mut public).await);

    // Private stream: an oversized authentication frame is refused too.
    let (mut private, _) = connect_async(format!("{url}/ws/positions")).await.unwrap();
    let oversized = serde_json::json!({"action": "authenticate", "token": "x".repeat(100 * 1024)});
    let _ = private
        .send(ClientMessage::Text(oversized.to_string()))
        .await;
    assert!(ended(&mut private).await);
    server.abort();
}
