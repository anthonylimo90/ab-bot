//! Admin workspace management handlers (Platform Admin only).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::{AuditAction, AuditEvent, Claims};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

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

/// Workspace list response for admin view.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminWorkspaceListItem {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub setup_mode: String,
    pub total_budget: Decimal,
    pub auto_optimize_enabled: bool,
    pub owner_email: Option<String>,
    pub member_count: i64,
    pub active_wallet_count: i64,
    pub created_at: DateTime<Utc>,
}

/// Create workspace request (admin).
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    /// Workspace name.
    pub name: String,
    /// Workspace description.
    #[serde(default)]
    pub description: Option<String>,
    /// Owner email (must be an existing user or will send invite).
    pub owner_email: String,
    /// Initial setup mode.
    #[serde(default = "default_setup_mode")]
    pub setup_mode: String,
}

fn default_setup_mode() -> String {
    "manual".to_string()
}

/// Update workspace request (admin).
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateWorkspaceRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub setup_mode: Option<String>,
}

/// Workspace detail response.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceDetailResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub setup_mode: String,
    pub total_budget: Decimal,
    pub reserved_cash_pct: Decimal,
    pub auto_optimize_enabled: bool,
    pub optimization_interval_hours: i32,
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,
    pub trading_wallet_address: Option<String>,
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Query params for listing workspaces.
#[derive(Debug, Deserialize)]
pub struct ListWorkspacesQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

/// Database row for workspace list.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceListRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    setup_mode: String,
    total_budget: Decimal,
    auto_optimize_enabled: bool,
    created_at: DateTime<Utc>,
    owner_email: Option<String>,
    member_count: i64,
    active_wallet_count: i64,
}

/// Database row for workspace detail.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    setup_mode: String,
    total_budget: Decimal,
    reserved_cash_pct: Decimal,
    auto_optimize_enabled: bool,
    optimization_interval_hours: i32,
    min_roi_30d: Option<Decimal>,
    min_sharpe: Option<Decimal>,
    min_win_rate: Option<Decimal>,
    min_trades_30d: Option<i32>,
    trading_wallet_address: Option<String>,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// List all workspaces (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/workspaces",
    params(
        ("limit" = Option<i32>, Query, description = "Max results"),
        ("offset" = Option<i32>, Query, description = "Offset for pagination"),
    ),
    responses(
        (status = 200, description = "List of all workspaces", body = Vec<AdminWorkspaceListItem>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "admin_workspaces"
)]
pub async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListWorkspacesQuery>,
) -> ApiResult<Json<Vec<AdminWorkspaceListItem>>> {
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);

    let workspaces: Vec<WorkspaceListRow> = sqlx::query_as(
        r#"
        SELECT
            w.id, w.name, w.description, w.setup_mode, w.total_budget,
            w.auto_optimize_enabled, w.created_at,
            owner.email as owner_email,
            (SELECT COUNT(*) FROM workspace_members WHERE workspace_id = w.id) as member_count,
            (SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = w.id AND tier = 'active') as active_wallet_count
        FROM workspaces w
        LEFT JOIN workspace_members wm ON w.id = wm.workspace_id AND wm.role = 'owner'
        LEFT JOIN users owner ON wm.user_id = owner.id
        ORDER BY w.created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;

    let response: Vec<AdminWorkspaceListItem> = workspaces
        .into_iter()
        .map(|w| AdminWorkspaceListItem {
            id: w.id.to_string(),
            name: w.name,
            description: w.description,
            setup_mode: w.setup_mode,
            total_budget: w.total_budget,
            auto_optimize_enabled: w.auto_optimize_enabled,
            owner_email: w.owner_email,
            member_count: w.member_count,
            active_wallet_count: w.active_wallet_count,
            created_at: w.created_at,
        })
        .collect();

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("admin_workspaces_listed".to_string()),
        "workspaces",
        serde_json::json!({ "count": response.len() }),
    );

    Ok(Json(response))
}

