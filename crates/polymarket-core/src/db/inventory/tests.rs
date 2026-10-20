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
