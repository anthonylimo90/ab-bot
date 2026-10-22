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
