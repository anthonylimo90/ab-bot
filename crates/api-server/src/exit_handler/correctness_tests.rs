// Regression coverage for persisted position lifecycle invariants.
use super::*;
use crate::position_service::PositionService;
use polymarket_core::types::{ExitStrategy, PositionState};
use sqlx::Executor;

pub(crate) async fn setup(pool: &PgPool) -> PositionService {
    pool.execute(r#"
        CREATE TABLE positions (
            id UUID PRIMARY KEY, market_id TEXT, yes_entry_price NUMERIC,
            no_entry_price NUMERIC, quantity NUMERIC, entry_timestamp TIMESTAMPTZ,
            exit_strategy SMALLINT, state SMALLINT, unrealized_pnl NUMERIC,
            realized_pnl NUMERIC, exit_timestamp TIMESTAMPTZ,
            yes_exit_price NUMERIC, no_exit_price NUMERIC, failure_reason TEXT,
            retry_count INT, last_updated TIMESTAMPTZ, updated_at TIMESTAMPTZ,
            fee_model SMALLINT, resolution_payout_per_share NUMERIC,
            yes_entry_fee_shares NUMERIC, no_entry_fee_shares NUMERIC,
            held_yes_qty NUMERIC, held_no_qty NUMERIC, exited_yes_qty NUMERIC,
            exited_no_qty NUMERIC, resolution_winner TEXT, is_open BOOLEAN,
            opened_at TIMESTAMPTZ, source SMALLINT, source_signal_id UUID,
            current_price NUMERIC
        );
        CREATE TABLE markets(id TEXT, yes_price NUMERIC, no_price NUMERIC);
        CREATE TABLE orders(quantity NUMERIC, filled_quantity NUMERIC, price NUMERIC, side TEXT, status TEXT,id UUID);

        CREATE TABLE quant_signals(id UUID, kind TEXT, direction TEXT, metadata JSONB);
    "#).await.unwrap();
    pool.execute(include_str!(
        "../../../../migrations/069_execution_intents.sql"
    ))
    .await
    .unwrap();
    pool.execute("CREATE TABLE manual_inventory(cost_basis NUMERIC)")
        .await
        .unwrap();
    pool.execute(include_str!(
        "../../../../migrations/076_position_provenance.sql"
    ))
    .await
    .unwrap();
    pool.execute(include_str!(
        "../../../../migrations/078_position_order_links.sql"
    ))
    .await
    .unwrap();
    let events = include_str!("../../../../migrations/048_trade_events.sql")
        .replace("uuid_generate_v4()", "gen_random_uuid()");
    pool.execute(events.as_str()).await.unwrap();
    PositionService::new(pool.clone(), broadcast::channel(32).0)
}

fn context() -> EventContext {
    EventContext {
        wallet_address: Some("0xreview".into()),
        execution_mode: "live".into(),
        strategy: "arb".into(),
        source_label: "arb".into(),
    }
}

fn position() -> Position {
    Position::new(
        "market".into(),
        Decimal::new(4, 1),
        Decimal::ZERO,
        Decimal::TEN,
        ExitStrategy::ExitOnCorrection,
    )
}

async fn handler(pool: &PgPool) -> (ExitHandler, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/book",
        axum::routing::get(|| async {
            axum::Json(serde_json::json!({
                "market":"market","asset_id":"yes","timestamp":"2026-10-20T00:00:00Z",
                "bids":[{"price":"0.5","size":"1000"}],
                "asks":[{"price":"0.6","size":"1000"}]
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut clob = ClobClient::new(Some(format!("http://{address}")), None);
    clob.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let clob = Arc::new(clob);
    let executor = Arc::new(OrderExecutor::new(
        clob.clone(),
        trading_engine::executor::ExecutorConfig::default(),
    ));
    executor.set_live_mode(false);
    let cache = Arc::new(OutcomeTokenCache::new(
        clob.clone(),
        Arc::new(RwLock::new(HashSet::new())),
    ));
    cache.alias("market", &("yes".into(), "no".into())).await;
    let h = ExitHandler::new(
        Arc::new(RwLock::new(ExitHandlerConfig::default())),
        executor,
        Arc::new(CircuitBreaker::new(Default::default())),
        clob,
        broadcast::channel(32).0,
        broadcast::channel(32).0,
        pool.clone(),
        Arc::new(RwLock::new(HashSet::new())),
        Arc::new(AtomicI64::new(0)),
        cache,
    );
    (h, server)
}

#[sqlx::test(migrations = false)]
async fn review_entry_fill_survives_reload(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    service.repo().insert(&p).await.unwrap();
    service
        .record_entry_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(45, 2),
            Decimal::from(8),
            &context(),
            None,
        )
        .await
        .unwrap();
    let row: (Decimal, Decimal, Decimal) =
        sqlx::query_as("SELECT yes_entry_price,quantity,held_yes_qty FROM positions WHERE id=$1")
            .bind(p.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row,
        (Decimal::new(45, 2), Decimal::from(8), Decimal::from(8)),
        "actual fill price and size must be persisted"
    );
}

#[sqlx::test(migrations = false)]
async fn review_partial_exit_does_not_discard_remaining_holdings(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.state = PositionState::Closing;
    p.held_yes_qty = Decimal::TEN;
    service.repo().insert(&p).await.unwrap();
    service
        .record_exit_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(5, 1),
            Decimal::from(4),
            &context(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(p.held_yes_qty, Decimal::from(6));
    let result = service
        .close_position(
            &mut p,
            CloseMethod::MarketExit { fee: Decimal::ZERO },
            &context(),
        )
        .await;
    assert!(
        result.is_err(),
        "partial exit incorrectly closed with held={}, exited={}",
        p.held_yes_qty,
        p.exited_yes_qty
    );
}

#[sqlx::test(migrations = false)]
async fn review_only_one_worker_can_claim_an_exit(pool: PgPool) {
    let service = setup(&pool).await;
    let mut first = position();
    first.state = PositionState::ExitReady;
    first.held_yes_qty = Decimal::TEN;
    service.repo().insert(&first).await.unwrap();
    let mut second = first.clone();
    let ctx = context();
    let (a, b) = tokio::join!(
        service.mark_closing(&mut first, &ctx),
        service.mark_closing(&mut second, &ctx)
    );
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "both workers acquired the same position exit"
    );
}

#[sqlx::test(migrations = false)]
async fn review_reconciler_preserves_explicit_recovery_holdings(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.state = PositionState::ExitFailed;
    p.held_yes_qty = Decimal::TEN;
    p.failure_reason = Some(FailureReason::ConnectivityError {
        message: "connection reset".into(),
    });
    service.repo().insert(&p).await.unwrap();
    crate::position_reconciler::reconcile_cycle(&pool)
        .await
        .unwrap();
    let row: (i16, Decimal, bool) =
        sqlx::query_as("SELECT state,held_yes_qty,is_open FROM positions WHERE id=$1")
            .bind(p.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        row,
        (6, Decimal::TEN, true),
        "reconciliation must preserve known holdings"
    );
}

#[sqlx::test(migrations = false)]
async fn review_reconciler_accepts_legacy_text_reasons(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.state = PositionState::ExitFailed;
    service.repo().insert(&p).await.unwrap();
    sqlx::query("UPDATE positions SET failure_reason='legacy exit failure' WHERE id=$1")
        .bind(p.id)
        .execute(&pool)
        .await
        .unwrap();
    let result = crate::position_reconciler::reconcile_cycle(&pool).await;
    assert!(
        result.is_ok(),
        "plain-text failure aborts reconciliation: {:?}",
        result.err()
    );
}

#[sqlx::test(migrations = false)]
async fn review_live_holdings_cannot_be_paper_closed(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.execution_mode = "live".into();
    p.wallet_address = Some("0xreview".into());
    service.repo().insert(&p).await.unwrap();
    service
        .record_entry_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(4, 1),
            Decimal::TEN,
            &context(),
            None,
        )
        .await
        .unwrap();
    service.mark_open(&mut p, &context()).await.unwrap();
    service
        .mark_exit_ready(&mut p, "manual", &context())
        .await
        .unwrap();
    let (h, server) = handler(&pool).await;
    assert!(h.execute_exit(&mut p).await.is_err());
    server.abort();
    assert_ne!(
        p.state,
        PositionState::Closed,
        "live holdings closed by simulated fills while live mode is disabled"
    );
}

#[sqlx::test(migrations = false)]
async fn review_one_percent_gain_does_not_hit_fifteen_percent_target(pool: PgPool) {
    setup(&pool).await;
    let (h, server) = handler(&pool).await;
    let mut p = position();
    p.unrealized_pnl = p.entry_cost() / Decimal::from(100);
    let should_exit = h.generic_quant_exit(&p, &ExitHandlerConfig::default());
    for (ratio, expected) in [
        (Decimal::new(1499, 4), false),
        (Decimal::new(15, 2), true),
        (-Decimal::new(999, 4), false),
        (-Decimal::new(10, 2), true),
    ] {
        p.unrealized_pnl = p.entry_cost() * ratio;
        assert_eq!(
            h.generic_quant_exit(&p, &ExitHandlerConfig::default()),
            expected
        );
    }
    server.abort();
    assert!(
        !should_exit,
        "1% gain incorrectly triggers default 15% profit target"
    );
}

#[sqlx::test(migrations = false)]
async fn review_control_full_exit_preserves_accounting(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.state = PositionState::Closing;
    p.held_yes_qty = Decimal::TEN;
    service.repo().insert(&p).await.unwrap();
    service
        .record_exit_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(5, 1),
            Decimal::TEN,
            &context(),
            None,
        )
        .await
        .unwrap();
    service
        .close_position(
            &mut p,
            CloseMethod::MarketExit { fee: Decimal::ZERO },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(p.state, PositionState::Closed);
    assert_eq!(p.held_yes_qty, Decimal::ZERO);
    assert_eq!(p.exited_yes_qty, Decimal::TEN);
    assert_eq!(p.realized_pnl, Some(Decimal::ONE));
}

#[sqlx::test(migrations = false)]
async fn partial_exits_accumulate_weighted_proceeds(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.state = PositionState::Closing;
    p.held_yes_qty = Decimal::TEN;
    service.repo().insert(&p).await.unwrap();
    service
        .record_exit_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(5, 1),
            Decimal::from(4),
            &context(),
            None,
        )
        .await
        .unwrap();
    let mut p = service.load(p.id).await.unwrap();
    assert_eq!(p.held_outcomes(), (true, false));
    service
        .record_exit_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(6, 1),
            Decimal::from(6),
            &context(),
            None,
        )
        .await
        .unwrap();
    service
        .close_position(
            &mut p,
            CloseMethod::MarketExit { fee: Decimal::ZERO },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(p.realized_pnl, Some(Decimal::new(16, 1)));
}

#[sqlx::test(migrations = false)]
async fn two_exit_workers_submit_only_one_sale_and_stale_writes_cannot_restore_holdings(
    pool: PgPool,
) {
    let service = setup(&pool).await;
    let mut first = position();
    first.execution_mode = "paper".into();
    first.held_yes_qty = Decimal::TEN;
    first.state = PositionState::ExitReady;
    service.repo().insert(&first).await.unwrap();
    let mut second = first.clone();
    let (handler, server) = handler(&pool).await;
    let (a, b) = tokio::join!(
        handler.execute_exit(&mut first),
        handler.execute_exit(&mut second)
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(handler.order_executor.metrics().orders_submitted, 1);
    let stored = service.load(first.id).await.unwrap();
    assert_eq!(stored.state, PositionState::Closed);
    assert_eq!(stored.exited_yes_qty, Decimal::TEN);
    let stale = if first.version < second.version {
        &mut first
    } else {
        &mut second
    };
    assert!(service.repo().update(stale).await.is_err());
    assert_eq!(
        service.load(stored.id).await.unwrap().held_yes_qty,
        Decimal::ZERO
    );
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn fill_receipt_and_inventory_roll_back_together_and_replay_cannot_double_fill(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    service.repo().insert(&p).await.unwrap();
    let mut stale = p.clone();
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO position_order_links(id,position_id,leg,side,requested_quantity) VALUES($1,$2,'yes','buy',10)").bind(id).bind(p.id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO execution_intents(id,market_id,signed_order,status) VALUES($1,'market','{}','unknown')").bind(id).execute(&pool).await.unwrap();
    service.mark_open(&mut p, &context()).await.unwrap();
    assert!(service
        .record_entry_fill(
            &mut stale,
            Leg::Yes,
            Decimal::new(5, 1),
            Decimal::TEN,
            &context(),
            Some(id)
        )
        .await
        .is_err());
    let settled: bool = sqlx::query_scalar("SELECT settled FROM position_order_links WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!settled);
    service
        .record_entry_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(5, 1),
            Decimal::TEN,
            &context(),
            Some(id),
        )
        .await
        .unwrap();
    let mut replay = service.load(p.id).await.unwrap();
    assert!(service
        .record_entry_fill(
            &mut replay,
            Leg::Yes,
            Decimal::new(5, 1),
            Decimal::TEN,
            &context(),
            Some(id)
        )
        .await
        .is_err());
    assert_eq!(service.load(p.id).await.unwrap().held_yes_qty, Decimal::TEN);
}

#[sqlx::test(migrations = false)]
async fn share_fee_resolution_uses_actual_unequal_leg_basis_without_a_second_fee(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    p.held_yes_qty = Decimal::TEN;
    p.held_no_qty = Decimal::from(7);
    p.no_entry_price = Decimal::new(5, 1);
    p.state = PositionState::Open;
    service.repo().insert(&p).await.unwrap();
    service
        .close_position(
            &mut p,
            CloseMethod::ResolutionWithWinner {
                yes_wins: true,
                fee: Decimal::new(2, 2),
            },
            &context(),
        )
        .await
        .unwrap();
    assert_eq!(p.realized_pnl, Some(Decimal::new(25, 1)));
    assert!(service
        .close_position(
            &mut p,
            CloseMethod::ResolutionWithWinner {
                yes_wins: true,
                fee: Decimal::ZERO
            },
            &context()
        )
        .await
        .is_err());
}
