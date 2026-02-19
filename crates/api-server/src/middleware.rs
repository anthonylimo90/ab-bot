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
    let claims = match state.jwt_auth.validate_token(token) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "Token validation failed");
            return unauthorized_response("Invalid or expired token");
        }
    };

    // Log successful authentication
    tracing::debug!(user_id = %claims.sub, role = ?claims.role, "Authenticated request");

    // Sync RBAC roles with JWT role
    // This ensures RBAC permissions are available for fine-grained checks
    let rbac_role = match claims.role {
        UserRole::Viewer => "viewer",
        UserRole::Trader => "trader",
        UserRole::PlatformAdmin => "platform_admin",
    };

    // Assign the role to the user in RBAC (idempotent operation)
    let _ = state.rbac.assign_role(&claims.sub, rbac_role).await;

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
