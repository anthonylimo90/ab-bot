//! On-chain USDC withdrawals: capacity checks, a durable submission journal and
//! a reconciler that resolves every journaled transfer from the chain.
//!
//! The recipient always receives USDC.e: from the wallet's USDC.e with a
//! transfer, or from its pUSD (the exchange's collateral) unwrapped by the
//! Collateral Offramp straight to the recipient.
//!
//! A transfer moves through `pending` (row written, nothing signed) →
//! `submitting` (signed; hash, nonce and raw bytes journaled before the
//! broadcast) → `submitted` (a node holds it) → `confirmed` / `failed`.
//! Only evidence decides the last step: a final receipt confirms or reverts a
//! transfer; a deterministic refusal, or its nonce used by another final
//! transaction on two looks minutes apart, proves it was never sent. Failed
//! transfers with a hash are still watched for a receipt for a day, so a wrong
//! verdict is corrected rather than repeated by a retry.
//!
//! A transfer left waiting below the network's gas price is signed again at
//! the same nonce for more gas (journaled before it is sent). Only one
//! transaction per nonce can be mined, but any of them may be, so every hash
//! broadcast for a withdrawal is kept and each is checked for a receipt.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use polymarket_core::api::polygon::{
    legacy_call, legacy_nonce_and_gas_price, BroadcastOutcome, CashBalances, MinedTransaction,
    WithdrawalSource,
};
use polymarket_core::api::PolygonClient;
use rust_decimal::Decimal;
use sqlx::pool::PoolConnection;
use sqlx::{PgConnection, PgPool, Postgres};
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;
use crate::workspace_scope::CANONICAL_WORKSPACE_SQL;

/// How often unresolved withdrawals are checked against the chain.
const RECONCILE_INTERVAL: StdDuration = StdDuration::from_secs(30);
/// A transfer still unmined this long after it was signed, paying less for
/// gas than the network now asks, is signed again at its nonce for more.
const FEE_BUMP_AFTER_MINUTES: i64 = 5;
/// A journaled row still `pending` this long was never signed: nothing was sent.
const UNSIGNED_GRACE_MINUTES: i32 = 10;
/// A transfer unknown to the node this long is sent again (and no more often).
const REBROADCAST_AFTER_SECS: i64 = 120;
/// How far apart the two looks that find a transfer's nonce used must be.
const SUPERSEDED_CONFIRM_MINUTES: i64 = 5;
/// How long failed transfers with a hash are still checked for a receipt.
const SECOND_CHANCE_HOURS: i32 = 24;
/// The error of a transfer whose final receipt says it reverted.
const REVERTED_ERROR: &str = "Transaction reverted on-chain; no USDC moved";
/// Statuses of a withdrawal that may still move funds.
pub(crate) const UNRESOLVED_STATUSES: [&str; 3] = ["pending", "submitting", "submitted"];

/// Advisory lock key for everything that spends from `address`'s nonces.
pub(crate) fn wallet_lock_key(address: &str) -> String {
    format!("wallet-withdrawal:{}", address.to_lowercase())
}

/// Session-level advisory locks held on one dedicated connection.
///
/// Work done under the locks goes through [`SpendLock::conn`], so it never
/// waits for another pool connection while holding them. Dropping the lock
/// without [`SpendLock::release`] (an early return, a panic, a cancelled
/// task) closes that connection, which frees its locks. A session cut off
/// on the way (a proxy, a network fault) may live on at the server holding
/// them, so whenever the connection is given up its session is also ended
/// from another one.
pub(crate) struct SpendLock {
    conn: Option<PoolConnection<Postgres>>,
    pool: PgPool,
    session: Session,
}

/// A database session: its process id and start time, which together name
/// it even once the process id is reused.
#[derive(Debug, Clone, Copy)]
struct Session {
    pid: i32,
    started: Option<DateTime<Utc>>,
}

impl Session {
    async fn of(conn: &mut PgConnection) -> Result<Self, sqlx::Error> {
        let (pid, started) = sqlx::query_as(
            "SELECT pg_backend_pid(), \
             (SELECT backend_start FROM pg_stat_activity WHERE pid = pg_backend_pid())",
        )
        .fetch_one(conn)
        .await?;
        Ok(Self { pid, started })
    }

    /// End the session from another connection, if it still runs.
    async fn end(self, pool: PgPool) {
        let ended = tokio::time::timeout(
            StdDuration::from_secs(10),
            sqlx::query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE pid = $1 AND backend_start IS NOT DISTINCT FROM $2",
            )
            .bind(self.pid)
            .bind(self.started)
            .execute(&pool),
        )
        .await;
        if !matches!(ended, Ok(Ok(_))) {
            warn!(
                pid = self.pid,
                "Could not end a spend lock's database session; its locks are freed when the server drops it"
            );
        }
    }
}

