//! Recovery preview/run handlers for safe account unwinds.

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use polymarket_core::types::{FailureReason, PositionState};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::Claims;

use crate::error::{ApiError, ApiResult};
use crate::exit_handler::ExitHandlerConfig;
use crate::position_service::EventContext;
use crate::state::AppState;
use crate::wallet_inventory::{
    canonical_refresh_status, force_refresh_canonical_wallet_inventory,
    load_canonical_inventory_summary, load_canonical_orphan_inventory,
    recover_canonical_orphan_inventory,
};
use crate::workspace_scope::resolve_canonical_workspace_membership;

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
    pub orphan_inventory: RecoveryBucketSummary,
    pub stalled: RecoveryBucketSummary,
    pub suspect_inventory: RecoveryBucketSummary,
    pub open_monitoring: RecoveryBucketSummary,
    pub other_blocked: RecoveryBucketSummary,
    pub inventory_last_synced_at: Option<DateTime<Utc>>,
    pub inventory_refresh_in_progress: bool,
    pub inventory_last_refresh_error: Option<String>,
    pub inventory_last_scanned_block: Option<i64>,
    pub inventory_backfill_cursor_block: Option<i64>,
    pub inventory_backfill_completed_at: Option<DateTime<Utc>>,
    pub inventory_backfill_in_progress: bool,
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
    pub orphan_inventory_attempted: i64,
    pub orphan_inventory_succeeded: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, FromRow)]
struct WorkspaceRecoveryFlags {
    exit_handler_enabled: bool,
}

#[derive(Debug, FromRow)]
struct EffectivePositionRow {
    state: i16,
    quantity: Decimal,
    current_price: Option<Decimal>,
    entry_value: Decimal,
    unrealized_pnl: Decimal,
    failure_reason: Option<FailureReason>,
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
    /// Whether live trading and the exit handler are switched on (by the
    /// workspace, or forced by env), reported but never modified here.
    live_trading_enabled: bool,
    exit_handler_enabled: bool,
}

