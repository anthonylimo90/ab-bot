// Withdrawal capacity and chain reconciliation against a scripted JSON-RPC node.
use super::*;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::Executor;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const TRADING: &str = "0x1111111111111111111111111111111111111111";
const OTHER: &str = "0x2222222222222222222222222222222222222222";
const DEST: &str = "0x3333333333333333333333333333333333333333";

/// The chain as the mock node reports it.
#[derive(Default)]
struct Chain {
    /// Mined transactions by hash: `true` succeeded, `false` reverted.
    receipts: HashMap<String, bool>,
    /// Transactions the node knows (pending or mined).
    known: HashSet<String>,
    /// Mined ("latest") and mined-plus-pending transaction counts.
    latest_nonce: u64,
    pending_nonce: u64,
    /// Raw transactions sent to the node.
    broadcasts: Vec<String>,
}

async fn mock_node(chain: Arc<Mutex<Chain>>) -> PolygonClient {
    let app = Router::new().route(
        "/",
        post(move |Json(request): Json<Value>| {
            let chain = chain.clone();
            async move {
                let mut chain = chain.lock().unwrap();
                let params = &request["params"];
                let result = match request["method"].as_str().unwrap() {
                    "eth_getTransactionReceipt" => {
                        match chain.receipts.get(params[0].as_str().unwrap()) {
                            Some(ok) => json!({
                                "status": if *ok { "0x1" } else { "0x0" },
                                "blockNumber": "0x10",
                                "gasUsed": "0x5208"
                            }),
                            None => Value::Null,
                        }
                    }
                    "eth_getTransactionByHash" => {
                        if chain.known.contains(params[0].as_str().unwrap()) {
                            json!({"hash": params[0]})
                        } else {
                            Value::Null
                        }
                    }
                    "eth_getTransactionCount" => {
                        let count = if params[1] == "pending" {
                            chain.pending_nonce
                        } else {
                            chain.latest_nonce
                        };
                        json!(format!("{count:#x}"))
                    }
                    "eth_sendRawTransaction" => {
                        // Test transactions are "0xraw" followed by their hash.
                        let raw = params[0].as_str().unwrap().to_string();
                        let hash = raw.trim_start_matches("0xraw").to_string();
                        chain.broadcasts.push(raw);
                        chain.known.insert(hash.clone());
                        json!(hash)
                    }
                    other => panic!("unexpected RPC method {other}"),
                };
                Json(json!({"jsonrpc": "2.0", "id": 1, "result": result}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    PolygonClient::new(format!("http://{address}"))
}

async fn setup(pool: &PgPool) -> (Uuid, Uuid) {
    pool.execute(
        r#"
        CREATE TABLE users (id UUID PRIMARY KEY);
        CREATE TABLE workspaces (
            id UUID PRIMARY KEY, trading_wallet_address TEXT,
            live_trading_enabled BOOLEAN, exit_handler_enabled BOOLEAN,
            arb_auto_execute BOOLEAN, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE TABLE positions (id UUID, state SMALLINT, execution_mode TEXT);
        CREATE TABLE orders (
            quantity NUMERIC, filled_quantity NUMERIC, price NUMERIC, side TEXT,
            status TEXT, execution_mode TEXT, wallet_address TEXT
        );
        CREATE TABLE cash_flow_events (
            id UUID PRIMARY KEY, workspace_id UUID NOT NULL, event_type TEXT NOT NULL,
            amount NUMERIC NOT NULL, currency TEXT NOT NULL DEFAULT 'USDC', note TEXT,
            occurred_at TIMESTAMPTZ NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            created_by UUID
        );
        "#,
    )
    .await
    .unwrap();
    for migration in [
        include_str!("../../../../migrations/059_wallet_withdrawals.sql"),
        include_str!("../../../../migrations/061_withdrawal_idempotency.sql"),
        include_str!("../../../../migrations/069_execution_intents.sql"),
        include_str!("../../../../migrations/082_withdrawal_journal.sql"),
    ] {
        pool.execute(migration).await.unwrap();
    }
    let user = Uuid::new_v4();
    let workspace = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id) VALUES ($1)")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workspaces (id, trading_wallet_address, live_trading_enabled) VALUES ($1, $2, TRUE)",
    )
    .bind(workspace)
    .bind(TRADING)
    .execute(pool)
    .await
    .unwrap();
    (user, workspace)
}

struct Row<'a> {
    wallet: &'a str,
    amount: i64,
    status: &'a str,
    tx_hash: Option<&'a str>,
    nonce: Option<i64>,
    journaled: bool,
    age_minutes: i64,
}

async fn insert(pool: &PgPool, user: Uuid, workspace: Uuid, row: Row<'_>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO wallet_withdrawals (id, user_id, workspace_id, wallet_address, \
         destination_address, amount, status, tx_hash, nonce, raw_tx, journaled, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW() - make_interval(mins => $12))",
    )
    .bind(id)
    .bind(user)
    .bind(workspace)
    .bind(row.wallet)
    .bind(DEST)
    .bind(Decimal::from(row.amount))
    .bind(row.status)
    .bind(row.tx_hash)
    .bind(row.nonce)
    .bind(row.tx_hash.map(|hash| format!("0xraw{hash}")))
    .bind(row.journaled)
    .bind(row.age_minutes as i32)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn status(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, error FROM wallet_withdrawals WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn capacity_keeps_in_flight_withdrawals_and_live_buys_and_waits_for_trades(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let balance_units = 100_000_000; // 100 USDC
    insert(
        &pool,
        user,
        workspace,
        Row {
            wallet: TRADING,
            amount: 5,
            status: "submitted",
            tx_hash: Some("0xa"),
            nonce: Some(1),
            journaled: true,
            age_minutes: 0,
        },
    )
    .await;
    // 7 of 10 shares still to buy at 0.50, and a market buy of 2 (no price).
    pool.execute(
        "INSERT INTO orders VALUES \
         (10, 3, 0.5, 'buy', 'open', 'live', NULL), \
         (2, 0, NULL, 'buy', 'pending', 'live', NULL), \
         (50, 0, 0.5, 'sell', 'open', 'live', NULL), \
         (50, 0, 0.5, 'buy', 'open', 'paper', NULL)",
    )
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let capacity = withdrawal_capacity(&mut conn, TRADING, balance_units)
        .await
        .unwrap();
    assert_eq!(capacity.available_usdc, Decimal::from(100));
    assert_eq!(capacity.in_flight_withdrawals, Decimal::from(5));
    assert_eq!(capacity.reserved_for_open_buys, Decimal::new(55, 1));
    assert_eq!(capacity.max_withdrawable, Decimal::new(895, 1));
    assert!(capacity.blocking_reasons.is_empty());

    // Another vault wallet carries no trading reserve.
    let other = withdrawal_capacity(&mut conn, OTHER, balance_units)
        .await
        .unwrap();
    assert_eq!(other.max_withdrawable, Decimal::from(100));

    // A live entry in flight or an unresolved transfer blocks a withdrawal.
    pool.execute("INSERT INTO positions VALUES (gen_random_uuid(), 0, 'live')")
        .await
        .unwrap();
    insert(
        &pool,
        user,
        workspace,
        Row {
            wallet: OTHER,
            amount: 1,
            status: "submitting",
            tx_hash: Some("0xb"),
            nonce: Some(4),
            journaled: true,
            age_minutes: 0,
        },
    )
    .await;
    let blocked = withdrawal_capacity(&mut conn, TRADING, balance_units)
        .await
        .unwrap();
    assert_eq!(blocked.blocking_reasons.len(), 1, "{blocked:?}");
    let blocked_other = withdrawal_capacity(&mut conn, OTHER, balance_units)
        .await
        .unwrap();
    assert_eq!(blocked_other.blocking_reasons.len(), 1, "{blocked_other:?}");
}

#[sqlx::test(migrations = false)]
async fn every_unresolved_withdrawal_is_settled_from_the_chain(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain {
        latest_nonce: 6,
        pending_nonce: 6,
        ..Default::default()
    }));
    let client = mock_node(chain.clone()).await;
    let row = |wallet, status, tx_hash, nonce, journaled, age_minutes| Row {
        wallet,
        amount: 10,
        status,
        tx_hash,
        nonce,
        journaled,
        age_minutes,
    };

    let confirmed = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xc1"), Some(1), true, 1),
    )
    .await;
    let confirmed_other = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitted", Some("0xc2"), Some(1), true, 1),
    )
    .await;
    let reverted = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xr1"), Some(2), true, 1),
    )
    .await;
    let superseded = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitting", Some("0xs1"), Some(5), true, 1),
    )
    .await;
    let lost = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitting", Some("0xl1"), Some(6), true, 5),
    )
    .await;
    let waiting = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitted", Some("0xw1"), Some(6), true, 1),
    )
    .await;
    let never_signed = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "pending", None, None, true, 20),
    )
    .await;
    let legacy = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "pending", None, None, false, 600),
    )
    .await;
    {
        let mut chain = chain.lock().unwrap();
        chain.receipts.insert("0xc1".into(), true);
        chain.receipts.insert("0xc2".into(), true);
        chain.receipts.insert("0xr1".into(), false);
        chain.known.insert("0xw1".into());
    }

    for _ in 0..2 {
        reconcile_with(&pool, |_| {
            let client = client.clone();
            async move { Some(client) }
        })
        .await
        .unwrap();
    }

    assert_eq!(status(&pool, confirmed).await.0, "confirmed");
    assert_eq!(status(&pool, confirmed_other).await.0, "confirmed");
    let (reverted_status, reverted_error) = status(&pool, reverted).await;
    assert_eq!(reverted_status, "failed");
    assert!(reverted_error.unwrap().contains("reverted"));
    // Nonce 5 is mined (latest 6) but not by this transfer: it never will be.
    let (superseded_status, superseded_error) = status(&pool, superseded).await;
    assert_eq!(superseded_status, "failed");
    assert!(superseded_error.unwrap().contains("nonce"));
    // Nonce 6 is still free and the node lost the transfer: sent again.
    assert_eq!(status(&pool, lost).await.0, "submitted");
    assert_eq!(
        chain.lock().unwrap().broadcasts,
        vec!["0xraw0xl1".to_string()]
    );
    // Still in the mempool: left alone.
    assert_eq!(status(&pool, waiting).await.0, "submitted");
    assert_eq!(status(&pool, never_signed).await.0, "failed");
    assert_eq!(status(&pool, legacy).await.0, "pending");

    // One cash flow, once, for the trading wallet only, keyed by the withdrawal.
    let flows: Vec<(Uuid, Decimal, String)> =
        sqlx::query_as("SELECT id, amount, event_type FROM cash_flow_events")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        flows,
        vec![(confirmed, Decimal::from(-10), "withdrawal".to_string())]
    );
}

#[sqlx::test(migrations = false)]
async fn a_busy_wallet_is_left_to_the_request_sending_from_it(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain::default()));
    chain.lock().unwrap().receipts.insert("0xc1".into(), true);
    let client = mock_node(chain).await;
    let id = insert(
        &pool,
        user,
        workspace,
        Row {
            wallet: TRADING,
            amount: 10,
            status: "submitted",
            tx_hash: Some("0xc1"),
            nonce: Some(1),
            journaled: true,
            age_minutes: 1,
        },
    )
    .await;

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(wallet_lock_key(TRADING))
        .execute(&mut *holder)
        .await
        .unwrap();
    let reconcile = || {
        reconcile_with(&pool, |_| {
            let client = client.clone();
            async move { Some(client) }
        })
    };
    reconcile().await.unwrap();
    assert_eq!(status(&pool, id).await.0, "submitted");

    holder.commit().await.unwrap();
    reconcile().await.unwrap();
    assert_eq!(status(&pool, id).await.0, "confirmed");
}
