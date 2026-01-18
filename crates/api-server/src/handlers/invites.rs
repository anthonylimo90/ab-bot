//! Workspace invite handlers.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use axum::extract::{Path, State};
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

use auth::{AuditAction, AuditEvent, Claims};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Invite response.
#[derive(Debug, Serialize, ToSchema)]
pub struct InviteResponse {
    pub id: String,
    pub email: String,
    pub role: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub inviter_email: Option<String>,
}

/// Create invite request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateInviteRequest {
    /// Email of the person to invite.
    pub email: String,
    /// Role to assign: "admin", "member", or "viewer".
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "member".to_string()
}

/// Accept invite request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AcceptInviteRequest {
    /// Password for new account (if registering).
    pub password: Option<String>,
    /// Display name (if registering).
    pub name: Option<String>,
}

/// Accept invite response.
#[derive(Debug, Serialize, ToSchema)]
pub struct AcceptInviteResponse {
    pub workspace_id: String,
    pub workspace_name: String,
    pub role: String,
    pub is_new_user: bool,
}

/// Public invite info (for invite acceptance page).
#[derive(Debug, Serialize, ToSchema)]
pub struct InviteInfoResponse {
    pub workspace_name: String,
    pub inviter_email: String,
    pub role: String,
    pub email: String,
    pub expires_at: DateTime<Utc>,
    pub user_exists: bool,
}

/// Generate a secure random invite token.
fn generate_invite_token() -> String {
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

/// Get user's role in a workspace.
async fn get_user_role(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let role: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(role.map(|(r,)| r))
}

/// List pending invites for a workspace (owner/admin only).
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/invites",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "List of pending invites", body = Vec<InviteResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to view invites"),
    ),
    security(("bearer_auth" = [])),
    tag = "invites"
)]
pub async fn list_invites(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<Vec<InviteResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Check caller has permission
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can view invites".into(),
        ));
    }

    #[derive(sqlx::FromRow)]
    struct InviteRow {
        id: Uuid,
        email: String,
        role: String,
        expires_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
        inviter_email: Option<String>,
    }

    let invites: Vec<InviteRow> = sqlx::query_as(
        r#"
        SELECT wi.id, wi.email, wi.role, wi.expires_at, wi.created_at, u.email as inviter_email
        FROM workspace_invites wi
        LEFT JOIN users u ON wi.invited_by = u.id
        WHERE wi.workspace_id = $1 AND wi.accepted_at IS NULL AND wi.expires_at > NOW()
        ORDER BY wi.created_at DESC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;

    let response: Vec<InviteResponse> = invites
        .into_iter()
        .map(|i| InviteResponse {
            id: i.id.to_string(),
            email: i.email,
            role: i.role,
            expires_at: i.expires_at,
            created_at: i.created_at,
            inviter_email: i.inviter_email,
        })
        .collect();

    Ok(Json(response))
}

