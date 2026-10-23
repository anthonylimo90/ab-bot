use super::*;
use sqlx::Executor;

async fn setup(pool: &PgPool) {
    pool.execute("CREATE TABLE position_order_links(id UUID)")
        .await
        .unwrap();
    pool.execute(include_str!(
        "../../../../../migrations/069_execution_intents.sql"
    ))
    .await
    .unwrap();
    pool.execute(
        "CREATE TABLE orders (
        id UUID PRIMARY KEY, exchange_order_id TEXT, execution_mode TEXT DEFAULT 'paper', client_order_id TEXT UNIQUE, market_id TEXT, outcome TEXT,
        side TEXT, order_type TEXT, status TEXT, quantity NUMERIC,
        filled_quantity NUMERIC DEFAULT 0, price NUMERIC, avg_fill_price NUMERIC,
        stop_price NUMERIC, time_in_force TEXT, created_at TIMESTAMPTZ DEFAULT NOW(),
        updated_at TIMESTAMPTZ DEFAULT NOW(), filled_at TIMESTAMPTZ
    )",
    )
    .await
    .unwrap();
    pool.execute("CREATE TABLE positions(state SMALLINT,quantity NUMERIC,yes_entry_price NUMERIC,no_entry_price NUMERIC,held_yes_qty NUMERIC,held_no_qty NUMERIC)").await.unwrap();
    pool.execute(include_str!(
        "../../../../../migrations/077_manual_inventory.sql"
    ))
    .await
    .unwrap();
    pool.execute(include_str!(
        "../../../../../migrations/083_manual_fill_cost_bound.sql"
    ))
    .await
    .unwrap();
}

fn order() -> PlaceOrderRequest {
    PlaceOrderRequest {
        market_id: "market".into(),
        outcome: "yes".into(),
        side: OrderSide::Buy,
        order_type: OrderType::Limit,
        quantity: Decimal::TEN,
        price: Some(Decimal::new(5, 1)),
        stop_price: None,
        time_in_force: "GTC".into(),
        client_order_id: Some("one-intent".into()),
    }
}

#[sqlx::test(migrations = false)]
async fn concurrent_and_restarted_replays_reserve_one_order_before_execution(pool: PgPool) {
    setup(&pool).await;
    let request = order();
    let (a, b) = tokio::join!(
        reserve_order(&pool, &request, false, None, Decimal::from(1000)),
        reserve_order(&pool, &request, false, None, Decimal::from(1000))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.0, b.0);
    assert_ne!(a.1, b.1);
    assert_eq!(
        reserve_order(&pool, &request, false, None, Decimal::from(1000))
            .await
            .unwrap(),
        (a.0, false)
    );
    sqlx::query("UPDATE orders SET status='filled',filled_quantity=quantity WHERE id=$1")
        .bind(a.0)
        .execute(&pool)
        .await
        .unwrap();
    let replay = load_order(
        &pool,
        reserve_order(&pool, &request, false, None, Decimal::from(1000))
            .await
            .unwrap()
            .0,
    )
    .await
    .unwrap();
    assert_eq!(replay.status, "filled");
    assert_eq!(replay.filled_quantity, Decimal::TEN);
    let changed = PlaceOrderRequest {
        quantity: Decimal::ONE,
        ..request
    };
    assert!(matches!(
        reserve_order(&pool, &changed, false, None, Decimal::from(1000)).await,
        Err(ApiError::Conflict(_))
    ));
}

#[test]
fn client_intent_key_is_required() {
    let mut request = order();
    request.client_order_id = None;
    assert!(validate_order_request(&request).is_err());
    request.client_order_id = Some(" ".into());
    assert!(validate_order_request(&request).is_err());
}

