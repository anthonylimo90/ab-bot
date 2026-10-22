// Withdrawal capacity and chain reconciliation against a scripted JSON-RPC node.
use super::*;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::Executor;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub(crate) const TRADING: &str = "0x1111111111111111111111111111111111111111";
const OTHER: &str = "0x2222222222222222222222222222222222222222";
pub(crate) const DEST: &str = "0x3333333333333333333333333333333333333333";
/// The block test receipts are mined in, unless a test says otherwise.
const MINED_BLOCK: u64 = 16;

/// The chain as the mock node reports it.
pub(crate) struct Chain {
    /// Mined transactions by hash: success (`false` reverted) and block.
    pub receipts: HashMap<String, (bool, u64)>,
    /// Transactions the node knows (pending or mined).
    pub known: HashSet<String>,
    /// Transaction counts: mined at the final block, mined at the head, and
    /// mined plus pending.
    pub final_nonce: u64,
    pub latest_nonce: u64,
    pub pending_nonce: u64,
    pub head: u64,
    /// The `finalized` block, when the node reports one.
    pub finalized: Option<u64>,
    pub usdc_units: u128,
    pub pol_wei: u128,
    /// When set, `eth_sendRawTransaction` answers with this error.
    pub send_error: Option<String>,
    /// Whether accepted transactions are mined at once (in `MINED_BLOCK`).
    pub mine_on_send: bool,
    /// Raw transactions sent to the node.
    pub broadcasts: Vec<String>,
    /// Receipt `status` values that replace the canonical "0x1"/"0x0".
    pub raw_statuses: HashMap<String, Value>,
}

impl Default for Chain {
    fn default() -> Self {
        Self {
            receipts: HashMap::new(),
            known: HashSet::new(),
            final_nonce: 0,
            latest_nonce: 0,
            pending_nonce: 0,
            head: 200,
            finalized: Some(100),
            usdc_units: 0,
            pol_wei: 0,
            send_error: None,
            mine_on_send: false,
            broadcasts: Vec::new(),
            raw_statuses: HashMap::new(),
        }
    }
}

impl Chain {
    pub(crate) fn mine(&mut self, hash: &str, success: bool) {
        self.receipts
            .insert(hash.to_string(), (success, MINED_BLOCK));
        self.known.insert(hash.to_string());
    }
}

/// Hash of a raw transaction: test transactions are "0xraw" followed by
/// their hash; real signed ones hash their bytes.
fn raw_hash(raw: &str) -> String {
    match raw.strip_prefix("0xraw") {
        Some(hash) => hash.to_string(),
        None => {
            let bytes = hex::decode(raw.trim_start_matches("0x")).unwrap();
            format!("{:#x}", alloy_primitives::keccak256(bytes))
        }
    }
}

