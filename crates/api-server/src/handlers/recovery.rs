//! Recovery preview/run handlers for safe account unwinds.

use axum::extract::{Path, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::{FailureReason, PositionState};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::Claims;

use crate::error::{ApiError, ApiResult};
use crate::runtime_sync;
use crate::state::AppState;

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
pub struct RecoveryBucketSummary {
    pub positions: i64,
    pub marked_value: Decimal,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecoveryPreviewResponse {
    pub workspace_id: String,
    pub generated_at: DateTime<Utc>,
    pub safe_recovery: RecoveryBucketSummary,
    pub recoverable_now: RecoveryBucketSummary,
    pub liquidity_blocked: RecoveryBucketSummary,
    pub stalled: RecoveryBucketSummary,
    pub suspect_inventory: RecoveryBucketSummary,
    pub open_monitoring: RecoveryBucketSummary,
    pub other_blocked: RecoveryBucketSummary,
    pub live_running: bool,
    pub live_ready: bool,
    pub exit_handler_running: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecoveryRunResponse {
    pub workspace_id: String,
    pub triggered_at: DateTime<Utc>,
    pub live_running: bool,
    pub live_ready: bool,
    pub exit_handler_running: bool,
    pub allowance_cache_refreshed: bool,
    pub safe_exit_failures_requeued: i64,
    pub stalled_positions_reopened: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, FromRow)]
struct WorkspaceRecoveryFlags {
    live_trading_enabled: bool,
    exit_handler_enabled: bool,
}

#[derive(Debug, FromRow)]
struct EffectivePositionRow {
    state: i16,
    quantity: Decimal,
    current_price: Option<Decimal>,
    entry_value: Decimal,
    unrealized_pnl: Decimal,
    failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureBucket {
    SuspectInventory,
    LiquidityBlocked,
    SafeRetry,
    Other,
}

#[derive(Debug, Clone)]
struct RuntimeSnapshot {
    live_running: bool,
    live_ready: bool,
    exit_handler_running: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/recovery/preview",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Safe recovery preview", body = RecoveryPreviewResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Workspace not found")
    ),
    security(("bearer_auth" = [])),
    tag = "recovery"
)]
pub async fn preview_recovery(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<RecoveryPreviewResponse>> {
    let workspace_id = parse_and_require_member(&state.pool, &claims, &workspace_id).await?;
    let preview = build_recovery_preview(state.as_ref(), workspace_id).await?;
    Ok(Json(preview))
}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/recovery/run",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Safe recovery initiated", body = RecoveryRunResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Workspace not found")
    ),
    security(("bearer_auth" = [])),
    tag = "recovery"
)]
pub async fn run_recovery(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<RecoveryRunResponse>> {
    let workspace_id = parse_and_require_member(&state.pool, &claims, &workspace_id).await?;
    ensure_workspace_runtime_enabled(&state.pool, workspace_id).await?;
    runtime_sync::reconcile_runtime_service_toggles(state.as_ref()).await;

    let allowance_cache_refreshed = if state.order_executor.is_live_ready().await {
        state
            .order_executor
            .refresh_clob_allowance_cache()
            .await
            .is_ok()
    } else {
        false
    };

    let repo = PositionRepository::new(state.pool.clone());
    let mut safe_exit_failures_requeued = 0_i64;
    let mut stalled_positions_reopened = 0_i64;

    for mut position in repo.get_needing_recovery().await.map_err(map_anyhow)? {
        match position.state {
            PositionState::ExitFailed => {
                if should_requeue_exit_failure(position.failure_reason.as_ref()) {
                    position.state = PositionState::ExitReady;
                    position.failure_reason = None;
                    position.last_updated = Utc::now();
                    repo.update(&position).await.map_err(map_anyhow)?;
                    safe_exit_failures_requeued += 1;
                }
            }
            PositionState::Stalled => {
                if position.attempt_stalled_recovery().is_some() {
                    repo.update(&position).await.map_err(map_anyhow)?;
                    stalled_positions_reopened += 1;
                }
            }
            _ => {}
        }
    }

    let runtime = runtime_snapshot(state.as_ref(), workspace_id).await?;
    let mut warnings = Vec::new();
    if !runtime.live_ready {
        warnings.push("Live trading wallet/API credentials are not ready".to_string());
    }
    if !runtime.exit_handler_running {
        warnings.push("Exit handler heartbeat is stale or the worker is not running".to_string());
    }
    if !allowance_cache_refreshed {
        warnings
            .push("CLOB allowance cache was not refreshed during this recovery run".to_string());
    }

    Ok(Json(RecoveryRunResponse {
        workspace_id: workspace_id.to_string(),
        triggered_at: Utc::now(),
        live_running: runtime.live_running,
        live_ready: runtime.live_ready,
        exit_handler_running: runtime.exit_handler_running,
        allowance_cache_refreshed,
        safe_exit_failures_requeued,
        stalled_positions_reopened,
        warnings,
    }))
}

async fn parse_and_require_member(
    pool: &sqlx::PgPool,
    claims: &Claims,
    workspace_id: &str,
) -> ApiResult<Uuid> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    let member: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT role
        FROM workspace_members
        WHERE workspace_id = $1 AND user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    member
        .map(|_| workspace_id)
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))
}