#[derive(Default, Deserialize)]
pub struct PreviewQuery {
    #[serde(default)]
    refresh: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/recovery/preview",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        ("refresh" = Option<bool>, Query, description = "Refresh inventory before returning the preview")
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
    Path(_workspace_id): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> ApiResult<Json<RecoveryPreviewResponse>> {
    // Plain previews are read-only and stay available to every canonical member.
    // A refresh triggers on-chain scans and writes, so it needs owner/admin.
    let access = if query.refresh {
        RecoveryAccess::OwnerOrAdmin
    } else {
        RecoveryAccess::AnyMember
    };
    let workspace_id = authorize_recovery(&state.pool, &claims, access).await?;
    if query.refresh {
        force_refresh_canonical_wallet_inventory(state.as_ref())
            .await
            .map_err(map_anyhow)?;
    }
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
    Path(_workspace_id): Path<String>,
) -> ApiResult<Json<RecoveryRunResponse>> {
    // Recovery submits exit orders and mutates position state: owner/admin only.
    // It must never change runtime flags (live trading / exit handler); those are
    // operator decisions, so the runtime status is only reported back below.
    let workspace_id =
        authorize_recovery(&state.pool, &claims, RecoveryAccess::OwnerOrAdmin).await?;
    // Only the trading process records what recovery sells into the circuit
    // breaker; another (the next version, during a deploy) would not.
    if !state.circuit_breaker.is_leader() {
        return Err(ApiError::ServiceUnavailable(
            "Another server process is trading; retry recovery in a minute".into(),
        ));
    }

    force_refresh_canonical_wallet_inventory(state.as_ref())
        .await
        .map_err(map_anyhow)?;

    let allowance_cache_refreshed = if state.order_executor.is_live_ready().await {
        state
            .order_executor
            .refresh_clob_allowance_cache()
            .await
            .is_ok()
    } else {
        false
    };

    let execution_mode = if state.order_executor.is_live_ready().await {
        "live"
    } else {
        "paper"
    };
    let ctx = EventContext {
        wallet_address: None,
        execution_mode: execution_mode.to_string(),
        strategy: "recovery".to_string(),
        source_label: "recovery".to_string(),
    };

    let mut safe_exit_failures_requeued = 0_i64;
    let mut stalled_positions_reopened = 0_i64;

    for mut position in state
        .position_service
        .repo()
        .get_needing_recovery()
        .await
        .map_err(map_anyhow)?
    {
        // One failing row must not abandon the rest of the recovery run.
        let outcome = match position.state {
            PositionState::ExitFailed
                if should_requeue_exit_failure(position.failure_reason.as_ref()) =>
            {
                state
                    .position_service
                    .attempt_exit_recovery(&mut position, &ctx)
                    .await
                    .map(|recovered| {
                        if recovered {
                            safe_exit_failures_requeued += 1;
                        }
                    })
            }
            // Held shares go straight to the exit queue; reopening a stalled
            // row by heuristic could strand them in an entry state.
            PositionState::Stalled if position.has_open_exposure() => state
                .position_service
                .requeue_stalled_for_exit(&mut position, &ctx)
                .await
                .map(|requeued| {
                    if requeued {
                        stalled_positions_reopened += 1;
                    }
                }),
            PositionState::Stalled => state
                .position_service
                .attempt_stalled_recovery(&mut position, &ctx)
                .await
                .map(|recovered| {
                    if recovered.is_some() {
                        stalled_positions_reopened += 1;
                    }
                }),
            _ => Ok(()),
        };
        if let Err(error) = outcome {
            tracing::warn!(position_id = %position.id, %error, "Recovery skipped position");
        }
    }

    let orphan_retry_backoff_secs = if let Some(config) = state.exit_handler_config.as_ref() {
        config.read().await.failed_exit_retry_backoff_secs
    } else {
        ExitHandlerConfig::from_env().failed_exit_retry_backoff_secs
    };
    let orphan_recovery =
        recover_canonical_orphan_inventory(state.as_ref(), orphan_retry_backoff_secs).await;
    let refreshed = match &orphan_recovery {
        Ok(sold) if sold.succeeded > 0 => force_refresh_canonical_wallet_inventory(state.as_ref())
            .await
            .map(drop),
        _ => Ok(()),
    };
    // Recorded whatever came of the run: requeues and sales already happened.
    if let Ok(user_id) = Uuid::parse_str(&claims.sub) {
        super::audit_change(
            &state,
            user_id,
            ::auth::AuditAction::Custom("recovery_run".into()),
            format!("workspace/{workspace_id}"),
            serde_json::json!({
                "safe_exit_failures_requeued": safe_exit_failures_requeued,
                "stalled_positions_reopened": stalled_positions_reopened,
                "orphan_inventory_attempted": orphan_recovery.as_ref().map(|r| r.attempted).ok(),
                "orphan_inventory_succeeded": orphan_recovery.as_ref().map(|r| r.succeeded).ok(),
                "error": orphan_recovery
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .or_else(|| refreshed.as_ref().err().map(ToString::to_string)),
            }),
        )
        .await;
    }
    let orphan_recovery = orphan_recovery.map_err(map_anyhow)?;
    refreshed.map_err(map_anyhow)?;

    let runtime = runtime_snapshot(state.as_ref(), workspace_id).await?;
    let mut warnings = Vec::new();
    if !runtime.live_ready {
        warnings.push("Live trading wallet/API credentials are not ready".to_string());
    }
    if !runtime.live_trading_enabled {
        warnings.push("Live trading is off; recovery does not turn it on".to_string());
    }
    if !runtime.exit_handler_enabled {
        warnings.push(
            "Exit handler is off; recovery does not turn it on, so requeued exits will not execute until an owner does"
                .to_string(),
        );
    } else if !runtime.exit_handler_running {
        warnings.push("Exit handler heartbeat is stale or the worker is not running".to_string());
    }
    if !allowance_cache_refreshed {
        warnings
            .push("CLOB allowance cache was not refreshed during this recovery run".to_string());
    }
    if orphan_recovery.attempted > 0 {
        warnings.push(format!(
            "Attempted orphan inventory recovery for {} token balances ({} successful submits)",
            orphan_recovery.attempted, orphan_recovery.succeeded
        ));
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
        orphan_inventory_attempted: orphan_recovery.attempted as i64,
        orphan_inventory_succeeded: orphan_recovery.succeeded as i64,
        warnings,
    }))
}

