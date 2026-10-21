use super::*;
use auth::{key_vault::KeyVaultProvider, KeyVault, TradingWallet};
use sqlx::Executor;

#[sqlx::test(migrations = false)]
async fn personal_wallet_preferences_do_not_change_the_workspace_signer(pool: PgPool) {
    let [owner, stranger, _] = setup(&pool).await;
    let vault = vault();
    let bot_address = request(KEY_ONE).address.to_lowercase();
    vault
        .store_wallet_key(&bot_address, &hex::decode(KEY_ONE).unwrap())
        .await
        .unwrap();
    pool.execute(
        "CREATE TABLE workspaces (
        id UUID PRIMARY KEY DEFAULT gen_random_uuid(), trading_wallet_address TEXT,
        live_trading_enabled BOOLEAN DEFAULT true, exit_handler_enabled BOOLEAN DEFAULT false,
        arb_auto_execute BOOLEAN DEFAULT false, created_at TIMESTAMPTZ DEFAULT NOW()
    )",
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO workspaces (trading_wallet_address) VALUES ($1)")
        .bind(&bot_address)
        .execute(&pool)
        .await
        .unwrap();
    let first = request(KEY_TWO);
    let second = request("0000000000000000000000000000000000000000000000000000000000000003");
    register_wallet(&pool, &vault, owner, &first).await.unwrap();
    register_wallet(&pool, &vault, owner, &second)
        .await
        .unwrap();
    assert!(matches!(
        set_personal_primary_wallet(&pool, stranger, &second.address).await,
        Err(ApiError::NotFound(_))
    ));
    set_personal_primary_wallet(&pool, owner, &second.address)
        .await
        .unwrap();
    assert_eq!(
        crate::state::resolve_startup_wallet_address(&pool, None)
            .await
            .unwrap(),
        Some(bot_address.clone())
    );
    disconnect_wallet(&pool, &vault, owner, &second.address)
        .await
        .unwrap();
    assert_eq!(
        resolve_primary_wallet_address(&pool, owner).await.unwrap(),
        Some(first.address.to_lowercase())
    );
    assert_eq!(
        crate::state::resolve_startup_wallet_address(&pool, None)
            .await
            .unwrap(),
        Some(bot_address.clone())
    );
    assert_eq!(
        vault.get_wallet_key(&bot_address).await.unwrap().unwrap(),
        hex::decode(KEY_ONE).unwrap()
    );
}

// Public test keys only; never fund these addresses.
const KEY_ONE: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const KEY_TWO: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const ADDRESS_ONE: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";

fn request(key: &str) -> StoreWalletRequest {
    StoreWalletRequest {
        address: TradingWallet::from_private_key(key)
            .unwrap()
            .address_string(),
        private_key: key.into(),
        label: None,
    }
}

fn vault() -> KeyVault {
    KeyVault::new(
        KeyVaultProvider::Memory,
        b"wallet-ownership-test-master-key".to_vec(),
    )
}

async fn setup(pool: &PgPool) -> [Uuid; 3] {
    pool.execute("CREATE TABLE users (id UUID PRIMARY KEY)")
        .await
        .unwrap();
    pool.execute(include_str!(
        "../../../../../migrations/008_user_wallets.sql"
    ))
    .await
    .unwrap();
    let users = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    for user in users {
        sqlx::query("INSERT INTO users (id) VALUES ($1)")
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
    }
    users
}

#[test]
fn key_must_derive_requested_address() {
    assert_eq!(
        validate_wallet_key(ADDRESS_ONE, KEY_ONE).unwrap(),
        hex::decode(KEY_ONE).unwrap()
    );
    assert!(matches!(
        validate_wallet_key(ADDRESS_ONE, KEY_TWO),
        Err(ApiError::BadRequest(_))
    ));
    assert!(matches!(
        validate_wallet_key(ADDRESS_ONE, "invalid"),
        Err(ApiError::BadRequest(_))
    ));
    assert!(matches!(
        validate_wallet_key(ADDRESS_ONE, &"0".repeat(64)),
        Err(ApiError::BadRequest(_))
    ));
    assert!(validate_wallet_key(
        &ADDRESS_ONE.to_uppercase().replacen("0X", "0x", 1),
        &format!(" 0x{KEY_ONE} ")
    )
    .is_ok());
}

#[sqlx::test(migrations = false)]
async fn second_account_cannot_replace_an_owned_key(pool: PgPool) {
    let [owner, other, _] = setup(&pool).await;
    let vault = vault();
    let req = request(KEY_ONE);
    register_wallet(&pool, &vault, owner, &req).await.unwrap();
    assert!(matches!(
        register_wallet(&pool, &vault, other, &req).await,
        Err(ApiError::Conflict(_))
    ));
    let keys = vault.get_wallet_key(&req.address).await.unwrap().unwrap();
    assert_eq!(keys, hex::decode(KEY_ONE).unwrap());
    let owners: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM user_wallets")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(owners, vec![owner]);
}

