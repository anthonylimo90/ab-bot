//! Authentication handlers for user registration and login.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::jwt::Claims;
use auth::{AuditAction, AuditEvent, UserRole};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// User registration request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    /// Email address.
    pub email: String,
    /// Password (min 8 characters).
    pub password: String,
    /// Display name (optional).
    #[serde(default)]
    pub name: Option<String>,
}

/// User login request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    /// Email address.
    pub email: String,
    /// Password.
    pub password: String,
}

/// Authentication response with token and user info.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthResponse {
    /// JWT access token.
    pub token: String,
    /// User information.
    pub user: UserInfo,
}

/// User information.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserInfo {
    /// User ID.
    pub id: String,
    /// Email address.
    pub email: String,
    /// Display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User role.
    pub role: String,
    /// Account creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Forgot password request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ForgotPasswordRequest {
    /// Email address.
    pub email: String,
}

/// Forgot password response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ForgotPasswordResponse {
    /// Response message.
    pub message: String,
}

/// Reset password request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    /// Password reset token.
    pub token: String,
    /// New password (min 8 characters).
    pub password: String,
}

/// Reset password response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ResetPasswordResponse {
    /// Response message.
    pub message: String,
}

/// Database row for user.
#[derive(Debug, sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    password_hash: String,
    role: i16,
    name: Option<String>,
    created_at: DateTime<Utc>,
}

impl UserRow {
    fn role_string(&self) -> String {
        match self.role {
            0 => "Viewer".to_string(),
            1 => "Trader".to_string(),
            2 => "Admin".to_string(),
            _ => "Viewer".to_string(),
        }
    }

    fn user_role(&self) -> UserRole {
        match self.role {
            0 => UserRole::Viewer,
            1 => UserRole::Trader,
            2 => UserRole::Admin,
            _ => UserRole::Viewer,
        }
    }
}

/// Register a new user account.
#[utoipa::path(
    post,
    path = "/api/v1/auth/register",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "User registered successfully", body = AuthResponse),
        (status = 400, description = "Invalid request"),
        (status = 409, description = "Email already registered"),
    ),
    tag = "auth"
)]
pub async fn register(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<RegisterRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<AuthResponse>)> {
    // Handle JSON parsing errors explicitly
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Register request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    // Validate email format
    if !req.email.contains('@') || req.email.len() < 5 {
        return Err(ApiError::BadRequest("Invalid email address".into()));
    }

    // Validate password length
    if req.password.len() < 8 {
        return Err(ApiError::BadRequest(
            "Password must be at least 8 characters".into(),
        ));
    }

    // Check if email already exists
    let existing: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE email = $1")
        .bind(&req.email)
        .fetch_optional(&state.pool)
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict("Email already registered".into()));
    }

    // Hash the password
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(req.password.as_bytes(), &salt)
        .map_err(|e| ApiError::Internal(format!("Password hashing failed: {}", e)))?
        .to_string();

    // Create user
    let user_id = Uuid::new_v4();
    let role: i16 = 1; // Trader by default
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO users (id, email, password_hash, role, name, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $6)
        "#,
    )
    .bind(user_id)
    .bind(&req.email)
    .bind(&password_hash)
    .bind(role)
    .bind(&req.name)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Log user registration
    let audit_event = AuditEvent::builder(AuditAction::UserCreated, format!("user/{}", user_id))
        .user(user_id.to_string())
        .details(serde_json::json!({
            "email": &req.email,
            "role": "Trader",
            "source": "self_registration"
        }))
        .build();
    state.audit_logger.log(audit_event);

    // Generate JWT token
    let token = state
        .jwt_auth
        .create_token(&user_id.to_string(), UserRole::Trader)
        .map_err(|e| ApiError::Internal(format!("Token generation failed: {}", e)))?;

    let response = AuthResponse {
        token,
        user: UserInfo {
            id: user_id.to_string(),
            email: req.email,
            name: req.name,
            role: "Trader".to_string(),
            created_at: now,
        },
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Login with email and password.
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = AuthResponse),
        (status = 401, description = "Invalid credentials"),
    ),
    tag = "auth"
)]
pub async fn login(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> ApiResult<Json<AuthResponse>> {
    // Handle JSON parsing errors explicitly
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Login request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    // Find user by email
    let user: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT id, email, password_hash, role, name, created_at
        FROM users
        WHERE email = $1
        "#,
    )
    .bind(&req.email)
    .fetch_optional(&state.pool)
    .await?;

    let user = match user {
        Some(u) => u,
        None => {
            // Log failed login attempt (user not found)
            state.audit_logger.log_login(&req.email, None, false);
            return Err(ApiError::Unauthorized("Invalid credentials".into()));
        }
    };

    // Verify password
    let parsed_hash = PasswordHash::new(&user.password_hash)
        .map_err(|_| ApiError::Internal("Invalid password hash in database".into()))?;

    if Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        // Log failed login attempt (wrong password)
        state
            .audit_logger
            .log_login(&user.id.to_string(), None, false);
        return Err(ApiError::Unauthorized("Invalid credentials".into()));
    }

    // Update last login
    let _ = sqlx::query("UPDATE users SET last_login = $1 WHERE id = $2")
        .bind(Utc::now())
        .bind(user.id)
        .execute(&state.pool)
        .await;

    // Log successful login
    state
        .audit_logger
        .log_login(&user.id.to_string(), None, true);

    // Compute role before moving user fields
    let role = user.role_string();
    let user_role = user.user_role();

    // Generate JWT token
    let token = state
        .jwt_auth
        .create_token(&user.id.to_string(), user_role)
        .map_err(|e| ApiError::Internal(format!("Token generation failed: {}", e)))?;

    let response = AuthResponse {
        token,
        user: UserInfo {
            id: user.id.to_string(),
            email: user.email,
            name: user.name,
            role,
            created_at: user.created_at,
        },
    };

    Ok(Json(response))
}

