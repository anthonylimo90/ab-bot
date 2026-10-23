use sqlx::{Executor, PgPool};

#[sqlx::test(migrations = false)]
async fn cleanup_preserves_text_failure_reasons_and_all_unresolved_exposure(pool: PgPool) {
    pool.execute("CREATE TABLE positions (id SERIAL, state SMALLINT, failure_reason TEXT, held_yes_qty NUMERIC DEFAULT 0, held_no_qty NUMERIC DEFAULT 0, entry_timestamp TIMESTAMPTZ DEFAULT NOW()-INTERVAL '2 days', exit_timestamp TIMESTAMPTZ, is_open BOOLEAN DEFAULT TRUE, last_updated TIMESTAMPTZ DEFAULT NOW(), updated_at TIMESTAMPTZ DEFAULT NOW()); INSERT INTO positions (state,failure_reason,held_yes_qty) VALUES (5,'legacy reason',0),(6,'{\"connectivity_error\":{\"message\":\"lost response\"}}',10),(7,NULL,5),(4,'closed despite holdings',2),(4,NULL,0)").await.unwrap();
    pool.execute(include_str!(
        "../../../migrations/068_cleanup_stale_positions.sql"
    ))
    .await
    .unwrap();
    let states: Vec<i16> = sqlx::query_scalar("SELECT state FROM positions ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(states, vec![5, 6, 7, 4, 4]);
    pool.execute(include_str!(
        "../../../migrations/071_restore_closed_exposure.sql"
    ))
    .await
    .unwrap();
    let states: Vec<i16> = sqlx::query_scalar("SELECT state FROM positions ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(states, vec![5, 6, 7, 7, 4]);
    let row: (String, rust_decimal::Decimal, bool) =
        sqlx::query_as("SELECT failure_reason,held_yes_qty,is_open FROM positions WHERE id=4")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "closed despite holdings");
    assert_eq!(row.1, rust_decimal::Decimal::from(2));
    assert!(row.2);
    // The repair is safe to rerun.
    pool.execute(include_str!(
        "../../../migrations/071_restore_closed_exposure.sql"
    ))
    .await
    .unwrap();
}

#[sqlx::test(migrations = false)]
async fn execution_migrations_preserve_unknown_historical_provenance(pool: PgPool) {
    pool.execute(
        "CREATE TABLE orders (id UUID PRIMARY KEY); INSERT INTO orders VALUES (gen_random_uuid())",
    )
    .await
    .unwrap();
    pool.execute(include_str!(
        "../../../migrations/069_execution_intents.sql"
    ))
    .await
    .unwrap();
    pool.execute(include_str!(
        "../../../migrations/070_order_exchange_identity.sql"
    ))
    .await
    .unwrap();
    let mode: String = sqlx::query_scalar("SELECT execution_mode FROM orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(mode, "unknown");
}

#[sqlx::test(migrations = false)]
async fn paper_inventory_is_merged_under_one_key(pool: PgPool) {
    use rust_decimal::Decimal;
    pool.execute(
        "CREATE TABLE orders (id SERIAL, execution_mode TEXT, wallet_address TEXT);
         CREATE TABLE manual_inventory (execution_mode TEXT NOT NULL, wallet_address TEXT NOT NULL, \
             market_id TEXT NOT NULL, outcome TEXT NOT NULL, quantity NUMERIC NOT NULL DEFAULT 0, \
             cost_basis NUMERIC NOT NULL DEFAULT 0, \
             PRIMARY KEY (execution_mode, wallet_address, market_id, outcome));
         INSERT INTO manual_inventory VALUES
             ('paper', 'unknown', 'm', 'yes', 2, 1),
             ('paper', '0xabc', 'm', 'yes', 3, 1.5),
             ('paper', '0xabc', 'm', 'no', 4, 2),
             ('live', '0xabc', 'm', 'yes', 5, 2.5);
         INSERT INTO orders (execution_mode, wallet_address) VALUES
             ('paper', '0xabc'), ('paper', NULL), ('live', '0xabc');",
    )
    .await
    .unwrap();
    pool.execute(include_str!(
        "../../../migrations/102_paper_inventory_one_key.sql"
    ))
    .await
    .unwrap();
    let inventory: Vec<(String, String, String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT execution_mode, wallet_address, outcome, quantity, cost_basis \
         FROM manual_inventory ORDER BY execution_mode, outcome",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        inventory,
        vec![
            (
                "live".into(),
                "0xabc".into(),
                "yes".into(),
                Decimal::from(5),
                Decimal::new(25, 1)
            ),
            (
                "paper".into(),
                "unknown".into(),
                "no".into(),
                Decimal::from(4),
                Decimal::from(2)
            ),
            (
                "paper".into(),
                "unknown".into(),
                "yes".into(),
                Decimal::from(5),
                Decimal::new(25, 1)
            ),
        ]
    );
    let wallets: Vec<Option<String>> =
        sqlx::query_scalar("SELECT wallet_address FROM orders ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(wallets, vec![None, None, Some("0xabc".into())]);
}

#[sqlx::test(migrations = false)]
#[ignore = "requires an isolated TimescaleDB test service"]
async fn complete_schema_migrates_with_sqlx_transactions(pool: PgPool) {
    let migrator = sqlx::migrate::Migrator::new(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations"
    )))
    .await
    .unwrap();
    migrator.run(&pool).await.unwrap();
    let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    // Every migration on disk was applied (not a hard-coded version that goes stale).
    let latest = migrator.iter().map(|m| m.version).max().unwrap();
    assert!(latest >= 80);
    assert_eq!(version, latest);
}