/// Create a new invite (owner/admin only).
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/invites",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    request_body = CreateInviteRequest,
    responses(
        (status = 201, description = "Invite created", body = InviteResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to create invites"),
        (status = 409, description = "User already a member or invite pending"),
    ),
    security(("bearer_auth" = [])),
    tag = "invites"
)]
pub async fn create_invite(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateInviteRequest>,
) -> ApiResult<(StatusCode, Json<InviteResponse>)> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Validate email
    if !req.email.contains('@') || req.email.len() < 5 {
        return Err(ApiError::BadRequest("Invalid email address".into()));
    }

    // Validate role
    let role = req.role.to_lowercase();
    if !["admin", "member", "viewer"].contains(&role.as_str()) {
        return Err(ApiError::BadRequest(
            "Role must be 'admin', 'member', or 'viewer'".into(),
        ));
    }

    // Check caller has permission
    let caller_role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&caller_role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can invite members".into(),
        ));
    }

    // Only owner can invite admins
    if role == "admin" && caller_role != "owner" {
        return Err(ApiError::Forbidden("Only owner can invite as admin".into()));
    }

    // Check if user is already a member
    let existing_user: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&req.email)
        .fetch_optional(&state.pool)
        .await?;

    if let Some((existing_id,)) = existing_user {
        let existing_member = get_user_role(&state.pool, workspace_id, existing_id).await?;
        if existing_member.is_some() {
            return Err(ApiError::Conflict(
                "User is already a member of this workspace".into(),
            ));
        }
    }

    // Check for pending invite
    let pending: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM workspace_invites
        WHERE workspace_id = $1 AND email = $2 AND accepted_at IS NULL AND expires_at > NOW()
        "#,
    )
    .bind(workspace_id)
    .bind(&req.email)
    .fetch_optional(&state.pool)
    .await?;

    if pending.is_some() {
        return Err(ApiError::Conflict(
            "An invite is already pending for this email".into(),
        ));
    }

    // Generate token
    let token = generate_invite_token();
    let token_hash = hash_token(&token);
    let expires_at = Utc::now() + Duration::days(7);
    let invite_id = Uuid::new_v4();
    let now = Utc::now();

    // Create invite
    sqlx::query(
        r#"
        INSERT INTO workspace_invites (id, workspace_id, email, role, token_hash, invited_by, expires_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(invite_id)
    .bind(workspace_id)
    .bind(&req.email)
    .bind(&role)
    .bind(&token_hash)
    .bind(user_id)
    .bind(expires_at)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Get inviter email for response
    let inviter: Option<(String,)> = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?;

    // Send invite email if configured
    if let Some(email_client) = &state.email_client {
        // Get workspace name
        let workspace: Option<(String,)> =
            sqlx::query_as("SELECT name FROM workspaces WHERE id = $1")
                .bind(workspace_id)
                .fetch_optional(&state.pool)
                .await?;

        if let Some((workspace_name,)) = workspace {
            let invite_link = format!(
                "{}/invite/{}",
                std::env::var("DASHBOARD_URL")
                    .unwrap_or_else(|_| "http://localhost:3002".to_string()),
                token
            );

            // Note: You'd implement send_workspace_invite on the email client
            let subject = format!("You've been invited to join {} on AB-Bot", workspace_name);
            let body = format!(
                "You've been invited to join the workspace '{}' as a {}.\n\n\
                Click the link below to accept:\n{}\n\n\
                This invite expires in 7 days.",
                workspace_name, role, invite_link
            );

            if let Err(e) = email_client.send_simple(&req.email, &subject, &body).await {
                tracing::error!(error = %e, "Failed to send invite email");
            } else {
                tracing::info!(email = %req.email, "Invite email sent");
            }
        }
    } else {
        tracing::info!(
            token = %token,
            email = %req.email,
            "Invite created (email not configured)"
        );
    }

    // Audit log
    let event = AuditEvent::builder(
        AuditAction::Custom("workspace_invite_created".to_string()),
        format!("invite/{}", invite_id),
    )
    .user(claims.sub.clone())
    .details(serde_json::json!({
        "workspace_id": workspace_id.to_string(),
        "email": &req.email,
        "role": &role
    }))
    .build();
    state.audit_logger.log(event);

    Ok((
        StatusCode::CREATED,
        Json(InviteResponse {
            id: invite_id.to_string(),
            email: req.email,
            role,
            expires_at,
            created_at: now,
            inviter_email: inviter.map(|(e,)| e),
        }),
    ))
}

/// Revoke a pending invite (owner/admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/invites/{invite_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        ("invite_id" = String, Path, description = "Invite ID")
    ),
    responses(
        (status = 204, description = "Invite revoked"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to revoke invites"),
        (status = 404, description = "Invite not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "invites"
)]
pub async fn revoke_invite(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((workspace_id, invite_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;
    let invite_id = Uuid::parse_str(&invite_id)
        .map_err(|_| ApiError::BadRequest("Invalid invite ID format".into()))?;

    // Check caller has permission
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can revoke invites".into(),
        ));
    }

    let result = sqlx::query(
        "DELETE FROM workspace_invites WHERE id = $1 AND workspace_id = $2 AND accepted_at IS NULL",
    )
    .bind(invite_id)
    .bind(workspace_id)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(
            "Invite not found or already accepted".into(),
        ));
    }

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("workspace_invite_revoked".to_string()),
        &invite_id.to_string(),
        serde_json::json!({ "workspace_id": workspace_id.to_string() }),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Get invite info by token (public endpoint).
