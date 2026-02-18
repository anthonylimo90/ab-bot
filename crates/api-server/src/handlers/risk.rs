//! Risk monitoring handlers.

use axum::extract::{Path, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::Claims;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// Re-use the workspace membership check pattern
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
    pub peak_value: Decimal,
    pub current_value: Decimal,
    pub consecutive_losses: u32,
    pub trips_today: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_state: Option<RecoveryStateResponse>,
    pub config: CircuitBreakerConfigResponse,
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
    }
}

fn stop_type_label(stop_type: i16) -> String {
    match stop_type {
        0 => "fixed".to_string(),
        1 => "percentage".to_string(),
        2 => "trailing".to_string(),
        3 => "time_based".to_string(),
        _ => "unknown".to_string(),
    }
}

#[derive(FromRow)]
struct StopLossStatsRow {
    total: i64,
    active: i64,
    executed: i64,
    fixed: i64,
    percentage: i64,
    trailing: i64,
    time_based: i64,
}

#[derive(FromRow)]
struct RecentStopRow {
    id: Uuid,
    position_id: Uuid,
    market_id: String,
    stop_type: i16,
    executed_at: DateTime<Utc>,
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
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<RiskStatusResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Read circuit breaker state from AppState (in-memory, no DB hit)
    let cb_state = state.circuit_breaker.state().await;
    let cb_config = state.circuit_breaker.config().await;

    let recovery_response = cb_state
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
        });

    let circuit_breaker = CircuitBreakerResponse {
        tripped: cb_state.tripped,
        trip_reason: cb_state.trip_reason.as_ref().map(trip_reason_to_string),
        tripped_at: cb_state.tripped_at,
        resume_at: cb_state.resume_at,
        daily_pnl: cb_state.daily_pnl,
        peak_value: cb_state.peak_value,
        current_value: cb_state.current_value,
        consecutive_losses: cb_state.consecutive_losses,
        trips_today: cb_state.trips_today,
        recovery_state: recovery_response,
        config: CircuitBreakerConfigResponse {
            max_daily_loss: cb_config.max_daily_loss,
            max_drawdown_pct: cb_config.max_drawdown_pct,
            max_consecutive_losses: cb_config.max_consecutive_losses,
            cooldown_minutes: cb_config.cooldown_minutes,
            enabled: cb_config.enabled,
        },
    };

    // Query stop-loss aggregate stats from DB
    let stats_row: StopLossStatsRow = sqlx::query_as(
        r#"
        SELECT
            COUNT(*) as total,
            COUNT(*) FILTER (WHERE activated = TRUE AND executed = FALSE) as active,
            COUNT(*) FILTER (WHERE executed = TRUE) as executed,
            COUNT(*) FILTER (WHERE stop_type = 0) as fixed,
            COUNT(*) FILTER (WHERE stop_type = 1) as percentage,
            COUNT(*) FILTER (WHERE stop_type = 2) as trailing,
            COUNT(*) FILTER (WHERE stop_type = 3) as time_based
        FROM stop_loss_rules sl
        JOIN positions p ON p.id = sl.position_id
        JOIN workspace_wallet_allocations wwa
          ON LOWER(wwa.wallet_address) = LOWER(p.source_wallet)
        WHERE wwa.workspace_id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    // Query recent executed stop-loss rules
    let recent_rows: Vec<RecentStopRow> = sqlx::query_as(
        r#"
        SELECT sl.id, sl.position_id, sl.market_id, sl.stop_type, sl.executed_at
        FROM stop_loss_rules sl
        JOIN positions p ON p.id = sl.position_id
        JOIN workspace_wallet_allocations wwa
          ON LOWER(wwa.wallet_address) = LOWER(p.source_wallet)
        WHERE wwa.workspace_id = $1
          AND sl.executed = TRUE
          AND sl.executed_at IS NOT NULL
        ORDER BY executed_at DESC
        LIMIT 10
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;

    let recent_executions: Vec<RecentStopExecution> = recent_rows
        .into_iter()
        .map(|r| RecentStopExecution {
            id: r.id.to_string(),
            position_id: r.position_id.to_string(),
            market_id: r.market_id,
            stop_type: stop_type_label(r.stop_type),
            executed_at: r.executed_at,
        })
        .collect();

    let stop_loss = StopLossStatsResponse {
        total_rules: stats_row.total,
        active_rules: stats_row.active,
        executed_rules: stats_row.executed,
        fixed_stops: stats_row.fixed,
        percentage_stops: stats_row.percentage,
        trailing_stops: stats_row.trailing,
        time_based_stops: stats_row.time_based,
        recent_executions,
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
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<CircuitBreakerResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    let role = match role {
        Some(r) => r,
        None => return Err(ApiError::Forbidden("Not a member of this workspace".into())),
    };
    if !can_manage_risk(&role) {
        return Err(ApiError::Forbidden(
            "Only workspace owners/admins can manage circuit breaker controls".into(),
        ));
    }

    state
        .circuit_breaker
        .manual_trip(Some("Manual trip from dashboard".to_string()))
        .await;

    let cb_state = state.circuit_breaker.state().await;
    let cb_config = state.circuit_breaker.config().await;

    let recovery_response = cb_state
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
        });

    Ok(Json(CircuitBreakerResponse {
        tripped: cb_state.tripped,
        trip_reason: cb_state.trip_reason.as_ref().map(trip_reason_to_string),
        tripped_at: cb_state.tripped_at,
        resume_at: cb_state.resume_at,
        daily_pnl: cb_state.daily_pnl,
        peak_value: cb_state.peak_value,
        current_value: cb_state.current_value,
        consecutive_losses: cb_state.consecutive_losses,
        trips_today: cb_state.trips_today,
        recovery_state: recovery_response,
        config: CircuitBreakerConfigResponse {
            max_daily_loss: cb_config.max_daily_loss,
            max_drawdown_pct: cb_config.max_drawdown_pct,
            max_consecutive_losses: cb_config.max_consecutive_losses,
            cooldown_minutes: cb_config.cooldown_minutes,
            enabled: cb_config.enabled,
        },
    }))
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
    Path(workspace_id): Path<String>,
) -> ApiResult<Json<CircuitBreakerResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let workspace_id = Uuid::parse_str(&workspace_id)
        .map_err(|_| ApiError::BadRequest("Invalid workspace ID format".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    let role = match role {
        Some(r) => r,
        None => return Err(ApiError::Forbidden("Not a member of this workspace".into())),
    };
    if !can_manage_risk(&role) {
        return Err(ApiError::Forbidden(
            "Only workspace owners/admins can manage circuit breaker controls".into(),
        ));
    }

    state.circuit_breaker.reset().await;

    let cb_state = state.circuit_breaker.state().await;
    let cb_config = state.circuit_breaker.config().await;

    Ok(Json(CircuitBreakerResponse {
        tripped: cb_state.tripped,
        trip_reason: cb_state.trip_reason.as_ref().map(trip_reason_to_string),
        tripped_at: cb_state.tripped_at,
        resume_at: cb_state.resume_at,
        daily_pnl: cb_state.daily_pnl,
        peak_value: cb_state.peak_value,
        current_value: cb_state.current_value,
        consecutive_losses: cb_state.consecutive_losses,
        trips_today: cb_state.trips_today,
        recovery_state: None,
        config: CircuitBreakerConfigResponse {
            max_daily_loss: cb_config.max_daily_loss,
            max_drawdown_pct: cb_config.max_drawdown_pct,
            max_consecutive_losses: cb_config.max_consecutive_losses,
            cooldown_minutes: cb_config.cooldown_minutes,
            enabled: cb_config.enabled,
        },
    }))
}
