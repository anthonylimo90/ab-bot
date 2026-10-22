//! On-chain USDC withdrawals: capacity checks, a durable submission journal and
//! a reconciler that resolves every journaled transfer from the chain.
//!
//! A transfer moves through `pending` (row written, nothing signed) →
//! `submitting` (signed; hash, nonce and raw bytes journaled before the
//! broadcast) → `submitted` (a node holds it) → `confirmed` / `failed`.
//! Only evidence decides the last step: a final receipt confirms or reverts a
//! transfer; a deterministic refusal, or its nonce used by another final
//! transaction on two looks minutes apart, proves it was never sent. Failed
//! transfers with a hash are still watched for a receipt for a day, so a wrong
//! verdict is corrected rather than repeated by a retry.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use polymarket_core::api::polygon::{BroadcastOutcome, MinedTransaction};
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
/// task) closes that connection, which frees its locks: they cannot leak.
pub(crate) struct SpendLock {
    conn: Option<PoolConnection<Postgres>>,
}

impl SpendLock {
    /// Wait for the spend lock of `wallet`.
    pub(crate) async fn acquire(pool: &PgPool, wallet: &str) -> Result<Self, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .bind(wallet_lock_key(wallet))
            .execute(&mut *conn)
            .await?;
        Ok(Self { conn: Some(conn) })
    }

    /// The spend lock of `wallet`, or `None` while another holder has it.
    pub(crate) async fn try_acquire(
        pool: &PgPool,
        wallet: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
                .bind(wallet_lock_key(wallet))
                .fetch_one(&mut *conn)
                .await?;
        Ok(locked.then_some(Self { conn: Some(conn) }))
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

    /// Free every lock and return the connection to the pool.
    pub(crate) async fn release(mut self) {
        if let Some(mut conn) = self.conn.take() {
            if sqlx::query("SELECT pg_advisory_unlock_all()")
                .execute(&mut *conn)
                .await
                .is_err()
            {
                drop(conn.detach());
            }
        }
    }
}

impl Drop for SpendLock {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Closing the session is what releases session-level locks.
            drop(conn.detach());
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
    /// USDC committed to the trading wallet's open live buy orders.
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
            // could leave a hedge half built.
            let (entries_in_flight, unresolved_orders): (bool, bool) = sqlx::query_as(
                "SELECT \
                    EXISTS(SELECT 1 FROM positions WHERE state = 0 AND execution_mode = 'live'), \
                    EXISTS(SELECT 1 FROM execution_intents WHERE status IN ('submitting', 'unknown'))",
            )
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
            // A market buy has no price; count it at the $1 maximum.
            reserved_for_open_buys = sqlx::query_scalar(
                "SELECT COALESCE(SUM((quantity - COALESCE(filled_quantity, 0)) * COALESCE(price, 1)), 0) \
                 FROM orders \
                 WHERE execution_mode = 'live' AND side = 'buy' \
                   AND status IN ('pending', 'open', 'partially_filled') \
                   AND (wallet_address IS NULL OR LOWER(wallet_address) = LOWER($1))",
            )
            .bind(source)
            .fetch_one(&mut *conn)
            .await?;
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
        let available_usdc = Decimal::from(balance_units) / Decimal::from(1_000_000u64);
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

/// How much may leave `source` now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WithdrawalCapacity {
    /// On-chain USDC balance.
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

/// Settle a transfer from its final receipt: `confirmed` (recording the cash
/// flow when it left the trading wallet) or `failed` when it reverted. A
/// transfer failed by an earlier verdict is confirmed if its receipt proves it
/// was mined. Idempotent; `false` when nothing changed.
pub(crate) async fn finalize_mined(
    conn: &mut PgConnection,
    withdrawal_id: Uuid,
    mined: &MinedTransaction,
) -> Result<bool, sqlx::Error> {
    let mut tx = sqlx::Connection::begin(conn).await?;
    let settled: Option<SettledWithdrawal> = sqlx::query_as(
        "UPDATE wallet_withdrawals SET \
            status = CASE WHEN $2 THEN 'confirmed' ELSE 'failed' END, \
            confirmed_at = CASE WHEN $2 THEN NOW() ELSE confirmed_at END, \
            error = CASE WHEN $2 THEN NULL ELSE $3 END, \
            superseded_seen_at = NULL, \
            updated_at = NOW() \
         WHERE id = $1 AND tx_hash IS NOT NULL \
           AND (status IN ('submitting', 'submitted') OR (status = 'failed' AND $2)) \
         RETURNING workspace_id, wallet_address, destination_address, amount, user_id, tx_hash, \
                   trading_wallet",
    )
    .bind(withdrawal_id)
    .bind(mined.success)
    .bind(REVERTED_ERROR)
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
    user_id: Uuid,
    wallet_address: String,
    status: String,
    tx_hash: Option<String>,
    nonce: Option<i64>,
    raw_tx: Option<String>,
    requested_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    superseded_seen_at: Option<DateTime<Utc>>,
}

/// Run the reconciler forever.
pub fn spawn_withdrawal_reconciler(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = reconcile_withdrawals(&state).await {
                warn!(%error, "Withdrawal reconciliation failed");
            }
            tokio::time::sleep(RECONCILE_INTERVAL).await;
        }
    });
}