#[sqlx::test(migrations = false)]
async fn exchange_snapshots_record_only_confirmed_quantities_and_keep_progress(pool: PgPool) {
    setup(&pool).await;
    let (id, _) = reserve_order(&pool, &order(), true, Some("0xreview"), Decimal::from(1000))
        .await
        .unwrap();
    sqlx::query("UPDATE orders SET exchange_order_id='exchange-id', status='open' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let mut snapshot = polymarket_core::api::clob::ExchangeOrder {
        associate_trades: vec![],
        id: "exchange-id".into(),
        status: "LIVE".into(),
        original_size: "10".into(),
        size_matched: "0".into(),
    };
    let row = load_order(&pool, id).await.unwrap();
    apply_exchange_snapshot(&pool, &row, &snapshot)
        .await
        .unwrap();
    assert_eq!(
        load_order(&pool, id).await.unwrap().filled_quantity,
        Decimal::ZERO
    );
    snapshot.size_matched = "4".into();
    apply_exchange_snapshot(&pool, &row, &snapshot)
        .await
        .unwrap();
    let partial = load_order(&pool, id).await.unwrap();
    assert_eq!(partial.status, "partially_filled");
    assert_eq!(partial.filled_quantity, Decimal::from(4));
    assert!(partial.avg_fill_price.is_none());
    // Fills of unknown price are costed at the 0.50 limit, not at $1.
    let (quantity, cost_basis): (Decimal, Decimal) = sqlx::query_as(
        "SELECT quantity, cost_basis FROM manual_inventory WHERE wallet_address = '0xreview'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((quantity, cost_basis), (Decimal::from(4), Decimal::from(2)));
    snapshot.size_matched = "3".into();
    assert!(apply_exchange_snapshot(&pool, &partial, &snapshot)
        .await
        .is_err());
    snapshot.size_matched = "10".into();
    snapshot.status = "CANCELED".into();
    apply_exchange_snapshot(&pool, &partial, &snapshot)
        .await
        .unwrap();
    assert_eq!(load_order(&pool, id).await.unwrap().status, "filled");
}

#[sqlx::test(migrations = false)]
async fn a_rounded_market_buy_and_a_buy_given_more_shares_count_as_filled(pool: PgPool) {
    setup(&pool).await;
    let reserve = |key: &'static str| {
        let pool = pool.clone();
        async move {
            let mut request = order();
            request.client_order_id = Some(key.into());
            let (id, _) =
                reserve_order(&pool, &request, true, Some("0xreview"), Decimal::from(1000))
                    .await
                    .unwrap();
            sqlx::query(
                "UPDATE orders SET exchange_order_id='exchange-id', status='open' WHERE id=$1",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            id
        }
    };
    let snapshot = |size: &str, matched: &str| polymarket_core::api::clob::ExchangeOrder {
        associate_trades: vec![],
        id: "exchange-id".into(),
        status: "MATCHED".into(),
        original_size: size.into(),
        size_matched: matched.into(),
    };
    // A market buy of 10, signed (as the exchange rounds it) for 9.99 shares
    // and matched in full: filled, not cancelled.
    let market = reserve("market-buy").await;
    let mut row = load_order(&pool, market).await.unwrap();
    row.order_type = "market".into();
    apply_exchange_snapshot(&pool, &row, &snapshot("9.99", "9.99"))
        .await
        .unwrap();
    assert_eq!(load_order(&pool, market).await.unwrap().status, "filled");
    // A buy given a better price as more shares: filled, all of them kept.
    let limit = reserve("limit-buy").await;
    let row = load_order(&pool, limit).await.unwrap();
    apply_exchange_snapshot(&pool, &row, &snapshot("10", "10.2"))
        .await
        .unwrap();
    let stored = load_order(&pool, limit).await.unwrap();
    assert_eq!(stored.status, "filled");
    assert_eq!(stored.filled_quantity, "10.2".parse::<Decimal>().unwrap());
    // A sell never parts with more than its quantity.
    let mut sell = load_order(&pool, reserve("sell").await).await.unwrap();
    sell.side = "sell".into();
    assert!(
        apply_exchange_snapshot(&pool, &sell, &snapshot("10", "10.2"))
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = false)]
async fn an_order_closed_by_the_exchange_keeps_its_fills_and_its_close(pool: PgPool) {
    setup(&pool).await;
    for closed in [
        "CANCELED_MARKET_RESOLVED",
        "ORDER_STATUS_CANCELED_MARKET_RESOLVED",
        "INVALID",
    ] {
        // A row of its own each time: the same client id would replay one.
        let mut request = order();
        request.client_order_id = Some(format!("closed-{closed}"));
        let (id, created) =
            reserve_order(&pool, &request, true, Some("0xreview"), Decimal::from(1000))
                .await
                .unwrap();
        assert!(created);
        sqlx::query("UPDATE orders SET exchange_order_id='exchange-id', status='open' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let mut snapshot = polymarket_core::api::clob::ExchangeOrder {
            associate_trades: vec![],
            id: "exchange-id".into(),
            status: closed.into(),
            original_size: "10".into(),
            size_matched: "4".into(),
        };
        let row = load_order(&pool, id).await.unwrap();
        apply_exchange_snapshot(&pool, &row, &snapshot)
            .await
            .unwrap();
        let stored = load_order(&pool, id).await.unwrap();
        assert_eq!(stored.status, "cancelled", "{closed}");
        assert_eq!(stored.filled_quantity, Decimal::from(4));
        // A later, stale answer does not reopen it.
        snapshot.status = "LIVE".into();
        apply_exchange_snapshot(&pool, &stored, &snapshot)
            .await
            .unwrap();
        assert_eq!(load_order(&pool, id).await.unwrap().status, "cancelled");
    }
}

#[sqlx::test(migrations = false)]
async fn preflight_failure_rejects_only_intents_that_never_crossed_submission_boundary(
    pool: PgPool,
) {
    setup(&pool).await;
    let request = order();
    let (id, _) = reserve_order(&pool, &request, true, Some("0xreview"), Decimal::from(1000))
        .await
        .unwrap();
    reject_unsubmitted_order(&pool, id).await.unwrap();
    assert_eq!(load_order(&pool, id).await.unwrap().status, "rejected");
    assert_eq!(
        reserve_order(&pool, &request, true, Some("0xreview"), Decimal::from(1000))
            .await
            .unwrap(),
        (id, false)
    );
    let request = PlaceOrderRequest {
        client_order_id: Some("submitted-intent".into()),
        ..request
    };
    let (id, _) = reserve_order(&pool, &request, true, Some("0xreview"), Decimal::from(1000))
        .await
        .unwrap();
    sqlx::query("INSERT INTO execution_intents (id,market_id,signed_order,status) VALUES ($1,'market','{}','unknown')").bind(id).execute(&pool).await.unwrap();
    reject_unsubmitted_order(&pool, id).await.unwrap();
    assert_eq!(load_order(&pool, id).await.unwrap().status, "pending");
}

#[sqlx::test(migrations = false)]
async fn manual_reservations_and_fills_share_one_atomic_exposure_budget(pool: PgPool) {
    setup(&pool).await;
    let first = order();
    let second = PlaceOrderRequest {
        client_order_id: Some("second".into()),
        ..order()
    };
    let (a, b) = tokio::join!(
        reserve_order(&pool, &first, true, Some("0xreview"), Decimal::from(7)),
        reserve_order(&pool, &second, true, Some("0xreview"), Decimal::from(7))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let id = a.or(b).unwrap().0;
    sqlx::query(
        "UPDATE orders SET status='filled',filled_quantity=10,avg_fill_price=0.5 WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let exposure: Decimal =
        sqlx::query_scalar(polymarket_core::db::positions::TOTAL_ACTIVE_EXPOSURE_SQL)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(exposure, Decimal::from(5));
    let sell = PlaceOrderRequest {
        side: OrderSide::Sell,
        quantity: Decimal::from(4),
        client_order_id: Some("sell".into()),
        ..order()
    };
    let (sell_id, _) = reserve_order(&pool, &sell, true, Some("0xreview"), Decimal::from(7))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE orders SET status='filled',filled_quantity=4,avg_fill_price=0.6 WHERE id=$1",
    )
    .bind(sell_id)
    .execute(&pool)
    .await
    .unwrap();
    let held: (Decimal, Decimal) =
        sqlx::query_as("SELECT quantity,cost_basis FROM manual_inventory")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(held, (Decimal::from(6), Decimal::from(3)));
    assert!(reserve_order(
        &pool,
        &PlaceOrderRequest {
            client_order_id: Some("wrong-wallet".into()),
            ..sell
        },
        true,
        Some("0xother"),
        Decimal::from(100)
    )
    .await
    .is_err());
}

#[sqlx::test(migrations = false)]
async fn unsupported_protective_orders_never_reserve_inventory(pool: PgPool) {
    setup(&pool).await;
    for kind in [OrderType::StopLoss, OrderType::TakeProfit] {
        assert!(validate_order_request(&PlaceOrderRequest {
            order_type: kind,
            stop_price: Some(Decimal::new(4, 1)),
            ..order()
        })
        .is_err());
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