impl SpendLock {
    /// Wait for the spend lock of `wallet`.
    #[cfg(test)]
    pub(crate) async fn acquire(pool: &PgPool, wallet: &str) -> Result<Self, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(wallet_lock_key(wallet))
            .execute(&mut *conn)
            .await?;
        Ok(Self {
            session: Session::of(&mut conn).await?,
            conn: Some(conn),
            pool: pool.clone(),
        })
    }

    /// The spend lock of `wallet`, waiting up to `wait` for another holder
    /// to release it; `None` when it does not.
    pub(crate) async fn acquire_within(
        pool: &PgPool,
        wallet: &str,
        wait: StdDuration,
    ) -> Result<Option<Self>, sqlx::Error> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(lock) = Self::try_acquire(pool, wallet).await? {
                return Ok(Some(lock));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(StdDuration::from_millis(200)).await;
        }
    }

    /// The spend lock of `wallet`, or `None` while another holder has it.
    pub(crate) async fn try_acquire(
        pool: &PgPool,
        wallet: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        // Named first, so a session that takes the lock and then stops
        // answering can still be ended.
        let session = match Session::of(&mut conn).await {
            Ok(session) => session,
            Err(error) => {
                drop(conn.detach());
                return Err(error);
            }
        };
        let locked: Result<bool, _> =
            sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
                .bind(wallet_lock_key(wallet))
                .fetch_one(&mut *conn)
                .await;
        match locked {
            Ok(true) => Ok(Some(Self {
                conn: Some(conn),
                pool: pool.clone(),
                session,
            })),
            // Holding nothing, the connection goes back to the pool.
            Ok(false) => Ok(None),
            Err(error) => {
                // It may hold the lock: closed, never pooled, and ended.
                drop(conn.detach());
                session.end(pool.clone()).await;
                Err(error)
            }
        }
    }

    pub(crate) fn conn(&mut self) -> &mut PgConnection {
        self.conn.as_mut().expect("held until released")
    }

    /// Also hold the lock new positions and manual orders take, so no entry
    /// starts while the wallet's capacity is being decided.
    pub(crate) async fn lock_entries(&mut self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT pg_advisory_lock(hashtext('position-exposure'))")
            .execute(self.conn())
            .await
            .map(drop)
    }

    pub(crate) async fn unlock_entries(&mut self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT pg_advisory_unlock(hashtext('position-exposure'))")
            .execute(self.conn())
            .await
            .map(drop)
    }

    /// Whether the session holding the locks still answers, within a few
    /// seconds.
    pub(crate) async fn is_alive(&mut self) -> bool {
        matches!(
            tokio::time::timeout(
                StdDuration::from_secs(5),
                sqlx::query("SELECT 1").execute(self.conn())
            )
            .await,
            Ok(Ok(_))
        )
    }

    /// Give up locks whose session no longer answers: the connection is
    /// closed, never returned to the pool, and the session ended.
    pub(crate) async fn abandon(mut self) {
        if let Some(conn) = self.conn.take() {
            drop(conn.detach());
            self.session.end(self.pool.clone()).await;
        }
    }

    /// Free every lock and return the connection to the pool; a session that
    /// does not answer within seconds is abandoned instead.
    pub(crate) async fn release(mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let unlocked = tokio::time::timeout(
            StdDuration::from_secs(10),
            sqlx::query("SELECT pg_advisory_unlock_all()").execute(&mut *conn),
        )
        .await;
        if !matches!(unlocked, Ok(Ok(_))) {
            drop(conn.detach());
            self.session.end(self.pool.clone()).await;
        }
    }
}

impl Drop for SpendLock {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Closing the session is what releases session-level locks.
            drop(conn.detach());
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(self.session.end(self.pool.clone()));
            }
        }
    }
}

/// Whether `address` is the canonical workspace's live trading signer, whose
/// balance is the trading equity the circuit breaker measures.
pub(crate) async fn is_trading_wallet(
    conn: &mut PgConnection,
    address: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS(SELECT 1 FROM workspaces \
         WHERE LOWER(trading_wallet_address) = LOWER($1) AND id = ({CANONICAL_WORKSPACE_SQL}))"
    ))
    .bind(address)
    .fetch_one(conn)
    .await
}

/// What must stay in `source` whatever its balance: read from the database
/// before the balance, so a transfer settling in between can only make the
/// capacity look smaller, never larger.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WithdrawalHolds {
    /// Withdrawals from this wallet not yet resolved (they may still be mined).
    pub in_flight_withdrawals: Decimal,
    /// USDC committed to the trading wallet's open live buy orders, and to
    /// its matched buys whose trades may not have moved the USDC on chain
    /// yet.
    pub reserved_for_open_buys: Decimal,
    /// Whether `source` is the live trading wallet.
    pub trading_wallet: bool,
    /// Conditions that block any withdrawal right now.
    pub blocking_reasons: Vec<String>,
}

