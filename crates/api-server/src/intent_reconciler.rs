//! Resolve durable submission uncertainty from authenticated exchange evidence.
use crate::{
    position_service::{CloseMethod, EventContext, Leg},
    state::AppState,
};
use chrono::{DateTime, Duration, Utc};
use polymarket_core::api::clob::ExchangeOrder;
use polymarket_core::types::{FailureReason, Position, PositionState};
use rust_decimal::Decimal;
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

/// A `submitting`/`unknown` intent that the exchange still has no record of
/// after this long, and that no trade involves, was never executed.
const ABSENCE_GRACE_MINUTES: i64 = 10;
/// How long after its expiry a browser-wallet order of unknown outcome
/// still fences its wallet.
const EXPIRED_WALLET_ORDER_GRACE_SECS: i64 = 600;

#[derive(FromRow)]
struct Intent {
    id: Uuid,
    status: String,
    created_at: DateTime<Utc>,
    exchange_order_id: Option<String>,
    maker: Option<String>,
    token_id: Option<String>,
    position_id: Option<Uuid>,
    leg: Option<String>,
    side: Option<String>,
    applied_quantity: Option<Decimal>,
    has_order: bool,
    /// The order id the exchange's answer named, when it differs from the
    /// order's own hash recorded as its identity.
    named_order_id: Option<String>,
}

/// Authoritative execution outcome of one exchange order.
struct Outcome {
    quantity: Decimal,
    price: Decimal,
    /// The exchange's record; `None` when the order was never received.
    snapshot: Option<ExchangeOrder>,
}

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = reconcile(&state).await {
                tracing::warn!(%error, "Intent reconciliation failed; reservations retained");
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

pub async fn reconcile(state: &AppState) -> anyhow::Result<()> {
    // Retire only attempts which never crossed the durable POST boundary. The
    // submitter checks these markers while holding the same lock before POST.
    let wallet = state.order_executor.wallet_address().await;
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('live-order-submission'))")
        .execute(&mut *tx)
        .await?;
    // Orders signed by browser wallets cannot be read back from the exchange:
    // the server keeps no credentials for those wallets. One whose outcome
    // is unknown can no longer fill once its expiry has long passed, so it
    // stops fencing its wallet; nothing here depends on its outcome. The
    // engine's own orders never match: they are signed by the trading
    // wallet without an expiry (fill-or-kill or good-till-cancelled), and
    // carry a position link or an order row.
    let expired = sqlx::query("UPDATE execution_intents i SET status='reconciled',updated_at=NOW() WHERE i.status IN ('submitting','unknown') AND ($1::text IS NULL OR LOWER(i.signed_order->>'maker')<>LOWER($1)) AND NOT EXISTS(SELECT 1 FROM position_order_links l WHERE l.id=i.id) AND NOT EXISTS(SELECT 1 FROM orders o WHERE o.id=i.id) AND (CASE WHEN i.signed_order->>'expiration' ~ '^[0-9]{1,15}$' THEN (i.signed_order->>'expiration')::bigint END) BETWEEN 1 AND EXTRACT(EPOCH FROM NOW())::bigint-$2")
        .bind(wallet.as_deref())
        .bind(EXPIRED_WALLET_ORDER_GRACE_SECS)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if expired > 0 {
        tracing::warn!(
            count = expired,
            "Browser-wallet orders with an unknown outcome have expired; they no longer fence their wallets"
        );
    }
    sqlx::query("UPDATE position_order_links l SET settled=TRUE WHERE NOT settled AND created_at<NOW()-INTERVAL '5 minutes' AND NOT EXISTS(SELECT 1 FROM execution_intents i WHERE i.id=l.id)")
        .execute(&mut *tx).await?;
    // An order the exchange refused holds nothing: settle its link, as the
    // submitter would have had it not stopped first.
    sqlx::query("UPDATE position_order_links l SET settled=TRUE FROM execution_intents i WHERE i.id=l.id AND i.status='rejected' AND NOT l.settled")
        .execute(&mut *tx).await?;
    sqlx::query("UPDATE orders o SET status='rejected',updated_at=NOW() WHERE status='pending' AND execution_mode='live' AND created_at<NOW()-INTERVAL '10 minutes' AND NOT EXISTS(SELECT 1 FROM execution_intents i WHERE i.id=o.id)")
        .execute(&mut *tx).await?;
    tx.commit().await?;
    // Finish interrupted local bookkeeping after all order receipts are settled:
    // a complete entry opens, held or partly exited inventory is queued for
    // exit, and an entry that acquired nothing has failed.
    sqlx::query(
        "UPDATE positions p SET state=CASE \
           WHEN state=0 AND (held_yes_qty>0 OR held_no_qty>0) \
                AND (yes_entry_price<=0 OR held_yes_qty>0) AND (no_entry_price<=0 OR held_no_qty>0) THEN 1 \
           WHEN held_yes_qty>0 OR held_no_qty>0 OR exited_yes_qty>0 OR exited_no_qty>0 THEN 2 \
           ELSE 5 END, \
         version=version+1,last_updated=NOW() \
         WHERE state IN (0,3) AND execution_mode='live' AND last_updated<NOW()-INTERVAL '5 minutes' \
           AND EXISTS(SELECT 1 FROM position_order_links l WHERE l.position_id=p.id) \
           AND NOT EXISTS(SELECT 1 FROM position_order_links l WHERE l.position_id=p.id AND NOT settled)",
    )
    .execute(&state.pool)
    .await?;
    // Paper fills are simulated synchronously and leave no order links, so a
    // paper entry or exit still in flight after five minutes was interrupted.
    sqlx::query(
        "UPDATE positions SET state=CASE \
           WHEN state=0 AND (held_yes_qty>0 OR held_no_qty>0) \
                AND (yes_entry_price<=0 OR held_yes_qty>0) AND (no_entry_price<=0 OR held_no_qty>0) THEN 1 \
           WHEN held_yes_qty>0 OR held_no_qty>0 OR exited_yes_qty>0 OR exited_no_qty>0 THEN 2 \
           ELSE 5 END, \
         version=version+1,last_updated=NOW() \
         WHERE state IN (0,3) AND execution_mode='paper' AND last_updated<NOW()-INTERVAL '5 minutes'",
    )
    .execute(&state.pool)
    .await?;
    let Some(wallet) = wallet else {
        return Ok(());
    };
    // Every intent of the trading wallet is reconciled, including orphan
    // inventory sells that have neither a position link nor an order row.
    let intents: Vec<Intent> = sqlx::query_as("SELECT i.id,i.status,i.created_at,i.exchange_order_id,i.response->>'orderID' AS named_order_id,i.signed_order->>'maker' AS maker,i.signed_order->>'tokenId' AS token_id,l.position_id,l.leg,l.side,l.applied_quantity,EXISTS(SELECT 1 FROM orders o WHERE o.id=i.id) AS has_order FROM execution_intents i LEFT JOIN position_order_links l ON l.id=i.id WHERE LOWER(i.signed_order->>'maker')=LOWER($1) AND i.status IN ('submitting','unknown','acknowledged') AND i.updated_at<NOW()-INTERVAL '30 seconds' AND (l.id IS NULL OR NOT l.settled) ORDER BY CASE WHEN i.status IN ('submitting','unknown') THEN 0 WHEN l.id IS NOT NULL THEN 1 ELSE 2 END,i.updated_at,i.id LIMIT 100")
        .bind(wallet).fetch_all(&state.pool).await?;
    for intent in intents {
        let id = intent.id;
        if let Err(error) = reconcile_one(state, intent).await {
            tracing::debug!(%error, "Intent still awaiting authoritative reconciliation");
            // Rotate resting orders and unavailable snapshots through the queue;
            // this is a polling timestamp, never a release of their reservation.
            sqlx::query("UPDATE execution_intents SET updated_at=NOW() WHERE id=$1 AND status IN ('submitting','unknown','acknowledged')")
                .bind(id).execute(&state.pool).await?;
        }
    }
    Ok(())
}