async fn build_recovery_preview(
    state: &AppState,
    workspace_id: Uuid,
) -> ApiResult<RecoveryPreviewResponse> {
    let runtime = runtime_snapshot(state, workspace_id).await?;
    let mut recoverable_now = RecoveryBucketSummary::default();
    let mut liquidity_blocked = RecoveryBucketSummary::default();
    let mut stalled = RecoveryBucketSummary::default();
    let mut suspect_inventory = RecoveryBucketSummary::default();
    let mut open_monitoring = RecoveryBucketSummary::default();
    let mut other_blocked = RecoveryBucketSummary::default();

    for row in load_effective_positions(&state.pool).await? {
        let marked_value = effective_marked_value(&row);
        match row.state {
            2 | 3 => accumulate(&mut recoverable_now, marked_value),
            7 => accumulate(&mut stalled, marked_value),
            6 => match classify_failure_reason(row.failure_reason.as_deref()) {
                FailureBucket::SuspectInventory => accumulate(&mut suspect_inventory, marked_value),
                FailureBucket::LiquidityBlocked => accumulate(&mut liquidity_blocked, marked_value),
                FailureBucket::SafeRetry | FailureBucket::Other => {
                    accumulate(&mut other_blocked, marked_value)
                }
            },
            1 => accumulate(&mut open_monitoring, marked_value),
            _ => accumulate(&mut other_blocked, marked_value),
        }
    }

    let safe_recovery = RecoveryBucketSummary {
        positions: recoverable_now.positions
            + liquidity_blocked.positions
            + stalled.positions
            + open_monitoring.positions
            + other_blocked.positions,
        marked_value: recoverable_now.marked_value
            + liquidity_blocked.marked_value
            + stalled.marked_value
            + open_monitoring.marked_value
            + other_blocked.marked_value,
    };

    Ok(RecoveryPreviewResponse {
        workspace_id: workspace_id.to_string(),
        generated_at: Utc::now(),
        safe_recovery,
        recoverable_now,
        liquidity_blocked,
        stalled,
        suspect_inventory,
        open_monitoring,
        other_blocked,
        live_running: runtime.live_running,
        live_ready: runtime.live_ready,
        exit_handler_running: runtime.exit_handler_running,
    })
}

async fn runtime_snapshot(state: &AppState, workspace_id: Uuid) -> ApiResult<RuntimeSnapshot> {
    let flags = sqlx::query_as::<_, WorkspaceRecoveryFlags>(
        r#"
        SELECT
            COALESCE(live_trading_enabled, FALSE) AS live_trading_enabled,
            COALESCE(exit_handler_enabled, FALSE) AS exit_handler_enabled
        FROM workspaces
        WHERE id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("Workspace not found".into()))?;

    let now_epoch = Utc::now().timestamp();
    let heartbeat_max_age_secs = 120_i64;
    let exit_heartbeat_ts = state
        .exit_handler_heartbeat
        .load(std::sync::atomic::Ordering::Relaxed);
    let exit_handler_alive =
        exit_heartbeat_ts > 0 && (now_epoch - exit_heartbeat_ts) < heartbeat_max_age_secs;
    let live_mode = state.order_executor.is_live();
    let live_ready = state.order_executor.is_live_ready().await;

    Ok(RuntimeSnapshot {
        live_running: flags.live_trading_enabled && live_mode && live_ready,
        live_ready,
        exit_handler_running: flags.exit_handler_enabled && exit_handler_alive,
    })
}