/// Create a new workspace (admin only).
/// If the owner email doesn't exist, an invite will be sent to that email.
#[utoipa::path(
    post,
    path = "/api/v1/admin/workspaces",
    request_body = CreateWorkspaceRequest,
    responses(
        (status = 201, description = "Workspace created (owner added or invite sent)", body = WorkspaceDetailResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "admin_workspaces"
)]
pub async fn create_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> ApiResult<(StatusCode, Json<WorkspaceDetailResponse>)> {
    // Validate name
    if req.name.trim().is_empty() || req.name.len() > 100 {
        return Err(ApiError::BadRequest("Name must be 1-100 characters".into()));
    }

    // Validate setup mode
    if !["manual", "automatic"].contains(&req.setup_mode.to_lowercase().as_str()) {
        return Err(ApiError::BadRequest(
            "Setup mode must be 'manual' or 'automatic'".into(),
        ));
    }

    // Validate email format
    if !req.owner_email.contains('@') || req.owner_email.len() < 5 {
        return Err(ApiError::BadRequest("Invalid email address".into()));
    }

    // Find owner by email (may not exist)
    let owner: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&req.owner_email)
        .fetch_optional(&state.pool)
        .await?;

    let admin_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid admin ID".into()))?;

    // Create workspace in transaction
    let mut tx = state.pool.begin().await?;

    let workspace_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO workspaces (id, name, description, setup_mode, created_by, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $6)
        "#,
    )
    .bind(workspace_id)
    .bind(&req.name)
    .bind(&req.description)
    .bind(req.setup_mode.to_lowercase())
    .bind(admin_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    // If owner exists, add them directly; otherwise create an invite
    if let Some((owner_id,)) = owner {
        // Add owner as workspace member
        sqlx::query(
            r#"
            INSERT INTO workspace_members (workspace_id, user_id, role, joined_at)
            VALUES ($1, $2, 'owner', $3)
            "#,
        )
        .bind(workspace_id)
        .bind(owner_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        // Create user settings if not exists and set default workspace
        sqlx::query(
            r#"
            INSERT INTO user_settings (user_id, default_workspace_id, created_at, updated_at)
            VALUES ($1, $2, $3, $3)
            ON CONFLICT (user_id) DO UPDATE SET
                default_workspace_id = COALESCE(user_settings.default_workspace_id, $2),
                updated_at = $3
            "#,
        )
        .bind(owner_id)
        .bind(workspace_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        // Commit transaction - owner exists and is added
        tx.commit().await?;
    } else {
        // Owner doesn't exist - we must be able to send them an invite
        // Check if email is configured first
        let email_client = state.email_client.as_ref().ok_or_else(|| {
            ApiError::BadRequest(
                "Cannot invite non-existent user: email is not configured. Please use an existing user's email.".into()
            )
        })?;

        // Create invite record
        let token = generate_invite_token();
        let token_hash = hash_token(&token);
        let expires_at = Utc::now() + Duration::days(7);
        let invite_id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO workspace_invites (id, workspace_id, email, role, token_hash, invited_by, expires_at, created_at)
            VALUES ($1, $2, $3, 'owner', $4, $5, $6, $7)
            "#,
        )
        .bind(invite_id)
        .bind(workspace_id)
        .bind(&req.owner_email)
        .bind(&token_hash)
        .bind(admin_id)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        // Try to send the invite email BEFORE committing
        let invite_link = format!(
            "{}/invite/{}",
            std::env::var("DASHBOARD_URL").unwrap_or_else(|_| "http://localhost:3002".to_string()),
            token
        );

        let subject = format!("You've been invited to own {} on AB-Bot", req.name);
        let body = format!(
            "You've been invited to be the owner of the workspace '{}'.\n\n\
            Click the link below to accept and create your account:\n{}\n\n\
            This invite expires in 7 days.",
            req.name, invite_link
        );

        // Send email - if it fails, rollback the transaction
        email_client
            .send_simple(&req.owner_email, &subject, &body)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, email = %req.owner_email, "Failed to send workspace owner invite email");
                ApiError::Internal(format!(
                    "Failed to send invite email to '{}'. Please try again or use an existing user's email.",
                    req.owner_email
                ))
            })?;

        // Email sent successfully - now commit the transaction
        tx.commit().await?;

        tracing::info!(email = %req.owner_email, workspace = %req.name, "Workspace owner invite email sent");
    }

    // Audit log
    let event = AuditEvent::builder(
        AuditAction::Custom("workspace_created".to_string()),
        format!("workspace/{}", workspace_id),
    )
    .user(claims.sub.clone())
    .details(serde_json::json!({
        "name": &req.name,
        "owner_email": &req.owner_email,
        "owner_invited": owner.is_none(),
        "created_by_admin": &claims.sub
    }))
    .build();
    state.audit_logger.log(event);

    let response = WorkspaceDetailResponse {
        id: workspace_id.to_string(),
        name: req.name,
        description: req.description,
        setup_mode: req.setup_mode.to_lowercase(),
        total_budget: Decimal::ZERO,
        reserved_cash_pct: Decimal::new(10, 0),
        auto_optimize_enabled: false,
        optimization_interval_hours: 24,
        min_roi_30d: Some(Decimal::new(5, 0)),
        min_sharpe: Some(Decimal::ONE),
        min_win_rate: Some(Decimal::new(50, 0)),
        min_trades_30d: Some(10),
        trading_wallet_address: None,
        created_by: Some(admin_id.to_string()),
        created_at: now,
        updated_at: now,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// Get a workspace by ID (admin only).
#[utoipa::path(
    get,
    path = "/api/v1/admin/workspaces/{workspace_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Workspace details", body = WorkspaceDetailResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "admin_workspaces"
)]
pub async fn get_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<WorkspaceDetailResponse>> {
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    let workspace: Option<WorkspaceRow> = sqlx::query_as(
        r#"
        SELECT id, name, description, setup_mode, total_budget, reserved_cash_pct,
               auto_optimize_enabled, optimization_interval_hours,
               min_roi_30d, min_sharpe, min_win_rate, min_trades_30d,
               trading_wallet_address, created_by, created_at, updated_at
        FROM workspaces
        WHERE id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;

    let workspace = workspace.ok_or_else(|| ApiError::NotFound("Workspace not found".into()))?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("admin_workspace_viewed".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({ "name": &workspace.name }),
    );

    Ok(Json(WorkspaceDetailResponse {
        id: workspace.id.to_string(),
        name: workspace.name,
        description: workspace.description,
        setup_mode: workspace.setup_mode,
        total_budget: workspace.total_budget,
        reserved_cash_pct: workspace.reserved_cash_pct,
        auto_optimize_enabled: workspace.auto_optimize_enabled,
        optimization_interval_hours: workspace.optimization_interval_hours,
        min_roi_30d: workspace.min_roi_30d,
        min_sharpe: workspace.min_sharpe,
        min_win_rate: workspace.min_win_rate,
        min_trades_30d: workspace.min_trades_30d,
        trading_wallet_address: workspace.trading_wallet_address,
        created_by: workspace.created_by.map(|id| id.to_string()),
        created_at: workspace.created_at,
        updated_at: workspace.updated_at,
    }))
}

/// Update a workspace (admin only).
#[utoipa::path(
    put,
    path = "/api/v1/admin/workspaces/{workspace_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    request_body = UpdateWorkspaceRequest,
    responses(
        (status = 200, description = "Workspace updated", body = WorkspaceDetailResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "admin_workspaces"
)]
pub async fn update_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> ApiResult<Json<WorkspaceDetailResponse>> {
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Check if workspace exists
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM workspaces WHERE id = $1")
        .bind(workspace_id)
        .fetch_optional(&state.pool)
        .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound("Workspace not found".into()));
    }

    // Build update
    let now = Utc::now();
    let mut updates = Vec::new();

    if let Some(ref name) = req.name {
        if name.trim().is_empty() || name.len() > 100 {
            return Err(ApiError::BadRequest("Name must be 1-100 characters".into()));
        }
        updates.push(("name", name.clone()));
    }

    if let Some(ref description) = req.description {
        updates.push(("description", description.clone()));
    }

    if let Some(ref setup_mode) = req.setup_mode {
        if !["manual", "automatic"].contains(&setup_mode.to_lowercase().as_str()) {
            return Err(ApiError::BadRequest(
                "Setup mode must be 'manual' or 'automatic'".into(),
            ));
        }
        updates.push(("setup_mode", setup_mode.to_lowercase()));
    }

    if !updates.is_empty() {
        // Dynamic query
        let set_clause: Vec<String> = updates
            .iter()
            .enumerate()
            .map(|(i, (col, _))| format!("{} = ${}", col, i + 2))
            .collect();

        let query = format!(
            "UPDATE workspaces SET {}, updated_at = ${} WHERE id = $1",
            set_clause.join(", "),
            updates.len() + 2
        );

        let mut q = sqlx::query(&query).bind(workspace_id);
        for (_, val) in &updates {
            q = q.bind(val);
        }
        q = q.bind(now);
        q.execute(&state.pool).await?;
    }

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("admin_workspace_updated".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({
            "fields_changed": updates.iter().map(|(k, _)| *k).collect::<Vec<_>>()
        }),
    );

    // Fetch updated workspace
    get_workspace(
        State(state),
        Extension(claims),
        Path(workspace_id.to_string()),
    )
    .await
}

/// Delete a workspace (admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/admin/workspaces/{workspace_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 204, description = "Workspace deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - Admin role required"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "admin_workspaces"
)]
pub async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<StatusCode> {
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Get workspace name for audit log
    let workspace: Option<(String,)> = sqlx::query_as("SELECT name FROM workspaces WHERE id = $1")
        .bind(workspace_id)
        .fetch_optional(&state.pool)
        .await?;

    let workspace_name = workspace
        .as_ref()
        .map(|(name,)| name.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let result = sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Workspace not found".into()));
    }

    // Audit log
    let event = AuditEvent::builder(
        AuditAction::Custom("workspace_deleted".to_string()),
        format!("workspace/{}", workspace_id),
    )
    .user(claims.sub.clone())
    .details(serde_json::json!({
        "name": workspace_name,
        "deleted_by_admin": &claims.sub
    }))
    .build();
    state.audit_logger.log(event);

    Ok(StatusCode::NO_CONTENT)
}
