//! Resolve durable submission uncertainty from authenticated exchange evidence.
use crate::{
    position_service::{CloseMethod, EventContext, Leg},
    state::AppState,
};
use polymarket_core::types::{FailureReason, PositionState};
use rust_decimal::Decimal;
use sqlx::FromRow;
use std::sync::Arc;
use uuid::Uuid;

#[derive(FromRow)]
struct Intent {
    id: Uuid,
    exchange_order_id: Option<String>,
    maker: Option<String>,
    position_id: Option<Uuid>,
    leg: Option<String>,
    side: Option<String>,
    applied_quantity: Option<Decimal>,
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
    // submitter checks this receipt while holding the same lock before POST.
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('live-order-submission'))")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE position_order_links l SET settled=TRUE WHERE NOT settled AND created_at<NOW()-INTERVAL '5 minutes' AND NOT EXISTS(SELECT 1 FROM execution_intents i WHERE i.id=l.id)")
        .execute(&mut *tx).await?;
    tx.commit().await?;
    // Finish interrupted local bookkeeping after all order receipts are settled.
    sqlx::query("UPDATE positions p SET state=CASE WHEN held_yes_qty>0 OR held_no_qty>0 OR exited_yes_qty>0 OR exited_no_qty>0 THEN 2 ELSE 5 END,version=version+1,last_updated=NOW() WHERE state IN (0,3) AND execution_mode='live' AND last_updated<NOW()-INTERVAL '5 minutes' AND EXISTS(SELECT 1 FROM position_order_links l WHERE l.position_id=p.id) AND NOT EXISTS(SELECT 1 FROM position_order_links l WHERE l.position_id=p.id AND NOT settled)")
        .execute(&state.pool).await?;
    let Some(wallet) = state.order_executor.wallet_address().await else {
        return Ok(());
    };
    let intents: Vec<Intent> = sqlx::query_as("SELECT i.id,i.exchange_order_id,i.signed_order->>'maker' AS maker,l.position_id,l.leg,l.side,l.applied_quantity FROM execution_intents i LEFT JOIN position_order_links l ON l.id=i.id WHERE LOWER(i.signed_order->>'maker')=LOWER($1) AND (l.id IS NOT NULL OR EXISTS(SELECT 1 FROM orders o WHERE o.id=i.id)) AND i.status IN ('submitting','unknown','acknowledged') AND i.updated_at<NOW()-INTERVAL '30 seconds' AND (l.id IS NULL OR NOT l.settled) ORDER BY CASE WHEN i.status IN ('submitting','unknown') THEN 0 WHEN l.id IS NOT NULL THEN 1 ELSE 2 END,i.updated_at,i.id LIMIT 100")
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
    let snapshot = state.order_executor.get_exchange_order(exchange_id).await?;
    if snapshot.id != exchange_id {
        anyhow::bail!("Exchange identity mismatch");
    }
    let filled = snapshot.size_matched.parse::<Decimal>()?;
    let original = snapshot.original_size.parse::<Decimal>()?;
    if original <= Decimal::ZERO || filled < Decimal::ZERO || filled > original {
        anyhow::bail!("Invalid fill quantity");
    }
    let terminal = filled == original
        || matches!(
            snapshot.status.to_ascii_lowercase().as_str(),
            "canceled" | "cancelled" | "expired" | "rejected"
        );
    if !terminal {
        anyhow::bail!("Exchange order remains active");
    }
    let (quantity, price) = state
        .order_executor
        .confirmed_exchange_fill(maker, &snapshot)
        .await?;
    if let Some(position_id) = intent.position_id {
        let mut position = state.position_service.load(position_id).await?;
        if position.execution_mode != "live"
            || position.wallet_address.as_deref().map(str::to_lowercase)
                != Some(maker.to_lowercase())
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
        position.state = if quantity.is_zero()
            && !position.has_open_exposure()
            && intent.side.as_deref() == Some("buy")
        {
            PositionState::EntryFailed
        } else {
            PositionState::ExitReady
        };
        if quantity > Decimal::ZERO {
            match intent.side.as_deref() {
                Some("buy") => {
                    state
                        .position_service
                        .record_entry_fill(
                            &mut position,
                            leg,
                            price,
                            quantity,
                            &ctx,
                            Some(intent.id),
                        )
                        .await?
                }
                Some("sell") => {
                    state
                        .position_service
                        .record_exit_fill(
                            &mut position,
                            leg,
                            price,
                            quantity,
                            &ctx,
                            Some(intent.id),
                        )
                        .await?
                }
                _ => anyhow::bail!("Missing intent side"),
            }
        } else {
            state
                .position_service
                .repo()
                .update_with_receipt(&mut position, intent.id, Decimal::ZERO)
                .await?;
        }
        if position.has_open_exposure() {
            // Queue the remaining confirmed inventory for a fresh, exclusive exit.
            position.state = PositionState::ExitReady;
            state.position_service.repo().update(&mut position).await?;
        } else if intent.side.as_deref() == Some("sell") {
            state
                .position_service
                .close_position(
                    &mut position,
                    CloseMethod::MarketExit {
                        fee: Decimal::new(2, 2),
                    },
                    &ctx,
                )
                .await?;
        } else {
            state
                .position_service
                .mark_entry_failed(
                    &mut position,
                    FailureReason::OrderRejected {
                        message: "Exchange confirmed no fill".into(),
                    },
                    &ctx,
                )
                .await?;
        }
    } else {
        crate::handlers::trading::reconcile_manual_intent(&state.pool, intent.id, &snapshot, price)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
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
        assert_eq!(position.state, PositionState::ExitReady);
        let row:(String,Decimal,bool)=sqlx::query_as("SELECT i.status,l.applied_quantity,l.settled FROM execution_intents i JOIN position_order_links l USING(id) WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(row, ("reconciled".into(), Decimal::TEN, true));
        server.abort();
    }
}
