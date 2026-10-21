//! Shared database fixtures for security regression tests.
//!
//! Applies the real migrations that define users, sessions and workspaces so
//! tests exercise the production schema (constraints included) instead of a
//! hand-written approximation. Only migrations that run on plain PostgreSQL
//! (no TimescaleDB) are included.

use auth::UserRole;
use sqlx::{Executor, PgPool};
use uuid::Uuid;

/// Real migrations, in order, that define the user/workspace/session schema.
const MIGRATIONS: &[&str] = &[
    include_str!("../../../../migrations/001_initial_schema.sql"),
    include_str!("../../../../migrations/002_trading_tables.sql"),
    include_str!("../../../../migrations/007_add_user_name.sql"),
    include_str!("../../../../migrations/010_password_reset_tokens.sql"),
    include_str!("../../../../migrations/011_user_onboarding.sql"),
    include_str!("../../../../migrations/012_allow_owner_invite_role.sql"),
    include_str!("../../../../migrations/013_optimizer_status.sql"),
    include_str!("../../../../migrations/015_wallet_automation.sql"),
    include_str!("../../../../migrations/016_wallet_authentication.sql"),
    include_str!("../../../../migrations/017_workspace_walletconnect.sql"),
    include_str!("../../../../migrations/019_auth_challenge_cleanup.sql"),
    include_str!("../../../../migrations/022_workspace_trading_config.sql"),
    include_str!("../../../../migrations/029_dynamic_tuning.sql"),
    include_str!("../../../../migrations/030_dynamic_tuner_state.sql"),
    include_str!("../../../../migrations/031_dynamic_config_history_actions.sql"),
    include_str!("../../../../migrations/032_workspace_cb_config.sql"),
    include_str!("../../../../migrations/033_workspace_exit_handler.sql"),
    include_str!("../../../../migrations/072_persistent_sessions.sql"),
];

/// Create the production user/workspace/session schema in an empty test database.
pub(crate) async fn real_schema(pool: &PgPool) {
    for migration in MIGRATIONS {
        pool.execute(*migration).await.unwrap();
    }
}

fn role_number(role: UserRole) -> i16 {
    match role {
        UserRole::Viewer => 0,
        UserRole::Trader => 1,
        UserRole::PlatformAdmin => 2,
    }
}

/// Insert an email/password user with the given platform role.
pub(crate) async fn user(pool: &PgPool, role: UserRole) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, 'unused', $3)")
        .bind(id)
        .bind(format!("{id}@example.test"))
        .bind(role_number(role))
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Insert a workspace with every runtime flag off, created `age_secs` ago so
/// tests control the canonical tie-break order (older wins).
pub(crate) async fn workspace(pool: &PgPool, name: &str, age_secs: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workspaces (id, name, created_at) VALUES ($1, $2, NOW() - make_interval(secs => $3))",
    )
    .bind(id)
    .bind(name)
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .unwrap();
    id
}

pub(crate) async fn add_member(pool: &PgPool, workspace: Uuid, user: Uuid, role: &str) {
    sqlx::query("INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(workspace)
        .bind(user)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
}

/// Insert a user and make them a member of `workspace` with `role`.
pub(crate) async fn member(
    pool: &PgPool,
    workspace: Uuid,
    role: &str,
    platform_role: UserRole,
) -> Uuid {
    let id = user(pool, platform_role).await;
    add_member(pool, workspace, id, role).await;
    id
}

/// Issue a persisted-session bearer token for `user` (platform role must match the row).
pub(crate) async fn token(state: &crate::state::AppState, user: Uuid, role: UserRole) -> String {
    crate::middleware::issue_session_token(state, user, role)
        .await
        .unwrap()
}

/// Send one request through `app` and return the status and JSON body
/// (`Value::Null` when the body is empty or not JSON).
pub(crate) async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .uri(uri)
        .method(method)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let body = body.map(|b| b.to_string()).unwrap_or_else(|| "{}".into());
    let response = app
        .clone()
        .oneshot(request.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// `(live_trading_enabled, exit_handler_enabled, arb_auto_execute)` of a workspace.
pub(crate) async fn flags(pool: &PgPool, workspace: Uuid) -> (bool, bool, bool) {
    sqlx::query_as(
        "SELECT COALESCE(live_trading_enabled, FALSE), COALESCE(exit_handler_enabled, FALSE), COALESCE(arb_auto_execute, FALSE) FROM workspaces WHERE id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .unwrap()
}
