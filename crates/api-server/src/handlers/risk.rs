//! Risk monitoring handlers.

use axum::extract::{Path, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::Claims;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::workspace_scope::resolve_canonical_workspace_membership;

async fn require_canonical_workspace_member(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> ApiResult<(Uuid, String)> {
    resolve_canonical_workspace_membership(pool, user_id)
        .await?
        .map(|workspace| (workspace.id, workspace.role))
        .ok_or_else(|| ApiError::Forbidden("Not a member of the canonical workspace".into()))
}

fn can_manage_risk(role: &str) -> bool {
    role == "owner" || role == "admin"
}

/// Circuit breaker configuration response.
#[derive(Debug, Serialize, ToSchema)]
pub struct CircuitBreakerConfigResponse {
    pub max_daily_loss: Decimal,
    pub max_drawdown_pct: Decimal,
    pub max_consecutive_losses: u32,
    pub cooldown_minutes: i64,
    pub enabled: bool,
    /// Drawdown from the all-time peak that latches a halt until manual
    /// reset; 0 means off.
    pub hard_kill_drawdown_pct: Decimal,
}

impl From<&risk_manager::circuit_breaker::CircuitBreakerConfig> for CircuitBreakerConfigResponse {
    fn from(config: &risk_manager::circuit_breaker::CircuitBreakerConfig) -> Self {
        Self {
            max_daily_loss: config.max_daily_loss,
            max_drawdown_pct: config.max_drawdown_pct,
            max_consecutive_losses: config.max_consecutive_losses,
            cooldown_minutes: config.cooldown_minutes,
            enabled: config.enabled,
            hard_kill_drawdown_pct: config.hard_kill_drawdown_pct,
        }
    }
}

/// Recovery state response.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecoveryStateResponse {
    pub current_stage: u32,
    pub total_stages: u32,
    pub capacity_pct: Decimal,
    pub started_at: DateTime<Utc>,
    pub next_stage_at: Option<DateTime<Utc>>,
    pub trades_this_stage: u32,
    pub recovery_pnl: Decimal,
}

/// Circuit breaker status response.
#[derive(Debug, Serialize, ToSchema)]
pub struct CircuitBreakerResponse {
    pub tripped: bool,
    pub trip_reason: Option<String>,
    pub tripped_at: Option<DateTime<Utc>>,
    pub resume_at: Option<DateTime<Utc>>,
    pub daily_pnl: Decimal,
    /// All-time equity peak: the hard kill switch's reference.
    pub peak_value: Decimal,
    /// Reference for the soft max-drawdown trip; rebased when trading
    /// resumes after a drawdown cooldown and on reset.
    pub drawdown_peak: Decimal,
    pub current_value: Decimal,
    /// Whether the equity values come from real snapshots. Until then the
    /// drawdown guards are not evaluated.
    pub equity_seeded: bool,
    pub consecutive_losses: u32,
    pub trips_today: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_state: Option<RecoveryStateResponse>,
    pub config: CircuitBreakerConfigResponse,
}

impl CircuitBreakerResponse {
    async fn current(state: &AppState) -> Self {
        let cb_state = state.circuit_breaker.state().await;
        let cb_config = state.circuit_breaker.config().await;
        Self {
            tripped: cb_state.tripped,
            trip_reason: cb_state.trip_reason.as_ref().map(trip_reason_to_string),
            tripped_at: cb_state.tripped_at,
            resume_at: cb_state.resume_at,
            daily_pnl: cb_state.daily_pnl,
            peak_value: cb_state.peak_value,
            drawdown_peak: cb_state.drawdown_peak,
            current_value: cb_state.current_value,
            equity_seeded: cb_state.portfolio_value_seeded,
            consecutive_losses: cb_state.consecutive_losses,
            trips_today: cb_state.trips_today,
            recovery_state: cb_state
                .recovery_state
                .as_ref()
                .map(|r| RecoveryStateResponse {
                    current_stage: r.current_stage,
                    total_stages: r.total_stages,
                    capacity_pct: r.capacity_pct(),
                    started_at: r.started_at,
                    next_stage_at: r.next_stage_at,
                    trades_this_stage: r.trades_this_stage,
                    recovery_pnl: r.recovery_pnl,
                }),
            config: CircuitBreakerConfigResponse::from(&cb_config),
        }
    }
}

/// A recently executed stop-loss rule.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecentStopExecution {
    pub id: String,
    pub position_id: String,
    pub market_id: String,
    pub stop_type: String,
    pub executed_at: DateTime<Utc>,
}

/// Stop-loss aggregate statistics.
#[derive(Debug, Serialize, ToSchema)]
pub struct StopLossStatsResponse {
    pub total_rules: i64,
    pub active_rules: i64,
    pub executed_rules: i64,
    pub fixed_stops: i64,
    pub percentage_stops: i64,
    pub trailing_stops: i64,
    pub time_based_stops: i64,
    pub recent_executions: Vec<RecentStopExecution>,
}

/// Combined risk status response.
#[derive(Debug, Serialize, ToSchema)]
pub struct RiskStatusResponse {
    pub circuit_breaker: CircuitBreakerResponse,
    pub stop_loss: StopLossStatsResponse,
}