impl WithdrawalHolds {
    pub(crate) async fn load(conn: &mut PgConnection, source: &str) -> Result<Self, sqlx::Error> {
        let mut blocking_reasons = Vec::new();
        let (in_flight_withdrawals, unresolved_submission): (Decimal, bool) = sqlx::query_as(
            "SELECT COALESCE(SUM(amount), 0), COALESCE(BOOL_OR(status = 'submitting'), FALSE) \
             FROM wallet_withdrawals \
             WHERE LOWER(wallet_address) = LOWER($1) AND status = ANY($2)",
        )
        .bind(source)
        .bind(&UNRESOLVED_STATUSES[..])
        .fetch_one(&mut *conn)
        .await?;
        if unresolved_submission {
            blocking_reasons.push(
                "A previous withdrawal from this wallet has not been confirmed or rejected yet"
                    .into(),
            );
        }

        let trading_wallet = is_trading_wallet(conn, source).await?;
        let mut reserved_for_open_buys = Decimal::ZERO;
        if trading_wallet {
            // Live trades in flight need this wallet's USDC; withdrawing it
            // could leave a hedge half built. Orders other wallets signed
            // spend their own funds.
            let (entries_in_flight, unresolved_orders): (bool, bool) = sqlx::query_as(
                "SELECT \
                    EXISTS(SELECT 1 FROM positions WHERE state = 0 AND execution_mode = 'live'), \
                    EXISTS(SELECT 1 FROM execution_intents WHERE status IN ('submitting', 'unknown') \
                           AND LOWER(signed_order->>'maker') = LOWER($1))",
            )
            .bind(source)
            .fetch_one(&mut *conn)
            .await?;
            if entries_in_flight {
                blocking_reasons
                    .push("Live entries are in progress; retry when they settle".into());
            }
            if unresolved_orders {
                blocking_reasons.push(
                    "Live orders are still being submitted or reconciled; retry shortly".into(),
                );
            }
            let open_buys: Vec<(Decimal, Option<Decimal>)> = sqlx::query_as(
                "SELECT quantity - COALESCE(filled_quantity, 0), price \
                 FROM orders \
                 WHERE execution_mode = 'live' AND side = 'buy' \
                   AND status IN ('pending', 'open', 'partially_filled') \
                   AND (wallet_address IS NULL OR LOWER(wallet_address) = LOWER($1))",
            )
            .bind(source)
            .fetch_all(&mut *conn)
            .await?;
            reserved_for_open_buys = open_buys
                .into_iter()
                .map(|(remaining, price)| trading_engine::executor::open_buy_cost(remaining, price))
                .sum();
            // A buy the exchange matched moves the wallet's USDC only when
            // its trade settles on chain; a withdrawal mined first would make
            // that settlement fail with the shares already booked. Held: a
            // bot entry's buy from when its fill was booked (its order is
            // done then) for `SETTLING_BUY_MINUTES`, and a manual buy's fills
            // until its trades are confirmed (its order row holds the rest),
            // at most that long after the last was booked: a buy resting
            // for days has long settled its early fills.
            // Each at most its amount, plus the most taker fee it can carry.
            let settling: Vec<(Decimal, Decimal)> = sqlx::query_as(
                "SELECT (i.signed_order->>'makerAmount')::numeric / 1000000, l.applied_quantity \
                 FROM execution_intents i \
                 JOIN position_order_links l ON l.id = i.id \
                 WHERE i.status = 'reconciled' \
                   AND l.side = 'buy' AND l.applied_quantity > 0 \
                   AND LOWER(i.signed_order->>'maker') = LOWER($1) \
                   AND i.signed_order->>'makerAmount' ~ '^[0-9]{1,30}$' \
                   AND i.updated_at > NOW() - make_interval(mins => $2) \
                 UNION ALL \
                 SELECT o.filled_quantity * COALESCE(o.avg_fill_price, o.price, 1), \
                        o.filled_quantity \
                 FROM execution_intents i \
                 JOIN orders o ON o.id = i.id \
                 WHERE i.status = 'acknowledged' \
                   AND o.side = 'buy' AND o.execution_mode = 'live' \
                   AND o.filled_quantity > 0 \
                   AND o.updated_at > NOW() - make_interval(mins => $2) \
                   AND LOWER(i.signed_order->>'maker') = LOWER($1)",
            )
            .bind(source)
            .bind(SETTLING_BUY_MINUTES)
            .fetch_all(&mut *conn)
            .await?;
            // The fee at its highest, at a price of one half.
            let half = Decimal::new(5, 1);
            reserved_for_open_buys += settling
                .into_iter()
                .map(|(amount, shares)| {
                    amount + trading_engine::executor::taker_fee_allowance(shares, half)
                })
                .sum::<Decimal>();
        }
        Ok(Self {
            in_flight_withdrawals,
            reserved_for_open_buys,
            trading_wallet,
            blocking_reasons,
        })
    }

    /// The capacity of the wallet given its on-chain balance in USDC units.
    pub(crate) fn capacity(&self, balance_units: u128) -> WithdrawalCapacity {
        let available_usdc = polymarket_core::api::polygon::usdc_units_to_decimal(balance_units);
        WithdrawalCapacity {
            available_usdc,
            in_flight_withdrawals: self.in_flight_withdrawals,
            reserved_for_open_buys: self.reserved_for_open_buys,
            max_withdrawable: (available_usdc
                - self.in_flight_withdrawals
                - self.reserved_for_open_buys)
                .max(Decimal::ZERO),
            blocking_reasons: self.blocking_reasons.clone(),
        }
    }
}

/// How long after a bot entry's buy is booked its USDC counts as still
/// leaving the wallet: as long as the exit handler waits for a fill's shares
/// to settle.
const SETTLING_BUY_MINUTES: i32 = 30;

/// How much may leave a wallet holding `cash`, and what a withdrawal of
/// `amount` draws on: its USDC.e when that covers it, else its pUSD. One
/// withdrawal draws on one token, so the most that may leave is the larger
/// of the two capacities. Each token's capacity is its balance less every
/// hold (holds are not split by token), which never overstates it.
pub(crate) fn withdrawal_plan(
    holds: &WithdrawalHolds,
    cash: &CashBalances,
    amount: Decimal,
) -> (WithdrawalCapacity, Option<WithdrawalSource>) {
    let usdc = holds.capacity(cash.usdc_units);
    let pusd = holds.capacity(cash.pusd_units);
    let source = if amount <= usdc.max_withdrawable {
        Some(WithdrawalSource::Usdc)
    } else if amount <= pusd.max_withdrawable {
        Some(WithdrawalSource::Pusd)
    } else {
        None
    };
    let mut capacity = holds.capacity(cash.total_units());
    capacity.max_withdrawable = usdc.max_withdrawable.max(pusd.max_withdrawable);
    (capacity, source)
}