async fn reconcile_one(state: &AppState, intent: Intent) -> anyhow::Result<()> {
    let maker = intent
        .maker
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing maker"))?;
    if state
        .order_executor
        .wallet_address()
        .await
        .as_deref()
        .map(str::to_lowercase)
        != Some(maker.to_lowercase())
    {
        anyhow::bail!("Intent belongs to another wallet");
    }
    let exchange_id = intent
        .exchange_order_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Missing exchange identity"))?;
    // An answer may have named another id than the order's own hash: the
    // order is looked for, and its trades checked, under both.
    let named = intent
        .named_order_id
        .as_deref()
        .filter(|named| !named.is_empty() && *named != exchange_id);
    let mut found = state
        .order_executor
        .find_exchange_order(exchange_id)
        .await?;
    if let (None, Some(named)) = (&found, named) {
        found = state.order_executor.find_exchange_order(named).await?;
    }
    let outcome = match found {
        Some(snapshot) => {
            if snapshot.id != exchange_id && Some(snapshot.id.as_str()) != named {
                anyhow::bail!("Exchange identity mismatch");
            }
            let filled = snapshot.size_matched.parse::<Decimal>()?;
            let original = snapshot.original_size.parse::<Decimal>()?;
            if original <= Decimal::ZERO || filled < Decimal::ZERO || filled > original {
                anyhow::bail!("Invalid fill quantity");
            }
            let terminal = filled == original || snapshot.is_closed();
            if !terminal {
                // The exchange holds the order, so it is no longer in doubt:
                // a manual order, whose row keeps its cash reserved, stops
                // fencing the wallet and is followed like any resting order.
                if intent.has_order
                    && intent.position_id.is_none()
                    && matches!(intent.status.as_str(), "submitting" | "unknown")
                {
                    // The order row takes the exchange's id too, so the order
                    // can be refreshed and cancelled by it.
                    sqlx::query("WITH promoted AS (UPDATE execution_intents SET status='acknowledged',exchange_order_id=$2,updated_at=NOW() WHERE id=$1 AND status IN ('submitting','unknown') RETURNING id) UPDATE orders SET exchange_order_id=$2,updated_at=NOW() WHERE id IN (SELECT id FROM promoted)")
                        .bind(intent.id)
                        .bind(&snapshot.id)
                        .execute(&state.pool)
                        .await?;
                }
                anyhow::bail!("Exchange order remains active");
            }
            let (quantity, price) = state
                .order_executor
                .confirmed_exchange_fill(maker, &snapshot)
                .await?;
            Outcome {
                quantity,
                price,
                snapshot: Some(snapshot),
            }
        }
        None => {
            // Absence alone is weak evidence; it is accepted only for an
            // unacknowledged submission, after a grace period, and when no
            // trade of the wallet on that token involves the order.
            if !matches!(intent.status.as_str(), "submitting" | "unknown") {
                anyhow::bail!("Acknowledged order is missing from the exchange");
            }
            if Utc::now() - intent.created_at < Duration::minutes(ABSENCE_GRACE_MINUTES) {
                anyhow::bail!("Exchange has no record of the order yet");
            }
            let token_id = intent
                .token_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Missing token identity"))?;
            let after = (intent.created_at - Duration::minutes(5)).timestamp();
            for id in std::iter::once(exchange_id).chain(named) {
                if state
                    .order_executor
                    .exchange_order_has_trades(maker, id, token_id, after)
                    .await?
                {
                    anyhow::bail!("Trades involve an order the exchange cannot find");
                }
            }
            tracing::warn!(
                intent_id = %intent.id,
                exchange_order_id = %exchange_id,
                "Exchange never received an unresolved submission; releasing it as not executed"
            );
            Outcome {
                quantity: Decimal::ZERO,
                price: Decimal::ZERO,
                snapshot: None,
            }
        }
    };

    if let Some(position_id) = intent.position_id {
        apply_to_position(state, &intent, maker, position_id, &outcome).await
    } else if intent.has_order {
        match &outcome.snapshot {
            Some(snapshot) => {
                crate::handlers::trading::reconcile_manual_intent(
                    &state.pool,
                    intent.id,
                    snapshot,
                    outcome.price,
                )
                .await
            }
            None => {
                crate::handlers::trading::reject_never_executed_manual_intent(
                    &state.pool,
                    intent.id,
                )
                .await
            }
        }
    } else {
        // An orphan-inventory sale: wallet inventory refreshes from observed
        // balances, so only the submission itself needs a terminal status.
        sqlx::query("UPDATE execution_intents SET status=$2,updated_at=NOW() WHERE id=$1")
            .bind(intent.id)
            .bind(if outcome.snapshot.is_some() {
                "reconciled"
            } else {
                "rejected"
            })
            .execute(&state.pool)
            .await?;
        Ok(())
    }
}

