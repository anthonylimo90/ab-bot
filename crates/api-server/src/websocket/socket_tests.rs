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
