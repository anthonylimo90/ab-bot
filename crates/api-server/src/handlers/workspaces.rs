//! Workspace handlers for regular users.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use polymarket_core::types::ArbOpportunity;
use redis::AsyncCommands;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use url::Url;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use auth::{AuditAction, Claims};

use crate::crypto;
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
    pub walletconnect_project_id: Option<String>,
    pub polygon_rpc_url: Option<String>,
    /// Masked alchemy API key (shows only last 4 chars).
    pub alchemy_api_key: Option<String>,
    pub arb_auto_execute: bool,
    pub copy_trading_enabled: bool,
    pub live_trading_enabled: bool,
    pub my_role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Optimizer status response for automatic workspaces.
#[derive(Debug, Serialize, ToSchema)]
pub struct OptimizerStatusResponse {
    pub enabled: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub interval_hours: i32,
    pub criteria: OptimizerCriteria,
    pub active_wallet_count: i32,
    pub bench_wallet_count: i32,
    pub portfolio_metrics: PortfolioMetrics,
}

/// Optimizer selection criteria.
#[derive(Debug, Serialize, ToSchema)]
pub struct OptimizerCriteria {
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,
}

/// Aggregated portfolio metrics from active wallets.
#[derive(Debug, Serialize, ToSchema)]
pub struct PortfolioMetrics {
    pub total_roi_30d: Decimal,
    pub avg_sharpe: Decimal,
    pub avg_win_rate: Decimal,
    pub total_value: Decimal,
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
    pub walletconnect_project_id: Option<String>,
    pub polygon_rpc_url: Option<String>,
    pub alchemy_api_key: Option<String>,
    pub arb_auto_execute: Option<bool>,
    pub copy_trading_enabled: Option<bool>,
    pub live_trading_enabled: Option<bool>,
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
    walletconnect_project_id: Option<String>,
    polygon_rpc_url: Option<String>,
    alchemy_api_key: Option<String>,
    arb_auto_execute: bool,
    copy_trading_enabled: bool,
    live_trading_enabled: bool,
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
            w.trading_wallet_address, w.walletconnect_project_id,
            w.polygon_rpc_url, w.alchemy_api_key,
            COALESCE(w.arb_auto_execute, false) as arb_auto_execute,
            COALESCE(w.copy_trading_enabled, true) as copy_trading_enabled,
            COALESCE(w.live_trading_enabled, false) as live_trading_enabled,
            wm.role, w.created_at, w.updated_at
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

    // Decrypt then mask: show only last 4 chars of alchemy key
    let masked_alchemy_key = workspace.alchemy_api_key.as_ref().map(|stored| {
        // Try to decrypt (encrypted values); fall back to treating as plaintext
        // for backward compatibility with pre-encryption rows.
        let plaintext =
            crypto::decrypt_field(stored, &state.encryption_key).unwrap_or_else(|| stored.clone());
        if plaintext.len() > 4 {
            format!("••••••{}", &plaintext[plaintext.len() - 4..])
        } else {
            "••••••".to_string()
        }
    });

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
        walletconnect_project_id: workspace.walletconnect_project_id,
        polygon_rpc_url: workspace.polygon_rpc_url,
        alchemy_api_key: masked_alchemy_key,
        arb_auto_execute: workspace.arb_auto_execute,
        copy_trading_enabled: workspace.copy_trading_enabled,
        live_trading_enabled: workspace.live_trading_enabled,
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

    // Validate WalletConnect project ID format if provided
    if let Some(ref project_id) = req.walletconnect_project_id {
        if !project_id.is_empty() && !is_valid_walletconnect_project_id(project_id) {
            return Err(ApiError::BadRequest(
                "Invalid WalletConnect project ID format. Expected 32-character alphanumeric string.".into(),
            ));
        }
    }

    // Validate Alchemy API key if provided
    if let Some(ref key) = req.alchemy_api_key {
        if key.is_empty() {
            return Err(ApiError::BadRequest(
                "Alchemy API key cannot be empty".into(),
            ));
        }
    }

    // Validate Polygon RPC URL format if provided (SSRF protection)
    if let Some(ref url_str) = req.polygon_rpc_url {
        if !url_str.is_empty() {
            let parsed = Url::parse(url_str)
                .map_err(|_| ApiError::BadRequest("Invalid Polygon RPC URL format".into()))?;

            if parsed.scheme() != "https" {
                return Err(ApiError::BadRequest(
                    "Polygon RPC URL must use HTTPS".into(),
                ));
            }

            // Block private/internal network addresses
            if let Some(host) = parsed.host_str() {
                if is_private_host(host) {
                    return Err(ApiError::BadRequest(
                        "Polygon RPC URL must not point to a private/internal address".into(),
                    ));
                }
            } else {
                return Err(ApiError::BadRequest(
                    "Polygon RPC URL must include a valid host".into(),
                ));
            }
        }
    }