#[sqlx::test(migrations = false)]
async fn concurrent_accounts_cannot_both_register_one_address(pool: PgPool) {
    let [first, second, _] = setup(&pool).await;
    let vault = vault();
    let req = request(KEY_ONE);
    let (left, right) = tokio::join!(
        register_wallet(&pool, &vault, first, &req),
        register_wallet(&pool, &vault, second, &req),
    );
    assert!(matches!(
        (left, right),
        (Ok(_), Err(ApiError::Conflict(_))) | (Err(ApiError::Conflict(_)), Ok(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test(migrations = false)]
async fn concurrent_first_wallets_have_one_primary(pool: PgPool) {
    let [owner, _, _] = setup(&pool).await;
    let vault = vault();
    let first = request(KEY_ONE);
    let second = request(KEY_TWO);
    let (left, right) = tokio::join!(
        register_wallet(&pool, &vault, owner, &first),
        register_wallet(&pool, &vault, owner, &second),
    );
    let wallets = [left.unwrap(), right.unwrap()];
    assert_eq!(wallets.iter().filter(|wallet| wallet.is_primary).count(), 1);
    assert_eq!(vault.list_wallet_addresses().await.len(), 2);
}

#[sqlx::test(migrations = false)]
async fn legacy_shared_registration_cannot_destroy_another_accounts_key(pool: PgPool) {
    let [owner, legacy_owner, stranger] = setup(&pool).await;
    let vault = vault();
    let req = request(KEY_ONE);
    register_wallet(&pool, &vault, owner, &req).await.unwrap();
    sqlx::query("INSERT INTO user_wallets (user_id, address) VALUES ($1, $2)")
        .bind(legacy_owner)
        .bind(req.address.to_uppercase())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        disconnect_wallet(&pool, &vault, stranger, &req.address).await,
        Err(ApiError::NotFound(_))
    ));
    disconnect_wallet(&pool, &vault, legacy_owner, &req.address)
        .await
        .unwrap();
    assert_eq!(
        vault.get_wallet_key(&req.address).await.unwrap().unwrap(),
        hex::decode(KEY_ONE).unwrap()
    );
    let remaining: Uuid = sqlx::query_scalar("SELECT user_id FROM user_wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, owner);
}

#[sqlx::test(migrations = false)]
async fn existing_signer_material_is_never_overwritten(pool: PgPool) {
    let [owner, _, _] = setup(&pool).await;
    let vault = vault();
    let req = request(KEY_ONE);
    let prior = hex::decode(KEY_TWO).unwrap();
    vault.store_wallet_key(&req.address, &prior).await.unwrap();
    assert!(matches!(
        register_wallet(&pool, &vault, owner, &req).await,
        Err(ApiError::Conflict(_))
    ));
    assert_eq!(
        vault.get_wallet_key(&req.address).await.unwrap().unwrap(),
        prior
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrations = false)]
async fn mixed_case_legacy_rows_retain_the_shared_key(pool: PgPool) {
    let [owner, _, _] = setup(&pool).await;
    let vault = vault();
    let req = request(KEY_ONE);
    register_wallet(&pool, &vault, owner, &req).await.unwrap();
    sqlx::query("INSERT INTO user_wallets (user_id, address) VALUES ($1, $2)")
        .bind(owner)
        .bind(req.address.to_uppercase())
        .execute(&pool)
        .await
        .unwrap();
    disconnect_wallet(&pool, &vault, owner, &req.address)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        vault.get_wallet_key(&req.address).await.unwrap().unwrap(),
        hex::decode(KEY_ONE).unwrap()
    );
}

#[sqlx::test(migrations = false)]
async fn rejected_metadata_does_not_leave_a_new_key(pool: PgPool) {
    setup(&pool).await;
    let vault = vault();
    let req = request(KEY_ONE);
    assert!(register_wallet(&pool, &vault, Uuid::new_v4(), &req)
        .await
        .is_err());
    assert!(vault.get_wallet_key(&req.address).await.unwrap().is_none());
}

#[sqlx::test(migrations = false)]
async fn withdrawal_replay_checks_owner_and_original_transfer_details(pool: PgPool) {
    pool.execute("CREATE TABLE wallet_withdrawals (id UUID, user_id UUID, wallet_address TEXT, destination_address TEXT, asset TEXT, amount NUMERIC, status TEXT, tx_hash TEXT, error TEXT, requested_at TIMESTAMPTZ DEFAULT NOW(), confirmed_at TIMESTAMPTZ, client_request_id UUID, UNIQUE(wallet_address,client_request_id))").await.unwrap();
    let user = Uuid::new_v4();
    let key = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO wallet_withdrawals (id,user_id,wallet_address,destination_address,asset,amount,status,client_request_id) VALUES ($1,$2,'source','destination','USDC',10,'pending',$3)")
        .bind(id).bind(user).bind(key).execute(&pool).await.unwrap();
    let replay = existing_withdrawal(&pool, user, "source", "destination", Decimal::TEN, key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.id, id);
    assert_eq!(replay.status, "pending");
    assert!(
        existing_withdrawal(&pool, user, "source", "destination", Decimal::ONE, key)
            .await
            .is_err()
    );
    assert!(
        existing_withdrawal(&pool, user, "source", "changed", Decimal::TEN, key)
            .await
            .is_err()
    );
    assert!(existing_withdrawal(
        &pool,
        Uuid::new_v4(),
        "source",
        "destination",
        Decimal::TEN,
        key
    )
    .await
    .unwrap()
    .is_none());
}
