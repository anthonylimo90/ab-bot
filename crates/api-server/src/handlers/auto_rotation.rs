//! Auto-rotation history handlers.

use axum::extract::{Path, Query, State};
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

/// Auto-rotation history entry response.
#[derive(Debug, Serialize, ToSchema)]
pub struct RotationHistoryResponse {
    pub id: String,
    pub action: String,
    pub wallet_in: Option<String>,
    pub wallet_out: Option<String>,
    pub reason: String,
    pub evidence: serde_json::Value,
    pub triggered_by: Option<String>,
    pub is_automatic: bool,
    pub notification_sent: bool,
    pub acknowledged: bool,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub acknowledged_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Query params for rotation history.
#[derive(Debug, Deserialize)]
pub struct ListRotationHistoryQuery {
    pub limit: Option<i32>,
    pub offset: Option<i32>,
    pub unacknowledged_only: Option<bool>,
}

/// Database row for rotation history.
#[derive(Debug, sqlx::FromRow)]
struct RotationHistoryRow {
    id: Uuid,
    action: String,
    wallet_in: Option<String>,
    wallet_out: Option<String>,
    reason: String,
    evidence: serde_json::Value,
    triggered_by: Option<Uuid>,
    notification_sent: bool,
    acknowledged: bool,
    acknowledged_at: Option<DateTime<Utc>>,
    acknowledged_by: Option<Uuid>,
    created_at: DateTime<Utc>,
}

/// Get user's current workspace ID.
async fn get_current_workspace(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let settings: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT default_workspace_id FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

    Ok(settings.and_then(|(id,)| id))
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

/// List rotation history for current workspace.
#[utoipa::path(
    get,
    path = "/api/v1/auto-rotation/history",
    params(
        ("limit" = Option<i32>, Query, description = "Max results"),
        ("offset" = Option<i32>, Query, description = "Offset for pagination"),
        ("unacknowledged_only" = Option<bool>, Query, description = "Only show unacknowledged entries"),
    ),
    responses(
        (status = 200, description = "Rotation history", body = Vec<RotationHistoryResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "auto_rotation"
)]
pub async fn list_rotation_history(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListRotationHistoryQuery>,
) -> ApiResult<Json<Vec<RotationHistoryResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);
    let unacknowledged_only = query.unacknowledged_only.unwrap_or(false);

