use crate::{
    handlers::{activity, auth, signals, strategy_health, trade_flow, workspaces},
    middleware,
    state::AppState,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use sqlx::{Executor, PgPool};
use tower::ServiceExt;
use uuid::Uuid;

async fn setup(pool: &PgPool) -> (Uuid, Uuid) {
    pool.execute("CREATE TABLE users(id UUID PRIMARY KEY,role SMALLINT); CREATE TABLE workspaces(id UUID PRIMARY KEY,created_at TIMESTAMPTZ DEFAULT NOW(),live_trading_enabled BOOLEAN DEFAULT false,exit_handler_enabled BOOLEAN DEFAULT false,arb_auto_execute BOOLEAN DEFAULT false,trading_wallet_address TEXT); CREATE TABLE workspace_members(workspace_id UUID,user_id UUID,role TEXT)").await.unwrap();
    pool.execute(include_str!(
        "../../../migrations/072_persistent_sessions.sql"
    ))
    .await
    .unwrap();
    let user = Uuid::new_v4();
    let workspace = Uuid::new_v4();
    sqlx::query("INSERT INTO users VALUES($1,1,0)")
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
        .bind(workspace)
        .execute(pool)
        .await
        .unwrap();
    (user, workspace)
}
fn router(state: std::sync::Arc<AppState>) -> Router {
    Router::new()
        .route("/activity", get(activity::list_activity))
        .route("/signals", get(signals::get_recent_signals))
        .route("/performance", get(signals::get_strategy_performance))
        .route("/health", get(strategy_health::get_strategy_health))
        .route("/summary", get(trade_flow::get_trade_flow_summary))
        .route("/journeys", get(trade_flow::get_trade_flow_journeys))
        .route("/telemetry", get(trade_flow::get_arb_execution_telemetry))
        .route("/learning", get(trade_flow::get_learning_overview))
        .route("/logout", post(auth::logout))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::require_auth,
        ))
        .route("/invites/:token/accept", post(workspaces::accept_invite))
        .with_state(state)
}
async fn request(app: Router, path: &str, token: &str, post: bool) -> StatusCode {
    app.oneshot(
        Request::builder()
            .uri(path)
            .method(if post { "POST" } else { "GET" })
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}
#[sqlx::test(migrations = false)]
async fn private_projection_endpoints_recheck_canonical_membership(pool: PgPool) {
    let (user, workspace) = setup(&pool).await;
    let state = AppState::for_tests(pool.clone());
    let token = middleware::issue_session_token(&state, user, ::auth::UserRole::Trader)
        .await
        .unwrap();
    for path in [
        "/activity",
        "/signals",
        "/performance",
        "/health",
        "/summary",
        "/journeys",
        "/telemetry",
        "/learning",
    ] {
        assert_eq!(
            request(router(state.clone()), path, &token, false).await,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    sqlx::query("INSERT INTO workspace_members VALUES($1,$2,'member')")
        .bind(workspace)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_ne!(
        request(router(state.clone()), "/activity", &token, false).await,
        StatusCode::FORBIDDEN
    );
    pool.execute("DELETE FROM workspace_members").await.unwrap();
    assert_eq!(
        request(router(state), "/activity", &token, false).await,
        StatusCode::FORBIDDEN
    );
}
#[sqlx::test(migrations = false)]
async fn logout_and_password_reset_tokens_cannot_accept_invites_on_another_instance(pool: PgPool) {
    let (user, _) = setup(&pool).await;
    let first = AppState::for_tests(pool.clone());
    let second = AppState::for_tests(pool.clone());
    let token = middleware::issue_session_token(&first, user, ::auth::UserRole::Trader)
        .await
        .unwrap();
    assert_eq!(
        request(router(first.clone()), "/logout", &token, true).await,
        StatusCode::OK
    );
    assert_eq!(
        request(router(second.clone()), "/activity", &token, false).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            router(second.clone()),
            "/invites/invite/accept",
            &token,
            true
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
    let fresh = middleware::issue_session_token(&first, user, ::auth::UserRole::Trader)
        .await
        .unwrap();
    pool.execute("UPDATE users SET auth_version=auth_version+1")
        .await
        .unwrap();
    assert_eq!(
        request(router(second), "/invites/invite/accept", &fresh, true).await,
        StatusCode::UNAUTHORIZED
    );
}