async fn apply_to_position(
    state: &AppState,
    intent: &Intent,
    maker: &str,
    position_id: Uuid,
    outcome: &Outcome,
) -> anyhow::Result<()> {
    let mut position = state.position_service.load(position_id).await?;
    if position.execution_mode != "live"
        || position.wallet_address.as_deref().map(str::to_lowercase) != Some(maker.to_lowercase())
    {
        anyhow::bail!("Position provenance mismatch");
    }
    let applied = intent.applied_quantity.unwrap_or_default();
    if applied != Decimal::ZERO {
        anyhow::bail!("Fill already accounted; reload receipt");
    }
    let ctx = EventContext {
        execution_mode: "live".into(),
        wallet_address: Some(maker.into()),
        strategy: "recovery".into(),
        source_label: "recovery".into(),
    };
    let leg = match intent.leg.as_deref() {
        Some("yes") => Leg::Yes,
        Some("no") => Leg::No,
        _ => anyhow::bail!("Missing position leg"),
    };
    let selling = match intent.side.as_deref() {
        Some("buy") => false,
        Some("sell") => true,
        _ => anyhow::bail!("Missing intent side"),
    };
    let service = &state.position_service;
    if outcome.quantity > Decimal::ZERO {
        if selling {
            service
                .record_exit_fill(
                    &mut position,
                    leg,
                    outcome.price,
                    outcome.quantity,
                    &ctx,
                    Some(intent.id),
                )
                .await?;
        } else {
            service
                .record_entry_fill(
                    &mut position,
                    leg,
                    outcome.price,
                    outcome.quantity,
                    &ctx,
                    Some(intent.id),
                )
                .await?;
        }
    } else {
        service
            .repo()
            .update_with_receipt(&mut position, intent.id, Decimal::ZERO)
            .await?;
    }
    if outcome.snapshot.is_none() {
        sqlx::query("UPDATE execution_intents SET status='rejected',updated_at=NOW() WHERE id=$1")
            .bind(intent.id)
            .execute(&state.pool)
            .await?;
    }

    if selling {
        settle_after_sale(state, &mut position, &ctx).await
    } else {
        settle_after_purchase(state, &mut position, &ctx).await
    }
}

