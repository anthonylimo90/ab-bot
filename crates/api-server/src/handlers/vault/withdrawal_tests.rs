// The withdrawal send path against a scripted JSON-RPC node.
use super::*;
use crate::withdrawals::tests::{insert, mock_node, setup, status, Chain, Row, DEST};
use auth::TradingWallet;
use std::sync::Mutex;

// A public test key only; never fund this address.
const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";

struct Fixture {
    state: Arc<AppState>,
    chain: Arc<Mutex<Chain>>,
    client: PolygonClient,
    wallet: TradingWallet,
    user: Uuid,
    workspace: Uuid,
}

/// The signer is the canonical workspace's trading wallet and holds 100 USDC.
async fn fixture(pool: PgPool) -> Fixture {
    let (user, workspace) = setup(&pool).await;
    let wallet = TradingWallet::from_private_key(KEY).unwrap();
    sqlx::query("UPDATE workspaces SET trading_wallet_address = $1")
        .bind(wallet.address_string().to_lowercase())
        .execute(&pool)
        .await
        .unwrap();
    let chain = Arc::new(Mutex::new(Chain {
        usdc_units: 100_000_000,
        pol_wei: 1_000_000_000_000_000_000,
        final_nonce: 3,
        latest_nonce: 3,
        pending_nonce: 3,
        ..Default::default()
    }));
    let client = mock_node(chain.clone()).await;
    Fixture {
        state: AppState::for_tests(pool),
        chain,
        client,
        wallet,
        user,
        workspace,
    }
}

impl Fixture {
    fn request(&self, amount: i64, request_id: Uuid) -> WithdrawalRequest {
        WithdrawalRequest {
            user_id: self.user,
            workspace_id: self.workspace,
            source: self.wallet.address_string().to_lowercase(),
            destination: DEST.to_string(),
            amount: Decimal::from(amount),
            amount_units: amount as u128 * 1_000_000,
            request_id,
        }
    }

    async fn submit(&self, amount: i64, request_id: Uuid) -> ApiResult<WalletWithdrawalRow> {
        submit_withdrawal(
            self.state.clone(),
            self.client.clone(),
            self.wallet.clone(),
            self.request(amount, request_id),
        )
        .await
    }

    fn broadcasts(&self) -> usize {
        self.chain.lock().unwrap().broadcasts.len()
    }
}

#[sqlx::test(migrations = false)]
async fn a_sent_withdrawal_is_journaled_settled_and_replayed(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    f.chain.lock().unwrap().mine_on_send = true;
    let request_id = Uuid::new_v4();

    let row = f.submit(10, request_id).await.unwrap();
    assert_eq!(row.status, "confirmed");
    let (nonce, raw, trading): (i64, String, bool) = sqlx::query_as(
        "SELECT nonce, raw_tx, trading_wallet FROM wallet_withdrawals WHERE id = $1",
    )
    .bind(row.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(nonce, 3);
    assert_eq!(f.chain.lock().unwrap().broadcasts, vec![raw]);
    assert!(trading);
    let flows: Vec<(Uuid, Decimal)> = sqlx::query_as("SELECT id, amount FROM cash_flow_events")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(flows, vec![(row.id, Decimal::from(-10))]);

    // The same request again returns the transfer; nothing is sent twice.
    let replay = f.submit(10, request_id).await.unwrap();
    assert_eq!(replay.id, row.id);
    assert_eq!(f.broadcasts(), 1);
    assert!(matches!(
        f.submit(11, request_id).await,
        Err(ApiError::Conflict(_))
    ));
}

#[sqlx::test(migrations = false)]
async fn only_a_refusal_no_held_transfer_gets_fails_a_withdrawal(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    f.chain.lock().unwrap().send_error = Some("insufficient funds for gas * price + value".into());
    let refused = f.submit(10, Uuid::new_v4()).await.unwrap();
    assert_eq!(refused.status, "failed");
    assert!(refused
        .error
        .as_deref()
        .unwrap()
        .contains("insufficient funds"));

    // A provider retrying its own send can answer this for a transfer it holds.
    // The retry signs the same transfer at the same nonce, so it must differ
    // from the refused one to be journaled at all.
    f.chain.lock().unwrap().send_error = Some("nonce too low".into());
    let unclear = f.submit(10, Uuid::new_v4()).await.unwrap();
    assert_eq!(unclear.status, "submitting");
    assert_ne!(unclear.tx_hash, refused.tx_hash);
    assert!(unclear
        .error
        .unwrap()
        .contains("Do not start another transfer"));

    // A transfer that may still mine blocks the next one; nothing is sent.
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(f.broadcasts(), 2);
}

#[sqlx::test(migrations = false)]
async fn a_withdrawal_over_capacity_sends_nothing(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    // 95 of 100 USDC are already on their way out.
    insert(
        &pool,
        f.user,
        f.workspace,
        Row {
            wallet: &f.wallet.address_string().to_lowercase(),
            amount: 95,
            status: "submitted",
            tx_hash: Some("0xa1"),
            nonce: Some(2),
            journaled: true,
            age_minutes: 0,
        },
    )
    .await;
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::BadRequest(_))
    ));
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wallet_withdrawals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(f.broadcasts(), 0);

    // Five can still leave, and the request releases the entries lock.
    let row = f.submit(5, Uuid::new_v4()).await.unwrap();
    assert_eq!(status(&pool, row.id).await.0, "submitted");
    let mut entries = pool.begin().await.unwrap();
    let free: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext('position-exposure'))")
            .fetch_one(&mut *entries)
            .await
            .unwrap();
    assert!(free);
}