/// How much may leave `source` now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WithdrawalCapacity {
    /// On-chain cash balance (USDC.e and pUSD).
    pub available_usdc: Decimal,
    /// Withdrawals from this wallet not yet resolved (they may still be mined).
    pub in_flight_withdrawals: Decimal,
    /// USDC committed to the trading wallet's open live buy orders.
    pub reserved_for_open_buys: Decimal,
    pub max_withdrawable: Decimal,
    /// Conditions that block any withdrawal right now.
    pub blocking_reasons: Vec<String>,
}

/// A client-safe description of a chain client error: transport errors can
/// carry the RPC URL (and its API key), so only their kind is kept.
pub(crate) fn chain_error_message(error: &polymarket_core::Error) -> String {
    match error {
        polymarket_core::Error::Http(_) => "Polygon RPC request failed".to_string(),
        other => other.to_string().chars().take(500).collect(),
    }
}

/// What the chain says about a transaction.
#[derive(Debug)]
pub(crate) enum Receipt {
    /// No receipt.
    Unmined,
    /// Mined in a block that could still be reorganized away.
    Unsettled,
    /// Mined in a final block.
    Final(MinedTransaction),
}

/// The receipt of `tx_hash`, counted only once its block is final.
pub(crate) async fn final_receipt(
    client: &PolygonClient,
    tx_hash: &str,
) -> polymarket_core::Result<Receipt> {
    let Some(mined) = client.transaction_receipt(tx_hash).await? else {
        return Ok(Receipt::Unmined);
    };
    let safe_block = client.safe_block_number().await?;
    Ok(match mined.block_number {
        Some(block) if block <= safe_block => Receipt::Final(mined),
        _ => Receipt::Unsettled,
    })
}

#[derive(sqlx::FromRow)]
struct SettledWithdrawal {
    workspace_id: Option<Uuid>,
    wallet_address: String,
    destination_address: String,
    amount: Decimal,
    user_id: Uuid,
    tx_hash: Option<String>,
    trading_wallet: Option<bool>,
}

/// Settle a transfer from the final receipt of `tx_hash`, one of the
/// transactions broadcast for it: `confirmed` (recording the cash flow when it
/// left the trading wallet) or `failed` when it reverted. The mined hash
/// becomes the row's `tx_hash`. A transfer failed by an earlier verdict is
/// confirmed if its receipt proves it was mined. Idempotent; `false` when
/// nothing changed.
pub(crate) async fn finalize_mined(
    conn: &mut PgConnection,
    withdrawal_id: Uuid,
    tx_hash: &str,
    mined: &MinedTransaction,
) -> Result<bool, sqlx::Error> {
    let mut tx = sqlx::Connection::begin(conn).await?;
    let settled: Option<SettledWithdrawal> = sqlx::query_as(
        "UPDATE wallet_withdrawals SET \
            status = CASE WHEN $2 THEN 'confirmed' ELSE 'failed' END, \
            confirmed_at = CASE WHEN $2 THEN NOW() ELSE confirmed_at END, \
            error = CASE WHEN $2 THEN NULL ELSE $3 END, \
            superseded_seen_at = NULL, \
            replaced_tx_hashes = CASE WHEN tx_hash = $4 THEN replaced_tx_hashes \
                ELSE array_append(array_remove(replaced_tx_hashes, $4), tx_hash) END, \
            tx_hash = $4, \
            updated_at = NOW() \
         WHERE id = $1 AND (tx_hash = $4 OR replaced_tx_hashes @> ARRAY[$4]::text[]) \
           AND (status IN ('submitting', 'submitted') OR (status = 'failed' AND $2)) \
         RETURNING workspace_id, wallet_address, destination_address, amount, user_id, tx_hash, \
                   trading_wallet",
    )
    .bind(withdrawal_id)
    .bind(mined.success)
    .bind(REVERTED_ERROR)
    .bind(tx_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(settled) = settled else {
        return Ok(false);
    };
    if mined.success {
        // Only the trading wallet's balance is trading equity: a withdrawal from
        // any other wallet must not lower the circuit breaker's drawdown peaks.
        // The row records which it was when it was sent.
        let trading = match settled.trading_wallet {
            Some(trading) => trading,
            None => is_trading_wallet(&mut tx, &settled.wallet_address).await?,
        };
        if let (true, Some(workspace_id)) = (trading, settled.workspace_id) {
            // created_at is the insert's wall clock, not the transaction start,
            // so the breaker's cursor (which trails commits) cannot pass it.
            sqlx::query(
                "INSERT INTO cash_flow_events \
                    (id, workspace_id, event_type, amount, currency, note, occurred_at, created_by, \
                     created_at) \
                 VALUES ($1, $2, 'withdrawal', $3, 'USDC', $4, NOW(), $5, clock_timestamp()) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(withdrawal_id)
            .bind(workspace_id)
            .bind(-settled.amount)
            .bind(format!(
                "On-chain wallet withdrawal to {} (tx: {})",
                settled.destination_address,
                settled.tx_hash.as_deref().unwrap_or("unknown")
            ))
            .bind(settled.user_id)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(true)
}

/// Whether a withdrawal has journaled a transaction with `tx_hash`.
pub(crate) async fn hash_recorded(
    conn: &mut PgConnection,
    tx_hash: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM wallet_withdrawals \
         WHERE tx_hash = $1 OR replaced_tx_hashes @> ARRAY[$1]::text[])",
    )
    .bind(tx_hash)
    .fetch_one(conn)
    .await
}

/// Fail an unresolved withdrawal that certainly moved no funds.
pub(crate) async fn fail_unsent(
    conn: &mut PgConnection,
    withdrawal_id: Uuid,
    reason: &str,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE wallet_withdrawals SET status = 'failed', error = $2, superseded_seen_at = NULL, \
         updated_at = NOW() WHERE id = $1 AND status = ANY($3)",
    )
    .bind(withdrawal_id)
    .bind(reason)
    .bind(&UNRESOLVED_STATUSES[..])
    .execute(conn)
    .await?;
    Ok(updated.rows_affected() > 0)
}

#[derive(Debug, sqlx::FromRow)]
struct UnresolvedWithdrawal {
    id: Uuid,
    wallet_address: String,
    status: String,
    tx_hash: Option<String>,
    replaced_tx_hashes: Vec<String>,
    nonce: Option<i64>,
    raw_tx: Option<String>,
    requested_at: DateTime<Utc>,
    signed_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    superseded_seen_at: Option<DateTime<Utc>>,
}

/// The columns of an [`UnresolvedWithdrawal`].
const UNRESOLVED_COLUMNS: &str = "id, wallet_address, status, tx_hash, \
    replaced_tx_hashes, nonce, raw_tx, requested_at, signed_at, updated_at, superseded_seen_at";

/// The withdrawals the reconciler can decide, given `REVERTED_ERROR` as $1 and
/// `SECOND_CHANCE_HOURS` as $2.
const RECONCILABLE: &str = "(status IN ('submitting', 'submitted') AND tx_hash IS NOT NULL) \
    OR (status = 'pending' AND journaled AND tx_hash IS NULL) \
    OR (status = 'failed' AND tx_hash IS NOT NULL AND error IS DISTINCT FROM $1 \
        AND updated_at > NOW() - make_interval(hours => $2))";

impl UnresolvedWithdrawal {
    /// Every transaction broadcast for the transfer, `tx_hash` (the latest)
    /// first: any one of them may be the one mined.
    fn hashes<'a>(&'a self, tx_hash: &'a str) -> Vec<&'a str> {
        std::iter::once(tx_hash)
            .chain(self.replaced_tx_hashes.iter().rev().map(String::as_str))
            .collect()
    }
}

/// Run the reconciler forever.
pub fn spawn_withdrawal_reconciler(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            crate::background::pass("withdrawal reconciler", async {
                if let Err(error) = reconcile_withdrawals(&state).await {
                    warn!(%error, "Withdrawal reconciliation failed");
                }
            })
            .await;
            tokio::time::sleep(RECONCILE_INTERVAL).await;
        }
    });
}