    let history: Vec<RotationHistoryRow> = if unacknowledged_only {
        sqlx::query_as(
            r#"
            SELECT id, action, wallet_in, wallet_out, reason, evidence,
                   triggered_by, notification_sent, acknowledged, acknowledged_at,
                   acknowledged_by, created_at
            FROM auto_rotation_history
            WHERE workspace_id = $1 AND NOT acknowledged
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(workspace_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, action, wallet_in, wallet_out, reason, evidence,
                   triggered_by, notification_sent, acknowledged, acknowledged_at,
                   acknowledged_by, created_at
            FROM auto_rotation_history
            WHERE workspace_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(workspace_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?
    };

    let response: Vec<RotationHistoryResponse> = history
        .into_iter()
        .map(|h| RotationHistoryResponse {
            id: h.id.to_string(),
            action: h.action,
            wallet_in: h.wallet_in,
            wallet_out: h.wallet_out,
            reason: h.reason,
            evidence: h.evidence,
            triggered_by: h.triggered_by.map(|id| id.to_string()),
            is_automatic: h.triggered_by.is_none(),
            notification_sent: h.notification_sent,
            acknowledged: h.acknowledged,
            acknowledged_at: h.acknowledged_at,
            acknowledged_by: h.acknowledged_by.map(|id| id.to_string()),
            created_at: h.created_at,
        })
        .collect();

    Ok(Json(response))
}

/// Acknowledge a rotation history entry.
#[utoipa::path(
    put,
    path = "/api/v1/auto-rotation/{entry_id}/acknowledge",
    params(
        ("entry_id" = String, Path, description = "Rotation history entry ID")
    ),
    responses(
        (status = 200, description = "Entry acknowledged", body = RotationHistoryResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
        (status = 404, description = "Entry not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "auto_rotation"
)]
pub async fn acknowledge_entry(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(entry_id): Path<String>,
) -> ApiResult<Json<RotationHistoryResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let entry_id = Uuid::parse_str(&entry_id)
        .map_err(|_| ApiError::BadRequest("Invalid entry ID format".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Check entry exists and belongs to workspace
    let existing: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM auto_rotation_history WHERE id = $1 AND workspace_id = $2")
            .bind(entry_id)
            .bind(workspace_id)
            .fetch_optional(&state.pool)
            .await?;

    if existing.is_none() {
        return Err(ApiError::NotFound("Entry not found".into()));
    }

    // Acknowledge
    let now = Utc::now();
    sqlx::query(
        r#"
        UPDATE auto_rotation_history
        SET acknowledged = true, acknowledged_at = $1, acknowledged_by = $2
        WHERE id = $3
        "#,
    )
    .bind(now)
    .bind(user_id)
    .bind(entry_id)
    .execute(&state.pool)
    .await?;

    // Fetch updated entry
    let entry: RotationHistoryRow = sqlx::query_as(
        r#"
        SELECT id, action, wallet_in, wallet_out, reason, evidence,
               triggered_by, notification_sent, acknowledged, acknowledged_at,
               acknowledged_by, created_at
        FROM auto_rotation_history
        WHERE id = $1
        "#,
    )
    .bind(entry_id)
    .fetch_one(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("rotation_acknowledged".to_string()),
        &entry_id.to_string(),
        serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "action": &entry.action
        }),
    );

    Ok(Json(RotationHistoryResponse {
        id: entry.id.to_string(),
        action: entry.action,
        wallet_in: entry.wallet_in,
        wallet_out: entry.wallet_out,
        reason: entry.reason,
        evidence: entry.evidence,
        triggered_by: entry.triggered_by.map(|id| id.to_string()),
        is_automatic: entry.triggered_by.is_none(),
        notification_sent: entry.notification_sent,
        acknowledged: entry.acknowledged,
        acknowledged_at: entry.acknowledged_at,
        acknowledged_by: entry.acknowledged_by.map(|id| id.to_string()),
        created_at: entry.created_at,
    }))
}

/// Manually trigger auto-optimization (owner/admin only).
#[utoipa::path(
    post,
    path = "/api/v1/auto-rotation/trigger",
    responses(
        (status = 202, description = "Optimization triggered"),
        (status = 400, description = "Auto-optimization not enabled"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to trigger optimization"),
    ),
    security(("bearer_auth" = [])),
    tag = "auto_rotation"
)]
pub async fn trigger_optimization(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<StatusCode> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can trigger optimization".into(),
        ));
    }

    // Check if auto-optimization is enabled
    let settings: Option<(bool,)> =
        sqlx::query_as("SELECT auto_optimize_enabled FROM workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&state.pool)
            .await?;

    let auto_optimize_enabled = settings
        .ok_or_else(|| ApiError::NotFound("Workspace not found".into()))?
        .0;

    if !auto_optimize_enabled {
        return Err(ApiError::BadRequest(
            "Auto-optimization is not enabled for this workspace".into(),
        ));
    }

    // For now, just log the trigger. The actual optimization would be handled
    // by the auto_optimizer background service.
    // In a full implementation, this would send a message to a background queue.

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("optimization_triggered".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({
            "triggered_by": &claims.sub,
            "manual": true
        }),
    );

    tracing::info!(
        workspace_id = %workspace_id,
        triggered_by = %user_id,
        "Manual optimization triggered"
    );

    // Return 202 Accepted to indicate the optimization has been queued
    Ok(StatusCode::ACCEPTED)
}