pub(crate) async fn mock_node(chain: Arc<Mutex<Chain>>) -> PolygonClient {
    let app = Router::new().route(
        "/",
        post(move |Json(request): Json<Value>| {
            let chain = chain.clone();
            async move {
                let mut chain = chain.lock().unwrap();
                let params = &request["params"];
                let hex = |value: u128| json!(format!("{value:#x}"));
                let result = match request["method"].as_str().unwrap() {
                    "eth_getTransactionReceipt" => {
                        let hash = params[0].as_str().unwrap();
                        match chain.receipts.get(hash) {
                            Some((ok, block)) => json!({
                                "status": chain.raw_statuses.get(hash).cloned()
                                    .unwrap_or_else(|| json!(if *ok { "0x1" } else { "0x0" })),
                                "blockNumber": format!("{block:#x}"),
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
                    "eth_getTransactionCount" => hex(u128::from(match params[1].as_str() {
                        Some("pending") => chain.pending_nonce,
                        Some("latest") => chain.latest_nonce,
                        _ => chain.final_nonce,
                    })),
                    "eth_getBlockByNumber" => match chain.finalized {
                        Some(block) if params[0] == "finalized" => {
                            json!({"number": format!("{block:#x}")})
                        }
                        _ => Value::Null,
                    },
                    "eth_blockNumber" => hex(u128::from(chain.head)),
                    "eth_call" => hex(chain.usdc_units),
                    "eth_getBalance" => hex(chain.pol_wei),
                    "eth_gasPrice" => hex(30_000_000_000),
                    "eth_sendRawTransaction" => {
                        let raw = params[0].as_str().unwrap().to_string();
                        chain.broadcasts.push(raw.clone());
                        if let Some(message) = chain.send_error.clone() {
                            return Json(json!({
                                "jsonrpc": "2.0", "id": 1,
                                "error": {"code": -32000, "message": message}
                            }));
                        }
                        let hash = raw_hash(&raw);
                        chain.known.insert(hash.clone());
                        if chain.mine_on_send {
                            chain.mine(&hash, true);
                        }
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

/// The tables withdrawals touch, with one user and the canonical workspace,
/// whose trading wallet is `TRADING`.
pub(crate) async fn setup(pool: &PgPool) -> (Uuid, Uuid) {
    pool.execute(
        r#"
        CREATE TABLE IF NOT EXISTS users (id UUID PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS workspaces (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(), trading_wallet_address TEXT,
            live_trading_enabled BOOLEAN DEFAULT TRUE, exit_handler_enabled BOOLEAN,
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
        include_str!("../../../../migrations/088_withdrawal_reconcile_state.sql"),
        include_str!("../../../../migrations/089_withdrawal_mined_seen.sql"),
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

pub(crate) struct Row<'a> {
    pub wallet: &'a str,
    pub amount: i64,
    pub status: &'a str,
    pub tx_hash: Option<&'a str>,
    pub nonce: Option<i64>,
    pub journaled: bool,
    pub age_minutes: i64,
}

pub(crate) async fn insert(pool: &PgPool, user: Uuid, workspace: Uuid, row: Row<'_>) -> Uuid {
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

pub(crate) async fn status(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, error FROM wallet_withdrawals WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn cash_flows(pool: &PgPool) -> Vec<(Uuid, Decimal)> {
    sqlx::query_as("SELECT id, amount FROM cash_flow_events ORDER BY created_at")
        .fetch_all(pool)
        .await
        .unwrap()
}

fn row(
    wallet: &'static str,
    status: &'static str,
    tx_hash: Option<&'static str>,
    nonce: Option<i64>,
    age_minutes: i64,
) -> Row<'static> {
    Row {
        wallet,
        amount: 10,
        status,
        tx_hash,
        nonce,
        journaled: true,
        age_minutes,
    }
}

async fn reconcile(pool: &PgPool, client: &PolygonClient) {
    reconcile_with(pool, |_| {
        let client = client.clone();
        async move { Some(client) }
    })
    .await
    .unwrap();
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
    let holds = WithdrawalHolds::load(&mut conn, TRADING).await.unwrap();
    assert!(holds.trading_wallet);
    let capacity = holds.capacity(balance_units);
    assert_eq!(capacity.available_usdc, Decimal::from(100));
    assert_eq!(capacity.in_flight_withdrawals, Decimal::from(5));
    assert_eq!(capacity.reserved_for_open_buys, Decimal::new(55, 1));
    assert_eq!(capacity.max_withdrawable, Decimal::new(895, 1));
    assert!(capacity.blocking_reasons.is_empty());

    // Another vault wallet carries no trading reserve.
    let other = WithdrawalHolds::load(&mut conn, OTHER).await.unwrap();
    assert!(!other.trading_wallet);
    assert_eq!(
        other.capacity(balance_units).max_withdrawable,
        Decimal::from(100)
    );

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
    let blocked = WithdrawalHolds::load(&mut conn, TRADING).await.unwrap();
    assert_eq!(blocked.blocking_reasons.len(), 1, "{blocked:?}");
    let blocked_other = WithdrawalHolds::load(&mut conn, OTHER).await.unwrap();
    assert_eq!(blocked_other.blocking_reasons.len(), 1, "{blocked_other:?}");
}

#[sqlx::test(migrations = false)]
async fn every_unresolved_withdrawal_is_settled_from_the_chain(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain {
        final_nonce: 6,
        latest_nonce: 6,
        pending_nonce: 6,
        ..Default::default()
    }));
    let client = mock_node(chain.clone()).await;

    let confirmed = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xc1"), Some(1), 1),
    )
    .await;
    let confirmed_other = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitted", Some("0xc2"), Some(1), 1),
    )
    .await;
    let reverted = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xr1"), Some(2), 1),
    )
    .await;
    let unsettled = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xu1"), Some(3), 1),
    )
    .await;
    let lost = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitting", Some("0xl1"), Some(6), 5),
    )
    .await;
    let held = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitting", Some("0xk1"), Some(7), 0),
    )
    .await;
    let waiting = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitted", Some("0xw1"), Some(7), 1),
    )
    .await;
    let never_signed = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "pending", None, None, 20),
    )
    .await;
    let mut legacy = row(TRADING, "pending", None, None, 600);
    legacy.journaled = false;
    let legacy = insert(&pool, user, workspace, legacy).await;
    {
        let mut chain = chain.lock().unwrap();
        chain.mine("0xc1", true);
        chain.mine("0xc2", true);
        chain.mine("0xr1", false);
        // Mined above the final block: it could still be reorganized away.
        chain.receipts.insert("0xu1".into(), (true, 150));
        chain.known.insert("0xk1".into());
        chain.known.insert("0xw1".into());
    }

    for _ in 0..2 {
        reconcile(&pool, &client).await;
    }

    assert_eq!(status(&pool, confirmed).await.0, "confirmed");
    assert_eq!(status(&pool, confirmed_other).await.0, "confirmed");
    let (reverted_status, reverted_error) = status(&pool, reverted).await;
    assert_eq!(reverted_status, "failed");
    assert_eq!(reverted_error.as_deref(), Some(REVERTED_ERROR));
    assert_eq!(status(&pool, unsettled).await.0, "submitted");
    // Nonce 6 is still free and the node lost the transfer: sent again, once.
    assert_eq!(status(&pool, lost).await.0, "submitted");
    // A node holds it: it was sent.
    assert_eq!(status(&pool, held).await.0, "submitted");
    assert_eq!(status(&pool, waiting).await.0, "submitted");
    assert_eq!(status(&pool, never_signed).await.0, "failed");
    assert_eq!(status(&pool, legacy).await.0, "pending");
    assert_eq!(
        chain.lock().unwrap().broadcasts,
        vec!["0xraw0xl1".to_string()]
    );

    // Seen mined: the breaker is told, though nothing is settled yet.
    let seen: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT mined_seen_at FROM wallet_withdrawals WHERE id = $1")
            .bind(unsettled)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(seen.is_some());

    // The block becomes final: settled then, not before.
    chain.lock().unwrap().finalized = Some(160);
    reconcile(&pool, &client).await;
    assert_eq!(status(&pool, unsettled).await.0, "confirmed");

    // One cash flow each, for the trading wallet only, keyed by the withdrawal.
    let flows = cash_flows(&pool).await;
    assert_eq!(flows.len(), 2, "{flows:?}");
    assert!(flows.contains(&(confirmed, Decimal::from(-10))));
    assert!(flows.contains(&(unsettled, Decimal::from(-10))));
}

#[sqlx::test(migrations = false)]
async fn a_used_nonce_fails_a_transfer_only_on_a_second_look_minutes_later(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain {
        final_nonce: 6,
        latest_nonce: 6,
        pending_nonce: 6,
        ..Default::default()
    }));
    let client = mock_node(chain.clone()).await;
    let superseded = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitting", Some("0xs1"), Some(5), 1),
    )
    .await;
    // Used only above the final block: not proof yet.
    let shallow = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitting", Some("0xs2"), Some(6), 1),
    )
    .await;
    chain.lock().unwrap().latest_nonce = 7;

    for _ in 0..2 {
        reconcile(&pool, &client).await;
    }
    assert_eq!(status(&pool, superseded).await.0, "submitting");
    assert_eq!(status(&pool, shallow).await.0, "submitting");
    let marked: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT superseded_seen_at FROM wallet_withdrawals WHERE id = $1")
            .bind(superseded)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(marked.is_some());

    pool.execute(
        "UPDATE wallet_withdrawals SET superseded_seen_at = superseded_seen_at - INTERVAL '6 minutes'",
    )
    .await
    .unwrap();
    reconcile(&pool, &client).await;
    let (superseded_status, superseded_error) = status(&pool, superseded).await;
    assert_eq!(superseded_status, "failed");
    assert!(superseded_error.unwrap().contains("nonce"));
    assert_eq!(status(&pool, shallow).await.0, "submitting");
    assert!(cash_flows(&pool).await.is_empty());
}

