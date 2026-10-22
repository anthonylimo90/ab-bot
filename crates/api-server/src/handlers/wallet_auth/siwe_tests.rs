//! Sign-in-with-Ethereum regression tests: messages must be issued for this site
//! and chain, and a nonce can be used exactly once, even under concurrent replay.
//! They run against the real schema and sign with a real secp256k1 key.

use super::*;
use crate::workspace_scope::test_fixture as fx;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use k256::ecdsa::SigningKey;
use serde_json::json;
use sqlx::PgPool;

struct Wallet {
    key: SigningKey,
    /// Lowercase `0x` address, as stored by the API.
    address: String,
}

impl Wallet {
    fn new(seed: u8) -> Self {
        let key = SigningKey::from_slice(&[seed; 32]).unwrap();
        let point = key.verifying_key().to_encoded_point(false);
        let hash = alloy_primitives::keccak256(&point.as_bytes()[1..]);
        Self {
            key,
            address: format!("0x{}", hex::encode(&hash[12..])),
        }
    }

    /// EIP-191 personal_sign over the SIWE message text.
    fn sign(&self, message: &str) -> String {
        let parsed: Message = message.parse().unwrap();
        let prehash = parsed.eip191_hash().unwrap();
        let (signature, recovery) = self.key.sign_prehash_recoverable(&prehash).unwrap();
        let mut bytes = signature.to_bytes().to_vec();
        bytes.push(recovery.to_byte() + 27);
        format!("0x{}", hex::encode(bytes))
    }
}

struct Fixture {
    app: Router,
    pool: PgPool,
}

async fn fixture(pool: &PgPool) -> Fixture {
    fx::real_schema(pool).await;
    let state = AppState::for_tests(pool.clone());
    let authed = |router: axum::routing::MethodRouter<Arc<AppState>>| {
        router.layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_auth,
        ))
    };
    let admin = |router: axum::routing::MethodRouter<Arc<AppState>>| {
        authed(router.layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_admin,
        )))
    };
    let app = Router::new()
        .route("/challenge", post(challenge))
        .route("/verify", post(verify))
        .route("/login", post(crate::handlers::auth::login))
        .route("/link", authed(post(link_wallet)))
        .route("/me", authed(get(crate::handlers::auth::get_current_user)))
        .route("/users", admin(get(crate::handlers::users::list_users)))
        .route(
            "/users/:user_id",
            admin(get(crate::handlers::users::get_user)),
        )
        .route(
            "/workspaces/:workspace_id/members",
            authed(get(crate::handlers::workspaces::list_members)),
        )
        .with_state(state);
    Fixture {
        app,
        pool: pool.clone(),
    }
}

