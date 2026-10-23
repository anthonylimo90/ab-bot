//! Authentication middleware for API routes.

use axum::{
    body::Body,
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;

use auth::jwt::{Claims, UserRole};

use crate::error::ErrorResponse;
use crate::state::AppState;

#[path = "middleware/rate_limit.rs"]
pub mod rate_limit;
#[cfg(test)]
#[path = "middleware/session_tests.rs"]
mod session_tests;

fn role_number(role: UserRole) -> i32 {
    match role {
        UserRole::Viewer => 0,
        UserRole::Trader => 1,
        UserRole::PlatformAdmin => 2,
    }
}

/// Claims for a freshly authenticated session. `expiry_hours` must come from the
/// configured `JwtAuth` (`jwt.expiry_hours()`), not a literal.
pub(crate) async fn session_claims(
    pool: &sqlx::PgPool,
    user: uuid::Uuid,
    role: UserRole,
    expiry_hours: i64,
) -> crate::error::ApiResult<Claims> {
    let current: Option<(i64, i32)> =
        sqlx::query_as("SELECT auth_version, role::integer FROM users WHERE id=$1")
            .bind(user)
            .fetch_optional(pool)
            .await?;
    let (version, current_role) = current
        .ok_or_else(|| crate::error::ApiError::Unauthorized("Account no longer exists".into()))?;
    if current_role != role_number(role) {
        return Err(crate::error::ApiError::Unauthorized(
            "Account role changed".into(),
        ));
    }
    let mut claims = Claims::new(user.to_string(), role, expiry_hours);
    claims.session_version = version;
    Ok(claims)
}

pub(crate) async fn issue_session_token(
    state: &AppState,
    user: uuid::Uuid,
    role: UserRole,
) -> crate::error::ApiResult<String> {
    issue_session_token_since(state, user, role, None).await
}

/// How long a session may be renewed from its sign-in before the user must
/// sign in again.
pub(crate) const MAX_SESSION_DAYS: i64 = 7;

/// A token for a session that began at `session_started_at` (now, when
/// `None`): a refresh carries its session's start forward.
pub(crate) async fn issue_session_token_since(
    state: &AppState,
    user: uuid::Uuid,
    role: UserRole,
    session_started_at: Option<i64>,
) -> crate::error::ApiResult<String> {
    let mut claims = session_claims(&state.pool, user, role, state.jwt_auth.expiry_hours()).await?;
    if let Some(started) = session_started_at {
        claims.auth_time = started;
    }
    state
        .jwt_auth
        .create_token_with_claims(&claims)
        .map_err(|_| crate::error::ApiError::Internal("Token generation failed".into()))
}

pub(crate) async fn authorize_session(
    pool: &sqlx::PgPool,
    jwt: &auth::jwt::JwtAuth,
    token: &str,
) -> crate::error::ApiResult<Claims> {
    let claims = validate_session_token(jwt, token)?;
    let user = uuid::Uuid::parse_str(&claims.sub)
        .map_err(|_| crate::error::ApiError::Unauthorized("Invalid subject".into()))?;
    let current: Option<(i64,i32,bool)> = sqlx::query_as("SELECT auth_version, role::integer, EXISTS(SELECT 1 FROM revoked_sessions WHERE jti=$2) FROM users WHERE id=$1")
        .bind(user).bind(&claims.jti).fetch_optional(pool).await?;
    require_current_session(&claims, current)?;
    Ok(claims)
}

pub(crate) fn validate_session_token(
    jwt: &auth::jwt::JwtAuth,
    token: &str,
) -> crate::error::ApiResult<Claims> {
    use crate::error::ApiError;
    let claims = jwt
        .validate_token(token)
        .map_err(|_| ApiError::Unauthorized("Invalid or expired token".into()))?;
    if claims.exp <= chrono::Utc::now().timestamp() {
        return Err(ApiError::Unauthorized("Token expired".into()));
    }
    Ok(claims)
}

pub(crate) fn require_current_session(
    claims: &Claims,
    current: Option<(i64, i32, bool)>,
) -> crate::error::ApiResult<()> {
    use crate::error::ApiError;
    match current {
        Some((version, role, false))
            if version == claims.session_version && role == role_number(claims.role) =>
        {
            Ok(())
        }
        _ => Err(ApiError::Unauthorized(
            "Session revoked or account changed".into(),
        )),
    }
}

/// Extract and validate JWT token from Authorization header.
/// On success, injects `Claims` into request extensions for use by handlers.
pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // Extract Authorization header
    let auth_header = match request.headers().get(AUTHORIZATION) {
        Some(header) => match header.to_str() {
            Ok(s) => s,
            Err(_) => {
                return unauthorized_response("Invalid authorization header encoding");
            }
        },
        None => {
            return unauthorized_response("Missing authorization header");
        }
    };

    // Check for Bearer prefix
    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t,
        None => {
            return unauthorized_response(
                "Invalid authorization format, expected 'Bearer <token>'",
            );
        }
    };

    // Validate token
    let claims = match authorize_session(&state.pool, &state.jwt_auth, token).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "Token validation failed");
            return e.into_response();
        }
    };

    // Log successful authentication
    tracing::debug!(user_id = %claims.sub, role = ?claims.role, "Authenticated request");

    // Authorization decisions use the validated claims and the database state
    // that `authorize_session` just re-checked.

    // Inject claims into request extensions
    request.extensions_mut().insert(claims);

    next.run(request).await
}

/// Middleware that requires trader role (Trader or Admin).
/// Must be applied AFTER `require_auth` middleware.
pub async fn require_trader(
    State(_state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Get claims from extensions (set by require_auth)
    let claims = match request.extensions().get::<Claims>() {
        Some(c) => c,
        None => {
            // This shouldn't happen if require_auth runs first
            return unauthorized_response("Not authenticated");
        }
    };

    // Check trader permission
    if !claims.role.can_trade() {
        return forbidden_response("Trader or Admin role required to execute trades");
    }

    next.run(request).await
}

/// Middleware that requires admin role.
/// Must be applied AFTER `require_auth` middleware.
pub async fn require_admin(
    State(_state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Get claims from extensions (set by require_auth)
    let claims = match request.extensions().get::<Claims>() {
        Some(c) => c,
        None => {
            return unauthorized_response("Not authenticated");
        }
    };

    // Check admin permission
    if !claims.role.can_configure() {
        return forbidden_response("Admin role required");
    }

    next.run(request).await
}

/// Helper to create an unauthorized (401) response.
fn unauthorized_response(message: &str) -> Response {
    let body = ErrorResponse::new("UNAUTHORIZED", message);
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

/// Helper to create a forbidden (403) response.
fn forbidden_response(message: &str) -> Response {
    let body = ErrorResponse::new("FORBIDDEN", message);
    (StatusCode::FORBIDDEN, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use auth::jwt::{JwtAuth, JwtConfig, UserRole};
    use axum::http::StatusCode;

    fn create_test_jwt_auth() -> JwtAuth {
        JwtAuth::new(JwtConfig {
            secret: "test-secret-key-12345".to_string(),
            expiry_hours: 1,
            ..Default::default()
        })
    }

    // Note: Full integration tests require a database connection
    // These tests verify the middleware logic with mocked dependencies

    #[test]
    fn test_unauthorized_response() {
        let response = unauthorized_response("Test message");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_forbidden_response() {
        let response = forbidden_response("Test message");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_jwt_token_creation() {
        let auth = create_test_jwt_auth();
        let token = auth.create_token("user123", UserRole::Trader).unwrap();
        assert!(!token.is_empty());

        let claims = auth.validate_token(&token).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.role, UserRole::Trader);
    }
}