#[sqlx::test(migrations = false)]
async fn a_failed_transfer_that_is_mined_after_all_is_confirmed(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain::default()));
    let client = mock_node(chain.clone()).await;
    let late = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "failed", Some("0xf1"), Some(1), 60),
    )
    .await;
    let expired = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "failed", Some("0xf2"), Some(2), 60 * 25),
    )
    .await;
    let reverted = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "failed", Some("0xf3"), Some(3), 60),
    )
    .await;
    sqlx::query("UPDATE wallet_withdrawals SET error = $2 WHERE id = $1")
        .bind(reverted)
        .bind(REVERTED_ERROR)
        .execute(&pool)
        .await
        .unwrap();

    // Nothing on chain: a failed transfer is never re-sent or re-judged.
    reconcile(&pool, &client).await;
    assert_eq!(status(&pool, late).await.0, "failed");
    assert!(chain.lock().unwrap().broadcasts.is_empty());

    {
        let mut chain = chain.lock().unwrap();
        chain.mine("0xf1", true);
        chain.mine("0xf2", true);
        chain.mine("0xf3", true);
    }
    reconcile(&pool, &client).await;
    let (late_status, late_error) = status(&pool, late).await;
    assert_eq!(late_status, "confirmed");
    assert_eq!(late_error, None);
    assert_eq!(cash_flows(&pool).await, vec![(late, Decimal::from(-10))]);
    // Older than the watch window, or settled by its own receipt: left alone.
    assert_eq!(status(&pool, expired).await.0, "failed");
    assert_eq!(status(&pool, reverted).await.0, "failed");
}

