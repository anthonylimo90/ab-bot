//! On-chain USDC withdrawals: capacity checks, a durable submission journal and
//! a reconciler that resolves every journaled transfer from the chain.
//!
//! A transfer moves through `pending` (row written, nothing signed) →
//! `submitting` (signed; hash, nonce and raw bytes journaled before the
//! broadcast) → `submitted` (a node accepted it) → `confirmed` / `failed`.
//! Only the chain decides the last step: a receipt confirms or reverts it, and
//! a transfer whose nonce was used by another transaction can never be mined.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use polymarket_core::api::polygon::{BroadcastOutcome, MinedTransaction};
use polymarket_core::api::PolygonClient;
use rust_decimal::Decimal;
use sqlx::{PgConnection, PgPool};
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;
use crate::workspace_scope::CANONICAL_WORKSPACE_SQL;

/// How often unresolved withdrawals are checked against the chain.
const RECONCILE_INTERVAL: StdDuration = StdDuration::from_secs(30);
/// A journaled row still `pending` this long was never signed: nothing was sent.
const UNSIGNED_GRACE_MINUTES: i64 = 10;
/// A `submitting` transfer unknown to the node this long is sent again.
const REBROADCAST_AFTER_SECS: i64 = 120;
/// Statuses of a withdrawal that may still move funds.
pub(crate) const UNRESOLVED_STATUSES: [&str; 3] = ["pending", "submitting", "submitted"];

