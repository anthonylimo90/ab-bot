use super::*;
use sqlx::Executor;

#[sqlx::test(migrations = false)]
async fn failed_later_inventory_batch_preserves_previous_snapshot(pool: PgPool) {
    pool.execute("CREATE TABLE positions(id UUID PRIMARY KEY)")
        .await
        .unwrap();
    let schema = include_str!("../../../../../migrations/057_wallet_inventory.sql");
    pool.execute(schema.split("CREATE TRIGGER").next().unwrap())
        .await
        .unwrap();
    let repo = WalletInventoryRepository::new(pool);
    let first = WalletInventoryUpsert {
        wallet_address: "wallet".into(),
        token_id: "0".into(),
        condition_id: None,
        outcome: None,
        linked_position_id: None,
        quantity: Decimal::from(7),
        cost_basis: None,
        current_price: None,
        marked_value: None,
        is_orphan: true,
        discovery_source: "inventory".into(),
        recovery_status: "observed".into(),
        last_exit_error: None,
        last_exit_attempted_at: None,
        first_observed_at: Utc::now(),
        last_observed_at: Utc::now(),
    };
    repo.upsert_entries(std::slice::from_ref(&first))
        .await
        .unwrap();
    let rows: Vec<_> = (0..501)
        .map(|token| WalletInventoryUpsert {
            token_id: token.to_string(),
            quantity: if token == 500 {
                -Decimal::ONE
            } else {
                Decimal::ONE
            },
            ..first.clone()
        })
        .collect();
    assert!(repo.upsert_entries(&rows).await.is_err());
    let retained = repo.list_wallet_entries("wallet").await.unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].quantity, Decimal::from(7));
}

#[sqlx::test(migrations = false)]
async fn stale_orphan_flags_cannot_sell_manual_or_partially_exited_inventory(pool: PgPool) {
    pool.execute("CREATE TABLE positions(id UUID PRIMARY KEY,market_id TEXT,execution_mode TEXT,wallet_address TEXT,held_yes_qty NUMERIC,held_no_qty NUMERIC,state SMALLINT DEFAULT 1); CREATE TABLE orders(execution_mode TEXT,wallet_address TEXT,market_id TEXT,outcome TEXT,status TEXT); CREATE TABLE manual_inventory(execution_mode TEXT,wallet_address TEXT,market_id TEXT,outcome TEXT,quantity NUMERIC)").await.unwrap();
    let schema = include_str!("../../../../../migrations/057_wallet_inventory.sql");
    pool.execute(schema.split("CREATE TRIGGER").next().unwrap())
        .await
        .unwrap();
    pool.execute("INSERT INTO manual_inventory VALUES('live','wallet','manual','yes',6); INSERT INTO positions(id,market_id,execution_mode,wallet_address,held_yes_qty,held_no_qty) VALUES(gen_random_uuid(),'partial','live','wallet',6,0); INSERT INTO wallet_inventory(wallet_address,token_id,condition_id,outcome,quantity,is_orphan) VALUES ('wallet','1','manual','yes',6,TRUE),('wallet','2','partial','yes',6,TRUE),('wallet','3','untracked','yes',6,TRUE),('wallet','4','pending','yes',6,TRUE); INSERT INTO orders VALUES('live','wallet','pending','yes','pending')").await.unwrap();
    let repo = WalletInventoryRepository::new(pool);
    let recoverable = repo.get_recoverable_orphans("wallet", 0).await.unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(recoverable[0].token_id, "3");
}

#[sqlx::test(migrations = false)]
async fn a_partial_sale_takes_its_share_of_the_cost_basis(pool: PgPool) {
    pool.execute("CREATE TABLE positions(id UUID PRIMARY KEY)")
        .await
        .unwrap();
    let schema = include_str!("../../../../../migrations/057_wallet_inventory.sql");
    pool.execute(schema.split("CREATE TRIGGER").next().unwrap())
        .await
        .unwrap();
    let repo = WalletInventoryRepository::new(pool);
    repo.upsert_entries(&[WalletInventoryUpsert {
        wallet_address: "wallet".into(),
        token_id: "token".into(),
        condition_id: None,
        outcome: None,
        linked_position_id: None,
        quantity: Decimal::from(100),
        cost_basis: Some(Decimal::from(50)),
        current_price: None,
        marked_value: None,
        is_orphan: true,
        discovery_source: "inventory".into(),
        recovery_status: "observed".into(),
        last_exit_error: None,
        last_exit_attempted_at: None,
        first_observed_at: Utc::now(),
        last_observed_at: Utc::now(),
    }])
    .await
    .unwrap();
    // 40 of 100 sold: 60 remain, carrying 60% of the basis.
    repo.record_disposal("WALLET", "token", Decimal::from(40))
        .await
        .unwrap();
    let row = &repo.list_wallet_entries("wallet").await.unwrap()[0];
    assert_eq!(row.quantity, Decimal::from(60));
    assert_eq!(row.cost_basis, Some(Decimal::from(30)));
    // Selling more than is held leaves nothing, and no basis.
    repo.record_disposal("wallet", "token", Decimal::from(80))
        .await
        .unwrap();
    let row = &repo.list_wallet_entries("wallet").await.unwrap()[0];
    assert_eq!(row.quantity, Decimal::ZERO);
    assert_eq!(row.cost_basis, Some(Decimal::ZERO));
}
