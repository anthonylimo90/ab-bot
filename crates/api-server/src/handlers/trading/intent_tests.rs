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
    pool.execute(include_str!(
        "../../../../../migrations/105_manual_realized_results.sql"
    ))
    .await
    .unwrap();
    pool.execute(include_str!(
        "../../../../../migrations/108_manual_resolution_results.sql"
    ))
    .await
    .unwrap();
}

/// A manual order of `mode`, filled by later updates.
async fn manual_order(pool: &PgPool, mode: &str, side: &str, price: Option<Decimal>) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO orders (id, execution_mode, wallet_address, market_id, outcome, side, \
         order_type, status, quantity, price) \
         VALUES ($1, $2, '0xw', 'm', 'yes', $3, 'limit', 'open', 10, $4)",
    )
    .bind(id)
    .bind(mode)
    .bind(side)
    .bind(price)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn fill(pool: &PgPool, id: uuid::Uuid, filled: i64, avg: Option<Decimal>) {
    sqlx::query("UPDATE orders SET filled_quantity = $2, avg_fill_price = $3 WHERE id = $1")
        .bind(id)
        .bind(Decimal::from(filled))
        .bind(avg)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn manual_holdings_record_their_result_at_resolution_once(pool: PgPool) {
    setup(&pool).await;
    // 10 YES at 0.50 ($5) and 4 NO at 0.50 ($2), both live.
    let yes = manual_order(&pool, "live", "buy", Some(Decimal::new(5, 1))).await;
    fill(&pool, yes, 10, Some(Decimal::new(5, 1))).await;
    let no = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO orders (id, execution_mode, wallet_address, market_id, outcome, side, \
         order_type, status, quantity, price) \
         VALUES ($1, 'live', '0xw', 'm', 'no', 'buy', 'limit', 'open', 4, 0.5)",
    )
    .bind(no)
    .execute(&pool)
    .await
    .unwrap();
    fill(&pool, no, 4, Some(Decimal::new(5, 1))).await;
    // An order still open in the market: its fill could still change the
    // holdings, so nothing is recorded yet.
    let resting = manual_order(&pool, "live", "sell", Some(Decimal::new(9, 1))).await;
    assert_eq!(
        crate::exit_handler::record_manual_resolution(&pool, "m", "yes")
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE orders SET status = 'filled' WHERE status = 'open'")
        .execute(&pool)
        .await
        .unwrap();
    let _ = resting;
    assert_eq!(
        crate::exit_handler::record_manual_resolution(&pool, "m", "yes")
            .await
            .unwrap(),
        2
    );
    let results: Vec<(String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT outcome, proceeds, realized_pnl FROM manual_realized_results \
         WHERE kind = 'resolution' ORDER BY outcome",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(results.len(), 2);
    // NO lost its $2; YES paid $10 for $5.
    assert_eq!(results[0].0, "no");
    assert_eq!(results[0].2.normalize(), Decimal::from(-2));
    assert_eq!(results[1].0, "yes");
    assert_eq!(results[1].1.normalize(), Decimal::from(10));
    assert_eq!(results[1].2.normalize(), Decimal::from(5));
    // Once only.
    assert_eq!(
        crate::exit_handler::record_manual_resolution(&pool, "m", "yes")
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = false)]
async fn live_manual_sales_record_their_realized_result(pool: PgPool) {
    setup(&pool).await;
    // 10 shares bought at 0.50: $5 of basis.
    let buy = manual_order(&pool, "live", "buy", Some(Decimal::new(5, 1))).await;
    fill(&pool, buy, 10, Some(Decimal::new(5, 1))).await;
    let results = || async {
        sqlx::query_as::<_, (Decimal, Decimal, Decimal, Decimal, bool)>(
            "SELECT quantity, proceeds, released_cost, realized_pnl, priced \
             FROM manual_realized_results ORDER BY occurred_at, quantity",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    assert!(results().await.is_empty());

    // 4 sold at 0.40: $1.60 for $2 of basis, a $0.40 loss.
    let priced = manual_order(&pool, "live", "sell", Some(Decimal::new(4, 1))).await;
    fill(&pool, priced, 4, Some(Decimal::new(4, 1))).await;
    // 3 more by an order whose fill price was not reported: at its 0.30
    // limit, the least it sold for.
    let unpriced = manual_order(&pool, "live", "sell", Some(Decimal::new(3, 1))).await;
    fill(&pool, unpriced, 3, None).await;
    let rows = results().await;
    assert_eq!(rows.len(), 2);
    let (q, proceeds, released, pnl, was_priced) = rows[0];
    assert_eq!(q, Decimal::from(4));
    assert_eq!(proceeds.normalize(), Decimal::new(16, 1));
    assert_eq!(released.normalize(), Decimal::from(2));
    assert_eq!(pnl.normalize(), Decimal::new(-4, 1));
    assert!(was_priced);
    let (q, proceeds, released, pnl, was_priced) = rows[1];
    assert_eq!(q, Decimal::from(3));
    assert_eq!(proceeds.normalize(), Decimal::new(9, 1));
    assert_eq!(released.normalize(), Decimal::new(15, 1));
    assert_eq!(pnl.normalize(), Decimal::new(-6, 1));
    assert!(!was_priced);

    // A market order with no reported price: counted as nothing received.
    let market = manual_order(&pool, "live", "sell", None).await;
    fill(&pool, market, 1, None).await;
    assert_eq!(results().await.len(), 3);
    let worst: Decimal =
        sqlx::query_scalar("SELECT realized_pnl FROM manual_realized_results WHERE order_id = $1")
            .bind(market)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(worst.normalize(), Decimal::new(-5, 1));

    // Paper sales reach no breaker.
    let paper_buy = manual_order(&pool, "paper", "buy", Some(Decimal::new(5, 1))).await;
    fill(&pool, paper_buy, 2, Some(Decimal::new(5, 1))).await;
    let paper_sell = manual_order(&pool, "paper", "sell", Some(Decimal::new(1, 1))).await;
    fill(&pool, paper_sell, 2, Some(Decimal::new(1, 1))).await;
    assert_eq!(results().await.len(), 3);
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
    let zero = Some((Decimal::ZERO, Decimal::ZERO));
    apply_exchange_snapshot(&pool, &row, &snapshot, zero)
        .await
        .unwrap();
    assert_eq!(
        load_order(&pool, id).await.unwrap().filled_quantity,
        Decimal::ZERO
    );
    // Matched, but its trade is still settling: nothing is booked yet.
    snapshot.size_matched = "4".into();
    apply_exchange_snapshot(&pool, &row, &snapshot, None)
        .await
        .unwrap();
    let settling = load_order(&pool, id).await.unwrap();
    assert_eq!(
        (settling.status.as_str(), settling.filled_quantity),
        ("open", Decimal::ZERO)
    );
    // Settled at 0.45: booked at that price, not at the 0.50 limit.
    let price = Decimal::new(45, 2);
    apply_exchange_snapshot(&pool, &row, &snapshot, Some((Decimal::from(4), price)))
        .await
        .unwrap();
    let partial = load_order(&pool, id).await.unwrap();
    assert_eq!(partial.status, "partially_filled");
    assert_eq!(partial.filled_quantity, Decimal::from(4));
    assert_eq!(partial.avg_fill_price, Some(price));
    let inventory = || {
        sqlx::query_as::<_, (Decimal, Decimal)>(
            "SELECT quantity, cost_basis FROM manual_inventory WHERE wallet_address = '0xreview'",
        )
        .fetch_one(&pool)
    };
    assert_eq!(
        inventory().await.unwrap(),
        (Decimal::from(4), Decimal::new(18, 1))
    );
    // Settled trades never exceed what the exchange matched.
    assert!(
        apply_exchange_snapshot(&pool, &partial, &snapshot, Some((Decimal::from(5), price)))
            .await
            .is_err()
    );
    snapshot.size_matched = "3".into();
    assert!(apply_exchange_snapshot(&pool, &partial, &snapshot, zero)
        .await
        .is_err());
    snapshot.size_matched = "10".into();
    snapshot.status = "CANCELED".into();
    apply_exchange_snapshot(
        &pool,
        &partial,
        &snapshot,
        Some((Decimal::TEN, Decimal::new(47, 2))),
    )
    .await
    .unwrap();
    assert_eq!(load_order(&pool, id).await.unwrap().status, "filled");
    // Ten at an average of 0.47 cost 4.70 in all.
    assert_eq!(
        inventory().await.unwrap(),
        (Decimal::TEN, Decimal::new(47, 1))
    );
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
    let half = Decimal::new(5, 1);
    apply_exchange_snapshot(
        &pool,
        &row,
        &snapshot("9.99", "9.99"),
        Some(("9.99".parse().unwrap(), half)),
    )
    .await
    .unwrap();
    assert_eq!(load_order(&pool, market).await.unwrap().status, "filled");
    // A buy given a better price as more shares: filled, all of them kept.
    let limit = reserve("limit-buy").await;
    let row = load_order(&pool, limit).await.unwrap();
    let more = "10.2".parse::<Decimal>().unwrap();
    apply_exchange_snapshot(&pool, &row, &snapshot("10", "10.2"), Some((more, half)))
        .await
        .unwrap();
    let stored = load_order(&pool, limit).await.unwrap();
    assert_eq!(stored.status, "filled");
    assert_eq!(stored.filled_quantity, "10.2".parse::<Decimal>().unwrap());
    // A sell never parts with more than its quantity.
    let mut sell = load_order(&pool, reserve("sell").await).await.unwrap();
    sell.side = "sell".into();
    assert!(
        apply_exchange_snapshot(&pool, &sell, &snapshot("10", "10.2"), Some((more, half)))
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
        let fill = Some((Decimal::from(4), Decimal::new(5, 1)));
        apply_exchange_snapshot(&pool, &row, &snapshot, fill)
            .await
            .unwrap();
        let stored = load_order(&pool, id).await.unwrap();
        assert_eq!(stored.status, "cancelled", "{closed}");
        assert_eq!(stored.filled_quantity, Decimal::from(4));
        // A later, stale answer does not reopen it.
        snapshot.status = "LIVE".into();
        apply_exchange_snapshot(&pool, &stored, &snapshot, fill)
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

#[test]
fn market_ids_are_stored_as_the_catalog_names_them() {
    assert_eq!(canonical_market_id(" 0xABcd12 "), "0xabcd12");
    assert_eq!(canonical_market_id("0XAB"), "0xab");
    // Not a hex condition id: only trimmed.
    assert_eq!(canonical_market_id(" Market-1 "), "Market-1");
    assert_eq!(canonical_market_id("0x"), "0x");
}