/// After a reconciled buy: a complete entry opens (and keeps its exit
/// strategy), an incomplete pair is flattened, and nothing held is a failure.
async fn settle_after_purchase(
    state: &AppState,
    position: &mut Position,
    ctx: &EventContext,
) -> anyhow::Result<()> {
    let service = &state.position_service;
    if !position.has_open_exposure() {
        service
            .mark_entry_failed(
                position,
                FailureReason::OrderRejected {
                    message: "Exchange confirmed no fill".into(),
                },
                ctx,
            )
            .await?;
        state.arb_dedup.write().await.remove(&position.market_id);
    } else if entry_complete(position) {
        if position.state == PositionState::Pending {
            service.mark_open(position, ctx).await?;
        }
    } else if position.state != PositionState::ExitReady {
        // One leg of a planned pair: flatten it rather than hold it naked.
        position.state = PositionState::ExitReady;
        service.repo().update(position).await?;
    }
    Ok(())
}

/// After a reconciled sale: remaining shares are queued for another exit and a
/// flat position closes, feeding the circuit breaker like any other close.
async fn settle_after_sale(
    state: &AppState,
    position: &mut Position,
    ctx: &EventContext,
) -> anyhow::Result<()> {
    let service = &state.position_service;
    service.write_off_unsellable_dust(position, ctx).await?;
    if position.has_open_exposure() {
        // Queue the remaining confirmed inventory for a fresh, exclusive exit.
        position.state = PositionState::ExitReady;
        service.repo().update(position).await?;
        return Ok(());
    }
    service
        .close_position(
            position,
            CloseMethod::MarketExit {
                fee: Decimal::new(2, 2),
            },
            ctx,
        )
        .await?;
    let realized_pnl = position.realized_pnl.unwrap_or_default();
    if let Err(error) = state
        .circuit_breaker
        .record_trade(realized_pnl, realized_pnl > Decimal::ZERO)
        .await
    {
        tracing::warn!(%error, "Failed to record reconciled close with circuit breaker");
    }
    state.arb_dedup.write().await.remove(&position.market_id);
    Ok(())
}

