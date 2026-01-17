//! User management handlers for admin operations.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::{AuditAction, Claims};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// User list response item.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserListItem {
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
    /// Last login timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_login: Option<DateTime<Utc>>,
}

/// Create user request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    /// Email address.
    pub email: String,
    /// Password (min 8 characters).
    pub password: String,
    /// Display name (optional).
    #[serde(default)]
    pub name: Option<String>,
    /// User role: "Viewer", "Trader", or "Admin".
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "Viewer".to_string()
}

/// Update user request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    /// New display name.
    #[serde(default)]
    pub name: Option<String>,
    /// New role: "Viewer", "Trader", or "Admin".
    #[serde(default)]
    pub role: Option<String>,
    /// New password (min 8 characters).
    #[serde(default)]
    pub password: Option<String>,
}

/// Database row for user list.
#[derive(Debug, sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    role: i16,
    name: Option<String>,
    created_at: DateTime<Utc>,
    last_login: Option<DateTime<Utc>>,
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
}

fn parse_role(role: &str) -> Result<i16, ApiError> {
    match role.to_lowercase().as_str() {
        "viewer" => Ok(0),
        "trader" => Ok(1),
        "admin" => Ok(2),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid role '{}'. Must be 'Viewer', 'Trader', or 'Admin'",
            role
        ))),
    }
}

/// List all users (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/users",
    responses(
        (status = 200, description = "List of all users", body = Vec<UserListItem>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "users"
)]
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<Vec<UserListItem>>> {
    let users: Vec<UserRow> = sqlx::query_as(
        r#"
        SELECT id, email, role, name, created_at, last_login
        FROM users
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let users: Vec<UserListItem> = users
        .into_iter()
        .map(|u| {
            let role = u.role_string();
            UserListItem {
                id: u.id.to_string(),
                email: u.email,
                name: u.name,
                role,
                created_at: u.created_at,
                last_login: u.last_login,
            }
        })
        .collect();

    // Log user list view
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::UserViewed,
        "list",
        serde_json::json!({ "count": users.len() }),
    );

    Ok(Json(users))
}

/// Create a new user (admin only).
#[utoipa::path(
    post,
    path = "/api/v1/users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created successfully", body = UserListItem),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 409, description = "Email already registered"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "users"
)]
pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<UserListItem>)> {
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

    // Parse role
    let role = parse_role(&req.role)?;

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

    // Log user creation by admin
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::UserCreated,
        &user_id.to_string(),
        serde_json::json!({
            "email": &req.email,
            "role": &req.role,
            "created_by": &claims.sub,
            "source": "admin_created"
        }),
    );

    let response = UserListItem {
        id: user_id.to_string(),
        email: req.email,
        name: req.name,
        role: req.role,
        created_at: now,
        last_login: None,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Get a specific user by ID (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/users/{user_id}",
    params(
        ("user_id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 200, description = "User details", body = UserListItem),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 404, description = "User not found"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "users"
)]
pub async fn get_user(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(user_id): Path<String>,
) -> ApiResult<Json<UserListItem>> {
    let user_id = Uuid::parse_str(&user_id)
        .map_err(|_| ApiError::BadRequest("Invalid user ID format".into()))?;

    let user: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT id, email, role, name, created_at, last_login
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    let user = user.ok_or_else(|| ApiError::NotFound("User not found".into()))?;

    // Log user view
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::UserViewed,
        &user_id.to_string(),
        serde_json::json!({ "email": &user.email }),
    );

    let role = user.role_string();
    Ok(Json(UserListItem {
        id: user.id.to_string(),
        email: user.email,
        name: user.name,
        role,
        created_at: user.created_at,
        last_login: user.last_login,
    }))
}

/// Update a user (admin only).
#[utoipa::path(
    patch,
    path = "/api/v1/users/{user_id}",
    params(
        ("user_id" = String, Path, description = "User ID")
    ),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "User updated", body = UserListItem),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 404, description = "User not found"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "users"
)]
pub async fn update_user(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(user_id): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> ApiResult<Json<UserListItem>> {
    let user_id = Uuid::parse_str(&user_id)
        .map_err(|_| ApiError::BadRequest("Invalid user ID format".into()))?;

    // Check if user exists
    let user: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT id, email, role, name, created_at, last_login
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    let user = user.ok_or_else(|| ApiError::NotFound("User not found".into()))?;

    // Build update query dynamically
    let mut updates = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(ref name) = req.name {
        updates.push(format!("name = ${}", params.len() + 2));
        params.push(name.clone());
    }

    if let Some(ref role) = req.role {
        let role_value = parse_role(role)?;
        updates.push(format!("role = ${}", params.len() + 2));
        params.push(role_value.to_string());
    }

    if let Some(ref password) = req.password {
        if password.len() < 8 {
            return Err(ApiError::BadRequest(
                "Password must be at least 8 characters".into(),
            ));
        }
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| ApiError::Internal(format!("Password hashing failed: {}", e)))?
            .to_string();
        updates.push(format!("password_hash = ${}", params.len() + 2));
        params.push(password_hash);
    }

    if updates.is_empty() {
        // Nothing to update, return current user
        let role = user.role_string();
        return Ok(Json(UserListItem {
            id: user.id.to_string(),
            email: user.email,
            name: user.name,
            role,
            created_at: user.created_at,
            last_login: user.last_login,
        }));
    }

    updates.push(format!("updated_at = ${}", params.len() + 2));
    let now = Utc::now();

    // Execute update with dynamic query
    let query = format!("UPDATE users SET {} WHERE id = $1", updates.join(", "));

    let mut query_builder = sqlx::query(&query).bind(user_id);
    for param in &params {
        query_builder = query_builder.bind(param);
    }
    query_builder = query_builder.bind(now);
    query_builder.execute(&state.pool).await?;

    // Log user update
    let mut changes = Vec::new();
    if req.name.is_some() {
        changes.push("name");
    }
    if req.role.is_some() {
        changes.push("role");
    }
    if req.password.is_some() {
        changes.push("password");
    }
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::UserUpdated,
        &user_id.to_string(),
        serde_json::json!({
            "fields_changed": changes,
            "updated_by": &claims.sub
        }),
    );

    // Fetch updated user
    let updated_user: UserRow = sqlx::query_as(
        r#"
        SELECT id, email, role, name, created_at, last_login
        FROM users
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;

    let role = updated_user.role_string();
    Ok(Json(UserListItem {
        id: updated_user.id.to_string(),
        email: updated_user.email,
        name: updated_user.name,
        role,
        created_at: updated_user.created_at,
        last_login: updated_user.last_login,
    }))
}

/// Delete a user (admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/users/{user_id}",
    params(
        ("user_id" = String, Path, description = "User ID")
    ),
    responses(
        (status = 204, description = "User deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 404, description = "User not found"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "users"
)]
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(user_id): Path<String>,
) -> ApiResult<StatusCode> {
    let user_id = Uuid::parse_str(&user_id)
        .map_err(|_| ApiError::BadRequest("Invalid user ID format".into()))?;

    // Fetch user email before deletion for audit log
    let user: Option<(String,)> = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?;

    let user_email = user
        .as_ref()
        .map(|(email,)| email.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let result = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("User not found".into()));
    }

    // Log user deletion (use sync for critical operation)
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::UserDeleted,
        &user_id.to_string(),
        serde_json::json!({
            "email": user_email,
            "deleted_by": &claims.sub
        }),
    );

    Ok(StatusCode::NO_CONTENT)
}