/// Minimum canonical-workspace role required for a recovery endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryAccess {
    /// Any member of the canonical workspace (read-only previews).
    AnyMember,
    /// Owner or admin of the canonical workspace (mutating operations).
    OwnerOrAdmin,
}

/// Authorize against the canonical workspace and return its id.
///
/// The workspace id in the URL is intentionally not trusted: recovery always
/// operates on the canonical workspace, so the caller's role there decides.
async fn authorize_recovery(
    pool: &sqlx::PgPool,
    claims: &Claims,
    access: RecoveryAccess,
) -> ApiResult<Uuid> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let membership = resolve_canonical_workspace_membership(pool, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of the canonical workspace".into()))?;
    if access == RecoveryAccess::OwnerOrAdmin
        && !matches!(membership.role.as_str(), "owner" | "admin")
    {
        return Err(ApiError::Forbidden(
            "Workspace owner or admin role required for recovery".into(),
        ));
    }
    Ok(membership.id)
}

async fn build_recovery_preview(
    state: &AppState,
    workspace_id: Uuid,
) -> ApiResult<RecoveryPreviewResponse> {
    let refresh_status = canonical_refresh_status(state).await.map_err(map_anyhow)?;
    let runtime = runtime_snapshot(state, workspace_id).await?;
    let inventory_summary = load_canonical_inventory_summary(state)
        .await
        .map_err(map_anyhow)?;
    let mut recoverable_now = RecoveryBucketSummary::default();
    let mut liquidity_blocked = RecoveryBucketSummary::default();
    let orphan_inventory = RecoveryBucketSummary {
        positions: inventory_summary
            .as_ref()
            .map(|summary| summary.orphan_positions)
            .unwrap_or(0),
        marked_value: inventory_summary
            .as_ref()
            .map(|summary| summary.orphan_marked_value)
            .unwrap_or(Decimal::ZERO),
    };
    let mut stalled = RecoveryBucketSummary::default();
    let mut suspect_inventory = RecoveryBucketSummary::default();
    let mut open_monitoring = RecoveryBucketSummary::default();
    let mut other_blocked = RecoveryBucketSummary::default();

    for row in load_effective_positions(&state.pool).await? {
        let marked_value = effective_marked_value(&row);
        match row.state {
            2 | 3 => accumulate(&mut recoverable_now, marked_value),
            7 => accumulate(&mut stalled, marked_value),
            6 => match classify_failure_reason(row.failure_reason.as_ref()) {
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

    for entry in load_canonical_orphan_inventory(state)
        .await
        .map_err(map_anyhow)?
    {
        let marked_value = entry
            .marked_value
            .or(entry.cost_basis)
            .unwrap_or(Decimal::ZERO);
        match entry.last_exit_error.as_deref() {
            Some(message) => match classify_failure_message(message) {
                FailureBucket::SuspectInventory => accumulate(&mut suspect_inventory, marked_value),
                FailureBucket::LiquidityBlocked => accumulate(&mut liquidity_blocked, marked_value),
                FailureBucket::SafeRetry => accumulate(&mut recoverable_now, marked_value),
                FailureBucket::Other => accumulate(&mut other_blocked, marked_value),
            },
            None => accumulate(&mut suspect_inventory, marked_value),
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
        orphan_inventory,
        stalled,
        suspect_inventory,
        open_monitoring,
        other_blocked,
        inventory_last_synced_at: refresh_status
            .as_ref()
            .and_then(|status| status.completed_at),
        inventory_refresh_in_progress: refresh_status.as_ref().is_some_and(|status| {
            status.finished_at.is_none()
                && Utc::now()
                    .signed_duration_since(status.started_at)
                    .num_minutes()
                    < 30
        }),
        inventory_last_refresh_error: refresh_status.and_then(|status| status.last_error),
        inventory_last_scanned_block: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_last_scanned_block),
        inventory_backfill_cursor_block: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_backfill_cursor_block),
        inventory_backfill_completed_at: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_backfill_completed_at),
        inventory_backfill_in_progress: inventory_summary
            .as_ref()
            .map(|summary| {
                summary.inventory_last_scanned_block.is_some()
                    && summary.inventory_backfill_completed_at.is_none()
            })
            .unwrap_or(false),
        live_running: runtime.live_running,
        live_ready: runtime.live_ready,
        exit_handler_running: runtime.exit_handler_running,
    })
}