#[sqlx::test(migrations = false)]
async fn a_withdrawal_of_trading_collateral_pays_usdc_out_through_the_offramp(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    {
        // The wallet's cash is now pUSD, as trading leaves it.
        let mut chain = f.chain.lock().unwrap();
        chain.usdc_units = 2_000_000;
        chain.pusd_units = 100_000_000;
    }
    // Two USDC.e cover a withdrawal of one: a plain transfer.
    let row = f.submit(1, Uuid::new_v4()).await.unwrap();
    assert_eq!(status(&pool, row.id).await.0, "submitted");
    let raw = f.chain.lock().unwrap().broadcasts[0].clone();
    let (to, _, _) = polymarket_core::api::polygon::legacy_call(&raw).unwrap();
    assert_eq!(
        to,
        polymarket_core::signing::USDC_ADDRESS.to_ascii_lowercase()
    );

    // Ten need the pUSD, unwrapped by the Offramp to the recipient.
    let row = f.submit(10, Uuid::new_v4()).await.unwrap();
    assert_eq!(status(&pool, row.id).await.0, "submitted");
    let raw = f.chain.lock().unwrap().broadcasts[1].clone();
    let (to, _, input) = polymarket_core::api::polygon::legacy_call(&raw).unwrap();
    assert_eq!(
        to,
        polymarket_core::signing::COLLATERAL_OFFRAMP_ADDRESS.to_ascii_lowercase()
    );
    // The recipient, not the wallet, is paid.
    assert!(hex::encode(&input).contains(&DEST.trim_start_matches("0x").to_ascii_lowercase()));
}

#[sqlx::test(migrations = false)]
async fn an_unwrap_the_node_expects_to_fail_sends_nothing(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    {
        let mut chain = f.chain.lock().unwrap();
        chain.usdc_units = 0;
        chain.pusd_units = 100_000_000;
        chain.gas_estimate = None;
    }
    match f.submit(10, Uuid::new_v4()).await {
        Err(ApiError::Conflict(message)) => assert!(message.contains("execution reverted")),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(f.broadcasts(), 0);
    let statuses: Vec<String> = sqlx::query_scalar("SELECT status FROM wallet_withdrawals")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(statuses, ["failed"]);

    // Too little POL for gas is the requester's to fix.
    {
        let mut chain = f.chain.lock().unwrap();
        chain.gas_estimate = Some(120_000);
        chain.pol_wei = 1;
    }
    match f.submit(10, Uuid::new_v4()).await {
        Err(ApiError::BadRequest(message)) => assert!(message.contains("Insufficient POL")),
        other => panic!("expected a bad request, got {other:?}"),
    }
    assert_eq!(f.broadcasts(), 0);
}

#[sqlx::test(migrations = false)]
async fn a_withdrawal_waits_only_so_long_for_another_spend_from_the_wallet(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let held = SpendLock::acquire(&pool, &f.wallet.address_string())
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let waited = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        f.submit(10, Uuid::new_v4()),
    )
    .await
    .expect("the request must not wait without end");
    assert!(matches!(waited, Err(ApiError::ServiceUnavailable(_))));
    assert!(started.elapsed() >= SPEND_LOCK_WAIT);
    assert_eq!(f.broadcasts(), 0);
    held.release().await;
    assert!(f.submit(10, Uuid::new_v4()).await.is_ok());
}

#[test]
fn protocol_contracts_are_not_withdrawal_destinations() {
    use polymarket_core::api::polygon::is_protocol_contract;
    for contract in [
        polymarket_core::signing::USDC_ADDRESS,
        polymarket_core::signing::COLLATERAL_ADDRESS,
        polymarket_core::signing::COLLATERAL_OFFRAMP_ADDRESS,
        polymarket_core::signing::CTF_EXCHANGE_ADDRESS,
        "0x0000000000000000000000000000000000000000",
        "0x4bfb41d5b3570defd03c39a9a4d8de6bd8b8982e",
    ] {
        assert!(is_protocol_contract(&contract.to_ascii_lowercase()));
    }
    assert!(!is_protocol_contract(DEST));
}

