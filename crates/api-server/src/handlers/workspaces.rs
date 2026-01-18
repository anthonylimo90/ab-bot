//! Workspace handlers for regular users.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::{AuditAction, Claims};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Workspace list item for user.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceListItem {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub my_role: String,
    pub member_count: i64,
    pub setup_complete: bool,
    pub created_at: DateTime<Utc>,
}

/// Workspace detail for user.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceResponse {
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
    pub my_role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Update workspace request (owner only).
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateWorkspaceRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub setup_mode: Option<String>,
    pub total_budget: Option<Decimal>,
    pub reserved_cash_pct: Option<Decimal>,
    pub auto_optimize_enabled: Option<bool>,
    pub optimization_interval_hours: Option<i32>,
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,
}

/// Workspace member response.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMemberResponse {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

/// Update member role request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateMemberRoleRequest {
    pub role: String,
}

/// Database row for user's workspace list.
#[derive(Debug, sqlx::FromRow)]
struct UserWorkspaceRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    role: String,
    member_count: i64,
    total_budget: Decimal,
    created_at: DateTime<Utc>,
}

/// Database row for workspace detail.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceDetailRow {
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
    role: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
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

/// List workspaces the current user belongs to.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces",
    responses(
        (status = 200, description = "List of user's workspaces", body = Vec<WorkspaceListItem>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<Vec<WorkspaceListItem>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspaces: Vec<UserWorkspaceRow> = sqlx::query_as(
        r#"
        SELECT
            w.id, w.name, w.description, wm.role, w.total_budget, w.created_at,
            (SELECT COUNT(*) FROM workspace_members WHERE workspace_id = w.id) as member_count
        FROM workspaces w
        INNER JOIN workspace_members wm ON w.id = wm.workspace_id
        WHERE wm.user_id = $1
        ORDER BY w.created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let response: Vec<WorkspaceListItem> = workspaces
        .into_iter()
        .map(|w| WorkspaceListItem {
            id: w.id.to_string(),
            name: w.name,
            description: w.description,
            my_role: w.role,
            member_count: w.member_count,
            setup_complete: w.total_budget > Decimal::ZERO,
            created_at: w.created_at,
        })
        .collect();

    Ok(Json(response))
}

/// Get current workspace (from user settings).
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/current",
    responses(
        (status = 200, description = "Current workspace details", body = WorkspaceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn get_current_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<WorkspaceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    // Get user's default workspace
    let settings: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT default_workspace_id FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    let workspace_id = settings
        .and_then(|(id,)| id)
        .ok_or_else(|| ApiError::NotFound("No current workspace set".into()))?;

    get_workspace(
        State(state),
        Extension(claims),
        Path(workspace_id.to_string()),
    )
    .await
}

/// Get a specific workspace.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Workspace details", body = WorkspaceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn get_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<WorkspaceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    let workspace: Option<WorkspaceDetailRow> = sqlx::query_as(
        r#"
        SELECT
            w.id, w.name, w.description, w.setup_mode, w.total_budget, w.reserved_cash_pct,
            w.auto_optimize_enabled, w.optimization_interval_hours,
            w.min_roi_30d, w.min_sharpe, w.min_win_rate, w.min_trades_30d,
            w.trading_wallet_address, wm.role, w.created_at, w.updated_at
        FROM workspaces w
        INNER JOIN workspace_members wm ON w.id = wm.workspace_id
        WHERE w.id = $1 AND wm.user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    let workspace = workspace.ok_or_else(|| {
        ApiError::Forbidden("Not a member of this workspace or workspace not found".into())
    })?;

    Ok(Json(WorkspaceResponse {
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
        my_role: workspace.role,
        created_at: workspace.created_at,
        updated_at: workspace.updated_at,
    }))
}

/// Update a workspace (owner only).
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    request_body = UpdateWorkspaceRequest,
    responses(
        (status = 200, description = "Workspace updated", body = WorkspaceResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not owner of this workspace"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn update_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> ApiResult<Json<WorkspaceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Check user is owner
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if role != "owner" {
        return Err(ApiError::Forbidden(
            "Only workspace owner can update settings".into(),
        ));
    }

    // Build dynamic update
    let now = Utc::now();
    let mut set_parts = vec!["updated_at = $2".to_string()];
    let mut param_idx = 3;

    macro_rules! add_param {
        ($field:ident, $col:expr) => {
            if req.$field.is_some() {
                set_parts.push(format!("{} = ${}", $col, param_idx));
                param_idx += 1;
            }
        };
    }

    add_param!(name, "name");
    add_param!(description, "description");
    add_param!(setup_mode, "setup_mode");
    add_param!(total_budget, "total_budget");
    add_param!(reserved_cash_pct, "reserved_cash_pct");
    add_param!(auto_optimize_enabled, "auto_optimize_enabled");
    add_param!(optimization_interval_hours, "optimization_interval_hours");
    add_param!(min_roi_30d, "min_roi_30d");
    add_param!(min_sharpe, "min_sharpe");
    add_param!(min_win_rate, "min_win_rate");
    add_param!(min_trades_30d, "min_trades_30d");

    let query = format!(
        "UPDATE workspaces SET {} WHERE id = $1",
        set_parts.join(", ")
    );

    let mut q = sqlx::query(&query).bind(workspace_id).bind(now);

    if let Some(ref name) = req.name {
        q = q.bind(name);
    }
    if let Some(ref description) = req.description {
        q = q.bind(description);
    }
    if let Some(ref setup_mode) = req.setup_mode {
        q = q.bind(setup_mode.to_lowercase());
    }
    if let Some(total_budget) = req.total_budget {
        q = q.bind(total_budget);
    }
    if let Some(reserved_cash_pct) = req.reserved_cash_pct {
        q = q.bind(reserved_cash_pct);
    }
    if let Some(auto_optimize_enabled) = req.auto_optimize_enabled {
        q = q.bind(auto_optimize_enabled);
    }
    if let Some(optimization_interval_hours) = req.optimization_interval_hours {
        q = q.bind(optimization_interval_hours);
    }
    if let Some(min_roi_30d) = req.min_roi_30d {
        q = q.bind(min_roi_30d);
    }
    if let Some(min_sharpe) = req.min_sharpe {
        q = q.bind(min_sharpe);
    }
    if let Some(min_win_rate) = req.min_win_rate {
        q = q.bind(min_win_rate);
    }
    if let Some(min_trades_30d) = req.min_trades_30d {
        q = q.bind(min_trades_30d);
    }

    q.execute(&state.pool).await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("workspace_updated".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({ "updated_by": &claims.sub }),
    );

    get_workspace(
        State(state),
        Extension(claims),
        Path(workspace_id.to_string()),
    )
    .await
}

/// Switch to a workspace (set as current).
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/switch",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Switched to workspace", body = WorkspaceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn switch_workspace(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<WorkspaceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Update user settings
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO user_settings (user_id, default_workspace_id, created_at, updated_at)
        VALUES ($1, $2, $3, $3)
        ON CONFLICT (user_id) DO UPDATE SET
            default_workspace_id = $2,
            updated_at = $3
        "#,
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(now)
    .execute(&state.pool)
    .await?;

    get_workspace(
        State(state),
        Extension(claims),
        Path(workspace_id.to_string()),
    )
    .await
}

/// List members of a workspace.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/members",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "List of workspace members", body = Vec<WorkspaceMemberResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<Vec<WorkspaceMemberResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    #[derive(sqlx::FromRow)]
    struct MemberRow {
        user_id: Uuid,
        email: String,
        name: Option<String>,
        role: String,
        joined_at: DateTime<Utc>,
    }

    let members: Vec<MemberRow> = sqlx::query_as(
        r#"
        SELECT wm.user_id, u.email, u.name, wm.role, wm.joined_at
        FROM workspace_members wm
        INNER JOIN users u ON wm.user_id = u.id
        WHERE wm.workspace_id = $1
        ORDER BY
            CASE wm.role
                WHEN 'owner' THEN 1
                WHEN 'admin' THEN 2
                WHEN 'member' THEN 3
                ELSE 4
            END,
            wm.joined_at
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;

    let response: Vec<WorkspaceMemberResponse> = members
        .into_iter()
        .map(|m| WorkspaceMemberResponse {
            user_id: m.user_id.to_string(),
            email: m.email,
            name: m.name,
            role: m.role,
            joined_at: m.joined_at,
        })
        .collect();

    Ok(Json(response))
}

/// Update a member's role (owner/admin only).
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/members/{member_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        ("member_id" = String, Path, description = "Member user ID")
    ),
    request_body = UpdateMemberRoleRequest,
    responses(
        (status = 200, description = "Member role updated", body = WorkspaceMemberResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to update roles"),
        (status = 404, description = "Member not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn update_member_role(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((workspace_id, member_id)): Path<(String, String)>,
    Json(req): Json<UpdateMemberRoleRequest>,
) -> ApiResult<Json<WorkspaceMemberResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;
    let member_id = Uuid::parse_str(&member_id)
        .map_err(|_| ApiError::BadRequest("Invalid member ID format".into()))?;

    // Check caller's role
    let caller_role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&caller_role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can update member roles".into(),
        ));
    }

    // Validate new role
    let new_role = req.role.to_lowercase();
    if !["admin", "member", "viewer"].contains(&new_role.as_str()) {
        return Err(ApiError::BadRequest(
            "Role must be 'admin', 'member', or 'viewer'".into(),
        ));
    }

    // Check target member exists and isn't owner
    let target_role = get_user_role(&state.pool, workspace_id, member_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Member not found in workspace".into()))?;

    if target_role == "owner" {
        return Err(ApiError::BadRequest("Cannot change owner's role".into()));
    }

    // Only owner can promote to admin
    if new_role == "admin" && caller_role != "owner" {
        return Err(ApiError::Forbidden(
            "Only owner can promote to admin".into(),
        ));
    }

    // Update role
    sqlx::query("UPDATE workspace_members SET role = $1 WHERE workspace_id = $2 AND user_id = $3")
        .bind(&new_role)
        .bind(workspace_id)
        .bind(member_id)
        .execute(&state.pool)
        .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("workspace_member_role_updated".to_string()),
        &member_id.to_string(),
        serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "old_role": target_role,
            "new_role": &new_role
        }),
    );

    // Fetch updated member
    #[derive(sqlx::FromRow)]
    struct MemberRow {
        user_id: Uuid,
        email: String,
        name: Option<String>,
        role: String,
        joined_at: DateTime<Utc>,
    }

    let member: MemberRow = sqlx::query_as(
        r#"
        SELECT wm.user_id, u.email, u.name, wm.role, wm.joined_at
        FROM workspace_members wm
        INNER JOIN users u ON wm.user_id = u.id
        WHERE wm.workspace_id = $1 AND wm.user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(member_id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(WorkspaceMemberResponse {
        user_id: member.user_id.to_string(),
        email: member.email,
        name: member.name,
        role: member.role,
        joined_at: member.joined_at,
    }))
}