/// Resolve unresolved withdrawals from the chain.
pub(crate) async fn reconcile_withdrawals(state: &AppState) -> anyhow::Result<()> {
    let client = crate::handlers::vault::canonical_polygon_client(state)
        .await
        .ok();
    reconcile_with(&state.pool, client.as_ref(), |address| async move {
        state
            .load_wallet_from_vault(&address)
            .await
            .map_err(|error| warn!(%error, "No signer to raise a withdrawal's gas price"))
            .ok()
    })
    .await
}

/// Whether a stalled trading-wallet transfer of `workspace_id` (unresolved,
/// or failed by a verdict its receipt may still overturn; requested more
/// than `stalled_minutes` ago and not yet seen mined) has a
/// receipt now, which is then recorded as `mined_seen_at`. The reconciler
/// notices a mined transfer only on its next pass; a caller that read the
/// wallet balance before calling this learns whether that balance may lack
/// a transfer. A transfer whose chain cannot be read counts as unmined.
pub(crate) async fn stalled_transfer_mined(
    pool: &PgPool,
    workspace_id: Uuid,
    stalled_minutes: i32,
    client: Option<&PolygonClient>,
) -> anyhow::Result<bool> {
    let Some(client) = client else {
        return Ok(false);
    };
    let stalled: Vec<(Uuid, String, Vec<String>)> = sqlx::query_as(
        "SELECT id, tx_hash, replaced_tx_hashes FROM wallet_withdrawals \
         WHERE workspace_id = $1 AND COALESCE(trading_wallet, TRUE) \
           AND tx_hash IS NOT NULL AND mined_seen_at IS NULL \
           AND requested_at <= NOW() - make_interval(mins => $2) \
           AND (status IN ('submitting', 'submitted') \
                OR (status = 'failed' AND error IS DISTINCT FROM $3 \
                    AND updated_at > NOW() - make_interval(hours => $4))) \
         ORDER BY requested_at DESC LIMIT 10",
    )
    .bind(workspace_id)
    .bind(stalled_minutes)
    .bind(REVERTED_ERROR)
    .bind(SECOND_CHANCE_HOURS)
    .fetch_all(pool)
    .await?;
    for (id, tx_hash, replaced) in stalled {
        for hash in std::iter::once(&tx_hash).chain(replaced.iter()) {
            match client.transaction_receipt(hash).await {
                Ok(Some(_)) => {
                    sqlx::query(
                        "UPDATE wallet_withdrawals SET mined_seen_at = NOW() \
                         WHERE id = $1 AND mined_seen_at IS NULL",
                    )
                    .bind(id)
                    .execute(pool)
                    .await?;
                    return Ok(true);
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(withdrawal_id = %id, error = %chain_error_message(&error), "Could not read a stalled withdrawal's receipt");
                }
            }
        }
    }
    Ok(false)
}