#[sqlx::test(migrations = false)]
async fn a_withdrawal_waits_behind_a_transaction_it_did_not_send(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    // Something else the wallet sent (a wrap, an approval) waits to be mined.
    f.chain.lock().unwrap().pending_nonce = 4;
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::ServiceUnavailable(message)) if message.contains("still pending")
    ));
    assert_eq!(f.broadcasts(), 0);
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wallet_withdrawals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);

    // Once it is mined, the withdrawal goes out at the next nonce.
    f.chain.lock().unwrap().latest_nonce = 4;
    let sent = f.submit(10, Uuid::new_v4()).await.unwrap();
    assert_eq!(sent.status, "submitted");
    // Its own transfer waiting to be mined is accounted for: another may
    // follow it at the nonce after.
    f.chain.lock().unwrap().pending_nonce = 5;
    let next = f.submit(10, Uuid::new_v4()).await.unwrap();
    assert_eq!(next.status, "submitted");
    assert_eq!(f.broadcasts(), 2);
    // Mined, though their rows still say `submitted`, they no longer account
    // for a transaction now pending: one sent from elsewhere.
    {
        let mut chain = f.chain.lock().unwrap();
        chain.latest_nonce = 6;
        chain.pending_nonce = 7;
    }
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::ServiceUnavailable(message)) if message.contains("still pending")
    ));
    // Nor does one at a nonce above what the node holds (dropped by it).
    sqlx::query("UPDATE wallet_withdrawals SET nonce = 7 WHERE id = $1")
        .bind(next.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::ServiceUnavailable(message)) if message.contains("still pending")
    ));
    // Nor one at a waiting nonce that the node no longer holds: what waits
    // there was sent from elsewhere.
    sqlx::query("UPDATE wallet_withdrawals SET nonce = 6 WHERE id = $1")
        .bind(next.id)
        .execute(&pool)
        .await
        .unwrap();
    let next_hash = next.tx_hash.clone().unwrap();
    f.chain.lock().unwrap().known.remove(&next_hash);
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::ServiceUnavailable(message)) if message.contains("still pending")
    ));
    assert_eq!(f.broadcasts(), 2);
    // Held by the node, it is the wallet's own: the next transfer follows it.
    f.chain.lock().unwrap().known.insert(next_hash);
    assert_eq!(
        f.submit(10, Uuid::new_v4()).await.unwrap().status,
        "submitted"
    );
    assert_eq!(f.broadcasts(), 3);
}

#[sqlx::test(migrations = false)]
async fn only_the_wallets_own_unresolved_orders_hold_its_withdrawals(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let journal = |maker: String| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO execution_intents(id,market_id,signed_order,status) VALUES($1,'m',$2,'unknown')")
                .bind(Uuid::new_v4())
                .bind(serde_json::json!({"maker": maker}))
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    // A browser wallet's order of unknown outcome spends that wallet's funds.
    journal("0x4444444444444444444444444444444444444444".into()).await;
    f.chain.lock().unwrap().mine_on_send = true;
    assert_eq!(
        f.submit(10, Uuid::new_v4()).await.unwrap().status,
        "confirmed"
    );
    // One of the trading wallet's own holds its withdrawals back.
    journal(f.wallet.address_string().to_uppercase().replace("0X", "0x")).await;
    assert!(matches!(
        f.submit(10, Uuid::new_v4()).await,
        Err(ApiError::Conflict(message)) if message.contains("Live orders")
    ));
    assert_eq!(f.broadcasts(), 1);
}

#[test]
fn an_amount_too_large_to_scale_is_refused_not_a_panic() {
    assert_eq!(
        decimal_to_usdc_units("12.5".parse().unwrap()).unwrap(),
        12_500_000
    );
    assert!(decimal_to_usdc_units(Decimal::MAX).is_err());
    assert!(decimal_to_usdc_units("80000000000000000000000".parse().unwrap()).is_err());
}

#[test]
fn a_checksummed_address_with_a_typo_is_refused() {
    // EIP-55's own example, checksummed, and the same address in one case.
    let checksummed = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
    let lower = checksummed.to_lowercase();
    assert_eq!(validate_wallet_address(checksummed).unwrap(), lower);
    assert_eq!(validate_wallet_address(&lower).unwrap(), lower);
    assert_eq!(
        validate_wallet_address(&format!("0x{}", lower[2..].to_uppercase())).unwrap(),
        lower
    );
    // One character changed in the checksummed form: refused.
    let typo = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAee";
    assert!(validate_wallet_address(typo).is_err());
}