fn trip_reason_to_string(reason: &risk_manager::circuit_breaker::TripReason) -> String {
    match reason {
        risk_manager::circuit_breaker::TripReason::DailyLossLimit => "daily_loss_limit".to_string(),
        risk_manager::circuit_breaker::TripReason::MaxDrawdown => "max_drawdown".to_string(),
        risk_manager::circuit_breaker::TripReason::ConsecutiveLosses => {
            "consecutive_losses".to_string()
        }
        risk_manager::circuit_breaker::TripReason::Manual => "manual".to_string(),
        risk_manager::circuit_breaker::TripReason::Connectivity => "connectivity".to_string(),
        risk_manager::circuit_breaker::TripReason::MarketConditions => {
            "market_conditions".to_string()
        }
        risk_manager::circuit_breaker::TripReason::HardKillSwitch => "hard_kill_switch".to_string(),
    }
}

/// Get risk monitoring status for a workspace.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/risk/status",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Risk status", body = RiskStatusResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member of this workspace"),
    ),
    security(("bearer_auth" = [])),
    tag = "risk"
)]
pub async fn get_risk_status(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
) -> ApiResult<Json<RiskStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    require_canonical_workspace_member(&state.pool, user_id).await?;

    // Read circuit breaker state from AppState (in-memory, no DB hit)
    let circuit_breaker = CircuitBreakerResponse::current(&state).await;

    // TODO: stop_loss_rules table has been dropped with the copy-trading system.
    // Return zeroed stats until a replacement stop-loss mechanism is implemented.
    let stop_loss = StopLossStatsResponse {
        total_rules: 0,
        active_rules: 0,
        executed_rules: 0,
        fixed_stops: 0,
        percentage_stops: 0,
        trailing_stops: 0,
        time_based_stops: 0,
        recent_executions: vec![],
    };

    Ok(Json(RiskStatusResponse {
        circuit_breaker,
        stop_loss,
    }))
}

/// Manually trip the circuit breaker.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/risk/circuit-breaker/trip",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Circuit breaker tripped", body = CircuitBreakerResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member / insufficient role"),
    ),
    security(("bearer_auth" = [])),
    tag = "risk"
)]
pub async fn manual_trip_circuit_breaker(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
) -> ApiResult<Json<CircuitBreakerResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let (_, role) = require_canonical_workspace_member(&state.pool, user_id).await?;
    if !can_manage_risk(&role) {
        return Err(ApiError::Forbidden(
            "Only workspace owners/admins can manage circuit breaker controls".into(),
        ));
    }

    let stored = state
        .circuit_breaker
        .manual_trip(Some("Manual trip from dashboard".to_string()))
        .await;
    super::audit_change(
        &state,
        user_id,
        ::auth::AuditAction::CircuitBreakerTripped,
        "circuit_breaker",
        serde_json::json!({ "manual": true, "stored": stored.is_ok() }),
    )
    .await;
    if let Err(error) = stored {
        tracing::error!(%error, "A manual trip could not be stored");
        return Err(ApiError::ServiceUnavailable(
            if state.circuit_breaker.is_leader() {
                "Trading is halted on this server, but the trip could not be saved: a restart \
                 would not keep it. Trip again once the database is reachable."
            } else {
                "The trip could not be saved, so trading was not halted. Trip again once the \
                 database is reachable."
            }
            .into(),
        ));
    }

    Ok(Json(CircuitBreakerResponse::current(&state).await))
}

/// Request to update circuit breaker configuration.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateCircuitBreakerConfigRequest {
    pub max_daily_loss: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub max_consecutive_losses: Option<u32>,
    pub cooldown_minutes: Option<i64>,
    pub enabled: Option<bool>,
}