/// [`reconcile_withdrawals`] reading the chain through `client`, with the
/// signer of a wallet supplied by `signer_for`.
async fn reconcile_with<S, SFut>(
    pool: &PgPool,
    client: Option<&PolygonClient>,
    signer_for: S,
) -> anyhow::Result<()>
where
    S: Fn(String) -> SFut,
    SFut: std::future::Future<Output = Option<auth::TradingWallet>>,
{
    // Only rows that can be decided: legacy rows sent without a recorded hash
    // are left to an operator and must not crowd the others out. Unresolved
    // rows come before the failed ones watched for a late receipt.
    let rows: Vec<UnresolvedWithdrawal> = sqlx::query_as(&format!(
        "SELECT {UNRESOLVED_COLUMNS} FROM wallet_withdrawals WHERE {RECONCILABLE} \
         ORDER BY status = 'failed', updated_at LIMIT 200"
    ))
    .bind(REVERTED_ERROR)
    .bind(SECOND_CHANCE_HOURS)
    .fetch_all(pool)
    .await?;

    for listed in rows {
        // Never race the request that is signing or sending from this wallet.
        let Some(mut lock) = SpendLock::try_acquire(pool, &listed.wallet_address).await? else {
            continue;
        };
        // Read it again under the lock: another replica may have settled it,
        // or signed a replacement, since it was listed.
        let row: Result<Option<UnresolvedWithdrawal>, sqlx::Error> = sqlx::query_as(&format!(
            "SELECT {UNRESOLVED_COLUMNS} FROM wallet_withdrawals \
             WHERE id = $3 AND ({RECONCILABLE})"
        ))
        .bind(REVERTED_ERROR)
        .bind(SECOND_CHANCE_HOURS)
        .bind(listed.id)
        .fetch_optional(lock.conn())
        .await;
        let row = match row {
            Ok(Some(row)) => row,
            Ok(None) => {
                lock.release().await;
                continue;
            }
            Err(error) => {
                lock.release().await;
                warn!(withdrawal_id = %listed.id, %error, "Could not re-read a withdrawal; trying the others");
                continue;
            }
        };
        let result = match row.tx_hash.as_deref() {
            None => fail_unsigned(lock.conn(), &row).await,
            Some(tx_hash) => match client {
                Some(client) => {
                    match reconcile_on_chain(lock.conn(), client, &row, tx_hash).await {
                        // Waiting at its nonce: perhaps for a gas price it does not pay.
                        Ok(true) if fee_bump_due(&row) => match underpriced(client, &row).await {
                            Ok(Some(latest)) => {
                                match signer_for(row.wallet_address.clone()).await {
                                    Some(wallet) => {
                                        bump_fee(
                                            lock.conn(),
                                            client,
                                            &wallet,
                                            &row,
                                            tx_hash,
                                            latest,
                                        )
                                        .await
                                    }
                                    None => Ok(()),
                                }
                            }
                            other => other.map(drop),
                        },
                        other => other.map(drop),
                    }
                }
                None => Err(anyhow::anyhow!("no Polygon client available")),
            },
        };
        lock.release().await;
        if let Err(error) = result {
            warn!(withdrawal_id = %row.id, error = %error, "Could not reconcile withdrawal yet");
        }
    }
    Ok(())
}

/// Journaled rows record a hash before anything is sent, so one still without
/// it this long after its last change was never signed.
async fn fail_unsigned(conn: &mut PgConnection, row: &UnresolvedWithdrawal) -> anyhow::Result<()> {
    let failed = sqlx::query(
        "UPDATE wallet_withdrawals SET status = 'failed', \
            error = 'The transfer was never signed or sent', updated_at = NOW() \
         WHERE id = $1 AND status = 'pending' AND journaled AND tx_hash IS NULL \
           AND updated_at < NOW() - make_interval(mins => $2)",
    )
    .bind(row.id)
    .bind(UNSIGNED_GRACE_MINUTES)
    .execute(conn)
    .await?;
    if failed.rows_affected() > 0 {
        info!(withdrawal_id = %row.id, "Failed a withdrawal that was never sent");
    }
    Ok(())
}