impl Fixture {
    /// Ask the API for a challenge and return the SIWE message to sign.
    async fn challenge(&self, wallet: &Wallet) -> String {
        let (status, body) = fx::call(
            &self.app,
            "POST",
            "/challenge",
            None,
            Some(json!({ "address": wallet.address })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        body["message"].as_str().unwrap().to_string()
    }

    async fn verify(&self, message: &str, signature: &str) -> (StatusCode, serde_json::Value) {
        fx::call(
            &self.app,
            "POST",
            "/verify",
            None,
            Some(json!({ "message": message, "signature": signature })),
        )
        .await
    }

    async fn link(
        &self,
        token: &str,
        message: &str,
        signature: &str,
    ) -> (StatusCode, serde_json::Value) {
        fx::call(
            &self.app,
            "POST",
            "/link",
            Some(token),
            Some(json!({ "message": message, "signature": signature })),
        )
        .await
    }

    /// An existing wallet-linked account, so the success path does not depend on signup.
    async fn wallet_user(&self, wallet: &Wallet) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, role, wallet_address) VALUES ($1, $2, 'unused', 1, $3)",
        )
        .bind(id)
        .bind(format!("{id}@example.test"))
        .bind(&wallet.address)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    async fn unused_challenges(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM auth_challenges WHERE used_at IS NULL")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

/// The ways a message can claim to be for somebody else.
fn tampered_messages(message: &str) -> Vec<(&'static str, String)> {
    let site = SiteBinding::development();
    vec![
        ("domain", message.replacen(&site.domain, "evil.example", 1)),
        (
            "uri",
            message.replacen(
                &format!("URI: {}", site.uri),
                "URI: https://evil.example",
                1,
            ),
        ),
        (
            "chain",
            message.replacen(&format!("Chain ID: {SIWE_CHAIN_ID}"), "Chain ID: 1", 1),
        ),
    ]
}

#[sqlx::test(migrations = false)]
async fn a_message_issued_by_this_site_logs_in(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    let user = f.wallet_user(&wallet).await;
    let message = f.challenge(&wallet).await;
    let (status, body) = f.verify(&message, &wallet.sign(&message)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["id"], user.to_string());
    assert_eq!(body["is_new_user"], false);
    let token = body["token"].as_str().unwrap();
    let (status, me) = fx::call(&f.app, "GET", "/me", Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], user.to_string());
    // The session lifetime is the configured one (1h in the test state), not a literal.
    let claims = AppState::for_tests(pool.clone())
        .jwt_auth
        .validate_token(token)
        .unwrap();
    assert_eq!(claims.exp - claims.iat, 3600);
}

#[sqlx::test(migrations = false)]
async fn messages_for_another_domain_uri_or_chain_are_rejected(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    f.wallet_user(&wallet).await;
    let message = f.challenge(&wallet).await;
    for (what, tampered) in tampered_messages(&message) {
        assert_ne!(tampered, message, "{what} tamper must change the message");
        // Correctly signed by the real wallet and carrying a live nonce, yet not for us.
        let (status, _) = f.verify(&tampered, &wallet.sign(&tampered)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
    }
    // Rejected before the nonce is touched, so the genuine message still works.
    assert_eq!(f.unused_challenges().await, 1);
    let (status, _) = f.verify(&message, &wallet.sign(&message)).await;
    assert_eq!(status, StatusCode::OK);
}

#[test]
fn site_binding_matches_domain_case_insensitively_and_ignores_trailing_slash() {
    let site = SiteBinding {
        domain: "App.Example.com:8443".into(),
        uri: "https://app.example.com:8443/".into(),
    };
    let message = |domain: &str, uri: &str, chain: u64| -> Message {
        format!(
            "{domain} wants you to sign in with your Ethereum account:\n0x6Da01670d8fc844e736095918bbE11fE8D564163\n\nSign in\n\nURI: {uri}\nVersion: 1\nChain ID: {chain}\nNonce: abcdefgh12345678\nIssued At: 2024-01-01T00:00:00Z"
        )
        .parse()
        .unwrap()
    };
    assert!(site
        .ensure_bound(&message(
            "app.example.com:8443",
            "https://app.example.com:8443",
            137
        ))
        .is_ok());
    for bad in [
        message("app.example.com", "https://app.example.com:8443", 137),
        message("evil.example:8443", "https://app.example.com:8443", 137),
        message("app.example.com:8443", "https://evil.example", 137),
        message(
            "app.example.com:8443",
            "https://app.example.com:8443/other",
            137,
        ),
        message("app.example.com:8443", "https://app.example.com:8443", 1),
    ] {
        assert!(site.ensure_bound(&bad).is_err());
    }
}

#[sqlx::test(migrations = false)]
async fn a_nonce_cannot_be_replayed(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    f.wallet_user(&wallet).await;
    let message = f.challenge(&wallet).await;
    let signature = wallet.sign(&message);
    assert_eq!(f.verify(&message, &signature).await.0, StatusCode::OK);
    assert_eq!(
        f.verify(&message, &signature).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = false)]
async fn concurrent_replays_mint_exactly_one_session(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    f.wallet_user(&wallet).await;
    let message = f.challenge(&wallet).await;
    let signature = wallet.sign(&message);
    let results =
        futures_util::future::join_all((0..8).map(|_| f.verify(&message, &signature))).await;
    let ok = results.iter().filter(|(s, _)| *s == StatusCode::OK).count();
    let unauthorized = results
        .iter()
        .filter(|(s, _)| *s == StatusCode::UNAUTHORIZED)
        .count();
    assert_eq!((ok, unauthorized), (1, 7), "{results:?}");
}

#[sqlx::test(migrations = false)]
async fn unknown_expired_and_foreign_nonces_are_rejected(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    let other = Wallet::new(9);
    f.wallet_user(&wallet).await;
    f.wallet_user(&other).await;

    // A nonce we never issued.
    let message = f.challenge(&wallet).await;
    let nonce = message
        .lines()
        .find_map(|l| l.strip_prefix("Nonce: "))
        .unwrap();
    let forged = message.replace(nonce, "0123456789abcdef");
    assert_eq!(
        f.verify(&forged, &wallet.sign(&forged)).await.0,
        StatusCode::UNAUTHORIZED
    );

    // A nonce issued to another address.
    let issued_to = message.parse::<Message>().unwrap().address;
    let other_address: [u8; 20] = hex::decode(&other.address[2..])
        .unwrap()
        .try_into()
        .unwrap();
    let stolen = message.replace(&siwe::eip55(&issued_to), &siwe::eip55(&other_address));
    assert_ne!(stolen, message);
    assert_eq!(
        f.verify(&stolen, &other.sign(&stolen)).await.0,
        StatusCode::UNAUTHORIZED
    );

    // An expired nonce, even with the message's own window still open.
    sqlx::query("UPDATE auth_challenges SET expires_at = NOW() - INTERVAL '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        f.verify(&message, &wallet.sign(&message)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = false)]
async fn a_failed_signature_check_still_burns_the_nonce(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    f.wallet_user(&wallet).await;
    let message = f.challenge(&wallet).await;
    // Signed by a different key than the address in the message.
    let wrong = Wallet::new(9).sign(&message);
    assert_eq!(f.verify(&message, &wrong).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(f.unused_challenges().await, 0);
    assert_eq!(
        f.verify(&message, &wallet.sign(&message)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = false)]
async fn link_wallet_enforces_site_binding_and_single_use(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    let owner = fx::user(&pool, auth::UserRole::Trader).await;
    let rival = fx::user(&pool, auth::UserRole::Trader).await;
    let state = AppState::for_tests(pool.clone());
    let owner_token = fx::token(&state, owner, auth::UserRole::Trader).await;
    let rival_token = fx::token(&state, rival, auth::UserRole::Trader).await;

    let message = f.challenge(&wallet).await;
    let signature = wallet.sign(&message);
    for (what, tampered) in tampered_messages(&message) {
        let (status, _) = f
            .link(&owner_token, &tampered, &wallet.sign(&tampered))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
    }
    let (status, body) = f.link(&owner_token, &message, &signature).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let linked: Option<String> =
        sqlx::query_scalar("SELECT wallet_address FROM users WHERE id = $1")
            .bind(owner)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked.as_deref(), Some(wallet.address.as_str()));

    // Another account replaying the same signed message gets nothing.
    let (status, _) = f.link(&rival_token, &message, &signature).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let rival_wallet: Option<String> =
        sqlx::query_scalar("SELECT wallet_address FROM users WHERE id = $1")
            .bind(rival)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rival_wallet, None);
}

/// Sign in with `wallet` and return the response.
async fn wallet_login(f: &Fixture, wallet: &Wallet) -> (StatusCode, serde_json::Value) {
    let message = f.challenge(wallet).await;
    f.verify(&message, &wallet.sign(&message)).await
}

async fn user_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn first_wallet_login_creates_a_wallet_only_account_and_returns_a_token(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    let (status, body) = wallet_login(&f, &wallet).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_new_user"], true);
    assert_eq!(body["user"]["wallet_address"], wallet.address);
    assert_eq!(body["user"]["role"], "Trader");
    assert!(body["user"].get("email").is_none());

    // No email or password, but the account is complete and usable.
    let (email, hash, linked): (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("SELECT email, password_hash, wallet_address FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((email, hash), (None, None));
    assert_eq!(linked.as_deref(), Some(wallet.address.as_str()));
    assert_eq!(f.unused_challenges().await, 0);

    // The token is a real session, and the account reads back without errors:
    // a missing email serializes as "" so the API shape is unchanged.
    let token = body["token"].as_str().unwrap();
    let (status, me) = fx::call(&f.app, "GET", "/me", Some(token), None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "");
    assert_eq!(me["role"], "Trader");

    // The second sign-in is an ordinary login of the same account.
    let (status, again) = wallet_login(&f, &wallet).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["is_new_user"], false);
    assert_eq!(again["user"]["id"], body["user"]["id"]);
    assert_eq!(user_count(&pool).await, 1);
}

#[sqlx::test(migrations = false)]
async fn concurrent_first_logins_of_one_wallet_create_one_account(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    // Two distinct, valid challenges for the same new wallet, redeemed at once.
    let mut signed = Vec::new();
    for _ in 0..2 {
        let message = f.challenge(&wallet).await;
        let signature = wallet.sign(&message);
        signed.push((message, signature));
    }
    let results = futures_util::future::join_all(signed.iter().map(|(m, s)| f.verify(m, s))).await;
    assert!(
        results.iter().all(|(s, _)| *s == StatusCode::OK),
        "{results:?}"
    );
    let created = results
        .iter()
        .filter(|(_, body)| body["is_new_user"] == true)
        .count();
    assert_eq!(created, 1, "{results:?}");
    assert_eq!(user_count(&pool).await, 1);
}

#[sqlx::test(migrations = false)]
async fn a_failed_account_creation_does_not_burn_the_nonce(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    let message = f.challenge(&wallet).await;
    let signature = wallet.sign(&message);
    sqlx::query(
        "ALTER TABLE users ADD CONSTRAINT test_reject_signup CHECK (wallet_address IS NULL OR role <> 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, _) = f.verify(&message, &signature).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    // Nonce consumption and account creation are one transaction.
    assert_eq!(f.unused_challenges().await, 1);
    sqlx::query("ALTER TABLE users DROP CONSTRAINT test_reject_signup")
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = f.verify(&message, &signature).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_new_user"], true);
}

#[sqlx::test(migrations = false)]
async fn password_login_is_refused_for_accounts_without_a_password(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, role, wallet_address) VALUES ($1, 'wallet@example.test', NULL, 1, $2)",
    )
    .bind(Uuid::new_v4())
    .bind(&wallet.address)
    .execute(&pool)
    .await
    .unwrap();
    for password in [
        "",
        "x",
        "correct horse battery staple",
        "$argon2id$v=19$m=19456,t=2,p=1$",
    ] {
        let (status, _) = fx::call(
            &f.app,
            "POST",
            "/login",
            None,
            Some(json!({"email": "wallet@example.test", "password": password})),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "password {password:?}");
    }
}

#[sqlx::test(migrations = false)]
async fn every_account_needs_a_credential_or_a_wallet(pool: PgPool) {
    fx::real_schema(&pool).await;
    // Neither a full email/password pair nor a wallet: rejected by the CHECK constraint.
    for (email, hash) in [
        (None, None),
        (Some("a@example.test"), None),
        (None, Some("h")),
    ] {
        let err = sqlx::query(
            "INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, $3, 1)",
        )
        .bind(Uuid::new_v4())
        .bind(email)
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap_err();
        let code = err
            .as_database_error()
            .and_then(|e| e.code().map(|c| c.to_string()));
        assert_eq!(code.as_deref(), Some("23514"), "{email:?} {hash:?}");
    }
    // A full credential pair, or a wallet alone, is fine.
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, role) VALUES ($1, 'ok@example.test', 'h', 1)",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO users (id, role, wallet_address) VALUES ($1, 1, '0x0000000000000000000000000000000000000001')",
    )
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = false)]
async fn user_and_member_listings_tolerate_accounts_without_email(pool: PgPool) {
    let f = fixture(&pool).await;
    let wallet = Wallet::new(7);
    let (_, login) = wallet_login(&f, &wallet).await;
    let wallet_user: Uuid = login["user"]["id"].as_str().unwrap().parse().unwrap();
    let wallet_token = login["token"].as_str().unwrap();
    let workspace = fx::workspace(&pool, "ws", 10).await;
    fx::add_member(&pool, workspace, wallet_user, "owner").await;

    let state = AppState::for_tests(pool.clone());
    let admin = fx::user(&pool, auth::UserRole::PlatformAdmin).await;
    let admin_token = fx::token(&state, admin, auth::UserRole::PlatformAdmin).await;

    let (status, users) = fx::call(&f.app, "GET", "/users", Some(&admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{users}");
    let listed = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == wallet_user.to_string())
        .expect("wallet account is listed");
    assert_eq!(listed["email"], "");
    let (status, one) = fx::call(
        &f.app,
        "GET",
        &format!("/users/{wallet_user}"),
        Some(&admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["email"], "");

    let (status, members) = fx::call(
        &f.app,
        "GET",
        &format!("/workspaces/{workspace}/members"),
        Some(wallet_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    assert_eq!(members[0]["email"], "");
}

#[test]
fn site_binding_is_required_outside_development() {
    let some = |value: &str| Some(value.to_string());
    let configured =
        SiteBinding::resolve(some("app.example.com"), some("https://app.example.com"), "");
    assert_eq!(configured.unwrap().domain, "app.example.com");
    // Unset (or half-set) binding: never fall back to localhost in a deployment.
    assert!(SiteBinding::resolve(None, None, "production").is_none());
    assert!(SiteBinding::resolve(None, None, "").is_none());
    assert!(SiteBinding::resolve(some("app.example.com"), None, "staging").is_none());
    assert_eq!(
        SiteBinding::resolve(None, None, "development")
            .unwrap()
            .domain,
        "localhost"
    );
}