/// Update circuit breaker configuration.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/risk/circuit-breaker/config",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    request_body = UpdateCircuitBreakerConfigRequest,
    responses(
        (status = 200, description = "Updated config", body = CircuitBreakerConfigResponse),
        (status = 400, description = "Invalid parameters"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member / insufficient role"),
    ),
    security(("bearer_auth" = [])),
    tag = "risk"
)]
pub async fn update_circuit_breaker_config(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
    Json(req): Json<UpdateCircuitBreakerConfigRequest>,
) -> ApiResult<Json<CircuitBreakerConfigResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let (workspace_id, role) = require_canonical_workspace_member(&state.pool, user_id).await?;
    if !can_manage_risk(&role) {
        return Err(ApiError::Forbidden(
            "Only workspace owners/admins can update circuit breaker config".into(),
        ));
    }

    // Merge onto the stored limits, not this process's copy of them: another
    // API process may have changed them a moment ago.
    let previous = crate::state::circuit_breaker_config(&state.pool)
        .await
        .map_err(|error| {
            tracing::error!(%error, "Could not read the stored circuit breaker limits");
            ApiError::ServiceUnavailable("The stored limits could not be read; retry".into())
        })?;
    let mut config = previous.clone();

    if let Some(v) = req.max_daily_loss {
        // The stored column is NUMERIC(20, 2).
        if !(0.0..1e15).contains(&v) {
            return Err(ApiError::BadRequest(
                "max_daily_loss must be at least 0 and below 1e15".into(),
            ));
        }
        config.max_daily_loss = Decimal::from_f64_retain(v)
            .ok_or_else(|| ApiError::BadRequest("max_daily_loss is not a number".into()))?;
    }
    if let Some(v) = req.max_drawdown_pct {
        if !(0.0..=1.0).contains(&v) {
            return Err(ApiError::BadRequest(
                "max_drawdown_pct must be between 0 and 1 (e.g., 0.20 for 20%)".into(),
            ));
        }
        config.max_drawdown_pct = Decimal::from_f64_retain(v)
            .ok_or_else(|| ApiError::BadRequest("max_drawdown_pct is not a number".into()))?;
    }
    if let Some(v) = req.max_consecutive_losses {
        if v < 1 {
            return Err(ApiError::BadRequest(
                "max_consecutive_losses must be >= 1".into(),
            ));
        }
        config.max_consecutive_losses = v;
    }
    if let Some(v) = req.cooldown_minutes {
        let most = risk_manager::circuit_breaker::MAX_WAIT_MINUTES;
        if !(0..=most).contains(&v) {
            return Err(ApiError::BadRequest(format!(
                "cooldown_minutes must be between 0 and {most} (a week)"
            )));
        }
        config.cooldown_minutes = v;
    }
    if let Some(v) = req.enabled {
        config.enabled = v;
    }

    config.validate().map_err(ApiError::BadRequest)?;
    // Stored first, then applied: a failed write changes nothing. Only the
    // limits this request sets are written, so concurrent updates of
    // different limits both stand.
    sqlx::query(
        r#"
        UPDATE workspaces
        SET cb_max_daily_loss = COALESCE($1, cb_max_daily_loss),
            cb_max_drawdown_pct = COALESCE($2, cb_max_drawdown_pct),
            cb_max_consecutive_losses = COALESCE($3, cb_max_consecutive_losses),
            cb_cooldown_minutes = COALESCE($4, cb_cooldown_minutes),
            cb_enabled = COALESCE($5, cb_enabled),
            updated_at = NOW()
        WHERE id = $6
        "#,
    )
    .bind(req.max_daily_loss.map(|_| config.max_daily_loss))
    .bind(req.max_drawdown_pct.map(|_| config.max_drawdown_pct))
    .bind(
        req.max_consecutive_losses
            .map(|_| config.max_consecutive_losses as i32),
    )
    .bind(req.cooldown_minutes.map(|_| config.cooldown_minutes as i32))
    .bind(req.enabled)
    .bind(workspace_id)
    .execute(&state.pool)
    .await?;
    state.circuit_breaker.update_config(config.clone()).await;
    super::audit_change(
        &state,
        user_id,
        ::auth::AuditAction::ConfigChange,
        "circuit_breaker/config",
        serde_json::json!({
            "before": {
                "max_daily_loss": previous.max_daily_loss,
                "max_drawdown_pct": previous.max_drawdown_pct,
                "max_consecutive_losses": previous.max_consecutive_losses,
                "cooldown_minutes": previous.cooldown_minutes,
                "enabled": previous.enabled,
            },
            "after": {
                "max_daily_loss": config.max_daily_loss,
                "max_drawdown_pct": config.max_drawdown_pct,
                "max_consecutive_losses": config.max_consecutive_losses,
                "cooldown_minutes": config.cooldown_minutes,
                "enabled": config.enabled,
            },
        }),
    )
    .await;

    tracing::info!(
        workspace_id = %workspace_id,
        user_id = %user_id,
        "Circuit breaker config updated via dashboard"
    );

    Ok(Json(CircuitBreakerConfigResponse::from(&config)))
}

/// Reset the circuit breaker.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/risk/circuit-breaker/reset",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Circuit breaker reset", body = CircuitBreakerResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not a member / insufficient role"),
    ),
    security(("bearer_auth" = [])),
    tag = "risk"
)]
pub async fn reset_circuit_breaker(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
) -> ApiResult<Json<CircuitBreakerResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let (_, role) = require_canonical_workspace_member(&state.pool, user_id).await?;
    if !can_manage_risk(&role) {
        return Err(ApiError::Forbidden(
            "Only workspace owners/admins can manage circuit breaker controls".into(),
        ));
    }

    let before = state.circuit_breaker.state().await;
    let stored = state.circuit_breaker.reset().await;
    super::audit_change(
        &state,
        user_id,
        ::auth::AuditAction::CircuitBreakerReset,
        "circuit_breaker",
        serde_json::json!({
            "was_tripped": before.tripped,
            "trip_reason": before.trip_reason.as_ref().map(trip_reason_to_string),
            "stored": stored.is_ok(),
        }),
    )
    .await;
    if let Err(error) = stored {
        tracing::error!(%error, "A circuit breaker reset holds on this server only");
        return Err(ApiError::ServiceUnavailable(
            "The reset could not be saved; a restart or another server keeps the trip. Retry \
             once the database is reachable."
                .into(),
        ));
    }

    Ok(Json(CircuitBreakerResponse::current(&state).await))
}