/// Refresh the current JWT token.
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    responses(
        (status = 200, description = "Token refreshed", body = AuthResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "auth"
)]
pub async fn refresh_token(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<AuthResponse>> {
    // Get user from database
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let user: UserRow = sqlx::query_as(
        r#"
        SELECT id, email, password_hash, role, name, created_at
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("User not found".into()))?;

    // Compute role before moving user fields
    let role = user.role_string();
    let user_role = user.user_role();

    // Generate new token
    let token = state
        .jwt_auth
        .create_token(&user.id.to_string(), user_role)
        .map_err(|e| ApiError::Internal(format!("Token generation failed: {}", e)))?;

    let response = AuthResponse {
        token,
        user: UserInfo {
            id: user.id.to_string(),
            email: user.email,
            name: user.name,
            role,
            created_at: user.created_at,
        },
    };

    Ok(Json(response))
}

/// Get the current authenticated user's information.
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    responses(
        (status = 200, description = "Current user info", body = UserInfo),
        (status = 401, description = "Unauthorized"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "auth"
)]
pub async fn get_current_user(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<UserInfo>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let user: UserRow = sqlx::query_as(
        r#"
        SELECT id, email, password_hash, role, name, created_at
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("User not found".into()))?;

    // Compute role before moving user fields
    let role = user.role_string();

    Ok(Json(UserInfo {
        id: user.id.to_string(),
        email: user.email,
        name: user.name,
        role,
        created_at: user.created_at,
    }))
}

/// Generate a secure random token for password reset.
fn generate_reset_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.gen();
    hex::encode(bytes)
}