#[sqlx::test(migrations = false)]
async fn a_refused_resend_leaves_the_transfer_to_the_chain(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain {
        final_nonce: 4,
        latest_nonce: 4,
        pending_nonce: 4,
        send_error: Some("nonce too low".into()),
        ..Default::default()
    }));
    let client = mock_node(chain.clone()).await;
    let id = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitting", Some("0xn1"), Some(4), 5),
    )
    .await;

    // Refused, but not in a way that proves it unsent; and not re-sent until
    // the interval has passed again.
    for _ in 0..2 {
        reconcile(&pool, &client).await;
    }
    assert_eq!(status(&pool, id).await.0, "submitting");
    assert_eq!(chain.lock().unwrap().broadcasts.len(), 1);
}

#[sqlx::test(migrations = false)]
async fn a_busy_wallet_is_left_to_the_request_sending_from_it(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain::default()));
    chain.lock().unwrap().mine("0xc1", true);
    let client = mock_node(chain).await;
    let id = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xc1"), Some(1), 1),
    )
    .await;

    let holder = SpendLock::acquire(&pool, TRADING).await.unwrap();
    reconcile(&pool, &client).await;
    assert_eq!(status(&pool, id).await.0, "submitted");

    holder.release().await;
    reconcile(&pool, &client).await;
    assert_eq!(status(&pool, id).await.0, "confirmed");
}