async fn ensure_workspace_runtime_enabled(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(
        r#"
        UPDATE workspaces
        SET live_trading_enabled = TRUE,
            exit_handler_enabled = TRUE,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(workspace_id)
    .execute(pool)
    .await?;

    Ok(())
}

async fn load_effective_positions(pool: &sqlx::PgPool) -> ApiResult<Vec<EffectivePositionRow>> {
    let rows = sqlx::query_as::<_, EffectivePositionRow>(
        r#"
        WITH active_positions AS (
            SELECT
                id,
                market_id,
                COALESCE(source, 0) AS source,
                state,
                quantity,
                current_price,
                COALESCE(entry_price, (yes_entry_price + no_entry_price), 0) AS entry_price,
                (quantity * COALESCE(entry_price, (yes_entry_price + no_entry_price), 0)) AS entry_value,
                COALESCE(unrealized_pnl, 0) AS unrealized_pnl,
                failure_reason,
                COALESCE(last_updated, updated_at, entry_timestamp) AS sort_updated
            FROM positions
            WHERE is_open = TRUE
        ),
        ranked_active AS (
            SELECT
                *,
                ROW_NUMBER() OVER (
                    PARTITION BY market_id, source
                    ORDER BY sort_updated DESC, id DESC
                ) AS rn
            FROM active_positions
        )
        SELECT
            state,
            quantity,
            current_price,
            entry_value,
            unrealized_pnl,
            failure_reason
        FROM ranked_active
        WHERE rn = 1
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

fn effective_marked_value(row: &EffectivePositionRow) -> Decimal {
    match row.current_price {
        Some(current_price) => row.quantity * current_price,
        None => (row.entry_value + row.unrealized_pnl).max(Decimal::ZERO),
    }
}

fn accumulate(bucket: &mut RecoveryBucketSummary, marked_value: Decimal) {
    bucket.positions += 1;
    bucket.marked_value += marked_value;
}

fn classify_failure_reason(reason: Option<&str>) -> FailureBucket {
    let Some(reason) = reason else {
        return FailureBucket::Other;
    };

    let reason = reason.to_ascii_lowercase();
    if reason.contains("404 not found") || reason.contains("insufficient conditional balance") {
        return FailureBucket::SuspectInventory;
    }
    if reason.contains("insufficient liquidity") || reason.contains("no bids available") {
        return FailureBucket::LiquidityBlocked;
    }
    if reason.contains("timeout")
        || reason.contains("temporarily unavailable")
        || reason.contains("connection")
        || reason.contains("network")
        || reason.contains("503")
        || reason.contains("502")
        || reason.contains("504")
    {
        return FailureBucket::SafeRetry;
    }

    FailureBucket::Other
}

fn should_requeue_exit_failure(reason: Option<&FailureReason>) -> bool {
    let Some(reason) = reason else {
        return false;
    };

    let message = match reason {
        FailureReason::OrderRejected { message }
        | FailureReason::ConnectivityError { message }
        | FailureReason::Unknown { message } => message.as_str(),
        FailureReason::OrderTimeout { .. } => return true,
        FailureReason::PriceSlippage { .. } => return true,
        FailureReason::StalePosition { .. } => return false,
        FailureReason::InsufficientBalance | FailureReason::MarketClosed => return false,
    };

    matches!(
        classify_failure_reason(Some(message)),
        FailureBucket::LiquidityBlocked | FailureBucket::SafeRetry
    )
}

fn map_anyhow(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}
