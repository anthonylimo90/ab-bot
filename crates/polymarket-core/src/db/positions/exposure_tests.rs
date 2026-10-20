use super::*;
use sqlx::Executor;

async fn setup(pool: &PgPool) {
    pool.execute("CREATE TABLE positions (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(), market_id TEXT DEFAULT 'market', yes_entry_price NUMERIC DEFAULT 0,
        no_entry_price NUMERIC DEFAULT 0, quantity NUMERIC DEFAULT 0, entry_timestamp TIMESTAMPTZ DEFAULT NOW(), exit_strategy SMALLINT,
        state SMALLINT DEFAULT 1, unrealized_pnl NUMERIC, failure_reason TEXT, retry_count INT, last_updated TIMESTAMPTZ,
        fee_model SMALLINT, resolution_payout_per_share NUMERIC, yes_entry_fee_shares NUMERIC, no_entry_fee_shares NUMERIC,
        held_yes_qty NUMERIC DEFAULT 0, held_no_qty NUMERIC DEFAULT 0, exited_yes_qty NUMERIC, exited_no_qty NUMERIC,
        resolution_winner TEXT, is_open BOOLEAN, opened_at TIMESTAMPTZ, source SMALLINT, source_signal_id UUID
    ); CREATE TABLE orders (quantity NUMERIC,filled_quantity NUMERIC,price NUMERIC,side TEXT,status TEXT)").await.unwrap();
    pool.execute(include_str!(
        "../../../../../migrations/069_execution_intents.sql"
    ))
    .await
    .unwrap();
}

#[sqlx::test(migrations = false)]
async fn exposure_includes_failed_stalled_closed_holdings_and_pending_reservations(pool: PgPool) {
    setup(&pool).await;
    pool.execute("INSERT INTO positions (state,held_yes_qty,held_no_qty,yes_entry_price,no_entry_price,quantity) VALUES
        (6,10,0,0.4,0,100),(7,0,5,0,0.6,100),(4,2,0,0.5,0,100),(0,1,0,0.4,0.5,10),(5,3,0,0.5,0,100);
        INSERT INTO orders VALUES (10,4,0.5,'buy','partially_filled')").await.unwrap();
    let repo = PositionRepository::new(pool.clone());
    assert_eq!(
        repo.get_total_active_exposure().await.unwrap(),
        Decimal::new(215, 1)
    );
    assert_eq!(
        repo.count_active_positions_for_market("market")
            .await
            .unwrap(),
        5
    );
    pool.execute("DROP TABLE orders").await.unwrap();
    assert!(repo.get_total_active_exposure().await.is_err());
}

#[sqlx::test(migrations = false)]
async fn concurrent_entries_reserve_prospective_exposure_atomically(pool: PgPool) {
    setup(&pool).await;
    let repo = PositionRepository::new(pool.clone());
    let first = Position::new(
        "market".into(),
        Decimal::new(4, 1),
        Decimal::new(5, 1),
        Decimal::TEN,
        ExitStrategy::HoldToResolution,
    );
    let second = Position::new(
        "other".into(),
        Decimal::new(4, 1),
        Decimal::new(5, 1),
        Decimal::TEN,
        ExitStrategy::HoldToResolution,
    );
    let (a, b) = tokio::join!(
        repo.insert_with_source_and_limit(&first, SOURCE_ARBITRAGE, None, Some(Decimal::from(15))),
        repo.insert_with_source_and_limit(
            &second,
            SOURCE_RECOMMENDATION,
            None,
            Some(Decimal::from(15))
        )
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        repo.get_total_active_exposure().await.unwrap(),
        Decimal::from(9)
    );
}

#[sqlx::test(migrations = false)]
async fn candidate_filter_preserves_all_state_and_basis_combinations(pool: PgPool) {
    setup(&pool).await;
    pool.execute(include_str!(
        "../../../../../migrations/075_exposure_candidates.sql"
    ))
    .await
    .unwrap();
    pool.execute("INSERT INTO positions(state,quantity,yes_entry_price,no_entry_price,held_yes_qty,held_no_qty)
        SELECT s,100,p,1-p,y,n FROM generate_series(0,7) s
        CROSS JOIN (VALUES(-0.1::numeric),(0::numeric),(0.4::numeric),(1::numeric)) prices(p)
        CROSS JOIN (VALUES(0::numeric),(10::numeric)) yes_qty(y)
        CROSS JOIN (VALUES(0::numeric),(20::numeric)) no_qty(n);
        INSERT INTO positions(state,quantity) SELECT 4,100 FROM generate_series(1,10000);
        INSERT INTO orders VALUES(10,2,0.4,'buy','pending'),(20,0,NULL,'buy','open');
        INSERT INTO execution_intents(id,market_id,signed_order,status)
        VALUES(gen_random_uuid(),'market','{\"makerAmount\":\"20000000\",\"side\":\"BUY\"}','unknown')").await.unwrap();
    let all_history =
        TOTAL_ACTIVE_EXPOSURE_SQL.replace(" WHERE state=0 OR held_yes_qty>0 OR held_no_qty>0", "");
    let before: Decimal = sqlx::query_scalar(&all_history)
        .fetch_one(&pool)
        .await
        .unwrap();
    let after = PositionRepository::new(pool)
        .get_total_active_exposure()
        .await
        .unwrap();
    assert_eq!(after, before);
}
