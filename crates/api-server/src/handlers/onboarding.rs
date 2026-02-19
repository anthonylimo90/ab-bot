//! Onboarding and setup wizard handlers.

use axum::extract::State;
use axum::Extension;
use axum::Json;
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::{AuditAction, AuditEvent, Claims};

use crate::error::{ApiError, ApiResult};
use crate::runtime_sync::reconcile_copy_runtime;
use crate::state::AppState;

/// Onboarding status response.
#[derive(Debug, Serialize, ToSchema)]
pub struct OnboardingStatusResponse {
    pub has_workspace: bool,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub onboarding_completed: bool,
    pub onboarding_step: i32,
    pub setup_mode: Option<String>,
    pub total_budget: Option<Decimal>,
    pub active_wallet_count: i64,
    pub bench_wallet_count: i64,
}

/// Set mode request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetModeRequest {
    pub mode: String,
}

/// Set budget request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetBudgetRequest {
    pub total_budget: Decimal,
    pub reserved_cash_pct: Option<Decimal>,
}

/// Auto-setup request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AutoSetupRequest {
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,
}

/// Auto-setup response.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutoSetupResponse {
    pub success: bool,
    pub message: String,
    pub selected_wallets: Vec<AutoSelectedWallet>,
}

/// Auto-selected wallet info.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutoSelectedWallet {
    pub address: String,
    pub allocation_pct: Decimal,
    pub roi_30d: Option<Decimal>,
    pub sharpe_ratio: Option<Decimal>,
    pub win_rate: Option<Decimal>,
    pub reason: String,
}

fn normalize_ratio_threshold(value: Decimal, fallback: f64) -> f64 {
    let parsed = value.to_string().parse::<f64>().unwrap_or(fallback);
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

/// Get onboarding status.
#[utoipa::path(
    get,
    path = "/api/v1/onboarding/status",
    responses(
        (status = 200, description = "Onboarding status", body = OnboardingStatusResponse),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "onboarding"
)]
pub async fn get_status(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<OnboardingStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    // Get user settings
    #[derive(sqlx::FromRow)]
    struct UserSettingsRow {
        onboarding_completed: bool,
        onboarding_step: i32,
        default_workspace_id: Option<Uuid>,
    }

    let settings: Option<UserSettingsRow> = sqlx::query_as(
        "SELECT onboarding_completed, onboarding_step, default_workspace_id FROM user_settings WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    let settings = settings.unwrap_or(UserSettingsRow {
        onboarding_completed: false,
        onboarding_step: 0,
        default_workspace_id: None,
    });

    // If user has a workspace, get workspace details
    if let Some(workspace_id) = settings.default_workspace_id {
        #[derive(sqlx::FromRow)]
        struct WorkspaceInfo {
            name: String,
            setup_mode: String,
            total_budget: Decimal,
        }

        let workspace: Option<WorkspaceInfo> =
            sqlx::query_as("SELECT name, setup_mode, total_budget FROM workspaces WHERE id = $1")
                .bind(workspace_id)
                .fetch_optional(&state.pool)
                .await?;

        if let Some(ws) = workspace {
            // Count wallets
            let active_count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND tier = 'active'",
            )
            .bind(workspace_id)
            .fetch_one(&state.pool)
            .await?;

            let bench_count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND tier = 'bench'",
            )
            .bind(workspace_id)
            .fetch_one(&state.pool)
            .await?;

            return Ok(Json(OnboardingStatusResponse {
                has_workspace: true,
                workspace_id: Some(workspace_id.to_string()),
                workspace_name: Some(ws.name),
                onboarding_completed: settings.onboarding_completed,
                onboarding_step: settings.onboarding_step,
                setup_mode: Some(ws.setup_mode),
                total_budget: Some(ws.total_budget),
                active_wallet_count: active_count.0,
                bench_wallet_count: bench_count.0,
            }));
        }
    }

    // No workspace
    Ok(Json(OnboardingStatusResponse {
        has_workspace: false,
        workspace_id: None,
        workspace_name: None,
        onboarding_completed: settings.onboarding_completed,
        onboarding_step: settings.onboarding_step,
        setup_mode: None,
        total_budget: None,
        active_wallet_count: 0,
        bench_wallet_count: 0,
    }))
}

