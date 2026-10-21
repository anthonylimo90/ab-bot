use super::*;
use sqlx::Executor;

fn jwt() -> auth::jwt::JwtAuth {
    auth::jwt::JwtAuth::new(auth::jwt::JwtConfig {
        secret: "persistent-session-test-secret-long-enough".into(),
        issuer: None,
        audience: None,
        expiry_hours: 1,
    })
}

#[sqlx::test(migrations = false)]
async fn account_and_credential_changes_revoke_signed_tokens_across_instances(pool: sqlx::PgPool) {
    pool.execute("CREATE TABLE users (id UUID PRIMARY KEY,role SMALLINT)")
        .await
        .unwrap();
    pool.execute(include_str!(
        "../../../../migrations/072_persistent_sessions.sql"
    ))
    .await
    .unwrap();
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,role) VALUES ($1,2)")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let claims = session_claims(&pool, id, UserRole::PlatformAdmin)
        .await
        .unwrap();
    let token = jwt().create_token_with_claims(&claims).unwrap();
    authorize_session(&pool, &jwt(), &token).await.unwrap();
    sqlx::query("UPDATE users SET auth_version=auth_version+1 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(jwt().validate_token(&token).is_ok());
    assert!(authorize_session(&pool, &jwt(), &token).await.is_err());
    let current = session_claims(&pool, id, UserRole::PlatformAdmin)
        .await
        .unwrap();
    let token = jwt().create_token_with_claims(&current).unwrap();
    authorize_session(&pool, &jwt(), &token).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(authorize_session(&pool, &jwt(), &token).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn logout_and_role_changes_are_checked_in_persistent_state(pool: sqlx::PgPool) {
    pool.execute("CREATE TABLE users (id UUID PRIMARY KEY,role SMALLINT)")
        .await
        .unwrap();
    pool.execute(include_str!(
        "../../../../migrations/072_persistent_sessions.sql"
    ))
    .await
    .unwrap();
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,role) VALUES ($1,1)")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let claims = session_claims(&pool, id, UserRole::Trader).await.unwrap();
    let token = jwt().create_token_with_claims(&claims).unwrap();
    sqlx::query("INSERT INTO revoked_sessions VALUES ($1,$2,NOW()+INTERVAL '1 hour')")
        .bind(&claims.jti)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(authorize_session(&pool, &jwt(), &token).await.is_err());
    let fresh = session_claims(&pool, id, UserRole::Trader).await.unwrap();
    let token = jwt().create_token_with_claims(&fresh).unwrap();
    authorize_session(&pool, &jwt(), &token).await.unwrap();
    sqlx::query("UPDATE users SET role=0 WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(authorize_session(&pool, &jwt(), &token).await.is_err());
}

// `state.rbac` is not consulted for any authorization decision, so authenticating
// a request must not write to it (it used to append a duplicate role entry per
// request under a global write lock).
#[sqlx::test(migrations = false)]
async fn authenticated_requests_do_not_write_in_memory_role_state(pool: sqlx::PgPool) {
    use crate::workspace_scope::test_fixture as fx;
    fx::real_schema(&pool).await;
    let state = AppState::for_tests(pool.clone());
    let user = fx::user(&pool, UserRole::Trader).await;
    let token = fx::token(&state, user, UserRole::Trader).await;
    let app = axum::Router::new()
        .route("/ping", axum::routing::get(|| async { "ok" }))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ))
        .with_state(state.clone());
    for _ in 0..25 {
        let (status, _) = fx::call(&app, "GET", "/ping", Some(&token), None).await;
        assert_eq!(status, axum::http::StatusCode::OK);
    }
    assert!(state
        .rbac
        .get_user_roles(&user.to_string())
        .await
        .is_empty());
}
