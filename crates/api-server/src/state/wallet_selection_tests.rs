use super::*;
use auth::KeyVaultProvider;
use sqlx::Executor;
use uuid::Uuid;

const KEY_ONE: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const KEY_TWO: &str = "0000000000000000000000000000000000000000000000000000000000000002";

fn address(key: &str) -> String {
    TradingWallet::from_private_key(key)
        .unwrap()
        .address_string()
        .to_lowercase()
}

fn vault() -> KeyVault {
    KeyVault::new(
        KeyVaultProvider::Memory,
        b"startup-wallet-selection-test-key".to_vec(),
    )
}

async fn setup(pool: &PgPool, configured: Option<&str>) -> Uuid {
    pool.execute("CREATE TABLE execution_intents (status TEXT); CREATE TABLE orders (execution_mode TEXT,status TEXT)").await.unwrap();
    pool.execute(
        "CREATE TABLE workspaces (
            id UUID PRIMARY KEY, trading_wallet_address TEXT,
            live_trading_enabled BOOLEAN DEFAULT false,
            exit_handler_enabled BOOLEAN DEFAULT false,
            arb_auto_execute BOOLEAN DEFAULT false,
            created_at TIMESTAMPTZ DEFAULT NOW()
        );
        CREATE TABLE positions (
            state SMALLINT NOT NULL,
            held_yes_qty NUMERIC NOT NULL DEFAULT 0,
            held_no_qty NUMERIC NOT NULL DEFAULT 0
        );
        CREATE TABLE user_wallets (
            address TEXT, is_primary BOOLEAN, updated_at TIMESTAMPTZ DEFAULT NOW(),
            created_at TIMESTAMPTZ DEFAULT NOW()
        );",
    )
    .await
    .unwrap();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, trading_wallet_address) VALUES ($1, $2)")
        .bind(id)
        .bind(configured)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test(migrations = false)]
async fn personal_primary_wallet_does_not_choose_the_trading_signer(pool: PgPool) {
    let pinned = address(KEY_ONE);
    setup(&pool, Some(&pinned)).await;
    sqlx::query("INSERT INTO user_wallets (address, is_primary) VALUES ($1, true)")
        .bind(address(KEY_TWO))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        resolve_startup_wallet_address(&pool, None).await.unwrap(),
        Some(pinned)
    );
}

#[sqlx::test(migrations = false)]
async fn unconfigured_workspace_does_not_adopt_a_personal_wallet(pool: PgPool) {
    setup(&pool, None).await;
    sqlx::query("INSERT INTO user_wallets (address, is_primary) VALUES ($1, true)")
        .bind(address(KEY_TWO))
        .execute(&pool)
        .await
        .unwrap();
    assert!(resolve_startup_wallet_address(&pool, None)
        .await
        .unwrap()
        .is_none());
}

#[sqlx::test(migrations = false)]
async fn operator_can_select_a_wallet_when_no_exposure_remains(pool: PgPool) {
    setup(&pool, Some(&address(KEY_ONE))).await;
    let selected = address(KEY_TWO);
    assert_eq!(
        resolve_startup_wallet_address(&pool, Some(&selected))
            .await
            .unwrap(),
        Some(selected)
    );
}

#[sqlx::test(migrations = false)]
async fn operator_cannot_rotate_signer_away_from_held_or_pending_positions(pool: PgPool) {
    setup(&pool, Some(&address(KEY_ONE))).await;
    let requested = address(KEY_TWO);
    // Include terminal-state holdings so an inconsistent legacy row cannot hide exposure.
    for (state, held) in [(0i16, 0i32), (1, 0), (6, 1), (7, 1), (4, 1)] {
        sqlx::query("INSERT INTO positions (state, held_yes_qty) VALUES ($1, $2)")
            .bind(state)
            .bind(held)
            .execute(&pool)
            .await
            .unwrap();
        assert!(resolve_startup_wallet_address(&pool, Some(&requested))
            .await
            .is_err());
        sqlx::query("DELETE FROM positions")
            .execute(&pool)
            .await
            .unwrap();
    }
}

