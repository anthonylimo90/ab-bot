use super::*;
use auth::UserRole;
use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use sqlx::Executor;
use tower::ServiceExt;

async fn setup(pool: &PgPool) -> Uuid {
    pool.execute(
        "CREATE TABLE workspaces (
            id UUID PRIMARY KEY, created_at TIMESTAMPTZ DEFAULT NOW(),
            trading_wallet_address TEXT, live_trading_enabled BOOLEAN DEFAULT false,
            exit_handler_enabled BOOLEAN DEFAULT false, arb_auto_execute BOOLEAN DEFAULT false
        );
        CREATE TABLE workspace_members (
            workspace_id UUID REFERENCES workspaces(id), user_id UUID, role TEXT,
            PRIMARY KEY (workspace_id, user_id)
        );
        CREATE TABLE trading_actions (id UUID)",
    )
    .await
    .unwrap();
    let workspace = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, live_trading_enabled) VALUES ($1, true)")
        .bind(workspace)
        .execute(pool)
        .await
        .unwrap();
    workspace
}

async fn add_member(pool: &PgPool, workspace: Uuid, role: &str) -> Uuid {
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO workspace_members VALUES ($1, $2, $3)")
        .bind(workspace)
        .bind(user)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    user
}

// The same typed guards required by all shared position/order handlers. A denied
// request must never reach the handler (represented here by a database write).
fn router(pool: &PgPool) -> Router {
    Router::new()
        .route(
            "/book",
            get(|_: WorkspaceReadAccess| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/trade",
            post(
                |_: WorkspaceTradeAccess, State(state): State<WorkspaceAccessState>| async move {
                    sqlx::query("INSERT INTO trading_actions VALUES ($1)")
                        .bind(Uuid::new_v4())
                        .execute(&state.0)
                        .await
                        .unwrap();
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(WorkspaceAccessState(pool.clone()))
}

async fn request(pool: &PgPool, path: &str, claims: Option<Claims>) -> StatusCode {
    let mut req = Request::builder()
        .method(if path == "/trade" { "POST" } else { "GET" })
        .uri(path)
        .body(Body::empty())
        .unwrap();
    if let Some(claims) = claims {
        req.extensions_mut().insert(claims);
    }
    router(pool).oneshot(req).await.unwrap().status()
}

fn claims(user: Uuid, role: UserRole) -> Option<Claims> {
    Some(Claims::new(user.to_string(), role, 1))
}

async fn action_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM trading_actions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn nonmembers_cannot_read_or_trade_even_with_platform_privileges(pool: PgPool) {
    setup(&pool).await;
    for role in [UserRole::Viewer, UserRole::Trader, UserRole::PlatformAdmin] {
        let user = Uuid::new_v4();
        for path in ["/book", "/trade"] {
            assert_eq!(
                request(&pool, path, claims(user, role)).await,
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(action_count(&pool).await, 0);
}

#[sqlx::test(migrations = false)]
async fn workspace_roles_allow_reads_but_viewers_cannot_trade(pool: PgPool) {
    let workspace = setup(&pool).await;
    for role in ["owner", "admin", "member", "viewer"] {
        let user = add_member(&pool, workspace, role).await;
        assert_eq!(
            request(&pool, "/book", claims(user, UserRole::Trader)).await,
            StatusCode::NO_CONTENT
        );
        let expected = if role == "viewer" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::NO_CONTENT
        };
        assert_eq!(
            request(&pool, "/trade", claims(user, UserRole::Trader)).await,
            expected
        );
    }
    assert_eq!(action_count(&pool).await, 3);
}

#[sqlx::test(migrations = false)]
async fn workspace_ownership_does_not_bypass_platform_read_only_role(pool: PgPool) {
    let workspace = setup(&pool).await;
    let owner = add_member(&pool, workspace, "owner").await;
    assert_eq!(
        request(&pool, "/book", claims(owner, UserRole::Viewer)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&pool, "/trade", claims(owner, UserRole::Viewer)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(action_count(&pool).await, 0);
}

#[sqlx::test(migrations = false)]
async fn membership_in_a_different_workspace_does_not_grant_bot_access(pool: PgPool) {
    setup(&pool).await;
    let other_workspace = Uuid::new_v4();
    // An older unrelated workspace must not win over the active trading workspace.
    sqlx::query("INSERT INTO workspaces (id, created_at) VALUES ($1, NOW() - INTERVAL '1 day')")
        .bind(other_workspace)
        .execute(&pool)
        .await
        .unwrap();
    let user = add_member(&pool, other_workspace, "owner").await;
    for path in ["/book", "/trade"] {
        assert_eq!(
            request(&pool, path, claims(user, UserRole::Trader)).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(action_count(&pool).await, 0);
}

#[sqlx::test(migrations = false)]
async fn permission_changes_apply_to_existing_claims_on_the_next_request(pool: PgPool) {
    let workspace = setup(&pool).await;
    let user = add_member(&pool, workspace, "member").await;
    let token_claims = claims(user, UserRole::Trader);
    assert_eq!(
        request(&pool, "/trade", token_claims.clone()).await,
        StatusCode::NO_CONTENT
    );
    sqlx::query("UPDATE workspace_members SET role = 'viewer' WHERE user_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        request(&pool, "/trade", token_claims.clone()).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&pool, "/book", token_claims.clone()).await,
        StatusCode::NO_CONTENT
    );
    sqlx::query("DELETE FROM workspace_members WHERE user_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    for path in ["/book", "/trade"] {
        assert_eq!(
            request(&pool, path, token_claims.clone()).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(action_count(&pool).await, 1);
}

#[sqlx::test(migrations = false)]
async fn missing_or_invalid_identity_and_unknown_roles_fail_closed(pool: PgPool) {
    let workspace = setup(&pool).await;
    let user = add_member(&pool, workspace, "unexpected_role").await;
    for path in ["/book", "/trade"] {
        assert_eq!(request(&pool, path, None).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            request(
                &pool,
                path,
                Some(Claims::new("invalid", UserRole::Trader, 1))
            )
            .await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&pool, path, claims(user, UserRole::Trader)).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(action_count(&pool).await, 0);
}

#[sqlx::test(migrations = false)]
async fn no_workspace_grants_no_access(pool: PgPool) {
    setup(&pool).await;
    pool.execute("DELETE FROM workspaces").await.unwrap();
    let user = Uuid::new_v4();
    for path in ["/book", "/trade"] {
        assert_eq!(
            request(&pool, path, claims(user, UserRole::PlatformAdmin)).await,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(action_count(&pool).await, 0);
}

/// The vault and the routes that fetch from the exchange on a miss spend the
/// workspace's shared resources, so an account outside the workspace (any
/// self-registered one) is refused before any of that work.
#[sqlx::test(migrations = false)]
async fn vault_and_exchange_fetching_routes_need_workspace_membership(pool: PgPool) {
    use crate::workspace_scope::test_fixture as fx;
    fx::real_schema(&pool).await;
    pool.execute(include_str!("../../../../migrations/008_user_wallets.sql"))
        .await
        .unwrap();
    let state = crate::state::AppState::for_tests(pool.clone());
    let app = crate::routes::create_router(state.clone());
    let workspace = fx::workspace(&pool, "trading", 0).await;
    let outsider = fx::user(&pool, UserRole::Trader).await;
    let viewer = fx::member(&pool, workspace, "viewer", UserRole::Trader).await;
    let member = fx::member(&pool, workspace, "member", UserRole::Trader).await;
    let outsider_token = fx::token(&state, outsider, UserRole::Trader).await;
    let viewer_token = fx::token(&state, viewer, UserRole::Trader).await;
    let member_token = fx::token(&state, member, UserRole::Trader).await;
    // A malformed address: a request that gets past the guard is refused by
    // validation, before the vault is touched.
    let store = serde_json::json!({"address": "not-an-address", "private_key": "0x00"});

    for (method, uri, body) in [
        ("POST", "/api/v1/vault/wallets", Some(store.clone())),
        ("GET", "/api/v1/vault/wallets/primary/balance", None),
        ("GET", "/api/v1/markets/unknown-market", None),
        ("GET", "/api/v1/markets/unknown-market/orderbook", None),
        ("GET", "/api/v1/wallets/0xabc/trades", None),
    ] {
        let (status, body_out) = fx::call(&app, method, uri, Some(&outsider_token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body_out}");
    }

    // Storing a signing key needs trading rights in the workspace.
    let (status, _) = fx::call(
        &app,
        "POST",
        "/api/v1/vault/wallets",
        Some(&viewer_token),
        Some(store.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = fx::call(
        &app,
        "POST",
        "/api/v1/vault/wallets",
        Some(&member_token),
        Some(store),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Any member may read a balance (none connected here).
    for token in [&viewer_token, &member_token] {
        let (status, body) = fx::call(
            &app,
            "GET",
            "/api/v1/vault/wallets/primary/balance",
            Some(token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 0);
}