/// Settle or advance one journaled transfer from the chain. `Ok(true)` when
/// it is still waiting to be mined at its nonce (a node holds it, or it was
/// just sent again), so a fee bump may be due.
async fn reconcile_on_chain(
    conn: &mut PgConnection,
    client: &PolygonClient,
    row: &UnresolvedWithdrawal,
    tx_hash: &str,
) -> anyhow::Result<bool> {
    let hashes = row.hashes(tx_hash);
    // They share a nonce, so at most one is mined: its receipt decides.
    for hash in &hashes {
        match final_receipt(client, hash).await? {
            Receipt::Final(mined) => {
                if finalize_mined(conn, row.id, hash, &mined).await? {
                    info!(withdrawal_id = %row.id, success = mined.success, "Withdrawal resolved on chain");
                }
                return Ok(false);
            }
            // Mined: wait for its block to become final. The balance already
            // lacks the amount; say so for the circuit breaker.
            Receipt::Unsettled => {
                sqlx::query(
                    "UPDATE wallet_withdrawals SET mined_seen_at = NOW() \
                     WHERE id = $1 AND mined_seen_at IS NULL",
                )
                .bind(row.id)
                .execute(conn)
                .await?;
                return Ok(false);
            }
            Receipt::Unmined => {}
        }
    }
    if row.status == "failed" {
        // Failed transfers are only watched for a receipt.
        return Ok(false);
    }

    // The first of its transactions (latest first) a node holds.
    let mut held = None;
    for (index, hash) in hashes.iter().enumerate() {
        if client.transaction_known(hash).await? {
            held = Some(index);
            break;
        }
    }
    if let Some(index) = held {
        // A node holds one: it was sent, and its nonce is not used by another.
        sqlx::query(
            "UPDATE wallet_withdrawals SET status = 'submitted', superseded_seen_at = NULL, \
             updated_at = NOW() \
             WHERE id = $1 AND (status = 'submitting' \
                                OR (status = 'submitted' AND superseded_seen_at IS NOT NULL))",
        )
        .bind(row.id)
        .execute(&mut *conn)
        .await?;
        // Only an earlier one: the latest (a replacement journaled but never
        // delivered) is sent again, at most once per interval, to replace it.
        if index > 0 && row.updated_at < Utc::now() - Duration::seconds(REBROADCAST_AFTER_SECS) {
            resend_latest(conn, client, row, tx_hash).await?;
        }
        return Ok(true);
    }
    let Some(nonce) = row.nonce.and_then(|nonce| u64::try_from(nonce).ok()) else {
        return Ok(false);
    };

    // Is the nonce used by a transaction deep enough not to be reorganized?
    let safe_block = client.safe_block_number().await?;
    let used = client
        .transaction_count_at(&row.wallet_address, safe_block)
        .await?
        > nonce;
    if used {
        match row.superseded_seen_at {
            None => {
                sqlx::query(
                    "UPDATE wallet_withdrawals SET superseded_seen_at = NOW() \
                     WHERE id = $1 AND superseded_seen_at IS NULL",
                )
                .bind(row.id)
                .execute(conn)
                .await?;
            }
            Some(first_seen)
                if Utc::now() - first_seen >= Duration::minutes(SUPERSEDED_CONFIRM_MINUTES) =>
            {
                // Still unknown and still superseded minutes later. A last look
                // for any receipt, then none of them can ever be mined.
                for hash in &hashes {
                    if client.transaction_receipt(hash).await?.is_some() {
                        return Ok(false);
                    }
                }
                if fail_unsent(
                    conn,
                    row.id,
                    "Another transaction used this transfer's nonce; it was never mined \
                     and no funds moved",
                )
                .await?
                {
                    info!(withdrawal_id = %row.id, "Withdrawal superseded; marked failed");
                }
            }
            Some(_) => {}
        }
        return Ok(false);
    }

    if row.superseded_seen_at.is_some() {
        sqlx::query("UPDATE wallet_withdrawals SET superseded_seen_at = NULL WHERE id = $1")
            .bind(row.id)
            .execute(&mut *conn)
            .await?;
    }
    // Still possible: send it again if the node lost it and nothing else
    // holds its nonce in the mempool, at most once per interval.
    if row.updated_at < Utc::now() - Duration::seconds(REBROADCAST_AFTER_SECS)
        && client
            .transaction_count(&row.wallet_address, "pending")
            .await?
            <= nonce
    {
        if row.raw_tx.is_none() {
            return Ok(false);
        }
        resend_latest(conn, client, row, tx_hash).await?;
        return Ok(true);
    }
    Ok(false)
}

/// Send the transfer's latest transaction (`tx_hash`, journaled as `raw_tx`)
/// again; an accepted one marks it `submitted`.
async fn resend_latest(
    conn: &mut PgConnection,
    client: &PolygonClient,
    row: &UnresolvedWithdrawal,
    tx_hash: &str,
) -> anyhow::Result<()> {
    let Some(raw) = row.raw_tx.as_deref() else {
        return Ok(());
    };
    let outcome = client.broadcast(raw, tx_hash).await;
    let accepted = matches!(outcome, Ok(BroadcastOutcome::Accepted));
    sqlx::query(
        "UPDATE wallet_withdrawals SET updated_at = NOW(), \
         status = CASE WHEN $2 THEN 'submitted' ELSE status END \
         WHERE id = $1 AND tx_hash = $3 AND status IN ('submitting', 'submitted')",
    )
    .bind(row.id)
    .bind(accepted)
    .bind(tx_hash)
    .execute(conn)
    .await?;
    match outcome {
        Ok(BroadcastOutcome::Accepted) => {
            info!(withdrawal_id = %row.id, "Re-sent an unresolved withdrawal")
        }
        Ok(BroadcastOutcome::Rejected(reason) | BroadcastOutcome::Uncertain(reason)) => {
            warn!(
                withdrawal_id = %row.id,
                %reason,
                unresolved_minutes = (Utc::now() - row.requested_at).num_minutes(),
                "Node did not take a re-sent withdrawal; its gas price is raised when the \
                 network asks more, otherwise it blocks this wallet's withdrawals until \
                 mined or superseded (see DEPLOY.md)"
            )
        }
        Err(error) => {
            warn!(
                withdrawal_id = %row.id,
                error = %chain_error_message(&error),
                unresolved_minutes = (Utc::now() - row.requested_at).num_minutes(),
                "Re-sending a withdrawal failed"
            )
        }
    }
    Ok(())
}

/// Whether an unresolved transfer was signed long enough ago that, still
/// unmined, it may be waiting for a gas price it does not pay.
fn fee_bump_due(row: &UnresolvedWithdrawal) -> bool {
    matches!(row.status.as_str(), "submitting" | "submitted")
        && row.signed_at.unwrap_or(row.requested_at)
            < Utc::now() - Duration::minutes(FEE_BUMP_AFTER_MINUTES)
}