/// Resolve unresolved withdrawals from the chain.
pub(crate) async fn reconcile_withdrawals(state: &AppState) -> anyhow::Result<()> {
    reconcile_with(&state.pool, |user_id| async move {
        crate::handlers::vault::resolve_polygon_client_for_user(state, user_id)
            .await
            .ok()
    })
    .await
}

/// [`reconcile_withdrawals`] with the chain client for each row's user
/// supplied by `client_for`.
async fn reconcile_with<F, Fut>(pool: &PgPool, client_for: F) -> anyhow::Result<()>
where
    F: Fn(Uuid) -> Fut,
    Fut: std::future::Future<Output = Option<PolygonClient>>,
{
    // Only rows that can be decided: legacy rows sent without a recorded hash
    // are left to an operator and must not crowd the others out. Unresolved
    // rows come before the failed ones watched for a late receipt.
    let rows: Vec<UnresolvedWithdrawal> = sqlx::query_as(
        "SELECT id, user_id, wallet_address, status, tx_hash, nonce, raw_tx, requested_at, \
                updated_at, superseded_seen_at \
         FROM wallet_withdrawals \
         WHERE (status IN ('submitting', 'submitted') AND tx_hash IS NOT NULL) \
            OR (status = 'pending' AND journaled AND tx_hash IS NULL) \
            OR (status = 'failed' AND tx_hash IS NOT NULL \
                AND error IS DISTINCT FROM $1 \
                AND updated_at > NOW() - make_interval(hours => $2)) \
         ORDER BY status = 'failed', updated_at LIMIT 200",
    )
    .bind(REVERTED_ERROR)
    .bind(SECOND_CHANCE_HOURS)
    .fetch_all(pool)
    .await?;

    for row in rows {
        // Never race the request that is signing or sending from this wallet.
        let Some(mut lock) = SpendLock::try_acquire(pool, &row.wallet_address).await? else {
            continue;
        };
        let result = match row.tx_hash.as_deref() {
            None => fail_unsigned(lock.conn(), &row).await,
            Some(tx_hash) => match client_for(row.user_id).await {
                Some(client) => reconcile_on_chain(lock.conn(), &client, &row, tx_hash).await,
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

async fn reconcile_on_chain(
    conn: &mut PgConnection,
    client: &PolygonClient,
    row: &UnresolvedWithdrawal,
    tx_hash: &str,
) -> anyhow::Result<()> {
    match final_receipt(client, tx_hash).await? {
        Receipt::Final(mined) => {
            if finalize_mined(conn, row.id, &mined).await? {
                info!(withdrawal_id = %row.id, success = mined.success, "Withdrawal resolved on chain");
            }
            return Ok(());
        }
        // Mined: wait for its block to become final.
        Receipt::Unsettled => return Ok(()),
        Receipt::Unmined => {}
    }
    if row.status == "failed" {
        // Failed transfers are only watched for a receipt.
        return Ok(());
    }

    if client.transaction_known(tx_hash).await? {
        // A node holds it: it was sent, and its nonce is not used by another.
        sqlx::query(
            "UPDATE wallet_withdrawals SET status = 'submitted', superseded_seen_at = NULL, \
             updated_at = NOW() \
             WHERE id = $1 AND (status = 'submitting' \
                                OR (status = 'submitted' AND superseded_seen_at IS NOT NULL))",
        )
        .bind(row.id)
        .execute(conn)
        .await?;
        return Ok(());
    }
    let Some(nonce) = row.nonce.and_then(|nonce| u64::try_from(nonce).ok()) else {
        return Ok(());
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
                // for any receipt, then it can never be mined.
                if client.transaction_receipt(tx_hash).await?.is_none()
                    && fail_unsent(
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
        return Ok(());
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
        let Some(raw) = row.raw_tx.as_deref() else {
            return Ok(());
        };
        let outcome = client.broadcast(raw, tx_hash).await;
        let accepted = matches!(outcome, Ok(BroadcastOutcome::Accepted));
        sqlx::query(
            "UPDATE wallet_withdrawals SET updated_at = NOW(), \
             status = CASE WHEN $2 THEN 'submitted' ELSE status END \
             WHERE id = $1 AND status IN ('submitting', 'submitted')",
        )
        .bind(row.id)
        .bind(accepted)
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
                    "Node did not take a re-sent withdrawal; it stays unresolved and blocks \
                     this wallet's withdrawals until mined or superseded (see DEPLOY.md)"
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
    }
    Ok(())
}

#[cfg(test)]
#[path = "withdrawals/tests.rs"]
pub(crate) mod tests;