/// Set workspace setup mode.
#[utoipa::path(
    put,
    path = "/api/v1/onboarding/mode",
    request_body = SetModeRequest,
    responses(
        (status = 200, description = "Mode set"),
        (status = 400, description = "Invalid mode"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not workspace owner"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "onboarding"
)]
pub async fn set_mode(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SetModeRequest>,
) -> ApiResult<Json<OnboardingStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if role != "owner" {
        return Err(ApiError::Forbidden(
            "Only workspace owner can change setup mode".into(),
        ));
    }

    // Validate mode
    let mode = req.mode.to_lowercase();
    if !["manual", "automatic"].contains(&mode.as_str()) {
        return Err(ApiError::BadRequest(
            "Mode must be 'manual' or 'automatic'".into(),
        ));
    }

    // Update workspace
    let now = Utc::now();
    sqlx::query("UPDATE workspaces SET setup_mode = $1, updated_at = $2 WHERE id = $3")
        .bind(&mode)
        .bind(now)
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    // Update onboarding step
    sqlx::query(
        r#"
        UPDATE user_settings SET onboarding_step = GREATEST(onboarding_step, 1), updated_at = $1
        WHERE user_id = $2
        "#,
    )
    .bind(now)
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("onboarding_mode_set".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({ "mode": &mode }),
    );

    get_status(State(state), Extension(claims)).await
}

/// Set workspace budget.
#[utoipa::path(
    put,
    path = "/api/v1/onboarding/budget",
    request_body = SetBudgetRequest,
    responses(
        (status = 200, description = "Budget set"),
        (status = 400, description = "Invalid budget"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not workspace owner"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "onboarding"
)]
pub async fn set_budget(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SetBudgetRequest>,
) -> ApiResult<Json<OnboardingStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if role != "owner" {
        return Err(ApiError::Forbidden(
            "Only workspace owner can set budget".into(),
        ));
    }

    // Validate budget
    if req.total_budget < Decimal::ZERO {
        return Err(ApiError::BadRequest("Budget cannot be negative".into()));
    }

    let reserved_cash_pct = req.reserved_cash_pct.unwrap_or(Decimal::new(10, 0));
    if reserved_cash_pct < Decimal::ZERO || reserved_cash_pct > Decimal::new(100, 0) {
        return Err(ApiError::BadRequest(
            "Reserved cash percentage must be between 0 and 100".into(),
        ));
    }

    // Update workspace
    let now = Utc::now();
    sqlx::query(
        "UPDATE workspaces SET total_budget = $1, reserved_cash_pct = $2, updated_at = $3 WHERE id = $4",
    )
    .bind(req.total_budget)
    .bind(reserved_cash_pct)
    .bind(now)
    .bind(workspace_id)
    .execute(&state.pool)
    .await?;

    // Update onboarding step
    sqlx::query(
        r#"
        UPDATE user_settings SET onboarding_step = GREATEST(onboarding_step, 2), updated_at = $1
        WHERE user_id = $2
        "#,
    )
    .bind(now)
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("onboarding_budget_set".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({
            "total_budget": req.total_budget.to_string(),
            "reserved_cash_pct": reserved_cash_pct.to_string()
        }),
    );

    get_status(State(state), Extension(claims)).await
}