/// Whether every leg the position planned to buy is held.
fn entry_complete(position: &Position) -> bool {
    position.has_open_exposure()
        && (position.yes_entry_price <= Decimal::ZERO || position.held_yes_qty > Decimal::ZERO)
        && (position.no_entry_price <= Decimal::ZERO || position.held_no_qty > Decimal::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use polymarket_core::{
        api::ClobClient,
        types::{ExitStrategy, Position},
    };
    use sqlx::PgPool;
    use std::sync::atomic::{AtomicBool, Ordering};
    use trading_engine::{executor::ExecutorConfig, OrderExecutor};

    #[sqlx::test(migrations = false)]
    async fn confirmed_exchange_evidence_recovers_once_and_unconfirmed_trades_stay_fenced(
        pool: PgPool,
    ) {
        crate::exit_handler::correctness_tests::setup(&pool).await;
        let confirmed = Arc::new(AtomicBool::new(false));
        let flag = confirmed.clone();
        let app=Router::new()
            .route("/auth/api-key",post(|| async {Json(serde_json::json!({"apiKey":"test","secret":"c2VjcmV0","passphrase":"test"}))}))
            .route("/data/order/:id",get(|| async {Json(serde_json::json!({"id":"exchange","status":"MATCHED","original_size":"10","size_matched":"10","associate_trades":["trade"]}))}))
            .route("/data/trades",get(move || {let flag=flag.clone(); async move {Json(serde_json::json!({"data":[{"id":"trade","taker_order_id":"exchange","status":if flag.load(Ordering::SeqCst){"CONFIRMED"}else{"MATCHED"},"size":"10","price":"0.5"}]}))}}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut clob = ClobClient::new(
            Some(format!("http://{}", listener.local_addr().unwrap())),
            None,
        );
        clob.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let wallet = auth::TradingWallet::from_private_key(&format!("{:064x}", 1)).unwrap();
        let executor = OrderExecutor::new_with_wallet(
            Arc::new(clob),
            wallet,
            ExecutorConfig {
                live_trading: true,
                ..Default::default()
            },
        )
        .with_database(pool.clone());
        executor.initialize_live_trading().await.unwrap();
        let maker = executor.wallet_address().await.unwrap();
        let mut state = AppState::for_tests(pool.clone());
        Arc::get_mut(&mut state).unwrap().order_executor = Arc::new(executor);
        let mut position = Position::new(
            "market".into(),
            Decimal::new(4, 1),
            Decimal::ZERO,
            Decimal::TEN,
            ExitStrategy::ExitOnCorrection,
        );
        position.execution_mode = "live".into();
        position.wallet_address = Some(maker.clone());
        state
            .position_service
            .repo()
            .insert(&position)
            .await
            .unwrap();
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO position_order_links(id,position_id,leg,side,requested_quantity) VALUES($1,$2,'yes','buy',10)").bind(id).bind(position.id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO execution_intents(id,market_id,exchange_order_id,signed_order,status,updated_at) VALUES($1,'market','exchange',$2,'unknown',NOW()-INTERVAL '1 minute')")
            .bind(id).bind(serde_json::json!({"maker":maker,"side":"BUY"})).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO execution_intents(id,market_id,signed_order,status,updated_at,created_at) SELECT gen_random_uuid(),'personal','{\"maker\":\"other-wallet\"}','acknowledged',NOW()-INTERVAL '1 hour',NOW()-INTERVAL '1 hour' FROM generate_series(1,150)").execute(&pool).await.unwrap();
        reconcile(&state).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(
            state
                .position_service
                .load(position.id)
                .await
                .unwrap()
                .held_yes_qty,
            Decimal::ZERO
        );
        confirmed.store(true, Ordering::SeqCst);
        sqlx::query(
            "UPDATE execution_intents SET updated_at=NOW()-INTERVAL '1 minute' WHERE id=$1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        reconcile(&state).await.unwrap();
        reconcile(&state).await.unwrap();
        let position = state.position_service.load(position.id).await.unwrap();
        assert_eq!(position.held_yes_qty, Decimal::TEN);
        assert_eq!(position.yes_entry_price, Decimal::new(5, 1));
        // Its only planned leg is now held: the entry opens instead of being
        // queued for an immediate sale.
        assert_eq!(position.state, PositionState::Open);
        let row:(String,Decimal,bool)=sqlx::query_as("SELECT i.status,l.applied_quantity,l.settled FROM execution_intents i JOIN position_order_links l USING(id) WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(row, ("reconciled".into(), Decimal::TEN, true));
        server.abort();
    }

    /// A local exchange: `/data/order/:id` answers `order` (404 when `None`)
    /// and `/data/trades` lists `trades`.
    async fn exchange(
        order: Option<serde_json::Value>,
        trades: serde_json::Value,
    ) -> (ClobClient, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/auth/api-key",
                post(|| async {
                    Json(serde_json::json!({"apiKey":"test","secret":"c2VjcmV0","passphrase":"test"}))
                }),
            )
            .route(
                "/data/order/:id",
                get(move |axum::extract::Path(id): axum::extract::Path<String>| {
                    let order = order.clone();
                    async move {
                        match order {
                            Some(order) if order["id"] == id.as_str() => Json(order).into_response(),
                            _ => axum::http::StatusCode::NOT_FOUND.into_response(),
                        }
                    }
                }),
            )
            .route(
                "/data/trades",
                get(move || {
                    let trades = trades.clone();
                    async move { Json(serde_json::json!({"data": trades, "next_cursor": "LTE="})) }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut clob = ClobClient::new(
            Some(format!("http://{}", listener.local_addr().unwrap())),
            None,
        );
        clob.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (clob, server)
    }

    async fn live_state(pool: &PgPool, clob: ClobClient) -> (Arc<AppState>, String) {
        crate::exit_handler::correctness_tests::setup(pool).await;
        let wallet = auth::TradingWallet::from_private_key(&format!("{:064x}", 1)).unwrap();
        let executor = OrderExecutor::new_with_wallet(
            Arc::new(clob),
            wallet,
            ExecutorConfig {
                live_trading: true,
                ..Default::default()
            },
        )
        .with_database(pool.clone());
        executor.initialize_live_trading().await.unwrap();
        let maker = executor.wallet_address().await.unwrap();
        let mut state = AppState::for_tests(pool.clone());
        Arc::get_mut(&mut state).unwrap().order_executor = Arc::new(executor);
        (state, maker)
    }

    async fn live_position(state: &AppState, maker: &str, yes: i64, no: i64) -> Position {
        let mut position = Position::new(
            "market".into(),
            Decimal::new(yes, 2),
            Decimal::new(no, 2),
            Decimal::TEN,
            ExitStrategy::HoldToResolution,
        );
        position.execution_mode = "live".into();
        position.wallet_address = Some(maker.to_string());
        state
            .position_service
            .repo()
            .insert(&position)
            .await
            .unwrap();
        position
    }

    async fn journal(
        pool: &PgPool,
        maker: &str,
        position: Option<(Uuid, &str, &str)>,
        status: &str,
        age_minutes: i64,
    ) -> Uuid {
        let id = Uuid::new_v4();
        if let Some((position_id, leg, side)) = position {
            sqlx::query("INSERT INTO position_order_links(id,position_id,leg,side,requested_quantity) VALUES($1,$2,$3,$4,10)")
                .bind(id).bind(position_id).bind(leg).bind(side).execute(pool).await.unwrap();
        }
        sqlx::query("INSERT INTO execution_intents(id,market_id,exchange_order_id,signed_order,status,created_at,updated_at) VALUES($1,'market','exchange',$2,$3,NOW()-make_interval(mins=>$4::int),NOW()-make_interval(mins=>$4::int))")
            .bind(id)
            .bind(serde_json::json!({"maker": maker, "tokenId": "token"}))
            .bind(status)
            .bind(age_minutes as i32)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[sqlx::test(migrations = false)]
    async fn completing_a_hedged_pair_opens_it_instead_of_selling_it(pool: PgPool) {
        let (clob, server) = exchange(
            Some(serde_json::json!({"id":"exchange","status":"MATCHED","original_size":"10","size_matched":"10","associate_trades":["trade"]})),
            serde_json::json!([{"id":"trade","taker_order_id":"exchange","status":"CONFIRMED","size":"10","price":"0.5"}]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        let mut position = live_position(&state, &maker, 48, 49).await;
        // The YES leg filled in-process; the NO leg's response was lost.
        position.held_yes_qty = Decimal::TEN;
        state
            .position_service
            .repo()
            .update(&mut position)
            .await
            .unwrap();
        journal(
            &pool,
            &maker,
            Some((position.id, "no", "buy")),
            "unknown",
            1,
        )
        .await;

        reconcile(&state).await.unwrap();

        let stored = state.position_service.load(position.id).await.unwrap();
        assert_eq!(stored.state, PositionState::Open);
        assert_eq!(
            (stored.held_yes_qty, stored.held_no_qty),
            (Decimal::TEN, Decimal::TEN)
        );
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn a_lone_leg_of_a_pair_is_flattened(pool: PgPool) {
        let (clob, server) = exchange(
            Some(serde_json::json!({"id":"exchange","status":"CANCELED","original_size":"10","size_matched":"0","associate_trades":[]})),
            serde_json::json!([]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        let mut position = live_position(&state, &maker, 48, 49).await;
        position.held_yes_qty = Decimal::TEN;
        state
            .position_service
            .repo()
            .update(&mut position)
            .await
            .unwrap();
        journal(
            &pool,
            &maker,
            Some((position.id, "no", "buy")),
            "unknown",
            1,
        )
        .await;

        reconcile(&state).await.unwrap();

        let stored = state.position_service.load(position.id).await.unwrap();
        assert_eq!(stored.state, PositionState::ExitReady);
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn an_order_the_exchange_never_received_releases_the_fence(pool: PgPool) {
        let (clob, server) = exchange(None, serde_json::json!([])).await;
        let (state, maker) = live_state(&pool, clob).await;
        let position = live_position(&state, &maker, 48, 0).await;
        let recent = journal(
            &pool,
            &maker,
            Some((position.id, "yes", "buy")),
            "submitting",
            2,
        )
        .await;

        // Inside the grace period absence proves nothing.
        reconcile(&state).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
            .bind(recent)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "submitting");

        sqlx::query("UPDATE execution_intents SET created_at=NOW()-INTERVAL '20 minutes', updated_at=NOW()-INTERVAL '20 minutes' WHERE id=$1")
            .bind(recent)
            .execute(&pool)
            .await
            .unwrap();
        reconcile(&state).await.unwrap();

        let row: (String, Decimal, bool) = sqlx::query_as("SELECT i.status,l.applied_quantity,l.settled FROM execution_intents i JOIN position_order_links l USING(id) WHERE id=$1")
            .bind(recent)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row, ("rejected".into(), Decimal::ZERO, true));
        let stored = state.position_service.load(position.id).await.unwrap();
        assert_eq!(stored.state, PositionState::EntryFailed);
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn a_refused_order_left_unsettled_releases_its_position(pool: PgPool) {
        let (clob, server) = exchange(None, serde_json::json!([])).await;
        let (state, maker) = live_state(&pool, clob).await;
        let position = live_position(&state, &maker, 48, 0).await;
        // The submitter stopped between the refusal and settling its link.
        let refused = journal(
            &pool,
            &maker,
            Some((position.id, "yes", "buy")),
            "rejected",
            0,
        )
        .await;
        let other = live_position(&state, &maker, 48, 0).await;
        let resting = journal(
            &pool,
            &maker,
            Some((other.id, "yes", "buy")),
            "acknowledged",
            0,
        )
        .await;
        reconcile(&state).await.unwrap();
        for (id, settled) in [(refused, true), (resting, false)] {
            let row: bool =
                sqlx::query_scalar("SELECT settled FROM position_order_links WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(row, settled);
        }
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn an_absent_order_with_trades_stays_fenced(pool: PgPool) {
        let (clob, server) = exchange(
            None,
            serde_json::json!([{"id":"t","taker_order_id":"other","maker_orders":[{"order_id":"exchange"}],"status":"MATCHED"}]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        let position = live_position(&state, &maker, 48, 0).await;
        let id = journal(
            &pool,
            &maker,
            Some((position.id, "yes", "buy")),
            "unknown",
            30,
        )
        .await;

        reconcile(&state).await.unwrap();

        let status: String = sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(
            state
                .position_service
                .load(position.id)
                .await
                .unwrap()
                .state,
            PositionState::Pending
        );
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn orphan_sales_and_unjournaled_manual_orders_are_resolved(pool: PgPool) {
        let (clob, server) = exchange(
            Some(serde_json::json!({"id":"exchange","status":"CANCELED","original_size":"10","size_matched":"0","associate_trades":[]})),
            serde_json::json!([]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        // An orphan-inventory sale has neither a position link nor an order row.
        let orphan = journal(&pool, &maker, None, "unknown", 1).await;
        // A manual order that never reached the journal.
        let manual = Uuid::new_v4();
        sqlx::query("INSERT INTO orders(id,status,execution_mode,created_at,quantity,filled_quantity) VALUES($1,'pending','live',NOW()-INTERVAL '15 minutes',10,0)")
            .bind(manual)
            .execute(&pool)
            .await
            .unwrap();

        reconcile(&state).await.unwrap();

        let orphan_status: String =
            sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
                .bind(orphan)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(orphan_status, "reconciled");
        let manual_status: String = sqlx::query_scalar("SELECT status FROM orders WHERE id=$1")
            .bind(manual)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(manual_status, "rejected");
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn a_manual_order_found_resting_stops_fencing_the_wallet(pool: PgPool) {
        let (clob, server) = exchange(
            Some(serde_json::json!({"id":"exchange","status":"LIVE","original_size":"10","size_matched":"0","associate_trades":[]})),
            serde_json::json!([]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        // Its row keeps the cash reserved while it rests. Journaled under
        // its own hash, it is found under the id the answer named.
        let manual = journal(&pool, &maker, None, "unknown", 1).await;
        sqlx::query(
            "UPDATE execution_intents SET exchange_order_id='0xhash',response=$2 WHERE id=$1",
        )
        .bind(manual)
        .bind(serde_json::json!({"success": false, "orderID": "exchange"}))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO orders(id,status,execution_mode,quantity,filled_quantity,exchange_order_id) VALUES($1,'pending','live',10,0,'0xhash')")
            .bind(manual)
            .execute(&pool)
            .await
            .unwrap();
        // Nothing reserves for these: they keep fencing until they close.
        let orphan = journal(&pool, &maker, None, "unknown", 1).await;
        let position = live_position(&state, &maker, 48, 0).await;
        let linked = journal(
            &pool,
            &maker,
            Some((position.id, "yes", "buy")),
            "unknown",
            1,
        )
        .await;
        reconcile(&state).await.unwrap();
        for (id, expected) in [
            (manual, "acknowledged"),
            (orphan, "unknown"),
            (linked, "unknown"),
        ] {
            let status: String =
                sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(status, expected);
        }
        // Both the intent and the order row now carry the exchange's id.
        let ids: (String, String) = sqlx::query_as("SELECT i.exchange_order_id, o.exchange_order_id FROM execution_intents i JOIN orders o USING(id) WHERE id=$1")
            .bind(manual)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(ids, ("exchange".to_string(), "exchange".to_string()));
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn trades_under_the_named_id_keep_an_unfound_order_fenced(pool: PgPool) {
        let (clob, server) = exchange(
            None,
            serde_json::json!([{"id":"t","taker_order_id":"exchange","status":"MATCHED"}]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        let id = journal(&pool, &maker, None, "unknown", 20).await;
        sqlx::query(
            "UPDATE execution_intents SET exchange_order_id='0xhash',response=$2 WHERE id=$1",
        )
        .bind(id)
        .bind(serde_json::json!({"success": false, "orderID": "exchange"}))
        .execute(&pool)
        .await
        .unwrap();
        reconcile(&state).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "unknown");
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn an_order_is_found_under_the_id_its_answer_named_and_closed_in_any_spelling(
        pool: PgPool,
    ) {
        let (clob, server) = exchange(
            Some(serde_json::json!({"id":"exchange","status":"ORDER_STATUS_CANCELED_MARKET_RESOLVED","original_size":"10","size_matched":"0","associate_trades":[]})),
            serde_json::json!([]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        let direct = journal(&pool, &maker, None, "unknown", 20).await;
        // Journaled under its own hash; the answer named the exchange's id.
        let named = journal(&pool, &maker, None, "unknown", 20).await;
        sqlx::query(
            "UPDATE execution_intents SET exchange_order_id='0xhash',response=$2 WHERE id=$1",
        )
        .bind(named)
        .bind(serde_json::json!({"success": false, "orderID": "exchange"}))
        .execute(&pool)
        .await
        .unwrap();
        reconcile(&state).await.unwrap();
        for id in [direct, named] {
            let status: String =
                sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            // Read from the exchange's record, not released as never sent.
            assert_eq!(status, "reconciled");
        }
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn a_browser_wallet_order_of_unknown_outcome_stops_fencing_once_expired(pool: PgPool) {
        let (clob, server) = exchange(None, serde_json::json!([])).await;
        let (state, maker) = live_state(&pool, clob).await;
        let now = Utc::now().timestamp();
        let mut ids = Vec::new();
        for (wallet, expiration) in [
            ("0xbrowser", now - 3600),
            // Expired moments ago, still within the grace.
            ("0xbrowser", now - 60),
            ("0xbrowser", now + 3600),
            // Never expires: how the engine signs every order.
            ("0xbrowser", 0),
            // The trading wallet's own orders are read from the exchange.
            (maker.as_str(), now - 3600),
        ] {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO execution_intents(id,market_id,exchange_order_id,signed_order,status) VALUES($1,'token','hash',$2,'unknown')")
                .bind(id)
                .bind(serde_json::json!({"maker": wallet, "tokenId": "token", "expiration": expiration.to_string()}))
                .execute(&pool)
                .await
                .unwrap();
            ids.push(id);
        }
        // One with a position link or an order row is never released so.
        for (link, order) in [(true, false), (false, true)] {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO execution_intents(id,market_id,exchange_order_id,signed_order,status) VALUES($1,'token','hash',$2,'unknown')")
                .bind(id)
                .bind(serde_json::json!({"maker": "0xbrowser", "tokenId": "token", "expiration": (now - 3600).to_string()}))
                .execute(&pool)
                .await
                .unwrap();
            if link {
                let position = live_position(&state, "0xbrowser", 48, 0).await;
                sqlx::query("INSERT INTO position_order_links(id,position_id,leg,side,requested_quantity,settled) VALUES($1,$2,'yes','buy',10,TRUE)")
                    .bind(id)
                    .bind(position.id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            if order {
                sqlx::query("INSERT INTO orders(id,status,execution_mode,quantity,filled_quantity) VALUES($1,'pending','live',10,0)")
                    .bind(id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            ids.push(id);
        }
        reconcile(&state).await.unwrap();
        let mut statuses = Vec::new();
        for id in ids {
            let status: String =
                sqlx::query_scalar("SELECT status FROM execution_intents WHERE id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            statuses.push(status);
        }
        assert_eq!(
            statuses,
            [
                "reconciled",
                "unknown",
                "unknown",
                "unknown",
                "unknown",
                "unknown",
                "unknown"
            ]
        );
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn reconciled_exit_closes_through_the_circuit_breaker(pool: PgPool) {
        let (clob, server) = exchange(
            Some(serde_json::json!({"id":"exchange","status":"MATCHED","original_size":"10","size_matched":"10","associate_trades":["trade"]})),
            serde_json::json!([{"id":"trade","taker_order_id":"exchange","status":"CONFIRMED","size":"10","price":"0.3"}]),
        )
        .await;
        let (state, maker) = live_state(&pool, clob).await;
        let mut position = live_position(&state, &maker, 50, 0).await;
        position.held_yes_qty = Decimal::TEN;
        position.state = PositionState::Closing;
        state
            .position_service
            .repo()
            .update(&mut position)
            .await
            .unwrap();
        state.arb_dedup.write().await.insert("market".into());
        journal(
            &pool,
            &maker,
            Some((position.id, "yes", "sell")),
            "unknown",
            1,
        )
        .await;

        reconcile(&state).await.unwrap();

        let stored = state.position_service.load(position.id).await.unwrap();
        assert_eq!(stored.state, PositionState::Closed);
        assert!(stored.realized_pnl.unwrap() < Decimal::ZERO);
        let breaker = state.circuit_breaker.state().await;
        assert_eq!(breaker.consecutive_losses, 1);
        assert!(breaker.daily_pnl < Decimal::ZERO);
        assert!(!state.arb_dedup.read().await.contains("market"));
        server.abort();
    }

    #[sqlx::test(migrations = false)]
    async fn interrupted_paper_entries_and_exits_are_finished(pool: PgPool) {
        crate::exit_handler::correctness_tests::setup(&pool).await;
        let state = AppState::for_tests(pool.clone());
        let mut ids = Vec::new();
        for (initial_state, yes, no, held_yes, held_no) in [
            (PositionState::Pending, 48, 50, 10, 10), // complete paper pair
            (PositionState::Pending, 48, 50, 10, 0),  // one leg of a pair
            (PositionState::Pending, 48, 0, 0, 0),    // nothing acquired
            (PositionState::Closing, 48, 0, 4, 0),    // interrupted exit
        ] {
            let mut position = Position::new(
                "market".into(),
                Decimal::new(yes, 2),
                Decimal::new(no, 2),
                Decimal::TEN,
                ExitStrategy::HoldToResolution,
            );
            position.execution_mode = "paper".into();
            position.held_yes_qty = Decimal::from(held_yes);
            position.held_no_qty = Decimal::from(held_no);
            position.state = initial_state;
            state
                .position_service
                .repo()
                .insert(&position)
                .await
                .unwrap();
            ids.push(position.id);
        }
        sqlx::query("UPDATE positions SET last_updated=NOW()-INTERVAL '10 minutes'")
            .execute(&pool)
            .await
            .unwrap();

        reconcile(&state).await.unwrap();

        let mut states = Vec::new();
        for id in ids {
            states.push(state.position_service.load(id).await.unwrap().state);
        }
        assert_eq!(
            states,
            [
                PositionState::Open,
                PositionState::ExitReady,
                PositionState::EntryFailed,
                PositionState::ExitReady
            ]
        );
    }
}