#[sqlx::test(migrations = false)]
async fn a_dropped_spend_lock_frees_the_wallet(pool: PgPool) {
    let mut held = SpendLock::acquire(&pool, TRADING).await.unwrap();
    held.lock_entries().await.unwrap();
    assert!(SpendLock::try_acquire(&pool, TRADING)
        .await
        .unwrap()
        .is_none());
    let mut entry = pool.begin().await.unwrap();
    let entries_free: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext('position-exposure'))")
            .fetch_one(&mut *entry)
            .await
            .unwrap();
    assert!(!entries_free, "entries wait while capacity is decided");
    entry.rollback().await.unwrap();
    // An early return or a cancelled task drops the lock without releasing it.
    drop(held);
    let again = tokio::time::timeout(
        StdDuration::from_secs(5),
        SpendLock::acquire(&pool, TRADING),
    )
    .await
    .expect("the dropped holder's session ended")
    .unwrap();
    let mut entries = pool.begin().await.unwrap();
    let free: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext('position-exposure'))")
            .fetch_one(&mut *entries)
            .await
            .unwrap();
    assert!(free);
    again.release().await;
}

#[sqlx::test(migrations = false)]
async fn only_an_explicit_receipt_status_settles_a_transfer(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain::default()));
    let client = mock_node(chain.clone()).await;
    let padded = insert(
        &pool,
        user,
        workspace,
        row(TRADING, "submitted", Some("0xp1"), Some(1), 1),
    )
    .await;
    let unreadable = insert(
        &pool,
        user,
        workspace,
        row(OTHER, "submitted", Some("0xp2"), Some(1), 1),
    )
    .await;
    {
        let mut chain = chain.lock().unwrap();
        chain.mine("0xp1", true);
        chain.mine("0xp2", true);
        chain.raw_statuses.insert("0xp1".into(), json!("0x01"));
        chain.raw_statuses.insert("0xp2".into(), Value::Null);
    }
    reconcile(&pool, &client).await;
    assert_eq!(status(&pool, padded).await.0, "confirmed");
    // A receipt without a readable status is not a revert: still unresolved.
    assert_eq!(status(&pool, unreadable).await.0, "submitted");
}

#[sqlx::test(migrations = false)]
async fn a_stalled_transfer_is_found_mined_before_the_reconciler_looks(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let chain = Arc::new(Mutex::new(Chain::default()));
    let client = mock_node(chain.clone()).await;
    let mined = || {
        let client = client.clone();
        stalled_transfer_mined(&pool, workspace, 30, move |_| {
            let client = client.clone();
            async move { Some(client) }
        })
    };
    let sent = |hash: &'static str, minutes: i32, trading: Option<bool>| {
        let pool = pool.clone();
        async move {
            let id = insert(
                &pool,
                user,
                workspace,
                row(TRADING, "submitted", Some(hash), Some(1), 0),
            )
            .await;
            sqlx::query(
                "UPDATE wallet_withdrawals SET requested_at = NOW() - make_interval(mins => $2), \
                 trading_wallet = $3 WHERE id = $1",
            )
            .bind(id)
            .bind(minutes)
            .bind(trading)
            .execute(&pool)
            .await
            .unwrap();
            id
        }
    };
    let stalled = sent("0xs1", 45, Some(true)).await;
    // Recent transfers are held for by their age; others' wallets never.
    sent("0xs2", 5, Some(true)).await;
    sent("0xs3", 45, Some(false)).await;
    let seen = || async {
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT mined_seen_at FROM wallet_withdrawals WHERE id = $1",
        )
        .bind(stalled)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    // Still unmined: nothing to hold for.
    assert!(!mined().await.unwrap());
    assert_eq!(seen().await, None);

    // Only the transfers it does not cover are mined: still nothing.
    {
        let mut chain = chain.lock().unwrap();
        chain.mine("0xs2", true);
        chain.mine("0xs3", true);
    }
    assert!(!mined().await.unwrap());

    // The stalled transfer is mined (its block not yet final): found, and
    // recorded so the breaker keeps holding until it settles.
    chain.lock().unwrap().mine("0xs1", true);
    assert!(mined().await.unwrap());
    assert!(seen().await.is_some());
    // Seen once, it is the reconciler's to settle; it is not read again.
    assert!(!mined().await.unwrap());
}