/// Trigger automatic wallet selection.
#[utoipa::path(
    post,
    path = "/api/v1/onboarding/auto-setup",
    request_body = AutoSetupRequest,
    responses(
        (status = 200, description = "Auto-setup completed", body = AutoSetupResponse),
        (status = 400, description = "Invalid criteria or not in automatic mode"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not workspace owner"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "onboarding"
)]
pub async fn auto_setup(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<AutoSetupRequest>,
) -> ApiResult<Json<AutoSetupResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if role != "owner" {
        return Err(ApiError::Forbidden(
            "Only workspace owner can run auto-setup".into(),
        ));
    }

    // Check workspace is in automatic mode
    let workspace: Option<(String,)> =
        sqlx::query_as("SELECT setup_mode FROM workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&state.pool)
            .await?;

    let setup_mode = workspace
        .ok_or_else(|| ApiError::NotFound("Workspace not found".into()))?
        .0;

    if setup_mode != "automatic" {
        return Err(ApiError::BadRequest(
            "Auto-setup is only available in automatic mode".into(),
        ));
    }

    // Update criteria in workspace
    let now = Utc::now();
    sqlx::query(
        r#"
        UPDATE workspaces SET
            min_roi_30d = COALESCE($1, min_roi_30d),
            min_sharpe = COALESCE($2, min_sharpe),
            min_win_rate = COALESCE($3, min_win_rate),
            min_trades_30d = COALESCE($4, min_trades_30d),
            updated_at = $5
        WHERE id = $6
        "#,
    )
    .bind(req.min_roi_30d)
    .bind(req.min_sharpe)
    .bind(req.min_win_rate)
    .bind(req.min_trades_30d)
    .bind(now)
    .bind(workspace_id)
    .execute(&state.pool)
    .await?;

    // Get criteria (either from request or workspace defaults)
    let min_roi = req.min_roi_30d.unwrap_or(Decimal::new(5, 0));
    let min_sharpe = req.min_sharpe.unwrap_or(Decimal::ONE);
    let min_win_rate = req.min_win_rate.unwrap_or(Decimal::new(50, 0));
    let min_trades = req.min_trades_30d.unwrap_or(10);
    let min_roi_norm = normalize_ratio_threshold(min_roi, 0.05);
    let min_win_rate_norm = normalize_ratio_threshold(min_win_rate, 0.50);

    // Query top wallets from wallet_success_metrics
    #[derive(sqlx::FromRow)]
    struct WalletCandidate {
        address: String,
        roi: Option<Decimal>,
        sharpe_ratio: Option<Decimal>,
        win_rate: Option<Decimal>,
        total_trades: i64,
    }

    let candidates: Vec<WalletCandidate> = sqlx::query_as(
        r#"
        WITH candidate_metrics AS (
            SELECT
                COALESCE(wsm.address, wf.address) as address,
                CASE
                    WHEN ABS(COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)) > 1
                        THEN COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0) / 100
                    ELSE COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)
                END as roi,
                COALESCE(wsm.sharpe_30d, 0) as sharpe_ratio,
                CASE
                    WHEN ABS(COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)) > 1
                        THEN COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0) / 100
                    ELSE COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)
                END as win_rate,
                COALESCE(wsm.trades_30d, wf.total_trades::bigint, 0) as total_trades
            FROM wallet_success_metrics wsm
            FULL OUTER JOIN wallet_features wf ON wf.address = wsm.address
            WHERE COALESCE(wsm.address, wf.address) IS NOT NULL
        )
        SELECT
            address,
            roi,
            sharpe_ratio,
            win_rate,
            total_trades
        FROM candidate_metrics
        WHERE
            roi >= $1::numeric
            AND sharpe_ratio >= $2::numeric
            AND win_rate >= $3::numeric
            AND total_trades >= $4
        ORDER BY
            (COALESCE(roi, 0) * 0.4 + COALESCE(sharpe_ratio, 0) * 0.3 + COALESCE(win_rate, 0) * 0.3) DESC
        LIMIT 5
        "#,
    )
    .bind(min_roi_norm)
    .bind(min_sharpe)
    .bind(min_win_rate_norm)
    .bind(min_trades as i64)
    .fetch_all(&state.pool)
    .await?;

    if candidates.is_empty() {
        return Ok(Json(AutoSetupResponse {
            success: false,
            message: "No wallets found matching the criteria. Try lowering the thresholds.".into(),
            selected_wallets: vec![],
        }));
    }

    // Clear existing allocations for this workspace
    sqlx::query("DELETE FROM workspace_wallet_allocations WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    // Calculate allocations (equal weight for now, could be risk-adjusted)
    let allocation_per_wallet = Decimal::new(100, 0) / Decimal::from(candidates.len() as i64);

    let mut selected_wallets = Vec::new();

    for candidate in &candidates {
        let allocation_id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO workspace_wallet_allocations (
                id, workspace_id, wallet_address, allocation_pct, tier,
                auto_assigned, auto_assigned_reason,
                backtest_roi, backtest_sharpe, backtest_win_rate,
                added_at, updated_at
            ) VALUES ($1, $2, $3, $4, 'active', true, $5, $6, $7, $8, $9, $9)
            "#,
        )
        .bind(allocation_id)
        .bind(workspace_id)
        .bind(&candidate.address)
        .bind(allocation_per_wallet)
        .bind(format!(
            "Auto-selected: ROI {:.2}%, Sharpe {:.2}, WinRate {:.1}%",
            candidate.roi.unwrap_or_default() * Decimal::new(100, 0),
            candidate.sharpe_ratio.unwrap_or_default(),
            candidate.win_rate.unwrap_or_default() * Decimal::new(100, 0)
        ))
        .bind(candidate.roi)
        .bind(candidate.sharpe_ratio)
        .bind(candidate.win_rate)
        .bind(now)
        .execute(&state.pool)
        .await?;

        selected_wallets.push(AutoSelectedWallet {
            address: candidate.address.clone(),
            allocation_pct: allocation_per_wallet,
            roi_30d: candidate.roi.map(|v| v * Decimal::new(100, 0)),
            sharpe_ratio: candidate.sharpe_ratio,
            win_rate: candidate.win_rate.map(|v| v * Decimal::new(100, 0)),
            reason: format!("Top performer with {} trades", candidate.total_trades),
        });
    }

    reconcile_copy_runtime(
        &state.pool,
        state.trade_monitor.as_ref(),
        state.copy_trader.as_ref(),
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to reconcile copy runtime: {e}")))?;

    // Log rotation history
    sqlx::query(
        r#"
        INSERT INTO auto_rotation_history (workspace_id, action, reason, evidence, triggered_by, created_at)
        VALUES ($1, 'add', 'Auto-setup initial selection', $2, $3, $4)
        "#,
    )
    .bind(workspace_id)
    .bind(serde_json::json!({
        "wallets": selected_wallets.iter().map(|w| &w.address).collect::<Vec<_>>(),
        "criteria": {
            "min_roi_30d": min_roi.to_string(),
            "min_sharpe": min_sharpe.to_string(),
            "min_win_rate": min_win_rate.to_string(),
            "min_trades_30d": min_trades
        }
    }))
    .bind(user_id)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Update onboarding step
    sqlx::query(
        r#"
        UPDATE user_settings SET onboarding_step = GREATEST(onboarding_step, 3), updated_at = $1
        WHERE user_id = $2
        "#,
    )
    .bind(now)
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    // Audit log
    let event = AuditEvent::builder(
        AuditAction::Custom("onboarding_auto_setup".to_string()),
        format!("workspace/{}", workspace_id),
    )
    .user(claims.sub.clone())
    .details(serde_json::json!({
        "wallets_selected": selected_wallets.len()
    }))
    .build();
    state.audit_logger.log(event);

    Ok(Json(AutoSetupResponse {
        success: true,
        message: format!(
            "Selected {} wallets based on your criteria",
            selected_wallets.len()
        ),
        selected_wallets,
    }))
}

/// Mark onboarding as complete.
#[utoipa::path(
    put,
    path = "/api/v1/onboarding/complete",
    responses(
        (status = 200, description = "Onboarding completed", body = OnboardingStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "onboarding"
)]
pub async fn complete_onboarding(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<OnboardingStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Mark onboarding complete
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO user_settings (user_id, onboarding_completed, onboarding_step, default_workspace_id, created_at, updated_at)
        VALUES ($1, true, 4, $2, $3, $3)
        ON CONFLICT (user_id) DO UPDATE SET
            onboarding_completed = true,
            onboarding_step = 4,
            updated_at = $3
        "#,
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("onboarding_completed".to_string()),
        &workspace_id.to_string(),
        serde_json::json!({}),
    );

    get_status(State(state), Extension(claims)).await
}