/// Remove a member from workspace (owner/admin only).
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/members/{member_id}",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        ("member_id" = String, Path, description = "Member user ID")
    ),
    responses(
        (status = 204, description = "Member removed"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to remove members"),
        (status = 404, description = "Member not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn remove_member(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path((workspace_id, member_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;
    let member_id = Uuid::parse_str(&member_id)
        .map_err(|_| ApiError::BadRequest("Invalid member ID format".into()))?;

    // Check caller's role
    let caller_role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&caller_role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can remove members".into(),
        ));
    }

    // Check target member exists and isn't owner
    let target_role = get_user_role(&state.pool, workspace_id, member_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Member not found in workspace".into()))?;

    if target_role == "owner" {
        return Err(ApiError::BadRequest("Cannot remove workspace owner".into()));
    }

    // Only owner can remove admins
    if target_role == "admin" && caller_role != "owner" {
        return Err(ApiError::Forbidden("Only owner can remove admins".into()));
    }

    // Remove member
    sqlx::query("DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(member_id)
        .execute(&state.pool)
        .await?;

    // Clear default workspace if this was it
    sqlx::query(
        r#"
        UPDATE user_settings
        SET default_workspace_id = NULL, updated_at = NOW()
        WHERE user_id = $1 AND default_workspace_id = $2
        "#,
    )
    .bind(member_id)
    .bind(workspace_id)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("workspace_member_removed".to_string()),
        &member_id.to_string(),
        serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "removed_role": target_role
        }),
    );

    Ok(StatusCode::NO_CONTENT)
}