/// Hash a token using SHA256.
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Request a password reset email.
#[utoipa::path(
    post,
    path = "/api/v1/auth/forgot-password",
    request_body = ForgotPasswordRequest,
    responses(
        (status = 200, description = "Reset email sent if account exists", body = ForgotPasswordResponse),
        (status = 400, description = "Invalid request"),
        (status = 429, description = "Too many requests"),
    ),
    tag = "auth"
)]
pub async fn forgot_password(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<ForgotPasswordRequest>, JsonRejection>,
) -> ApiResult<Json<ForgotPasswordResponse>> {
    // Handle JSON parsing errors explicitly
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Forgot password request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    // Validate email format
    if !req.email.contains('@') || req.email.len() < 5 {
        return Err(ApiError::BadRequest("Invalid email address".into()));
    }

    // Always return success to prevent email enumeration
    let response = ForgotPasswordResponse {
        message: "If an account exists with this email, a password reset link has been sent."
            .to_string(),
    };

    // Find user by email
    let user: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT id, email, password_hash, role, name, created_at
        FROM users
        WHERE email = $1
        "#,
    )
    .bind(&req.email)
    .fetch_optional(&state.pool)
    .await?;

    let user = match user {
        Some(u) => u,
        None => {
            // Log attempt but don't reveal user existence
            tracing::info!(email = %req.email, "Password reset requested for non-existent email");
            return Ok(Json(response));
        }
    };

    // Check rate limiting: max 3 reset requests per email per hour
    let recent_count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM password_reset_tokens
        WHERE user_id = $1 AND created_at > NOW() - INTERVAL '1 hour'
        "#,
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    if recent_count.0 >= 3 {
        tracing::warn!(user_id = %user.id, "Password reset rate limit exceeded");
        // Still return success to prevent enumeration
        return Ok(Json(response));
    }

    // Generate token and hash
    let token = generate_reset_token();
    let token_hash = hash_token(&token);
    let expires_at = Utc::now() + Duration::hours(1);

    // Store token hash in database
    sqlx::query(
        r#"
        INSERT INTO password_reset_tokens (user_id, token_hash, expires_at)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(user.id)
    .bind(&token_hash)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    // Log password reset request
    let audit_event = AuditEvent::builder(
        AuditAction::Custom("password_reset_requested".to_string()),
        format!("user/{}", user.id),
    )
    .user(user.id.to_string())
    .details(serde_json::json!({
        "email": &req.email,
    }))
    .build();
    state.audit_logger.log(audit_event);

    // Send email if email client is configured
    if let Some(email_client) = &state.email_client {
        if let Err(e) = email_client.send_password_reset(&req.email, &token).await {
            tracing::error!(error = %e, "Failed to send password reset email");
            // Don't fail the request, just log the error
        } else {
            tracing::info!(user_id = %user.id, "Password reset email sent");
        }
    } else {
        tracing::warn!("Email client not configured, password reset token generated but not sent");
        // In development, log the token for testing
        if std::env::var("ENVIRONMENT").unwrap_or_default() == "development" {
            tracing::info!(token = %token, "Development mode: password reset token");
        }
    }

    Ok(Json(response))
}

/// Reset password using a token.
#[utoipa::path(
    post,
    path = "/api/v1/auth/reset-password",
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, description = "Password updated successfully", body = ResetPasswordResponse),
        (status = 400, description = "Invalid request or token"),
    ),
    tag = "auth"
)]
pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<ResetPasswordRequest>, JsonRejection>,
) -> ApiResult<Json<ResetPasswordResponse>> {
    // Handle JSON parsing errors explicitly
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Reset password request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    // Validate password length
    if req.password.len() < 8 {
        return Err(ApiError::BadRequest(
            "Password must be at least 8 characters".into(),
        ));
    }

    // Hash the provided token
    let token_hash = hash_token(&req.token);

    // Find valid token
    #[derive(sqlx::FromRow)]
    struct ResetTokenRow {
        id: Uuid,
        user_id: Uuid,
    }

    let token_row: Option<ResetTokenRow> = sqlx::query_as(
        r#"
        SELECT id, user_id FROM password_reset_tokens
        WHERE token_hash = $1 AND expires_at > NOW() AND used_at IS NULL
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(&state.pool)
    .await?;

    let token_row = match token_row {
        Some(t) => t,
        None => {
            tracing::warn!("Invalid or expired password reset token used");
            return Err(ApiError::BadRequest(
                "Invalid or expired reset token".into(),
            ));
        }
    };

    // Hash the new password
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(req.password.as_bytes(), &salt)
        .map_err(|e| ApiError::Internal(format!("Password hashing failed: {}", e)))?
        .to_string();

    // Update password and mark token as used in a transaction
    let mut tx = state.pool.begin().await?;

    sqlx::query("UPDATE users SET password_hash = $1, updated_at = $2 WHERE id = $3")
        .bind(&password_hash)
        .bind(Utc::now())
        .bind(token_row.user_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("UPDATE password_reset_tokens SET used_at = $1 WHERE id = $2")
        .bind(Utc::now())
        .bind(token_row.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Log password reset
    let audit_event = AuditEvent::builder(
        AuditAction::Custom("password_reset_completed".to_string()),
        format!("user/{}", token_row.user_id),
    )
    .user(token_row.user_id.to_string())
    .build();
    state.audit_logger.log(audit_event);

    tracing::info!(user_id = %token_row.user_id, "Password reset completed");

    Ok(Json(ResetPasswordResponse {
        message: "Password updated successfully".to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing() {
        let password = "testpassword123";
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();

        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .unwrap()
            .to_string();

        let parsed_hash = PasswordHash::new(&hash).unwrap();
        assert!(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok());
        assert!(argon2
            .verify_password(b"wrongpassword", &parsed_hash)
            .is_err());
    }
}
