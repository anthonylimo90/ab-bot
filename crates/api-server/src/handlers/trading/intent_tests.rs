use super::*;
use sqlx::Executor;

async fn setup(pool: &PgPool) {
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
        reserve_order(&pool, &request, false),
        reserve_order(&pool, &request, false)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.0, b.0);
    assert_ne!(a.1, b.1);
    assert_eq!(
        reserve_order(&pool, &request, false).await.unwrap(),
        (a.0, false)
    );
    sqlx::query("UPDATE orders SET status='filled',filled_quantity=quantity WHERE id=$1")
        .bind(a.0)
        .execute(&pool)
        .await
        .unwrap();
    let replay = load_order(
        &pool,
        reserve_order(&pool, &request, false).await.unwrap().0,
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
        reserve_order(&pool, &changed, false).await,
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
    let (id, _) = reserve_order(&pool, &order(), true).await.unwrap();
    sqlx::query("UPDATE orders SET exchange_order_id='exchange-id', status='open' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let mut snapshot = polymarket_core::api::clob::ExchangeOrder {
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
async fn preflight_failure_rejects_only_intents_that_never_crossed_submission_boundary(
    pool: PgPool,
) {
    setup(&pool).await;
    let request = order();
    let (id, _) = reserve_order(&pool, &request, true).await.unwrap();
    reject_unsubmitted_order(&pool, id).await.unwrap();
    assert_eq!(load_order(&pool, id).await.unwrap().status, "rejected");
    assert_eq!(
        reserve_order(&pool, &request, true).await.unwrap(),
        (id, false)
    );
    let request = PlaceOrderRequest {
        client_order_id: Some("submitted-intent".into()),
        ..request
    };
    let (id, _) = reserve_order(&pool, &request, true).await.unwrap();
    sqlx::query("INSERT INTO execution_intents (id,market_id,signed_order,status) VALUES ($1,'market','{}','unknown')").bind(id).execute(&pool).await.unwrap();
    reject_unsubmitted_order(&pool, id).await.unwrap();
    assert_eq!(load_order(&pool, id).await.unwrap().status, "pending");
}