/// A transfer's latest transaction, when the network now asks more for gas
/// than it pays.
struct Underpriced {
    nonce: u64,
    gas_price: u128,
}

/// Whether the transfer's latest transaction (its journaled `raw_tx`) pays
/// less for gas than the network now asks, and so may be waiting for it.
async fn underpriced(
    client: &PolygonClient,
    row: &UnresolvedWithdrawal,
) -> anyhow::Result<Option<Underpriced>> {
    let (Some(raw), Some(nonce)) = (
        row.raw_tx.as_deref(),
        row.nonce.and_then(|nonce| u64::try_from(nonce).ok()),
    ) else {
        return Ok(None);
    };
    let Some((signed_nonce, gas_price)) = legacy_nonce_and_gas_price(raw) else {
        warn!(withdrawal_id = %row.id, "Journaled withdrawal is unreadable; its gas price is not raised");
        return Ok(None);
    };
    if signed_nonce != nonce {
        warn!(withdrawal_id = %row.id, "Journaled withdrawal does not match its nonce; its gas price is not raised");
        return Ok(None);
    }
    let network_price = client
        .gas_price_wei()
        .await
        .map_err(|error| anyhow::anyhow!(chain_error_message(&error)))?;
    Ok((network_price > gas_price).then_some(Underpriced { nonce, gas_price }))
}

/// Sign the underpriced transfer again at the same nonce, making the same
/// call with the same gas limit, paying the network's price (with the margin
/// new transfers carry) and at least a tenth more than before (what nodes
/// require of a replacement). The new transaction is journaled, keeping every
/// earlier hash, before it is sent: whichever one is mined settles the
/// transfer, and only one can be.
async fn bump_fee(
    conn: &mut PgConnection,
    client: &PolygonClient,
    wallet: &auth::TradingWallet,
    row: &UnresolvedWithdrawal,
    tx_hash: &str,
    latest: Underpriced,
) -> anyhow::Result<()> {
    let Underpriced { nonce, gas_price } = latest;
    let Some(original) = row.raw_tx.as_deref() else {
        return Ok(());
    };
    if !wallet
        .address_string()
        .eq_ignore_ascii_case(&row.wallet_address)
    {
        warn!(withdrawal_id = %row.id, "Signer does not match the withdrawal's wallet; its gas price is not raised");
        return Ok(());
    }

    let mut min_gas_price = gas_price + gas_price / 10 + 1;
    let mut replacement = None;
    for _ in 0..3 {
        let signed = client
            .prepare_replacement(wallet.signer(), original, nonce, min_gas_price)
            .await
            .map_err(|error| anyhow::anyhow!(chain_error_message(&error)))?;
        anyhow::ensure!(signed.nonce == nonce, "replacement signed at another nonce");
        if !hash_recorded(conn, &signed.tx_hash).await? {
            replacement = Some(signed);
            break;
        }
        min_gas_price = signed.gas_price + signed.gas_price / 10 + 1;
    }
    let Some(signed) = replacement else {
        warn!(withdrawal_id = %row.id, "Could not sign a replacement distinct from earlier transactions");
        return Ok(());
    };
    // It must be the same transfer as the transaction it replaces.
    let replaced = legacy_call(original);
    if replaced.is_none() || legacy_call(&signed.raw) != replaced {
        warn!(withdrawal_id = %row.id, "Replacement would not repeat the journaled transfer; its gas price is not raised");
        return Ok(());
    }

    // Journal it before it can leave.
    let journaled = sqlx::query(
        "UPDATE wallet_withdrawals SET \
            replaced_tx_hashes = array_append(replaced_tx_hashes, tx_hash), \
            tx_hash = $3, raw_tx = $4, signed_at = NOW(), status = 'submitting', \
            updated_at = NOW() \
         WHERE id = $1 AND tx_hash = $2 AND status IN ('submitting', 'submitted')",
    )
    .bind(row.id)
    .bind(tx_hash)
    .bind(&signed.tx_hash)
    .bind(&signed.raw)
    .execute(&mut *conn)
    .await?;
    if journaled.rows_affected() != 1 {
        return Ok(());
    }
    info!(
        withdrawal_id = %row.id,
        nonce,
        previous_gas_price = gas_price,
        gas_price = signed.gas_price,
        "Raised the gas price of a waiting withdrawal"
    );
    match client.broadcast(&signed.raw, &signed.tx_hash).await {
        Ok(BroadcastOutcome::Accepted) => {
            sqlx::query(
                "UPDATE wallet_withdrawals SET status = 'submitted', updated_at = NOW() \
                 WHERE id = $1 AND tx_hash = $2 AND status = 'submitting'",
            )
            .bind(row.id)
            .bind(&signed.tx_hash)
            .execute(conn)
            .await?;
        }
        // Journaled: the reconciler sends it again or settles it from the chain.
        Ok(BroadcastOutcome::Rejected(reason) | BroadcastOutcome::Uncertain(reason)) => {
            warn!(withdrawal_id = %row.id, %reason, "Node did not take a withdrawal's replacement")
        }
        Err(error) => {
            warn!(withdrawal_id = %row.id, error = %chain_error_message(&error), "Sending a withdrawal's replacement failed")
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "withdrawals/tests.rs"]
pub(crate) mod tests;
