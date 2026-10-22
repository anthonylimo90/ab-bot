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
            taker_fee_rate NUMERIC, taker_fee_exponent NUMERIC, fees_paid NUMERIC NOT NULL DEFAULT 0,
            held_yes_qty NUMERIC, held_no_qty NUMERIC, exited_yes_qty NUMERIC,
            exited_no_qty NUMERIC, resolution_winner TEXT, is_open BOOLEAN,
            opened_at TIMESTAMPTZ, source SMALLINT, source_signal_id UUID,
            current_price NUMERIC
        );
        CREATE TABLE markets(id TEXT, yes_price NUMERIC, no_price NUMERIC);
        CREATE TABLE orders(quantity NUMERIC, filled_quantity NUMERIC, price NUMERIC, side TEXT, status TEXT,id UUID, execution_mode TEXT NOT NULL DEFAULT 'unknown', created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), updated_at TIMESTAMPTZ, exchange_order_id TEXT);

        CREATE TABLE quant_signals(id UUID, kind TEXT, direction TEXT, metadata JSONB);
    "#).await.unwrap();
    pool.execute(include_str!(
        "../../../../migrations/069_execution_intents.sql"
    ))
    .await
    .unwrap();
    pool.execute(
        "CREATE TABLE manual_inventory(execution_mode TEXT, wallet_address TEXT, market_id TEXT, \
         outcome TEXT, quantity NUMERIC NOT NULL DEFAULT 0, cost_basis NUMERIC NOT NULL DEFAULT 0)",
    )
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
    handler_with(
        pool,
        book_router(serde_json::json!([{"price":"0.5","size":"1000"}])),
    )
    .await
}

fn book_router(bids: serde_json::Value) -> axum::Router {
    axum::Router::new().route(
        "/book",
        axum::routing::get(move || {
            let bids = bids.clone();
            async move {
                axum::Json(serde_json::json!({
                    "market":"market","asset_id":"yes","timestamp":"2026-10-20T00:00:00Z",
                    "bids": bids,
                    "asks":[{"price":"0.6","size":"1000"}]
                }))
            }
        }),
    )
}