    // Build dynamic update
    let now = Utc::now();
    let mut set_parts = vec!["updated_at = $2".to_string()];
    let mut param_idx = 3;

    // SAFETY: The $col arguments below MUST be hardcoded string literals (column names).
    // Never pass user-controlled input as $col — that would be SQL injection.
    macro_rules! add_param {
        ($field:ident, $col:literal) => {
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
    add_param!(walletconnect_project_id, "walletconnect_project_id");
    add_param!(polygon_rpc_url, "polygon_rpc_url");
    add_param!(alchemy_api_key, "alchemy_api_key");
    add_param!(arb_auto_execute, "arb_auto_execute");
    add_param!(copy_trading_enabled, "copy_trading_enabled");
    add_param!(live_trading_enabled, "live_trading_enabled");

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
    if let Some(ref walletconnect_project_id) = req.walletconnect_project_id {
        q = q.bind(walletconnect_project_id);
    }
    if let Some(ref polygon_rpc_url) = req.polygon_rpc_url {
        q = q.bind(polygon_rpc_url);
    }
    if let Some(ref alchemy_api_key) = req.alchemy_api_key {
        let encrypted = crypto::encrypt_field(alchemy_api_key, &state.encryption_key)
            .ok_or_else(|| ApiError::Internal("Failed to encrypt API key".into()))?;
        q = q.bind(encrypted);
    }
    if let Some(arb_auto_execute) = req.arb_auto_execute {
        q = q.bind(arb_auto_execute);
    }
    if let Some(copy_trading_enabled) = req.copy_trading_enabled {
        q = q.bind(copy_trading_enabled);
    }
    if let Some(live_trading_enabled) = req.live_trading_enabled {
        q = q.bind(live_trading_enabled);
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

/// Database row for optimizer status.
#[derive(Debug, sqlx::FromRow)]
struct OptimizerSettingsRow {
    auto_optimize_enabled: bool,
    optimization_interval_hours: i32,
    last_optimization_at: Option<DateTime<Utc>>,
    min_roi_30d: Option<Decimal>,
    min_sharpe: Option<Decimal>,
    min_win_rate: Option<Decimal>,
    min_trades_30d: Option<i32>,
    total_budget: Decimal,
}

/// Database row for wallet counts.
#[derive(Debug, sqlx::FromRow)]
struct WalletCountsRow {
    active_count: i64,
    bench_count: i64,
}

/// Database row for portfolio metrics.
#[derive(Debug, sqlx::FromRow)]
struct PortfolioMetricsRow {
    avg_roi: Decimal,
    avg_sharpe: Decimal,
    avg_win_rate: Decimal,
}

/// Get optimizer status for a workspace.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/optimizer-status",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Optimizer status", body = OptimizerStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
        (status = 404, description = "Workspace not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn get_optimizer_status(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<OptimizerStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Get workspace optimizer settings
    let settings: OptimizerSettingsRow = sqlx::query_as(
        r#"
        SELECT
            auto_optimize_enabled,
            optimization_interval_hours,
            last_optimization_at,
            min_roi_30d, min_sharpe, min_win_rate, min_trades_30d,
            total_budget
        FROM workspaces WHERE id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| ApiError::NotFound("Workspace not found".into()))?;

    // Get wallet counts by tier
    let counts: WalletCountsRow = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) FILTER (WHERE tier = 'active') as active_count,
            COUNT(*) FILTER (WHERE tier = 'bench') as bench_count
        FROM workspace_wallet_allocations WHERE workspace_id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    // Get aggregated portfolio metrics from active wallets
    let metrics: PortfolioMetricsRow = sqlx::query_as(
        r#"
        SELECT
            COALESCE(AVG(backtest_roi), 0) as avg_roi,
            COALESCE(AVG(backtest_sharpe), 0) as avg_sharpe,
            COALESCE(AVG(backtest_win_rate), 0) as avg_win_rate
        FROM workspace_wallet_allocations
        WHERE workspace_id = $1 AND tier = 'active'
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    // Calculate next run time
    let next_run_at = settings.last_optimization_at.map(|last_run| {
        last_run + chrono::Duration::hours(settings.optimization_interval_hours as i64)
    });

    Ok(Json(OptimizerStatusResponse {
        enabled: settings.auto_optimize_enabled,
        last_run_at: settings.last_optimization_at,
        next_run_at,
        interval_hours: settings.optimization_interval_hours,
        criteria: OptimizerCriteria {
            min_roi_30d: settings.min_roi_30d,
            min_sharpe: settings.min_sharpe,
            min_win_rate: settings.min_win_rate,
            min_trades_30d: settings.min_trades_30d,
        },
        active_wallet_count: counts.active_count as i32,
        bench_wallet_count: counts.bench_count as i32,
        portfolio_metrics: PortfolioMetrics {
            total_roi_30d: metrics.avg_roi,
            avg_sharpe: metrics.avg_sharpe,
            avg_win_rate: metrics.avg_win_rate,
            total_value: settings.total_budget,
        },
    }))
}

/// Service status for a single background service.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceStatusItem {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Aggregated service status response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceStatusResponse {
    pub harvester: ServiceStatusItem,
    pub metrics_calculator: ServiceStatusItem,
    pub copy_trading: ServiceStatusItem,
    pub arb_executor: ServiceStatusItem,
    pub live_trading: ServiceStatusItem,
}

const KEY_ARB_MIN_PROFIT_THRESHOLD: &str = "ARB_MIN_PROFIT_THRESHOLD";
const KEY_ARB_MONITOR_MAX_MARKETS: &str = "ARB_MONITOR_MAX_MARKETS";
const KEY_ARB_MONITOR_EXPLORATION_SLOTS: &str = "ARB_MONITOR_EXPLORATION_SLOTS";
const KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL: &str = "ARB_MONITOR_AGGRESSIVENESS_LEVEL";
const ARB_RUNTIME_STATS_LATEST: &str = "arb:runtime:stats:latest";

#[derive(Debug, sqlx::FromRow)]
struct DynamicConfigStatusRow {
    key: String,
    current_value: Decimal,
    default_value: Decimal,
    min_value: Decimal,
    max_value: Decimal,
    max_step_pct: Decimal,
    enabled: bool,
    last_good_value: Decimal,
    pending_eval: bool,
    last_applied_at: Option<DateTime<Utc>>,
    last_reason: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct DynamicTunerStateRow {
    last_run_at: Option<DateTime<Utc>>,
    last_run_status: Option<String>,
    last_run_reason: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct DynamicHistoryRow {
    id: i64,
    config_key: Option<String>,
    old_value: Option<Decimal>,
    new_value: Option<Decimal>,
    action: String,
    reason: String,
    metrics_snapshot: Option<serde_json::Value>,
    outcome_metrics: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct DynamicLastChangeRow {
    config_key: Option<String>,
    action: String,
    reason: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ScannerMarketInsightSnapshot {
    market_id: String,
    tier: String,
    total_score: f64,
    baseline_score: f64,
    opportunity_score: f64,
    hit_rate_score: f64,
    freshness_score: f64,
    sticky_score: f64,
    novelty_score: Option<f64>,
    rotation_score: Option<f64>,
    upside_score: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ArbRuntimeStatsSnapshot {
    monitored_markets: f64,
    #[serde(default)]
    core_markets: f64,
    #[serde(default)]
    exploration_markets: f64,
    #[serde(default)]
    last_rerank_at: Option<DateTime<Utc>>,
    #[serde(default)]
    last_resubscribe_at: Option<DateTime<Utc>>,
    #[serde(default)]
    selected_markets: Vec<ScannerMarketInsightSnapshot>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DynamicConfigItemResponse {
    pub key: String,
    pub current_value: f64,
    pub default_value: f64,
    pub min_value: f64,
    pub max_value: f64,
    pub max_step_pct: f64,
    pub enabled: bool,
    pub last_good_value: f64,
    pub pending_eval: bool,
    pub last_applied_at: Option<DateTime<Utc>>,
    pub last_reason: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DynamicSignalThresholdsResponse {
    pub min_net_profit_threshold_pct: f64,
    pub signal_cooldown_secs: i64,
    pub min_depth_usd: f64,
    pub trading_fee_pct: f64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ScannerMarketInsightResponse {
    pub market_id: String,
    pub tier: String,
    pub total_score: f64,
    pub baseline_score: f64,
    pub opportunity_score: f64,
    pub hit_rate_score: f64,
    pub freshness_score: f64,
    pub sticky_score: f64,
    pub novelty_score: Option<f64>,
    pub rotation_score: Option<f64>,
    pub upside_score: Option<f64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ScannerStatusResponse {
    pub monitored_markets: i64,
    pub core_markets: i64,
    pub exploration_markets: i64,
    pub last_rerank_at: Option<DateTime<Utc>>,
    pub last_resubscribe_at: Option<DateTime<Utc>>,
    pub selected_markets: Vec<ScannerMarketInsightResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpportunitySelectionStatusResponse {
    pub aggressiveness: String,
    pub aggressiveness_level: f64,
    pub exploration_slots: i64,
    pub max_markets_cap: i64,
    pub recommendation: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DynamicTunerStatusResponse {
    pub enabled: bool,
    pub apply_changes: bool,
    pub mode: String,
    pub current_regime: String,
    pub frozen: bool,
    pub freeze_reason: Option<String>,
    pub freeze_drawdown_threshold: f64,
    pub current_drawdown: f64,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_status: Option<String>,
    pub last_run_reason: Option<String>,
    pub last_change_at: Option<DateTime<Utc>>,
    pub last_change_action: Option<String>,
    pub last_change_reason: Option<String>,
    pub last_change_key: Option<String>,
    pub signal_thresholds: DynamicSignalThresholdsResponse,
    pub opportunity_selection: OpportunitySelectionStatusResponse,
    pub scanner_status: ScannerStatusResponse,
    pub dynamic_config: Vec<DynamicConfigItemResponse>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct DynamicHistoryQuery {
    /// Maximum results (default 50).
    #[serde(default = "default_dynamic_history_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_dynamic_history_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateOpportunitySelectionRequest {
    /// Aggressiveness profile: stable, balanced, discovery.
    pub aggressiveness: Option<String>,
    /// Number of exploration slots reserved in market selection.
    pub exploration_slots: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DynamicConfigHistoryEntryResponse {
    pub id: i64,
    pub config_key: Option<String>,
    pub old_value: Option<f64>,
    pub new_value: Option<f64>,
    pub action: String,
    pub reason: String,
    #[schema(value_type = Object)]
    pub metrics_snapshot: Option<serde_json::Value>,
    #[schema(value_type = Object)]
    pub outcome_metrics: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Get service status for a workspace.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/service-status",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Service status", body = ServiceStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn get_service_status(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<ServiceStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Check harvester: enabled by env
    let harvester_enabled = std::env::var("HARVESTER_ENABLED")
        .map(|v| v != "false")
        .unwrap_or(true);

    // Check metrics calculator: enabled by env
    let metrics_enabled = std::env::var("METRICS_CALCULATOR_ENABLED")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(true);

    // Check copy trading: needs POLYGON_RPC_URL or ALCHEMY_API_KEY
    let has_polygon =
        std::env::var("POLYGON_RPC_URL").is_ok() || std::env::var("ALCHEMY_API_KEY").is_ok();
    let copy_trading_env = std::env::var("COPY_TRADING_ENABLED")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // Check arb executor
    let arb_enabled = std::env::var("ARB_AUTO_EXECUTE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // Check live trading
    let live_trading = std::env::var("LIVE_TRADING")
        .map(|v| v == "true")
        .unwrap_or(false);
    let has_wallet_key = std::env::var("WALLET_PRIVATE_KEY").is_ok();

    Ok(Json(ServiceStatusResponse {
        harvester: ServiceStatusItem {
            running: harvester_enabled,
            reason: if !harvester_enabled {
                Some("HARVESTER_ENABLED is set to false".to_string())
            } else {
                None
            },
        },
        metrics_calculator: ServiceStatusItem {
            running: metrics_enabled,
            reason: if !metrics_enabled {
                Some("METRICS_CALCULATOR_ENABLED is disabled".to_string())
            } else {
                None
            },
        },
        copy_trading: ServiceStatusItem {
            running: copy_trading_env && has_polygon,
            reason: if !copy_trading_env {
                Some("COPY_TRADING_ENABLED is not set".to_string())
            } else if !has_polygon {
                Some("POLYGON_RPC_URL or ALCHEMY_API_KEY not configured".to_string())
            } else {
                None
            },
        },
        arb_executor: ServiceStatusItem {
            running: arb_enabled,
            reason: if !arb_enabled {
                Some("ARB_AUTO_EXECUTE is disabled".to_string())
            } else {
                None
            },
        },
        live_trading: ServiceStatusItem {
            running: live_trading && has_wallet_key,
            reason: if !live_trading {
                Some("LIVE_TRADING is not enabled".to_string())
            } else if !has_wallet_key {
                Some("No wallet key configured".to_string())
            } else {
                None
            },
        },
    }))
}

/// Get dynamic tuner status and active runtime thresholds.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/dynamic-tuning/status",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Dynamic tuner status", body = DynamicTunerStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn get_dynamic_tuner_status(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<DynamicTunerStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let rows: Vec<DynamicConfigStatusRow> = sqlx::query_as(
        r#"
        SELECT
            key, current_value, default_value, min_value, max_value,
            max_step_pct, enabled, last_good_value, pending_eval,
            last_applied_at, last_reason, updated_at
        FROM dynamic_config
        ORDER BY key
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let dynamic_config: Vec<DynamicConfigItemResponse> = rows
        .iter()
        .map(|row| DynamicConfigItemResponse {
            key: row.key.clone(),
            current_value: decimal_to_f64(row.current_value),
            default_value: decimal_to_f64(row.default_value),
            min_value: decimal_to_f64(row.min_value),
            max_value: decimal_to_f64(row.max_value),
            max_step_pct: decimal_to_f64(row.max_step_pct),
            enabled: row.enabled,
            last_good_value: decimal_to_f64(row.last_good_value),
            pending_eval: row.pending_eval,
            last_applied_at: row.last_applied_at,
            last_reason: row.last_reason.clone(),
            updated_at: row.updated_at,
        })
        .collect();

    let min_profit_ratio = rows
        .iter()
        .find(|row| row.key == KEY_ARB_MIN_PROFIT_THRESHOLD)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(|| env_f64(KEY_ARB_MIN_PROFIT_THRESHOLD, 0.005));
    let max_markets_cap = rows
        .iter()
        .find(|row| row.key == KEY_ARB_MONITOR_MAX_MARKETS)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(|| env_f64(KEY_ARB_MONITOR_MAX_MARKETS, 300.0));
    let exploration_slots = rows
        .iter()
        .find(|row| row.key == KEY_ARB_MONITOR_EXPLORATION_SLOTS)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(|| env_f64(KEY_ARB_MONITOR_EXPLORATION_SLOTS, 5.0));
    let aggressiveness_level = rows
        .iter()
        .find(|row| row.key == KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(env_aggressiveness_level);
    let aggressiveness = aggressiveness_label(aggressiveness_level).to_string();
    let recommendation = match aggressiveness.as_str() {
        "stable" => "Lower discovery, more stable execution.".to_string(),
        "discovery" => "Higher discovery, more rotation and churn.".to_string(),
        _ => "Balanced discovery and stability.".to_string(),
    };

    let signal_thresholds = DynamicSignalThresholdsResponse {
        min_net_profit_threshold_pct: min_profit_ratio * 100.0,
        signal_cooldown_secs: env_i64("ARB_SIGNAL_COOLDOWN_SECS", 60),
        min_depth_usd: env_f64("ARB_MIN_BOOK_DEPTH", 100.0),
        trading_fee_pct: env_f64(
            "ARB_TRADING_FEE_PCT",
            decimal_to_f64(ArbOpportunity::DEFAULT_FEE),
        ),
    };

    let runtime_state: Option<DynamicTunerStateRow> = sqlx::query_as(
        r#"
        SELECT last_run_at, last_run_status, last_run_reason
        FROM dynamic_tuner_state
        WHERE singleton = TRUE
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    let last_change: Option<DynamicLastChangeRow> = sqlx::query_as(
        r#"
        SELECT config_key, action, reason, created_at
        FROM dynamic_config_history
        WHERE action IN ('applied', 'rollback', 'recommended', 'manual_update')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.pool)
    .await?;

    let cb_state = state.circuit_breaker.state().await;
    let current_drawdown = if cb_state.peak_value > Decimal::ZERO {
        decimal_to_f64((cb_state.peak_value - cb_state.current_value) / cb_state.peak_value)
            .max(0.0)
    } else {
        0.0
    };
    let freeze_drawdown_threshold = env_f64("DYNAMIC_TUNER_FREEZE_DRAWDOWN", 0.20);
    let (frozen, freeze_reason) = if cb_state.tripped {
        (true, Some("circuit breaker is tripped".to_string()))
    } else if current_drawdown >= freeze_drawdown_threshold {
        (
            true,
            Some(format!(
                "drawdown {:.2}% exceeds freeze threshold {:.2}%",
                current_drawdown * 100.0,
                freeze_drawdown_threshold * 100.0
            )),
        )
    } else {
        (false, None)
    };

    let current_regime = format!("{:?}", *state.current_regime.read().await);
    let enabled = env_bool("DYNAMIC_TUNER_ENABLED", true);
    let apply_changes = env_bool("DYNAMIC_TUNER_APPLY", true);
    let (last_run_at, last_run_status, last_run_reason) = match runtime_state {
        Some(row) => (row.last_run_at, row.last_run_status, row.last_run_reason),
        None => (None, None, None),
    };
    let (last_change_at, last_change_action, last_change_reason, last_change_key) =
        match last_change {
            Some(row) => (
                Some(row.created_at),
                Some(row.action),
                Some(row.reason),
                row.config_key,
            ),
            None => (None, None, None, None),
        };

    let runtime_stats = fetch_arb_runtime_stats().await.unwrap_or_default();
    let scanner_status = ScannerStatusResponse {
        monitored_markets: runtime_stats.monitored_markets.round() as i64,
        core_markets: runtime_stats.core_markets.round() as i64,
        exploration_markets: runtime_stats.exploration_markets.round() as i64,
        last_rerank_at: runtime_stats.last_rerank_at,
        last_resubscribe_at: runtime_stats.last_resubscribe_at,
        selected_markets: runtime_stats
            .selected_markets
            .into_iter()
            .map(|market| ScannerMarketInsightResponse {
                market_id: market.market_id,
                tier: market.tier,
                total_score: market.total_score,
                baseline_score: market.baseline_score,
                opportunity_score: market.opportunity_score,
                hit_rate_score: market.hit_rate_score,
                freshness_score: market.freshness_score,
                sticky_score: market.sticky_score,
                novelty_score: market.novelty_score,
                rotation_score: market.rotation_score,
                upside_score: market.upside_score,
            })
            .collect(),
    };
    let opportunity_selection = OpportunitySelectionStatusResponse {
        aggressiveness,
        aggressiveness_level,
        exploration_slots: exploration_slots.round() as i64,
        max_markets_cap: max_markets_cap.round() as i64,
        recommendation,
    };

    Ok(Json(DynamicTunerStatusResponse {
        enabled,
        apply_changes,
        mode: if apply_changes {
            "apply".to_string()
        } else {
            "shadow".to_string()
        },
        current_regime,
        frozen,
        freeze_reason,
        freeze_drawdown_threshold,
        current_drawdown,
        last_run_at,
        last_run_status,
        last_run_reason,
        last_change_at,
        last_change_action,
        last_change_reason,
        last_change_key,
        signal_thresholds,
        opportunity_selection,
        scanner_status,
        dynamic_config,
    }))
}

/// Update opportunity-selection settings for arb market discovery.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/dynamic-tuning/opportunity-selection",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    request_body = UpdateOpportunitySelectionRequest,
    responses(
        (status = 200, description = "Updated opportunity-selection settings", body = OpportunitySelectionStatusResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn update_opportunity_selection_settings(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
    Json(req): Json<UpdateOpportunitySelectionRequest>,
) -> ApiResult<Json<OpportunitySelectionStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;
    if role != "owner" {
        return Err(ApiError::Forbidden(
            "Only workspace owner can update opportunity selection settings".into(),
        ));
    }

    if req.aggressiveness.is_none() && req.exploration_slots.is_none() {
        return Err(ApiError::BadRequest(
            "Provide at least one field to update".into(),
        ));
    }

    let mut updates: Vec<(String, Decimal, String)> = Vec::new();

    if let Some(aggressiveness) = req.aggressiveness.as_deref() {
        let level = aggressiveness_to_level(aggressiveness).ok_or_else(|| {
            ApiError::BadRequest("Aggressiveness must be stable, balanced, or discovery".into())
        })?;
        updates.push((
            KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL.to_string(),
            Decimal::from_f64_retain(level).unwrap_or(Decimal::new(1, 0)),
            format!("manual workspace update: aggressiveness={}", aggressiveness),
        ));
    }

    if let Some(exploration_slots) = req.exploration_slots {
        if !(1..=500).contains(&exploration_slots) {
            return Err(ApiError::BadRequest(
                "exploration_slots must be between 1 and 500".into(),
            ));
        }
        updates.push((
            KEY_ARB_MONITOR_EXPLORATION_SLOTS.to_string(),
            Decimal::new(exploration_slots, 0),
            format!(
                "manual workspace update: exploration_slots={}",
                exploration_slots
            ),
        ));
    }

    for (key, value, reason) in &updates {
        let old_value: Option<Decimal> =
            sqlx::query_scalar("SELECT current_value FROM dynamic_config WHERE key = $1")
                .bind(key)
                .fetch_optional(&state.pool)
                .await?;

        let (min_value, max_value, max_step_pct) =
            opportunity_dynamic_bounds(key).ok_or_else(|| {
                ApiError::Internal(format!("Unsupported dynamic config key: {}", key))
            })?;
        let clamped = (*value).max(min_value).min(max_value);

        sqlx::query(
            r#"
            INSERT INTO dynamic_config (
                key, current_value, default_value, min_value, max_value,
                max_step_pct, enabled, last_good_value, pending_eval, pending_baseline,
                last_applied_at, updated_by, last_reason
            )
            VALUES ($1, $2, $2, $3, $4, $5, TRUE, $2, FALSE, NULL, NULL, 'workspace_manual', $6)
            ON CONFLICT (key) DO UPDATE SET
                current_value = $2,
                min_value = $3,
                max_value = $4,
                max_step_pct = $5,
                last_good_value = $2,
                pending_eval = FALSE,
                pending_baseline = NULL,
                last_applied_at = NULL,
                updated_by = 'workspace_manual',
                last_reason = $6
            "#,
        )
        .bind(key)
        .bind(clamped)
        .bind(min_value)
        .bind(max_value)
        .bind(max_step_pct)
        .bind(reason)
        .execute(&state.pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO dynamic_config_history
                (config_key, old_value, new_value, action, reason)
            VALUES ($1, $2, $3, 'manual_update', $4)
            "#,
        )
        .bind(key)
        .bind(old_value)
        .bind(clamped)
        .bind(reason)
        .execute(&state.pool)
        .await?;
    }

    publish_manual_dynamic_updates(&updates)
        .await
        .map_err(|error| {
            ApiError::Internal(format!(
                "Failed publishing dynamic config updates to runtime subscribers: {}",
                error
            ))
        })?;

    // Return fresh status projection
    let dynamic_rows: Vec<DynamicConfigStatusRow> = sqlx::query_as(
        r#"
        SELECT
            key, current_value, default_value, min_value, max_value,
            max_step_pct, enabled, last_good_value, pending_eval,
            last_applied_at, last_reason, updated_at
        FROM dynamic_config
        WHERE key = ANY($1)
        ORDER BY key
        "#,
    )
    .bind(vec![
        KEY_ARB_MONITOR_EXPLORATION_SLOTS,
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL,
        KEY_ARB_MONITOR_MAX_MARKETS,
    ])
    .fetch_all(&state.pool)
    .await?;

    let max_markets_cap = dynamic_rows
        .iter()
        .find(|row| row.key == KEY_ARB_MONITOR_MAX_MARKETS)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(|| env_f64(KEY_ARB_MONITOR_MAX_MARKETS, 300.0));
    let exploration_slots = dynamic_rows
        .iter()
        .find(|row| row.key == KEY_ARB_MONITOR_EXPLORATION_SLOTS)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(|| env_f64(KEY_ARB_MONITOR_EXPLORATION_SLOTS, 5.0));
    let aggressiveness_level = dynamic_rows
        .iter()
        .find(|row| row.key == KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL)
        .map(|row| decimal_to_f64(row.current_value))
        .unwrap_or_else(env_aggressiveness_level);
    let aggressiveness = aggressiveness_label(aggressiveness_level).to_string();
    let recommendation = match aggressiveness.as_str() {
        "stable" => "Lower discovery, more stable execution.".to_string(),
        "discovery" => "Higher discovery, more rotation and churn.".to_string(),
        _ => "Balanced discovery and stability.".to_string(),
    };

    Ok(Json(OpportunitySelectionStatusResponse {
        aggressiveness,
        aggressiveness_level,
        exploration_slots: exploration_slots.round() as i64,
        max_markets_cap: max_markets_cap.round() as i64,
        recommendation,
    }))
}

/// List dynamic tuning history (changes, recommendations, rollbacks, evaluations).
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/dynamic-tuning/history",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        DynamicHistoryQuery
    ),
    responses(
        (status = 200, description = "Dynamic tuning history", body = Vec<DynamicConfigHistoryEntryResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
    ),
    security(("bearer_auth" = [])),
    tag = "workspaces"
)]
pub async fn get_dynamic_tuning_history(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
    Query(query): Query<DynamicHistoryQuery>,
) -> ApiResult<Json<Vec<DynamicConfigHistoryEntryResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let limit = query.limit.clamp(1, 200);
    let offset = query.offset.max(0);
    let rows: Vec<DynamicHistoryRow> = sqlx::query_as(
        r#"
        SELECT
            id, config_key, old_value, new_value, action, reason,
            metrics_snapshot, outcome_metrics, created_at
        FROM dynamic_config_history
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;

    let history = rows
        .into_iter()
        .map(|row| DynamicConfigHistoryEntryResponse {
            id: row.id,
            config_key: row.config_key,
            old_value: row.old_value.map(decimal_to_f64),
            new_value: row.new_value.map(decimal_to_f64),
            action: row.action,
            reason: row.reason,
            metrics_snapshot: row.metrics_snapshot,
            outcome_metrics: row.outcome_metrics,
            created_at: row.created_at,
        })
        .collect();

    Ok(Json(history))
}

fn env_bool(name: &str, fallback: bool) -> bool {
    std::env::var(name)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        })
        .unwrap_or(fallback)
}

fn env_aggressiveness_level() -> f64 {
    match std::env::var("ARB_MONITOR_AGGRESSIVENESS")
        .unwrap_or_else(|_| "balanced".to_string())
        .to_lowercase()
        .as_str()
    {
        "stable" | "conservative" => 0.0,
        "discovery" | "aggressive" => 2.0,
        _ => 1.0,
    }
}

fn aggressiveness_label(level: f64) -> &'static str {
    if level <= 0.5 {
        "stable"
    } else if level >= 1.5 {
        "discovery"
    } else {
        "balanced"
    }
}

fn aggressiveness_to_level(value: &str) -> Option<f64> {
    match value.trim().to_lowercase().as_str() {
        "stable" | "conservative" => Some(0.0),
        "balanced" => Some(1.0),
        "discovery" | "aggressive" => Some(2.0),
        _ => None,
    }
}

fn opportunity_dynamic_bounds(key: &str) -> Option<(Decimal, Decimal, Decimal)> {
    match key {
        KEY_ARB_MONITOR_EXPLORATION_SLOTS => Some((
            Decimal::new(1, 0),
            Decimal::new(500, 0),
            Decimal::new(25, 2),
        )),
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL => {
            Some((Decimal::ZERO, Decimal::new(2, 0), Decimal::new(100, 2)))
        }
        _ => None,
    }
}

async fn publish_manual_dynamic_updates(
    updates: &[(String, Decimal, String)],
) -> Result<(), redis::RedisError> {
    let redis_url = std::env::var("DYNAMIC_TUNER_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(redis_url.as_str())?;
    let mut redis = redis::aio::ConnectionManager::new(client).await?;

    for (key, value, reason) in updates {
        let payload = crate::dynamic_tuner::DynamicConfigUpdate {
            key: key.clone(),
            value: *value,
            reason: reason.clone(),
            source: "workspace_manual".to_string(),
            timestamp: Utc::now(),
            metrics: serde_json::json!({ "source": "workspace_settings" }),
        };
        let serialized = serde_json::to_string(&payload).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Failed serializing payload"))
        })?;
        let _: () = redis
            .publish(crate::dynamic_tuner::channels::CONFIG_UPDATES, serialized)
            .await?;
    }

    Ok(())
}

async fn fetch_arb_runtime_stats() -> Option<ArbRuntimeStatsSnapshot> {
    let redis_url = std::env::var("DYNAMIC_CONFIG_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = redis::Client::open(redis_url.as_str()).ok()?;
    let mut redis = redis::aio::ConnectionManager::new(client).await.ok()?;
    let payload: Option<String> = redis.get(ARB_RUNTIME_STATS_LATEST).await.ok()?;
    payload.and_then(|raw| serde_json::from_str::<ArbRuntimeStatsSnapshot>(&raw).ok())
}

fn env_f64(name: &str, fallback: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(fallback)
}

fn env_i64(name: &str, fallback: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(fallback)
}

fn decimal_to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

/// Validate WalletConnect project ID format.
/// Expected: 32-character alphanumeric string (hex).
fn is_valid_walletconnect_project_id(project_id: &str) -> bool {
    // WalletConnect project IDs are typically 32 character hex strings
    project_id.len() == 32 && project_id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Check if a hostname resolves to a private/internal network address.
/// Used to prevent SSRF attacks via user-supplied URLs.
fn is_private_host(host: &str) -> bool {
    // Check common private hostnames
    let lower = host.to_lowercase();
    if lower == "localhost"
        || lower == "0.0.0.0"
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return true;
    }

    // Check if the host is a private IP address
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()          // 127.0.0.0/8
                    || v4.is_private()     // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                    || v4.is_link_local()  // 169.254.0.0/16
                    || v4.is_unspecified() // 0.0.0.0
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()          // ::1
                    || v6.is_unspecified() // ::
            }
        };
    }

    false
}