#[utoipa::path(
    get,
    path = "/api/v1/invites/{token}",
    params(
        ("token" = String, Path, description = "Invite token")
    ),
    responses(
        (status = 200, description = "Invite information", body = InviteInfoResponse),
        (status = 404, description = "Invite not found or expired"),
    ),
    tag = "invites"
)]
pub async fn get_invite_info(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> ApiResult<Json<InviteInfoResponse>> {
    let token_hash = hash_token(&token);

    #[derive(sqlx::FromRow)]
    struct InviteInfoRow {
        workspace_id: Uuid,
        workspace_name: String,
        email: String,
        role: String,
        expires_at: DateTime<Utc>,
        inviter_email: String,
    }

    let invite: Option<InviteInfoRow> = sqlx::query_as(
        r#"
        SELECT
            wi.workspace_id, w.name as workspace_name, wi.email, wi.role, wi.expires_at,
            u.email as inviter_email
        FROM workspace_invites wi
        INNER JOIN workspaces w ON wi.workspace_id = w.id
        INNER JOIN users u ON wi.invited_by = u.id
        WHERE wi.token_hash = $1 AND wi.accepted_at IS NULL AND wi.expires_at > NOW()
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(&state.pool)
    .await?;

    let invite = invite.ok_or_else(|| ApiError::NotFound("Invite not found or expired".into()))?;

    // Check if user with this email already exists
    let user_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE email = $1")
        .bind(&invite.email)
        .fetch_optional(&state.pool)
        .await?;

    Ok(Json(InviteInfoResponse {
        workspace_name: invite.workspace_name,
        inviter_email: invite.inviter_email,
        role: invite.role,
        email: invite.email,
        expires_at: invite.expires_at,
        user_exists: user_exists.is_some(),
    }))
}

/// Accept an invite (public endpoint for registration, or authenticated for existing users).
#[utoipa::path(
    post,
    path = "/api/v1/invites/{token}/accept",
    params(
        ("token" = String, Path, description = "Invite token")
    ),
    request_body = AcceptInviteRequest,
    responses(
        (status = 200, description = "Invite accepted", body = AcceptInviteResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Invite not found or expired"),
        (status = 409, description = "Already a member"),
    ),
    tag = "invites"
)]
pub async fn accept_invite(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    Json(req): Json<AcceptInviteRequest>,
) -> ApiResult<Json<AcceptInviteResponse>> {
    let token_hash = hash_token(&token);

    #[derive(sqlx::FromRow)]
    struct InviteRow {
        id: Uuid,
        workspace_id: Uuid,
        workspace_name: String,
        email: String,
        role: String,
    }

    let invite: Option<InviteRow> = sqlx::query_as(
        r#"
        SELECT wi.id, wi.workspace_id, w.name as workspace_name, wi.email, wi.role
        FROM workspace_invites wi
        INNER JOIN workspaces w ON wi.workspace_id = w.id
        WHERE wi.token_hash = $1 AND wi.accepted_at IS NULL AND wi.expires_at > NOW()
        "#,
    )
    .bind(&token_hash)
    .fetch_optional(&state.pool)
    .await?;

    let invite = invite.ok_or_else(|| ApiError::NotFound("Invite not found or expired".into()))?;

    // Check if user exists
    let existing_user: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&invite.email)
        .fetch_optional(&state.pool)
        .await?;

    let mut tx = state.pool.begin().await?;
    let now = Utc::now();
    let is_new_user;
    let user_id;

    if let Some((uid,)) = existing_user {
        // Existing user
        user_id = uid;
        is_new_user = false;

        // Check if already a member
        let existing_member: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        )
        .bind(invite.workspace_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;

        if existing_member.is_some() {
            return Err(ApiError::Conflict(
                "Already a member of this workspace".into(),
            ));
        }
    } else {
        // New user - must provide password
        let password = req
            .password
            .ok_or_else(|| ApiError::BadRequest("Password is required for new account".into()))?;

        if password.len() < 8 {
            return Err(ApiError::BadRequest(
                "Password must be at least 8 characters".into(),
            ));
        }

        // Hash password
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| ApiError::Internal(format!("Password hashing failed: {}", e)))?
            .to_string();

        // Create user
        user_id = Uuid::new_v4();
        let role: i16 = 1; // Trader by default

        sqlx::query(
            r#"
            INSERT INTO users (id, email, password_hash, role, name, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $6)
            "#,
        )
        .bind(user_id)
        .bind(&invite.email)
        .bind(&password_hash)
        .bind(role)
        .bind(&req.name)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        is_new_user = true;
    }

    // Add to workspace
    sqlx::query(
        r#"
        INSERT INTO workspace_members (workspace_id, user_id, role, joined_at)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(invite.workspace_id)
    .bind(user_id)
    .bind(&invite.role)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // Mark invite as accepted
    sqlx::query("UPDATE workspace_invites SET accepted_at = $1 WHERE id = $2")
        .bind(now)
        .bind(invite.id)
        .execute(&mut *tx)
        .await?;

    // Set as default workspace if first workspace
    sqlx::query(
        r#"
        INSERT INTO user_settings (user_id, default_workspace_id, created_at, updated_at)
        VALUES ($1, $2, $3, $3)
        ON CONFLICT (user_id) DO UPDATE SET
            default_workspace_id = COALESCE(user_settings.default_workspace_id, $2),
            updated_at = $3
        "#,
    )
    .bind(user_id)
    .bind(invite.workspace_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // Audit log
    let event = AuditEvent::builder(
        AuditAction::Custom("workspace_invite_accepted".to_string()),
        format!("invite/{}", invite.id),
    )
    .user(user_id.to_string())
    .details(serde_json::json!({
        "workspace_id": invite.workspace_id.to_string(),
        "role": &invite.role,
        "is_new_user": is_new_user
    }))
    .build();
    state.audit_logger.log(event);

    Ok(Json(AcceptInviteResponse {
        workspace_id: invite.workspace_id.to_string(),
        workspace_name: invite.workspace_name,
        role: invite.role,
        is_new_user,
    }))
}