async fn runtime_snapshot(state: &AppState, workspace_id: Uuid) -> ApiResult<RuntimeSnapshot> {
    let flags = sqlx::query_as::<_, WorkspaceRecoveryFlags>(
        r#"
        SELECT COALESCE(exit_handler_enabled, FALSE) AS exit_handler_enabled
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
    let exit_handler_enabled = match &state.exit_handler_config {
        Some(config) => config.read().await.enabled,
        None => flags.exit_handler_enabled,
    };

    Ok(RuntimeSnapshot {
        live_running: live_mode && live_ready,
        live_ready,
        exit_handler_running: exit_handler_enabled && exit_handler_alive,
        live_trading_enabled: live_mode,
        exit_handler_enabled,
    })
}

async fn load_effective_positions(pool: &sqlx::PgPool) -> ApiResult<Vec<EffectivePositionRow>> {
    #[derive(Debug, FromRow)]
    struct EffectivePositionRowDb {
        state: i16,
        quantity: Decimal,
        current_price: Option<Decimal>,
        entry_value: Decimal,
        unrealized_pnl: Decimal,
        failure_reason: Option<String>,
    }

    let rows = sqlx::query_as::<_, EffectivePositionRowDb>(
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

    Ok(rows
        .into_iter()
        .map(|row| EffectivePositionRow {
            state: row.state,
            quantity: row.quantity,
            current_price: row.current_price,
            entry_value: row.entry_value,
            unrealized_pnl: row.unrealized_pnl,
            failure_reason: row
                .failure_reason
                .as_deref()
                .and_then(|value| serde_json::from_str::<FailureReason>(value).ok()),
        })
        .collect())
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

fn classify_failure_reason(reason: Option<&FailureReason>) -> FailureBucket {
    let Some(reason) = reason else {
        return FailureBucket::Other;
    };

    match reason {
        FailureReason::OrderTimeout { .. } | FailureReason::PriceSlippage { .. } => {
            FailureBucket::SafeRetry
        }
        FailureReason::InsufficientBalance => FailureBucket::SuspectInventory,
        FailureReason::MarketClosed | FailureReason::StalePosition { .. } => FailureBucket::Other,
        FailureReason::OrderRejected { message }
        | FailureReason::ConnectivityError { message }
        | FailureReason::Unknown { message }
        | FailureReason::OneLeggedEntry { message, .. } => classify_failure_message(message),
    }
}

fn should_requeue_exit_failure(reason: Option<&FailureReason>) -> bool {
    let Some(reason) = reason else {
        return false;
    };

    let message = match reason {
        FailureReason::OrderRejected { message }
        | FailureReason::ConnectivityError { message }
        | FailureReason::Unknown { message }
        | FailureReason::OneLeggedEntry { message, .. } => message.as_str(),
        FailureReason::OrderTimeout { .. } => return true,
        FailureReason::PriceSlippage { .. } => return true,
        FailureReason::StalePosition { .. } => return false,
        FailureReason::InsufficientBalance | FailureReason::MarketClosed => return false,
    };

    matches!(
        classify_failure_message(message),
        FailureBucket::LiquidityBlocked | FailureBucket::SafeRetry
    )
}

fn classify_failure_message(message: &str) -> FailureBucket {
    let reason = message.to_ascii_lowercase();
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

fn map_anyhow(error: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_scope::test_fixture as fx;
    use auth::UserRole;
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Router;
    use sqlx::PgPool;

    fn router(state: Arc<AppState>) -> Router {
        Router::new()
            .route(
                "/api/v1/workspaces/:workspace_id/recovery/preview",
                post(preview_recovery),
            )
            .route(
                "/api/v1/workspaces/:workspace_id/recovery/run",
                post(run_recovery),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::require_trader,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::require_auth,
            ))
            .with_state(state)
    }

    struct Fixture {
        state: Arc<AppState>,
        app: Router,
        workspace: Uuid,
        tokens: Vec<(&'static str, String)>,
    }

    /// One canonical workspace with an operator-disabled runtime and one user
    /// per workspace role, all with the Trader platform role (as the route requires).
    async fn fixture(pool: &PgPool) -> Fixture {
        fx::real_schema(pool).await;
        let state = AppState::for_tests(pool.clone());
        let workspace = fx::workspace(pool, "canonical", 100).await;
        let mut tokens = Vec::new();
        for role in ["owner", "admin", "member", "viewer"] {
            let user = fx::member(pool, workspace, role, UserRole::Trader).await;
            tokens.push((role, fx::token(&state, user, UserRole::Trader).await));
        }
        let app = router(state.clone());
        Fixture {
            state,
            app,
            workspace,
            tokens,
        }
    }

    impl Fixture {
        fn token(&self, role: &str) -> &str {
            &self.tokens.iter().find(|(r, _)| *r == role).unwrap().1
        }
        async fn post(&self, role: &str, path: &str) -> StatusCode {
            let uri = format!("/api/v1/workspaces/{}/recovery/{path}", self.workspace);
            fx::call(&self.app, "POST", &uri, Some(self.token(role)), None)
                .await
                .0
        }
    }

    #[sqlx::test(migrations = false)]
    async fn run_requires_canonical_owner_or_admin(pool: PgPool) {
        let f = fixture(&pool).await;
        for role in ["viewer", "member"] {
            assert_eq!(
                f.post(role, "run").await,
                StatusCode::FORBIDDEN,
                "{role} must not run recovery"
            );
            assert_eq!(
                f.post(role, "preview?refresh=true").await,
                StatusCode::FORBIDDEN,
                "{role} must not refresh inventory"
            );
        }
        // Owners and admins pass authorization. The test schema has no positions
        // table, so the handler itself fails afterwards; only the guard matters here.
        for role in ["owner", "admin"] {
            assert_ne!(f.post(role, "run").await, StatusCode::FORBIDDEN, "{role}");
            assert_ne!(
                f.post(role, "preview?refresh=true").await,
                StatusCode::FORBIDDEN,
                "{role}"
            );
        }
    }

    #[sqlx::test(migrations = false)]
    async fn plain_preview_stays_readable_by_every_member(pool: PgPool) {
        let f = fixture(&pool).await;
        for role in ["owner", "admin", "member", "viewer"] {
            assert_ne!(
                f.post(role, "preview").await,
                StatusCode::FORBIDDEN,
                "{role}"
            );
        }
        // Non-members of the canonical workspace are still rejected.
        let outsider = fx::user(&pool, UserRole::Trader).await;
        let token = fx::token(&f.state, outsider, UserRole::Trader).await;
        let uri = format!("/api/v1/workspaces/{}/recovery/preview", f.workspace);
        let (status, _) = fx::call(&f.app, "POST", &uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = false)]
    async fn recovery_never_changes_runtime_flags(pool: PgPool) {
        let f = fixture(&pool).await;
        assert_eq!(fx::flags(&pool, f.workspace).await, (false, false, false));
        for role in ["viewer", "member", "admin", "owner"] {
            f.post(role, "run").await;
            f.post(role, "preview?refresh=true").await;
            assert_eq!(
                fx::flags(&pool, f.workspace).await,
                (false, false, false),
                "{role} run must not arm live trading or the exit handler"
            );
        }
        // An operator who disabled only the exit handler keeps that choice too.
        sqlx::query("UPDATE workspaces SET live_trading_enabled = TRUE WHERE id = $1")
            .bind(f.workspace)
            .execute(&pool)
            .await
            .unwrap();
        f.post("owner", "run").await;
        assert_eq!(fx::flags(&pool, f.workspace).await, (true, false, false));
    }
}