async fn handler_with(
    pool: &PgPool,
    app: axum::Router,
) -> (ExitHandler, tokio::task::JoinHandle<()>) {
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

fn paper_exit_ready(held_yes: Decimal) -> Position {
    let mut p = position();
    p.execution_mode = "paper".into();
    p.held_yes_qty = held_yes;
    p.quantity = held_yes;
    p.state = PositionState::ExitReady;
    p
}

#[sqlx::test(migrations = false)]
async fn exit_sells_whole_lots_and_writes_off_unsellable_dust(pool: PgPool) {
    let service = setup(&pool).await;
    // A $25 market buy at 0.48 leaves fractional shares.
    let mut p = paper_exit_ready("52.0833".parse().unwrap());
    service.repo().insert(&p).await.unwrap();
    let (handler, server) = handler(&pool).await;

    handler.execute_exit(&mut p).await.unwrap();

    let stored = service.load(p.id).await.unwrap();
    assert_eq!(
        stored.state,
        PositionState::Closed,
        "{:?}",
        stored.failure_reason
    );
    assert_eq!(stored.held_yes_qty, Decimal::ZERO);
    assert_eq!(stored.exited_yes_qty, "52.0833".parse::<Decimal>().unwrap());
    // Only 52.08 shares were sold at 0.50; the 0.0033 remainder earned nothing.
    let proceeds = stored.yes_exit_price.unwrap() * stored.exited_yes_qty;
    assert_eq!(proceeds.round_dp(8), "26.04".parse::<Decimal>().unwrap());
    assert_eq!(handler.order_executor.metrics().orders_submitted, 1);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn exit_walks_thin_bids_within_slippage(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = paper_exit_ready(Decimal::from(500));
    service.repo().insert(&p).await.unwrap();
    // Worst-first, as the exchange sends them; the best level alone is too small.
    let (handler, server) = handler_with(
        &pool,
        book_router(serde_json::json!([
            {"price":"0.40","size":"1000"},
            {"price":"0.499","size":"300"},
            {"price":"0.50","size":"300"}
        ])),
    )
    .await;

    handler.execute_exit(&mut p).await.unwrap();

    let stored = service.load(p.id).await.unwrap();
    assert_eq!(
        stored.state,
        PositionState::Closed,
        "{:?}",
        stored.failure_reason
    );
    // 300 @ 0.50 + 200 @ 0.499; the 0.40 level is beyond the 1% tolerance.
    assert_eq!(stored.yes_exit_price, Some("0.4996".parse().unwrap()));
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn exit_without_liquidity_inside_slippage_fails_and_keeps_shares(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = paper_exit_ready(Decimal::from(500));
    service.repo().insert(&p).await.unwrap();
    let (handler, server) = handler_with(
        &pool,
        book_router(serde_json::json!([
            {"price":"0.40","size":"1000"},
            {"price":"0.50","size":"100"}
        ])),
    )
    .await;

    handler.execute_exit(&mut p).await.unwrap();

    let stored = service.load(p.id).await.unwrap();
    assert_eq!(stored.state, PositionState::ExitFailed);
    assert_eq!(stored.held_yes_qty, Decimal::from(500));
    assert_eq!(stored.retry_count, 1);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn failed_exits_keep_retrying_at_a_slowing_pace(pool: PgPool) {
    let service = setup(&pool).await;
    let (handler, server) = handler(&pool).await;
    let cfg = ExitHandlerConfig::default();

    let mut exhausted = paper_exit_ready(Decimal::TEN);
    exhausted.state = PositionState::ExitFailed;
    exhausted.retry_count = 7;
    service.repo().insert(&exhausted).await.unwrap();
    let mut recent = paper_exit_ready(Decimal::TEN);
    recent.state = PositionState::ExitFailed;
    recent.retry_count = 2;
    service.repo().insert(&recent).await.unwrap();
    sqlx::query("UPDATE positions SET last_updated = NOW() - INTERVAL '2 hours' WHERE id = $1")
        .bind(exhausted.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE positions SET last_updated = NOW() - INTERVAL '6 minutes' WHERE id = $1")
        .bind(recent.id)
        .execute(&pool)
        .await
        .unwrap();

    handler.process_failed_exits(&cfg).await.unwrap();

    // Past the old three-attempt cap, an hour-old failure is retried again...
    assert_eq!(
        service.load(exhausted.id).await.unwrap().state,
        PositionState::ExitReady
    );
    // ...while the second failure waits for its doubled (10 minute) backoff.
    assert_eq!(
        service.load(recent.id).await.unwrap().state,
        PositionState::ExitFailed
    );
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn resolution_settles_failed_exits_in_named_outcome_markets(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = paper_exit_ready(Decimal::TEN);
    p.state = PositionState::ExitFailed;
    p.retry_count = 9;
    service.repo().insert(&p).await.unwrap();

    let app = book_router(serde_json::json!([])).route(
        "/markets/:id",
        axum::routing::get(|| async {
            axum::Json(serde_json::json!({
                "condition_id": "market",
                "question": "Who wins?",
                "closed": true,
                "tokens": [
                    {"token_id": "no", "outcome": "Lakers", "winner": true},
                    {"token_id": "yes", "outcome": "Celtics", "winner": false}
                ]
            }))
        }),
    );
    let (handler, server) = handler_with(&pool, app).await;

    handler.check_market_resolutions().await.unwrap();

    let stored = service.load(p.id).await.unwrap();
    assert_eq!(stored.state, PositionState::Closed);
    assert_eq!(stored.resolution_winner.as_deref(), Some("no"));
    assert_eq!(stored.held_yes_qty, Decimal::ZERO);
    assert!(stored.realized_pnl.unwrap() < Decimal::ZERO);
    server.abort();
}

#[test]
fn failed_exit_retry_delay_doubles_and_caps() {
    assert_eq!(failed_exit_retry_delay_secs(300, 1), 300);
    assert_eq!(failed_exit_retry_delay_secs(300, 2), 600);
    assert_eq!(failed_exit_retry_delay_secs(300, 3), 1200);
    assert_eq!(failed_exit_retry_delay_secs(300, 5), 3600);
    assert_eq!(failed_exit_retry_delay_secs(300, 40), 3600);
    assert_eq!(failed_exit_retry_delay_secs(7200, 3), 7200);
}

#[sqlx::test(migrations = false)]
async fn an_unchanged_mark_is_not_rewritten_each_tick(pool: PgPool) {
    let service = setup(&pool).await;
    // A 2.5% gain: below every exit threshold.
    let (h, server) = handler_with(
        &pool,
        book_router(serde_json::json!([{"price":"0.41","size":"1000"}])),
    )
    .await;
    let mut p = position();
    p.state = PositionState::Open;
    p.held_yes_qty = Decimal::TEN;
    service.repo().insert(&p).await.unwrap();
    let version = || async {
        sqlx::query_scalar::<_, i64>("SELECT version FROM positions WHERE id = $1")
            .bind(p.id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let mut candidate = polymarket_core::db::positions::ExitCandidate {
        position: p.clone(),
        source: polymarket_core::db::positions::SOURCE_MANUAL,
        source_signal_id: None,
    };
    let cfg = ExitHandlerConfig::default();

    let before = version().await;
    h.evaluate_open_candidate(&mut candidate, None, &cfg)
        .await
        .unwrap();
    assert_eq!(version().await, before + 1, "a new mark is written");
    h.evaluate_open_candidate(&mut candidate, None, &cfg)
        .await
        .unwrap();
    assert_eq!(version().await, before + 1, "the same mark is not");
    assert_eq!(candidate.position.state, PositionState::Open);
    server.abort();
}

#[test]
fn only_a_fee_sized_shortfall_is_written_off() {
    let held = Decimal::from(100);
    // 0.07 × (1 − 0.5) of the shares bought at 0.50.
    assert_eq!(
        super::fee_shortfall(held, Decimal::new(965, 1)),
        Some(Decimal::new(35, 1))
    );
    assert_eq!(
        super::fee_shortfall(held, Decimal::from(90)),
        Some(Decimal::from(10))
    );
    // More missing than any fee keeps: a discrepancy, not a fee.
    assert_eq!(super::fee_shortfall(held, Decimal::from(89)), None);
    assert_eq!(super::fee_shortfall(held, Decimal::ZERO), None);
    // Enough (or more) in the wallet: nothing to write off.
    assert_eq!(super::fee_shortfall(held, held), None);
    assert_eq!(super::fee_shortfall(held, Decimal::from(150)), None);
}

#[sqlx::test(migrations = false)]
async fn missing_shares_written_off_lower_the_realized_pnl(pool: PgPool) {
    let service = setup(&pool).await;
    let ctx = context();
    // The wallet held 96.5 of the 100 shares recorded at 0.50; all 96.5 sold
    // at 0.60. Realized P&L under each fee model, closing with a 2% flat fee.
    let realized = |model: polymarket_core::types::PositionFeeModel| {
        let service = &service;
        let ctx = &ctx;
        async move {
            let mut p = position();
            p.fee_model = model;
            p.state = PositionState::Closing;
            p.yes_entry_price = Decimal::new(50, 2);
            p.held_yes_qty = Decimal::from(100);
            p.quantity = Decimal::from(100);
            service.repo().insert(&p).await.unwrap();
            service
                .write_off_missing_shares(&mut p, Leg::Yes, Decimal::new(35, 1), ctx)
                .await
                .unwrap();
            service
                .record_exit_fill(
                    &mut p,
                    Leg::Yes,
                    Decimal::new(60, 2),
                    Decimal::new(965, 1),
                    ctx,
                    None,
                )
                .await
                .unwrap();
            service
                .close_position(
                    &mut p,
                    CloseMethod::MarketExit {
                        fee: Decimal::new(2, 2),
                    },
                    ctx,
                )
                .await
                .unwrap();
            p.realized_pnl.unwrap()
        }
    };
    // Share-based: 96.5 × 0.60 − 100 × 0.50 = 7.90: the missing shares' cost
    // is lost (no fee charged in USDC here), not 10 from 100.
    assert_eq!(
        realized(polymarket_core::types::PositionFeeModel::ShareBased).await,
        Decimal::new(790, 2)
    );
    // Flat: the missing shares are lost too, and the 2% rate is charged on
    // the volume: 57.9 − 50 − 2% × (50 + 57.9) = 5.742.
    assert_eq!(
        realized(polymarket_core::types::PositionFeeModel::LegacyFlat).await,
        Decimal::new(5742, 3)
    );
}

/// An exit handler whose executor trades live as the wallet of private key 1
/// against a mock exchange reporting `balance` (base units) for any token.
async fn live_handler(
    pool: &PgPool,
    balance: Arc<std::sync::Mutex<String>>,
) -> (ExitHandler, String, tokio::task::JoinHandle<()>) {
    live_handler_with(pool, balance, true).await
}

/// [`live_handler`], with the executor's wallet loaded or (a signer that
/// failed to load) not; the wallet's address is returned either way.
async fn live_handler_with(
    pool: &PgPool,
    balance: Arc<std::sync::Mutex<String>>,
    load_wallet: bool,
) -> (ExitHandler, String, tokio::task::JoinHandle<()>) {
    use axum::{extract::State, routing::get, routing::post, Json};
    let app = axum::Router::new()
        .route(
            "/auth/api-key",
            post(|| async {
                Json(serde_json::json!({"apiKey": "key", "secret": "c2VjcmV0", "passphrase": "pass"}))
            }),
        )
        .route(
            "/clob-markets/:condition",
            get(|| async { Json(serde_json::json!({"fd": {"r": 0.07, "e": 1, "to": true}})) }),
        )
        .route(
            "/balance-allowance",
            get(
                |State(balance): State<Arc<std::sync::Mutex<String>>>,
                 axum::extract::Query(query): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| async move {
                    // "token:units,…" answers per token; anything else for all.
                    let balance = balance.lock().unwrap().clone();
                    let token = query.get("token_id").cloned().unwrap_or_default();
                    let balance = balance
                        .split(',')
                        .find_map(|entry| {
                            entry
                                .split_once(':')
                                .filter(|(id, _)| *id == token)
                                .map(|(_, units)| units.to_string())
                        })
                        .unwrap_or(balance);
                    Json(serde_json::json!({"balance": balance, "allowance": "0"}))
                },
            ),
        )
        .with_state(balance);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut clob = ClobClient::new(Some(format!("http://{address}")), None);
    clob.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let clob = Arc::new(clob);
    let executor = Arc::new(OrderExecutor::new(
        clob.clone(),
        trading_engine::executor::ExecutorConfig {
            live_trading: true,
            ..Default::default()
        },
    ));
    let signer = auth::TradingWallet::from_private_key(&format!("{:064x}", 1)).unwrap();
    let wallet = if load_wallet {
        executor.reload_wallet(signer).await.unwrap()
    } else {
        signer.address_string()
    };
    let cache = Arc::new(OutcomeTokenCache::new(
        clob.clone(),
        Arc::new(RwLock::new(HashSet::new())),
    ));
    let h = ExitHandler::new(
        Arc::new(RwLock::new(ExitHandlerConfig::default())),
        executor,
        Arc::new(CircuitBreaker::new(Default::default())),
        clob,
        broadcast::channel(32).0,
        broadcast::channel(32).0,
        pool.clone(),
        Arc::new(AtomicI64::new(0)),
        cache,
    );
    (h, wallet, server)
}

#[sqlx::test(migrations = false)]
async fn fee_shares_are_written_off_only_once_the_token_has_settled(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, wallet, server) = live_handler(&pool, balance.clone()).await;
    let set_balance = |shares: &str| {
        *balance.lock().unwrap() =
            (shares.parse::<Decimal>().unwrap() * Decimal::from(1_000_000)).to_string()
    };
    let ctx = context();
    let live = |held: i64| {
        let mut p = position();
        p.state = PositionState::Closing;
        p.execution_mode = "live".into();
        p.wallet_address = Some(wallet.clone());
        p.held_yes_qty = Decimal::from(held);
        p
    };
    let mut p = live(100);
    service.repo().insert(&p).await.unwrap();
    let held = |p: &Position| p.held_yes_qty;

    let intent_by = |maker: String, side: &'static str, status: &'static str| {
        sqlx::query(
            "INSERT INTO execution_intents (id, market_id, signed_order, status) VALUES \
             (gen_random_uuid(), 'market', jsonb_build_object('tokenId', 'yes', 'side', $1::text, \
              'maker', $3::text), $2)",
        )
        .bind(side)
        .bind(status)
        .bind(maker)
        .execute(&pool)
    };
    let intent = |side: &'static str, status: &'static str| intent_by(wallet.clone(), side, status);
    // A buy on the token placed minutes ago may still be settling: the
    // wallet can be short by shares on their way, so nothing is written off.
    set_balance("96.5");
    intent("BUY", "acknowledged").await.unwrap();
    h.align_leg_with_wallet(&mut p, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(held(&p), Decimal::from(100));

    // Once settled, a fee-sized shortfall is written off; a refused buy and
    // an exit sell placed since bring no shares and do not hold it back.
    pool.execute("UPDATE execution_intents SET created_at = NOW() - INTERVAL '31 minutes'")
        .await
        .unwrap();
    intent("BUY", "rejected").await.unwrap();
    intent("SELL", "acknowledged").await.unwrap();
    // Another wallet's buy of the token settles into that wallet.
    intent_by(
        "0x0000000000000000000000000000000000000009".into(),
        "BUY",
        "acknowledged",
    )
    .await
    .unwrap();
    h.align_leg_with_wallet(&mut p, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(held(&p), Decimal::new(965, 1));
    assert_eq!(p.exited_yes_qty, Decimal::new(35, 1));

    // An order whose outcome is unknown, even a sell, blocks it however old
    // it is: a sale that went through may not be recorded yet.
    let mut p = live(100);
    service.repo().insert(&p).await.unwrap();
    pool.execute(
        "UPDATE execution_intents SET status = 'unknown' WHERE signed_order->>'side' = 'SELL'",
    )
    .await
    .unwrap();
    h.align_leg_with_wallet(&mut p, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(held(&p), Decimal::from(100));
    pool.execute("UPDATE execution_intents SET status = 'reconciled' WHERE status = 'unknown'")
        .await
        .unwrap();

    // The wallet's other positions and manual orders hold the token too: the
    // first position 96.5, another 50 and manual orders 20. With 263 in the
    // wallet, 3.5 of the 266.5 they all record is missing, and this position
    // answers for its share of it.
    let mut other = live(50);
    other.id = uuid::Uuid::new_v4();
    service.repo().insert(&other).await.unwrap();
    sqlx::query("INSERT INTO manual_inventory VALUES ('live', LOWER($1), 'market', 'yes', 20, 10)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .unwrap();
    set_balance("263");
    h.align_leg_with_wallet(&mut p, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    let share = Decimal::new(35, 1) * Decimal::from(100) / Decimal::new(2665, 1);
    assert_eq!(held(&p), Decimal::from(100) - share);
    assert_eq!(p.exited_yes_qty, share);

    // A gap no fee explains is left for reconciliation: 300 of the 365.2 or
    // so that the holders record.
    let mut p = live(100);
    service.repo().insert(&p).await.unwrap();
    set_balance("300");
    h.align_leg_with_wallet(&mut p, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(held(&p), Decimal::from(100));

    // A position of another wallet is never measured against this one.
    let mut p = live(100);
    p.wallet_address = Some("0x0000000000000000000000000000000000000002".into());
    service.repo().insert(&p).await.unwrap();
    set_balance("96.5");
    h.align_leg_with_wallet(&mut p, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(held(&p), Decimal::from(100));
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_pair_resolved_without_a_winner_pays_only_what_the_wallet_holds(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, wallet, server) = live_handler(&pool, balance.clone()).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let set_balance = |shares: &str| {
        *balance.lock().unwrap() =
            (shares.parse::<Decimal>().unwrap() * Decimal::from(1_000_000)).to_string()
    };
    let live_pair = || {
        let mut p = position();
        p.state = PositionState::Open;
        p.execution_mode = "live".into();
        p.wallet_address = Some(wallet.clone());
        p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
        p.no_entry_price = Decimal::new(5, 1);
        p.held_yes_qty = Decimal::from(100);
        p.held_no_qty = Decimal::from(100);
        p
    };

    // A V1 buy kept 3.5 shares of each leg as its fee: the pair pays 96.5,
    // not the 100 recorded. Cost 0.40 + 0.50 a pair: 96.5 − 90 = 6.5.
    let mut p = live_pair();
    service.repo().insert(&p).await.unwrap();
    set_balance("96.5");
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, None)
        .await
        .unwrap());
    let stored = service.load(p.id).await.unwrap();
    assert_eq!(stored.state, PositionState::Closed);
    assert_eq!(stored.realized_pnl, Some(Decimal::new(65, 1)));

    // While an order of the wallet on the token has an unknown outcome the
    // holding cannot be measured: the close waits.
    let mut p = live_pair();
    p.id = uuid::Uuid::new_v4();
    service.repo().insert(&p).await.unwrap();
    sqlx::query(
        "INSERT INTO execution_intents (id, market_id, signed_order, status) VALUES \
         (gen_random_uuid(), 'market', jsonb_build_object('tokenId', 'yes', 'side', 'SELL', \
          'maker', $1::text), 'unknown')",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, None)
        .await
        .unwrap());
    assert_eq!(service.load(p.id).await.unwrap().state, PositionState::Open);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_resolved_winner_pays_what_the_wallet_holds_shared_among_its_positions(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, wallet, server) = live_handler(&pool, balance.clone()).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let set_balance = |shares: &str| {
        *balance.lock().unwrap() =
            (shares.parse::<Decimal>().unwrap() * Decimal::from(1_000_000)).to_string()
    };
    let live_yes = || {
        let mut p = position();
        p.id = uuid::Uuid::new_v4();
        p.state = PositionState::Open;
        p.execution_mode = "live".into();
        p.wallet_address = Some(wallet.clone());
        p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
        p.held_yes_qty = Decimal::from(100);
        p
    };
    // Two positions of 100 YES at 0.40; V1 buys kept 7 shares between them.
    let mut a = live_yes();
    let b = live_yes();
    service.repo().insert(&a).await.unwrap();
    service.repo().insert(&b).await.unwrap();
    set_balance("193");
    assert!(h
        .finalize_resolution_close(&mut a, 0, None, Some(true))
        .await
        .unwrap());
    // A pays 96.5 − 40; its sibling was measured at the same time, so its
    // own close, with A's shares still in the wallet, pays 96.5 too.
    assert_eq!(a.realized_pnl, Some(Decimal::new(565, 1)));
    let mut b = service.load(b.id).await.unwrap();
    assert_eq!(b.held_yes_qty, Decimal::new(965, 1));
    assert!(h
        .finalize_resolution_close(&mut b, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(b.realized_pnl, Some(Decimal::new(565, 1)));

    // Another wallet's holding cannot be measured here: it closes at its
    // record, and its close says so.
    let mut other = live_yes();
    other.wallet_address = Some("0x0000000000000000000000000000000000000002".into());
    service.repo().insert(&other).await.unwrap();
    assert!(h
        .finalize_resolution_close(&mut other, 0, None, Some(true))
        .await
        .unwrap());
    let note: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM trade_events WHERE position_id = $1 AND event_type = 'closed_via_resolution'",
    )
    .bind(other.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(note["unmeasured"][0]["leg"], "yes");

    // Already redeemed (or moved): the wallet holds none; the record stands
    // and the position closes rather than waiting for ever: 100 − 40.
    let mut c = live_yes();
    service.repo().insert(&c).await.unwrap();
    set_balance("0");
    assert!(h
        .finalize_resolution_close(&mut c, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(c.realized_pnl, Some(Decimal::from(60)));

    // The losing leg is never measured: it pays nothing either way.
    let mut d = live_yes();
    d.held_no_qty = Decimal::from(100);
    service.repo().insert(&d).await.unwrap();
    set_balance("0");
    assert!(h
        .finalize_resolution_close(&mut d, 0, None, Some(false))
        .await
        .unwrap());
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn only_live_results_reach_the_breaker_live_trading_on_or_off(pool: PgPool) {
    let service = setup(&pool).await;
    let resolved = |mode: &str, wallet: Option<String>| {
        let mut p = position();
        p.id = uuid::Uuid::new_v4();
        p.state = PositionState::Open;
        p.execution_mode = mode.into();
        p.wallet_address = wallet;
        p.held_yes_qty = Decimal::TEN;
        p
    };
    // Trading on paper (live trading switched off): a live position's loss
    // still counts, a paper position's win does not end its run.
    let (h, server) = handler(&pool).await;
    let mut p = resolved("live", Some("0xlive".into()));
    service.repo().insert(&p).await.unwrap();
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(false))
        .await
        .unwrap());
    let loss = p.realized_pnl.unwrap();
    assert!(loss < Decimal::ZERO);
    let mut won = resolved("paper", None);
    service.repo().insert(&won).await.unwrap();
    assert!(h
        .finalize_resolution_close(&mut won, 0, None, Some(true))
        .await
        .unwrap());
    assert!(won.realized_pnl.unwrap() > Decimal::ZERO);
    let breaker = h.circuit_breaker.state().await;
    assert_eq!(breaker.daily_pnl, loss);
    assert_eq!(breaker.consecutive_losses, 1);
    server.abort();

    // Trading live: a paper position left over does not count either.
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, _wallet, server) = live_handler(&pool, balance).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let mut p = resolved("paper", None);
    service.repo().insert(&p).await.unwrap();
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(false))
        .await
        .unwrap());
    assert!(p.realized_pnl.unwrap() < Decimal::ZERO);
    let breaker = h.circuit_breaker.state().await;
    assert_eq!(breaker.daily_pnl, Decimal::ZERO);
    assert_eq!(breaker.consecutive_losses, 0);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_market_stays_taken_while_a_position_in_it_is_active_or_holds_shares(pool: PgPool) {
    let service = setup(&pool).await;
    let repo = service.repo();
    assert!(!repo.has_active_in_market("market").await.unwrap());
    // An entry still settling.
    let mut p = position();
    p.state = PositionState::Pending;
    repo.insert(&p).await.unwrap();
    assert!(repo.has_active_in_market("market").await.unwrap());
    // An entry that failed after one leg filled, not flattened yet.
    p.state = PositionState::EntryFailed;
    p.held_yes_qty = Decimal::TEN;
    repo.update(&mut p).await.unwrap();
    assert!(repo.has_active_in_market("market").await.unwrap());
    // Failed with nothing held: the market is free.
    p.held_yes_qty = Decimal::ZERO;
    repo.update(&mut p).await.unwrap();
    assert!(!repo.has_active_in_market("market").await.unwrap());
    assert!(!repo.has_active_in_market("other").await.unwrap());
}

#[sqlx::test(migrations = false)]
async fn a_position_of_unknown_provenance_settles_at_its_record_once_resolved(pool: PgPool) {
    let service = setup(&pool).await;
    let (h, server) = handler(&pool).await;
    let mut p = position();
    p.state = PositionState::Open;
    p.execution_mode = "unknown".into();
    p.held_yes_qty = Decimal::TEN;
    service.repo().insert(&p).await.unwrap();
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(p.state, PositionState::Closed);
    let note: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM trade_events WHERE position_id = $1 AND event_type = 'closed_via_resolution'",
    )
    .bind(p.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        note,
        serde_json::json!({"unmeasured": [
            {"leg": "yes", "reason": "the position's execution provenance is unknown"}
        ]})
    );
    // Neither a paper nor a live result: the breaker is not told.
    let breaker = h.circuit_breaker.state().await;
    assert_eq!(breaker.daily_pnl, Decimal::ZERO);
    assert_eq!(breaker.consecutive_losses, 0);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_pair_without_a_winner_waits_for_it_and_an_unreadable_balance_waits_its_grace(
    pool: PgPool,
) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, wallet, server) = live_handler(&pool, balance.clone()).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let live_pair = || {
        let mut p = position();
        p.id = uuid::Uuid::new_v4();
        p.state = PositionState::Open;
        p.execution_mode = "live".into();
        p.wallet_address = Some(wallet.clone());
        p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
        p.no_entry_price = Decimal::new(5, 1);
        p.held_yes_qty = Decimal::from(100);
        p.held_no_qty = Decimal::from(100);
        p
    };
    // V1 buys kept a different number of shares of each leg: 95.8 YES and
    // 97.1 NO remain. Measured, the legs differ, so without a winner the
    // position waits rather than guess what the extra NO is worth.
    let mut p = live_pair();
    service.repo().insert(&p).await.unwrap();
    *balance.lock().unwrap() = "yes:95800000,no:97100000".into();
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, None)
        .await
        .unwrap());
    let stored = service.load(p.id).await.unwrap();
    assert_eq!(stored.state, PositionState::Open);
    assert_eq!(
        (stored.held_yes_qty, stored.held_no_qty),
        (Decimal::new(958, 1), Decimal::new(971, 1))
    );
    // Once NO is named the winner it pays what the wallet holds: 97.1 − 90.
    let mut p = stored;
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(false))
        .await
        .unwrap());
    assert_eq!(p.realized_pnl, Some(Decimal::new(71, 1)));

    // Still no winner a day after the first look found none: it closes at
    // the least it can pay, a dollar a pair (95.8 − 90), and names the NO
    // shares without a pair, valued at nothing.
    let mut q = live_pair();
    service.repo().insert(&q).await.unwrap();
    assert!(!h
        .finalize_resolution_close(&mut q, 0, None, None)
        .await
        .unwrap());
    let mut q = service.load(q.id).await.unwrap();
    assert!(!h
        .finalize_resolution_close(&mut q, 0, None, None)
        .await
        .unwrap());
    let waits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trade_events WHERE position_id = $1 AND event_type = 'resolution_waiting_for_winner'",
    )
    .bind(q.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(waits, 1);
    sqlx::query("UPDATE trade_events SET occurred_at = occurred_at - INTERVAL '25 hours' WHERE position_id = $1")
        .bind(q.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(h
        .finalize_resolution_close(&mut q, 0, None, None)
        .await
        .unwrap());
    assert_eq!(q.state, PositionState::Closed);
    assert_eq!(q.realized_pnl, Some(Decimal::new(58, 1)));
    let note: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM trade_events WHERE position_id = $1 AND event_type = 'closed_via_resolution'",
    )
    .bind(q.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        note,
        serde_json::json!({"unpaired": [{"leg": "no", "quantity": "1.3"}]})
    );

    // A balance that cannot be read is waited for, then, past the grace,
    // the record stands: the position closes, marked as not measured.
    let mut p = live_pair();
    service.repo().insert(&p).await.unwrap();
    *balance.lock().unwrap() = "unreadable".into();
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(service.load(p.id).await.unwrap().state, PositionState::Open);
    // A look that does not end at an unreadable balance (here, an order of
    // the wallet still settling) starts the grace afresh: the hour is one
    // unbroken stretch of unreadable balances.
    let since = std::time::Instant::now() - super::UNREADABLE_BALANCE_GRACE;
    h.balance_unreadable_since
        .lock()
        .unwrap()
        .insert((p.id, true), since);
    sqlx::query("INSERT INTO execution_intents (id, market_id, signed_order, status) VALUES (gen_random_uuid(), 'market', jsonb_build_object('tokenId', 'yes', 'side', 'SELL', 'maker', $1::text), 'unknown')")
        .bind(&wallet)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert!(!h
        .balance_unreadable_since
        .lock()
        .unwrap()
        .contains_key(&(p.id, true)));
    sqlx::query("UPDATE execution_intents SET status = 'reconciled'")
        .execute(&pool)
        .await
        .unwrap();
    // Unreadable again, it waits from the start.
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    // Past the grace, the record stands: the position closes, and its close
    // event says the holding was not measured (no second closing event).
    h.balance_unreadable_since
        .lock()
        .unwrap()
        .insert((p.id, true), since);
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(p.realized_pnl, Some(Decimal::from(10)));
    let closes: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, metadata FROM trade_events WHERE position_id = $1 AND state_to = 'closed'",
    )
    .bind(p.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(closes.len(), 1);
    assert_eq!(closes[0].0, "closed_via_resolution");
    assert_eq!(
        closes[0].1["unmeasured"][0]["reason"],
        super::BALANCE_UNREADABLE
    );

    // A pair with both balances unreadable: each leg's grace starts at the
    // same look, so it closes once one hour has passed, not two.
    let mut pair = live_pair();
    service.repo().insert(&pair).await.unwrap();
    *balance.lock().unwrap() = "unreadable".into();
    assert!(!h
        .finalize_resolution_close(&mut pair, 0, None, None)
        .await
        .unwrap());
    {
        let mut since = h.balance_unreadable_since.lock().unwrap();
        assert!(since.contains_key(&(pair.id, true)) && since.contains_key(&(pair.id, false)));
        for leg in [true, false] {
            since.insert(
                (pair.id, leg),
                std::time::Instant::now() - super::UNREADABLE_BALANCE_GRACE,
            );
        }
    }
    assert!(h
        .finalize_resolution_close(&mut pair, 0, None, None)
        .await
        .unwrap());

    // One-legged without a winner: nothing to pay is known; it waits.
    let mut p = live_pair();
    p.held_no_qty = Decimal::ZERO;
    service.repo().insert(&p).await.unwrap();
    *balance.lock().unwrap() = "100000000".into();
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, None)
        .await
        .unwrap());
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_live_executor_whose_signer_did_not_load_waits_its_grace(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new("100000000".to_string()));
    let (h, wallet, server) = live_handler_with(&pool, balance, false).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let mut p = position();
    p.state = PositionState::Open;
    p.execution_mode = "live".into();
    p.wallet_address = Some(wallet);
    p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    p.held_yes_qty = Decimal::from(100);
    service.repo().insert(&p).await.unwrap();
    // A restart may still load the signer and measure it: it waits.
    assert!(!h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(service.load(p.id).await.unwrap().state, PositionState::Open);
    assert!(h
        .balance_unreadable_since
        .lock()
        .unwrap()
        .contains_key(&(p.id, true)));
    // Past the grace, the record stands.
    h.balance_unreadable_since.lock().unwrap().insert(
        (p.id, true),
        std::time::Instant::now() - super::UNREADABLE_BALANCE_GRACE,
    );
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(p.realized_pnl, Some(Decimal::from(60)));
    // The close names the cause: no signer, not an unreadable balance.
    let note: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM trade_events WHERE position_id = $1 AND event_type = 'closed_via_resolution'",
    )
    .bind(p.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(note["unmeasured"][0]["reason"], super::SIGNER_NOT_LOADED);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn with_no_trading_wallet_loaded_a_resolution_closes_at_its_record_noted(pool: PgPool) {
    let service = setup(&pool).await;
    // The paper handler's executor never loads a wallet.
    let (h, server) = handler(&pool).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let mut p = position();
    p.state = PositionState::Open;
    p.execution_mode = "live".into();
    p.wallet_address = Some("0x0000000000000000000000000000000000000001".into());
    p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    p.held_yes_qty = Decimal::from(100);
    service.repo().insert(&p).await.unwrap();
    assert!(h
        .finalize_resolution_close(&mut p, 0, None, Some(true))
        .await
        .unwrap());
    assert_eq!(p.realized_pnl, Some(Decimal::from(60)));
    let note: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM trade_events WHERE position_id = $1 AND event_type = 'closed_via_resolution'",
    )
    .bind(p.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        note["unmeasured"][0]["reason"],
        "no trading wallet is loaded"
    );
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn shares_written_off_together_are_all_written_or_none(pool: PgPool) {
    let service = setup(&pool).await;
    let ctx = context();
    let mut a = position();
    a.held_yes_qty = Decimal::from(100);
    let mut b = position();
    b.id = uuid::Uuid::new_v4();
    b.held_yes_qty = Decimal::from(100);
    service.repo().insert(&a).await.unwrap();
    service.repo().insert(&b).await.unwrap();
    // B changed since it was read: its copy is stale.
    let stale_b = service.load(b.id).await.unwrap();
    let mut fresh_b = service.load(b.id).await.unwrap();
    service.repo().update(&mut fresh_b).await.unwrap();
    let a_read = service.load(a.id).await.unwrap();
    assert!(service
        .write_off_missing_shares_together(
            Leg::Yes,
            vec![
                (a_read, Decimal::new(35, 1), ctx.clone()),
                (stale_b, Decimal::new(35, 1), ctx.clone()),
            ],
        )
        .await
        .is_err());
    for id in [a.id, b.id] {
        assert_eq!(
            service.load(id).await.unwrap().held_yes_qty,
            Decimal::from(100)
        );
    }
    let told: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trade_events WHERE event_type = 'entry_fee_shares_written_off'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(told, 0);
    // Read afresh, both are written off, and the versions are the ones stored.
    let written = service
        .write_off_missing_shares_together(
            Leg::Yes,
            vec![
                (
                    service.load(a.id).await.unwrap(),
                    Decimal::new(35, 1),
                    ctx.clone(),
                ),
                (
                    service.load(b.id).await.unwrap(),
                    Decimal::new(35, 1),
                    ctx.clone(),
                ),
            ],
        )
        .await
        .unwrap();
    for mut position in written {
        assert_eq!(position.held_yes_qty, Decimal::new(965, 1));
        service.repo().update(&mut position).await.unwrap();
    }
}

#[sqlx::test(migrations = false)]
async fn holders_of_one_token_each_answer_for_their_share_of_the_fees(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, wallet, server) = live_handler(&pool, balance.clone()).await;
    let set_balance = |shares: &str| {
        *balance.lock().unwrap() =
            (shares.parse::<Decimal>().unwrap() * Decimal::from(1_000_000)).to_string()
    };
    let ctx = context();
    let live = || {
        let mut p = position();
        p.state = PositionState::Closing;
        p.execution_mode = "live".into();
        p.wallet_address = Some(wallet.clone());
        p.held_yes_qty = Decimal::from(100);
        p
    };
    // Two positions each recorded 100 and each lost 6.3 to fees: alone,
    // either gap (12.6 of 100) would look too large for a fee.
    let mut a = live();
    let mut b = live();
    b.id = uuid::Uuid::new_v4();
    service.repo().insert(&a).await.unwrap();
    service.repo().insert(&b).await.unwrap();
    set_balance("187.4");
    // While B has an order not yet applied (a sale it made but has not
    // recorded would make its holding look larger), nothing is written off.
    sqlx::query(
        "INSERT INTO position_order_links (id, position_id, leg, side, requested_quantity) \
         VALUES (gen_random_uuid(), $1, 'yes', 'sell', 5)",
    )
    .bind(b.id)
    .execute(&pool)
    .await
    .unwrap();
    h.align_leg_with_wallet(&mut a, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(a.held_yes_qty, Decimal::from(100));
    pool.execute("UPDATE position_order_links SET settled = TRUE")
        .await
        .unwrap();
    h.align_leg_with_wallet(&mut a, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(a.held_yes_qty, Decimal::new(937, 1));
    // B's 6.3 is written off with A's, at once, and told as B's own, in
    // the state it is in.
    let mut b = service.repo().get(b.id).await.unwrap().unwrap();
    assert_eq!(b.held_yes_qty, Decimal::new(937, 1));
    let told: (String, Option<String>) = sqlx::query_as(
        "SELECT metadata->>'quantity', state_to FROM trade_events \
         WHERE position_id = $1 AND event_type = 'entry_fee_shares_written_off'",
    )
    .bind(b.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(told, ("6.3".to_string(), Some("closing".to_string())));
    // A sells its 93.7; measuring B again finds nothing more missing.
    service
        .record_exit_fill(
            &mut a,
            Leg::Yes,
            Decimal::new(50, 2),
            Decimal::new(937, 1),
            &ctx,
            None,
        )
        .await
        .unwrap();
    set_balance("93.7");
    h.align_leg_with_wallet(&mut b, Leg::Yes, "yes", &ctx)
        .await
        .unwrap();
    assert_eq!(b.held_yes_qty, Decimal::new(937, 1));
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn an_arbitrage_position_pays_the_taker_fee_of_each_fill(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = position();
    p.execution_mode = "paper".into();
    p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    p.taker_fees = Some(polymarket_core::types::FeeSchedule::CONSERVATIVE);
    service.repo().insert(&p).await.unwrap();
    let ctx = context();

    // 10 YES bought at 0.40: 10 × 0.07 × 0.40 × 0.60 = 0.168 USDC of fee.
    service
        .record_entry_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(40, 2),
            Decimal::TEN,
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(p.fees_paid, Decimal::new(168, 3));
    let stored = service.repo().get(p.id).await.unwrap().unwrap();
    assert_eq!(stored.fees_paid, Decimal::new(168, 3));
    assert_eq!(stored.taker_fees, p.taker_fees);

    // Marked at a 0.50 bid: 5 − 4, less the fee paid so far.
    p.update_pnl(Decimal::new(50, 2), Decimal::ZERO, Decimal::new(2, 2));
    assert_eq!(p.unrealized_pnl, Decimal::new(832, 3));

    // Sold at 0.50: 10 × 0.07 × 0.25 = 0.175 more.
    p.state = PositionState::Closing;
    service
        .record_exit_fill(
            &mut p,
            Leg::Yes,
            Decimal::new(50, 2),
            Decimal::TEN,
            &ctx,
            None,
        )
        .await
        .unwrap();
    service
        .close_position(
            &mut p,
            CloseMethod::MarketExit {
                fee: Decimal::new(2, 2),
            },
            &ctx,
        )
        .await
        .unwrap();
    // 5 − 4 − 0.343: the fees charged, not the flat 2% of quant positions.
    assert_eq!(p.realized_pnl, Some(Decimal::new(657, 3)));

    // Without fee terms (positions opened before they were recorded) nothing
    // is charged.
    let mut legacy = position();
    legacy.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    service.repo().insert(&legacy).await.unwrap();
    service
        .record_entry_fill(
            &mut legacy,
            Leg::Yes,
            Decimal::new(40, 2),
            Decimal::TEN,
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(legacy.fees_paid, Decimal::ZERO);
}

#[sqlx::test(migrations = false)]
async fn no_leg_is_sold_until_both_legs_exchanges_are_known(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new("10000000".to_string()));
    // The mock exchange answers no neg-risk lookups.
    let (h, wallet, server) = live_handler(&pool, balance).await;
    h.token_cache
        .alias("market", &("yes".into(), "no".into()))
        .await;
    let mut p = position();
    p.execution_mode = "live".into();
    p.wallet_address = Some(wallet);
    p.held_yes_qty = Decimal::TEN;
    p.state = PositionState::ExitReady;
    service.repo().insert(&p).await.unwrap();
    h.execute_exit(&mut p).await.unwrap();
    let stored = service.repo().get(p.id).await.unwrap().unwrap();
    assert_eq!(stored.state, PositionState::ExitFailed);
    assert_eq!(stored.held_yes_qty, Decimal::TEN);
    assert!(stored
        .failure_reason
        .map(|reason| reason.to_string())
        .unwrap_or_default()
        .contains("exchange"));
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_position_opened_without_fee_terms_pays_its_markets_fees_on_exit(pool: PgPool) {
    let service = setup(&pool).await;
    let balance = Arc::new(std::sync::Mutex::new(String::new()));
    let (h, _, server) = live_handler(&pool, balance).await;
    let ctx = context();

    // Opened before fee terms were recorded (migration 092).
    let mut legacy = position();
    legacy.execution_mode = "paper".into();
    legacy.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    legacy.held_yes_qty = Decimal::TEN;
    legacy.state = PositionState::Closing;
    service.repo().insert(&legacy).await.unwrap();
    h.attach_fee_terms(&mut legacy).await;
    assert_eq!(
        legacy.taker_fees,
        Some(polymarket_core::types::FeeSchedule {
            rate: Decimal::new(7, 2),
            exponent: Decimal::ONE
        })
    );
    // Recorded at once, for a sale another path settles.
    assert_eq!(
        service
            .repo()
            .get(legacy.id)
            .await
            .unwrap()
            .unwrap()
            .taker_fees,
        legacy.taker_fees
    );
    // Sold at 0.50: 10 × 0.07 × 0.25 = 0.175, and the terms are kept.
    service
        .record_exit_fill(
            &mut legacy,
            Leg::Yes,
            Decimal::new(50, 2),
            Decimal::TEN,
            &ctx,
            None,
        )
        .await
        .unwrap();
    assert_eq!(legacy.fees_paid, Decimal::new(175, 3));
    let stored = service.repo().get(legacy.id).await.unwrap().unwrap();
    assert_eq!(stored.taker_fees, legacy.taker_fees);
    assert_eq!(stored.fees_paid, Decimal::new(175, 3));

    // Recorded terms stand, and flat-fee positions are left to their rate.
    let mut priced = position();
    priced.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    priced.taker_fees = Some(polymarket_core::types::FeeSchedule::FREE);
    h.attach_fee_terms(&mut priced).await;
    assert_eq!(
        priced.taker_fees,
        Some(polymarket_core::types::FeeSchedule::FREE)
    );
    let mut flat = position();
    h.attach_fee_terms(&mut flat).await;
    assert_eq!(flat.taker_fees, None);
    server.abort();
}

#[sqlx::test(migrations = false)]
async fn a_resolved_arbitrage_position_deducts_only_its_entry_fees(pool: PgPool) {
    let service = setup(&pool).await;
    let mut p = Position::new(
        "market".into(),
        Decimal::new(48, 2),
        Decimal::new(46, 2),
        Decimal::TEN,
        ExitStrategy::HoldToResolution,
    );
    p.fee_model = polymarket_core::types::PositionFeeModel::ShareBased;
    p.taker_fees = Some(polymarket_core::types::FeeSchedule::CONSERVATIVE);
    service.repo().insert(&p).await.unwrap();
    let ctx = context();
    for (leg, price) in [
        (Leg::Yes, Decimal::new(48, 2)),
        (Leg::No, Decimal::new(46, 2)),
    ] {
        service
            .record_entry_fill(&mut p, leg, price, Decimal::TEN, &ctx, None)
            .await
            .unwrap();
    }
    // 10 × 0.07 × (0.48 × 0.52 + 0.46 × 0.54) = 0.34860
    assert_eq!(p.fees_paid, Decimal::new(34860, 5));
    p.state = PositionState::Open;
    service
        .close_position(
            &mut p,
            CloseMethod::ResolutionWithWinner {
                yes_wins: true,
                fee: Decimal::new(2, 2),
            },
            &ctx,
        )
        .await
        .unwrap();
    // The pair pays 10; it cost 9.40; redemption is not a trade, so no fee.
    assert_eq!(p.realized_pnl, Some(Decimal::new(25140, 5)));
}