/// Advisory lock key for everything that spends from `address`'s nonces.
pub(crate) fn wallet_lock_key(address: &str) -> String {
    format!("wallet-withdrawal:{}", address.to_lowercase())
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

/// The capacity of `source` given its on-chain balance in USDC units.
pub(crate) async fn withdrawal_capacity(
    conn: &mut PgConnection,
    source: &str,
    balance_units: u128,
) -> Result<WithdrawalCapacity, sqlx::Error> {
    let available_usdc = Decimal::from(balance_units) / Decimal::from(1_000_000u64);
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
            "A previous withdrawal from this wallet has not been confirmed or rejected yet".into(),
        );
    }

    let mut reserved_for_open_buys = Decimal::ZERO;
    if is_trading_wallet(conn, source).await? {
        // Live trades in flight need this wallet's USDC; withdrawing it could
        // leave a hedge half built.
        let (entries_in_flight, unresolved_orders): (bool, bool) = sqlx::query_as(
            "SELECT \
                EXISTS(SELECT 1 FROM positions WHERE state = 0 AND execution_mode = 'live'), \
                EXISTS(SELECT 1 FROM execution_intents WHERE status IN ('submitting', 'unknown'))",
        )
        .fetch_one(&mut *conn)
        .await?;
        if entries_in_flight {
            blocking_reasons.push("Live entries are in progress; retry when they settle".into());
        }
        if unresolved_orders {
            blocking_reasons
                .push("Live orders are still being submitted or reconciled; retry shortly".into());
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

    let max_withdrawable =
        (available_usdc - in_flight_withdrawals - reserved_for_open_buys).max(Decimal::ZERO);
    Ok(WithdrawalCapacity {
        available_usdc,
        in_flight_withdrawals,
        reserved_for_open_buys,
        max_withdrawable,
        blocking_reasons,
    })
}

/// A client-safe description of a chain client error: transport errors can
/// carry the RPC URL (and its API key), so only their kind is kept.
pub(crate) fn chain_error_message(error: &polymarket_core::Error) -> String {
    match error {
        polymarket_core::Error::Http(_) => "Polygon RPC request failed".to_string(),
        other => other.to_string().chars().take(500).collect(),
    }
}

#[derive(sqlx::FromRow)]
struct SettledWithdrawal {
    workspace_id: Option<Uuid>,
    wallet_address: String,
    destination_address: String,
    amount: Decimal,
    user_id: Uuid,
    tx_hash: Option<String>,
}

/// Settle a mined transfer: `confirmed` (recording the cash flow when it left
/// the trading wallet) or `failed` when it reverted. Idempotent; `false` when
/// the row was already resolved.
pub(crate) async fn finalize_mined(
    pool: &PgPool,
    withdrawal_id: Uuid,
    mined: &MinedTransaction,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let settled: Option<SettledWithdrawal> =
        sqlx::query_as(
            "UPDATE wallet_withdrawals SET \
                status = CASE WHEN $2 THEN 'confirmed' ELSE 'failed' END, \
                confirmed_at = CASE WHEN $2 THEN NOW() ELSE confirmed_at END, \
                error = CASE WHEN $2 THEN NULL ELSE 'Transaction reverted on-chain; no USDC moved' END, \
                updated_at = NOW() \
             WHERE id = $1 AND status IN ('submitting', 'submitted') \
             RETURNING workspace_id, wallet_address, destination_address, amount, user_id, tx_hash",
        )
        .bind(withdrawal_id)
        .bind(mined.success)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(SettledWithdrawal {
        workspace_id,
        wallet_address: wallet,
        destination_address: destination,
        amount,
        user_id,
        tx_hash,
    }) = settled
    else {
        return Ok(false);
    };
    if mined.success {
        // Only the trading wallet's balance is trading equity: a withdrawal from
        // any other wallet must not lower the circuit breaker's drawdown peaks.
        if let Some(workspace_id) = workspace_id {
            if is_trading_wallet(&mut tx, &wallet).await? {
                sqlx::query(
                    "INSERT INTO cash_flow_events \
                        (id, workspace_id, event_type, amount, currency, note, occurred_at, created_by) \
                     VALUES ($1, $2, 'withdrawal', $3, 'USDC', $4, NOW(), $5) \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(withdrawal_id)
                .bind(workspace_id)
                .bind(-amount)
                .bind(format!(
                    "On-chain wallet withdrawal to {destination} (tx: {})",
                    tx_hash.as_deref().unwrap_or("unknown")
                ))
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(true)
}

/// Fail an unresolved withdrawal that certainly moved no funds.
pub(crate) async fn fail_unsent(
    pool: &PgPool,
    withdrawal_id: Uuid,
    reason: &str,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE wallet_withdrawals SET status = 'failed', error = $2, updated_at = NOW() \
         WHERE id = $1 AND status = ANY($3)",
    )
    .bind(withdrawal_id)
    .bind(reason)
    .bind(&UNRESOLVED_STATUSES[..])
    .execute(pool)
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
    journaled: bool,
    updated_at: DateTime<Utc>,
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
    let rows: Vec<UnresolvedWithdrawal> = sqlx::query_as(
        "SELECT id, user_id, wallet_address, status, tx_hash, nonce, raw_tx, journaled, updated_at \
         FROM wallet_withdrawals WHERE status = ANY($1) ORDER BY updated_at LIMIT 100",
    )
    .bind(&UNRESOLVED_STATUSES[..])
    .fetch_all(pool)
    .await?;

    for row in rows {
        // Never race the request that is signing or sending from this wallet.
        let mut guard = pool.begin().await?;
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(wallet_lock_key(&row.wallet_address))
                .fetch_one(&mut *guard)
                .await?;
        if !locked {
            continue;
        }
        let result = match row.tx_hash.as_deref() {
            None => reconcile_unsigned(pool, &row).await,
            Some(tx_hash) => match client_for(row.user_id).await {
                Some(client) => reconcile_on_chain(pool, &client, &row, tx_hash).await,
                None => Err(anyhow::anyhow!("no Polygon client available")),
            },
        };
        if let Err(error) = result {
            warn!(withdrawal_id = %row.id, error = %error, "Could not reconcile withdrawal yet");
        }
        guard.commit().await?;
    }
    Ok(())
}

/// Journaled rows record a hash before anything is sent, so one still without
/// it was never signed. Older rows may have been sent without a recorded hash
/// and are left for an operator.
async fn reconcile_unsigned(pool: &PgPool, row: &UnresolvedWithdrawal) -> anyhow::Result<()> {
    if row.journaled
        && row.status == "pending"
        && row.updated_at < Utc::now() - Duration::minutes(UNSIGNED_GRACE_MINUTES)
        && fail_unsent(pool, row.id, "The transfer was never signed or sent").await?
    {
        info!(withdrawal_id = %row.id, "Failed a withdrawal that was never sent");
    }
    Ok(())
}

async fn reconcile_on_chain(
    pool: &PgPool,
    client: &PolygonClient,
    row: &UnresolvedWithdrawal,
    tx_hash: &str,
) -> anyhow::Result<()> {
    if let Some(mined) = client.transaction_receipt(tx_hash).await? {
        finalize_mined(pool, row.id, &mined).await?;
        info!(withdrawal_id = %row.id, success = mined.success, "Withdrawal resolved on chain");
        return Ok(());
    }
    let Some(nonce) = row.nonce.and_then(|n| u64::try_from(n).ok()) else {
        return Ok(());
    };

    let mined_count = client
        .transaction_count(&row.wallet_address, "latest")
        .await?;
    if mined_count > nonce {
        // The nonce is used. Read the receipt again: this transfer may have
        // been the one mined since the first look.
        match client.transaction_receipt(tx_hash).await? {
            Some(mined) => {
                finalize_mined(pool, row.id, &mined).await?;
            }
            None => {
                fail_unsent(
                    pool,
                    row.id,
                    "Another transaction used this transfer's nonce; it was never mined and no funds moved",
                )
                .await?;
                info!(withdrawal_id = %row.id, "Withdrawal superseded; marked failed");
            }
        }
        return Ok(());
    }

    // Still possible. Send it again if the node lost it and nothing else
    // holds its nonce in the mempool.
    if row.status == "submitting"
        && row.updated_at < Utc::now() - Duration::seconds(REBROADCAST_AFTER_SECS)
        && !client.transaction_known(tx_hash).await?
        && client
            .transaction_count(&row.wallet_address, "pending")
            .await?
            <= nonce
    {
        let Some(raw) = row.raw_tx.clone() else {
            return Ok(());
        };
        let signed = polymarket_core::api::polygon::SignedTransaction {
            raw,
            tx_hash: tx_hash.to_string(),
            nonce,
        };
        match client.broadcast(&signed).await? {
            BroadcastOutcome::Accepted => {
                sqlx::query(
                    "UPDATE wallet_withdrawals SET status = 'submitted', updated_at = NOW() \
                     WHERE id = $1 AND status = 'submitting'",
                )
                .bind(row.id)
                .execute(pool)
                .await?;
                info!(withdrawal_id = %row.id, "Re-sent an unresolved withdrawal");
            }
            BroadcastOutcome::Rejected(reason) => {
                warn!(withdrawal_id = %row.id, %reason, "Node refused to re-send withdrawal");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "withdrawals/tests.rs"]
mod tests;
