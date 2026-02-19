//! Auto-rotation history handlers.

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::{AuditAction, Claims};

use crate::auto_optimizer::AutoOptimizer;
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

fn normalize_ratio_opt(value: Option<Decimal>, fallback: f64) -> f64 {
    let parsed = value
        .and_then(|d| d.to_string().parse::<f64>().ok())
        .unwrap_or(fallback);
    if parsed.abs() > 1.0 {
        parsed / 100.0
    } else {
        parsed
    }
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

/// Optimization result response.
#[derive(Debug, Serialize, ToSchema)]
pub struct OptimizationResultResponse {
    pub candidates_found: i32,
    pub wallets_promoted: i32,
    pub thresholds: OptimizationThresholds,
    pub message: String,
}

/// Thresholds used for optimization.
#[derive(Debug, Serialize, ToSchema)]
pub struct OptimizationThresholds {
    pub min_roi_30d: Option<f64>,
    pub min_sharpe: Option<f64>,
    pub min_win_rate: Option<f64>,
    pub min_trades_30d: Option<i32>,
}

/// Manually trigger auto-optimization (owner/admin only).
#[utoipa::path(
    post,
    path = "/api/v1/auto-rotation/trigger",
    responses(
        (status = 200, description = "Optimization completed successfully", body = OptimizationResultResponse),
        (status = 400, description = "Auto-optimization not enabled"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to trigger optimization"),
        (status = 500, description = "Optimization failed"),
    ),
    security(("bearer_auth" = [])),
    tag = "auto_rotation"
)]
pub async fn trigger_optimization(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<OptimizationResultResponse>> {
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

    // Check if auto-optimization is enabled and get thresholds
    #[allow(clippy::type_complexity)]
    let settings: Option<(
        bool,
        bool,
        bool,
        Option<rust_decimal::Decimal>,
        Option<rust_decimal::Decimal>,
        Option<rust_decimal::Decimal>,
        Option<i32>,
    )> = sqlx::query_as(
        r#"SELECT auto_optimize_enabled, auto_select_enabled, auto_demote_enabled,
                      min_roi_30d, min_sharpe, min_win_rate, min_trades_30d
               FROM workspaces WHERE id = $1"#,
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;

    let (
        auto_optimize_enabled,
        auto_select_enabled,
        auto_demote_enabled,
        min_roi,
        min_sharpe,
        min_win_rate,
        min_trades,
    ) = settings.ok_or_else(|| ApiError::NotFound("Workspace not found".into()))?;
    let min_roi_norm = normalize_ratio_opt(min_roi, 0.05);
    let min_win_rate_norm = normalize_ratio_opt(min_win_rate, 0.50);

    // Allow trigger if either old auto_optimize or new auto_select/demote is enabled
    if !auto_optimize_enabled && !auto_select_enabled && !auto_demote_enabled {
        return Err(ApiError::BadRequest(
            "Auto-optimization is not enabled for this workspace. Enable auto-select or auto-demote in settings.".into(),
        ));
    }

    // Get active wallet count before optimization
    let before_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND tier = 'active'"
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    tracing::info!(
        workspace_id = %workspace_id,
        triggered_by = %user_id,
        "Manual optimization triggered"
    );

    // Run the optimization synchronously
    let optimizer = AutoOptimizer::new(state.pool.clone())
        .with_runtime_handles(state.trade_monitor.clone(), state.copy_trader.clone());
    optimizer
        .optimize_workspace_by_id(workspace_id)
        .await
        .map_err(|e| {
            tracing::error!(workspace_id = %workspace_id, error = %e, "Optimization failed");
            ApiError::Internal(format!("Optimization failed: {}", e))
        })?;

    // Get active wallet count after optimization
    let after_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND tier = 'active'"
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    // Get count of candidates that meet thresholds
    let candidates_count: (i64,) = sqlx::query_as(
        r#"
        WITH candidate_metrics AS (
            SELECT
                COALESCE(wsm.address, wf.address) AS address,
                CASE
                    WHEN ABS(COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)) > 1
                        THEN COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0) / 100
                    ELSE COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)
                END AS roi_30d,
                COALESCE(wsm.sharpe_30d, 0) AS sharpe_30d,
                CASE
                    WHEN ABS(COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)) > 1
                        THEN COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0) / 100
                    ELSE COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)
                END AS win_rate_30d,
                COALESCE(wsm.trades_30d, wf.total_trades::integer, 0) AS trades_30d
            FROM wallet_success_metrics wsm
            FULL OUTER JOIN wallet_features wf ON wf.address = wsm.address
            WHERE COALESCE(wsm.address, wf.address) IS NOT NULL
        )
        SELECT COUNT(*) FROM candidate_metrics
        WHERE roi_30d >= $1::numeric
          AND sharpe_30d >= $2::numeric
          AND win_rate_30d >= $3::numeric
          AND trades_30d >= $4
        "#,
    )
    .bind(min_roi_norm)
    .bind(min_sharpe)
    .bind(min_win_rate_norm)
    .bind(min_trades)
    .fetch_one(&state.pool)
    .await?;

    let wallets_promoted = (after_count.0 - before_count.0).max(0) as i32;
    let candidates_found = candidates_count.0 as i32;

    // Audit log after successful optimization
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("optimization_completed".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({
            "triggered_by": &claims.sub,
            "manual": true,
            "candidates_found": candidates_found,
            "wallets_promoted": wallets_promoted
        }),
    );

    tracing::info!(
        workspace_id = %workspace_id,
        triggered_by = %user_id,
        candidates_found = candidates_found,
        wallets_promoted = wallets_promoted,
        "Manual optimization completed"
    );

    let message = if wallets_promoted > 0 {
        format!("{} wallet(s) promoted to active roster", wallets_promoted)
    } else if candidates_found == 0 {
        "No wallets meet current thresholds".to_string()
    } else {
        "Roster is full - no changes made".to_string()
    };

    Ok(Json(OptimizationResultResponse {
        candidates_found,
        wallets_promoted,
        thresholds: OptimizationThresholds {
            min_roi_30d: Some(min_roi_norm * 100.0),
            min_sharpe: min_sharpe.map(|d| d.to_string().parse().unwrap_or(1.0)),
            min_win_rate: Some(min_win_rate_norm * 100.0),
            min_trades_30d: min_trades,
        },
        message,
    }))
}
