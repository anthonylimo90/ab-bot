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