#[sqlx::test(migrations = false)]
async fn invalid_explicit_address_does_not_fall_back_to_a_workspace_wallet(pool: PgPool) {
    setup(&pool, Some(&address(KEY_ONE))).await;
    assert!(resolve_startup_wallet_address(&pool, Some("not-a-wallet"))
        .await
        .is_err());
}

#[tokio::test]
async fn selected_vault_key_takes_precedence_over_an_unrelated_environment_key() {
    let vault = vault();
    let selected = address(KEY_ONE);
    vault
        .store_wallet_key(&selected, &hex::decode(KEY_ONE).unwrap())
        .await
        .unwrap();
    let wallet = load_startup_trading_wallet(&vault, Some(&selected), Some(KEY_TWO))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wallet.address_string().to_lowercase(), selected);
}

#[tokio::test]
async fn environment_fallback_must_match_the_selected_wallet() {
    let vault = vault();
    let selected = address(KEY_ONE);
    assert!(
        load_startup_trading_wallet(&vault, Some(&selected), Some(KEY_TWO))
            .await
            .is_err()
    );
    assert!(load_startup_trading_wallet(&vault, Some(&selected), None)
        .await
        .is_err());
    let wallet = load_startup_trading_wallet(&vault, Some(&selected), Some(KEY_ONE))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wallet.address_string().to_lowercase(), selected);
}

#[tokio::test]
async fn corrupt_vault_binding_is_rejected_even_with_a_valid_environment_key() {
    let vault = vault();
    let selected = address(KEY_ONE);
    vault
        .store_wallet_key(&selected, &hex::decode(KEY_TWO).unwrap())
        .await
        .unwrap();
    assert!(
        load_startup_trading_wallet(&vault, Some(&selected), Some(KEY_ONE))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn environment_only_configuration_remains_supported() {
    let vault = vault();
    let wallet = load_startup_trading_wallet(&vault, None, Some(KEY_TWO))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wallet.address_string().to_lowercase(), address(KEY_TWO));
    assert!(load_startup_trading_wallet(&vault, None, None)
        .await
        .unwrap()
        .is_none());
}

#[sqlx::test(migrations = false)]
async fn signer_rotation_preserves_ambiguous_and_resting_live_order_exposure(pool: PgPool) {
    setup(&pool, Some(&address(KEY_ONE))).await;
    pool.execute("INSERT INTO execution_intents VALUES ('unknown')")
        .await
        .unwrap();
    assert!(
        resolve_startup_wallet_address(&pool, Some(&address(KEY_TWO)))
            .await
            .is_err()
    );
    pool.execute("DELETE FROM execution_intents; INSERT INTO orders VALUES ('live','open')")
        .await
        .unwrap();
    assert!(
        resolve_startup_wallet_address(&pool, Some(&address(KEY_TWO)))
            .await
            .is_err()
    );
    pool.execute("UPDATE orders SET status='cancelled'")
        .await
        .unwrap();
    assert!(
        resolve_startup_wallet_address(&pool, Some(&address(KEY_TWO)))
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = false)]
async fn permitted_rotation_pins_identity_so_restart_with_new_exposure_remains_valid(pool: PgPool) {
    setup(&pool, Some(&address(KEY_ONE))).await;
    pin_startup_trading_wallet(&pool, &address(KEY_TWO))
        .await
        .unwrap();
    pool.execute("INSERT INTO positions (state,held_yes_qty) VALUES (1,10)")
        .await
        .unwrap();
    assert_eq!(
        resolve_startup_wallet_address(&pool, Some(&address(KEY_TWO)))
            .await
            .unwrap(),
        Some(address(KEY_TWO))
    );
    pin_startup_trading_wallet(&pool, &address(KEY_TWO))
        .await
        .unwrap();
    assert!(pin_startup_trading_wallet(&pool, &address(KEY_ONE))
        .await
        .is_err());
}
