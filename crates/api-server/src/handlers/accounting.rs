//! Canonical account summary, history, and cash-flow handlers.

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use auth::Claims;

use crate::accounting_ledger::load_live_account_snapshot;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::workspace_scope::resolve_canonical_workspace_membership;

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountSummaryResponse {
    pub workspace_id: String,
    pub snapshot_time: DateTime<Utc>,
    pub wallet_address: Option<String>,
    pub cash_balance: Decimal,
    pub position_value: Decimal,
    pub total_equity: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl_24h: Decimal,
    pub net_cash_flows_24h: Decimal,
    pub open_positions: i64,
    pub open_markets: i64,
    pub unpriced_open_positions: i64,
    pub unpriced_position_cost_basis: Decimal,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountEquityPointResponse {
    pub snapshot_time: DateTime<Utc>,
    pub cash_balance: Decimal,
    pub position_value: Decimal,
    pub total_equity: Decimal,
    pub unrealized_pnl: Decimal,
    pub realized_pnl_24h: Decimal,
    pub net_cash_flows_24h: Decimal,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CashFlowEventResponse {
    pub id: String,
    pub event_type: String,
    pub amount: Decimal,
    pub currency: String,
    pub note: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountTradeEventResponse {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub strategy: String,
    pub source: String,
    pub event_type: String,
    pub execution_mode: String,
    pub market_id: String,
    pub position_id: Option<String>,
    pub reason: Option<String>,
    pub realized_pnl: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountHistoryResponse {
    pub summary: AccountSummaryResponse,
    pub equity_curve: Vec<AccountEquityPointResponse>,
    pub cash_flows: Vec<CashFlowEventResponse>,
    pub recent_trade_events: Vec<AccountTradeEventResponse>,
    pub snapshot_started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AccountHistoryQuery {
    #[serde(default = "default_hours")]
    pub hours: i64,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct CashFlowListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateCashFlowRequest {
    pub event_type: String,
    pub amount: Decimal,
    pub note: Option<String>,
    pub occurred_at: Option<DateTime<Utc>>,
}

fn default_hours() -> i64 {
    24
}

fn default_limit() -> i64 {
    100
}

#[derive(Debug, FromRow)]
struct AccountSnapshotRow {
    snapshot_time: DateTime<Utc>,
    cash_balance: Decimal,
    position_value: Decimal,
    total_equity: Decimal,
    unrealized_pnl: Decimal,
    realized_pnl_24h: Decimal,
    net_cash_flows_24h: Decimal,
}

#[derive(Debug, FromRow)]
struct CashFlowRow {
    id: Uuid,
    event_type: String,
    amount: Decimal,
    currency: String,
    note: Option<String>,
    occurred_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct TradeEventRow {
    id: Uuid,
    occurred_at: DateTime<Utc>,
    strategy: String,
    source: String,
    event_type: String,
    execution_mode: String,
    market_id: String,
    position_id: Option<Uuid>,
    reason: Option<String>,
    realized_pnl: Option<Decimal>,
    unrealized_pnl: Option<Decimal>,
}

async fn require_canonical_workspace_member(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> ApiResult<(Uuid, String)> {
    resolve_canonical_workspace_membership(pool, user_id)
        .await?
        .map(|workspace| (workspace.id, workspace.role))
        .ok_or_else(|| ApiError::Forbidden("Not a member of the canonical workspace".into()))
}

fn normalize_source_label(source: &str) -> String {
    match source {
        "arb" => "arbitrage".to_string(),
        "quant" => "quant".to_string(),
        other => other.to_string(),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/account/summary",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    responses(
        (status = 200, description = "Current account summary", body = AccountSummaryResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Workspace not found")
    ),
    security(("bearer_auth" = [])),
    tag = "accounting"
)]
pub async fn get_account_summary(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
) -> ApiResult<Json<AccountSummaryResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    require_canonical_workspace_member(&state.pool, user_id).await?;

    let snapshot = load_live_account_snapshot(state.as_ref())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Canonical workspace not found".into()))?;

    Ok(Json(AccountSummaryResponse {
        workspace_id: snapshot.workspace_id.to_string(),
        snapshot_time: snapshot.snapshot_time,
        wallet_address: snapshot.wallet_address,
        cash_balance: snapshot.cash_balance,
        position_value: snapshot.position_value,
        total_equity: snapshot.total_equity,
        unrealized_pnl: snapshot.unrealized_pnl,
        realized_pnl_24h: snapshot.realized_pnl_24h,
        net_cash_flows_24h: snapshot.net_cash_flows_24h,
        open_positions: snapshot.open_positions,
        open_markets: snapshot.open_markets,
        unpriced_open_positions: snapshot.unpriced_open_positions,
        unpriced_position_cost_basis: snapshot.unpriced_position_cost_basis,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/account/history",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        AccountHistoryQuery
    ),
    responses(
        (status = 200, description = "Canonical account history", body = AccountHistoryResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    ),
    security(("bearer_auth" = [])),
    tag = "accounting"
)]
pub async fn get_account_history(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
    Query(query): Query<AccountHistoryQuery>,
) -> ApiResult<Json<AccountHistoryResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let (workspace_id, _) = require_canonical_workspace_member(&state.pool, user_id).await?;

    let hours = query.hours.clamp(1, 24 * 30);
    let limit = query.limit.clamp(1, 500);
    let from = Utc::now() - chrono::Duration::hours(hours);

    let summary = get_account_summary(
        State(state.clone()),
        Extension(claims.clone()),
        Path(workspace_id.to_string()),
    )
    .await?
    .0;

    let mut equity_curve: Vec<AccountEquityPointResponse> =
        sqlx::query_as::<_, AccountSnapshotRow>(
            r#"
        SELECT
            snapshot_time,
            cash_balance,
            position_value,
            total_equity,
            unrealized_pnl,
            realized_pnl_24h,
            net_cash_flows_24h
        FROM account_snapshots
        WHERE workspace_id = $1
          AND snapshot_time >= $2
        ORDER BY snapshot_time ASC
        "#,
        )
        .bind(workspace_id)
        .bind(from)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(|row| AccountEquityPointResponse {
            snapshot_time: row.snapshot_time,
            cash_balance: row.cash_balance,
            position_value: row.position_value,
            total_equity: row.total_equity,
            unrealized_pnl: row.unrealized_pnl,
            realized_pnl_24h: row.realized_pnl_24h,
            net_cash_flows_24h: row.net_cash_flows_24h,
        })
        .collect();

    if equity_curve.is_empty() {
        equity_curve.push(AccountEquityPointResponse {
            snapshot_time: summary.snapshot_time,
            cash_balance: summary.cash_balance,
            position_value: summary.position_value,
            total_equity: summary.total_equity,
            unrealized_pnl: summary.unrealized_pnl,
            realized_pnl_24h: summary.realized_pnl_24h,
            net_cash_flows_24h: summary.net_cash_flows_24h,
        });
    } else if equity_curve
        .last()
        .map(|point| point.snapshot_time < summary.snapshot_time)
        .unwrap_or(true)
    {
        equity_curve.push(AccountEquityPointResponse {
            snapshot_time: summary.snapshot_time,
            cash_balance: summary.cash_balance,
            position_value: summary.position_value,
            total_equity: summary.total_equity,
            unrealized_pnl: summary.unrealized_pnl,
            realized_pnl_24h: summary.realized_pnl_24h,
            net_cash_flows_24h: summary.net_cash_flows_24h,
        });
    }

    let cash_flows = load_cash_flow_events(&state.pool, workspace_id, limit).await?;
    let trade_events = load_recent_trade_events(&state.pool, from, limit).await?;
    let snapshot_started_at = equity_curve.first().map(|point| point.snapshot_time);

    Ok(Json(AccountHistoryResponse {
        summary,
        equity_curve,
        cash_flows,
        recent_trade_events: trade_events,
        snapshot_started_at,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/account/cash-flows",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID"),
        CashFlowListQuery
    ),
    responses(
        (status = 200, description = "Cash flow events", body = Vec<CashFlowEventResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    ),
    security(("bearer_auth" = [])),
    tag = "accounting"
)]
pub async fn list_cash_flows(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
    Query(query): Query<CashFlowListQuery>,
) -> ApiResult<Json<Vec<CashFlowEventResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let (workspace_id, _) = require_canonical_workspace_member(&state.pool, user_id).await?;

    let rows = load_cash_flow_events(&state.pool, workspace_id, query.limit.clamp(1, 500)).await?;
    Ok(Json(rows))
}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/account/cash-flows",
    params(
        ("workspace_id" = String, Path, description = "Workspace ID")
    ),
    request_body = CreateCashFlowRequest,
    responses(
        (status = 200, description = "Cash flow recorded", body = CashFlowEventResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden")
    ),
    security(("bearer_auth" = [])),
    tag = "accounting"
)]
pub async fn create_cash_flow(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(_workspace_id): Path<String>,
    Json(req): Json<CreateCashFlowRequest>,
) -> ApiResult<Json<CashFlowEventResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;
    let (workspace_id, role) = require_canonical_workspace_member(&state.pool, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "Only workspace owners and admins can record cash flows".into(),
        ));
    }

    let event_type = req.event_type.trim().to_lowercase();
    let allowed = ["deposit", "withdrawal", "transfer", "fee", "adjustment"];
    if !allowed.contains(&event_type.as_str()) {
        return Err(ApiError::BadRequest(
            "event_type must be one of deposit, withdrawal, transfer, fee, adjustment".into(),
        ));
    }
    if req.amount == Decimal::ZERO {
        return Err(ApiError::BadRequest("amount must be non-zero".into()));
    }

    let id = Uuid::new_v4();
    let occurred_at = req.occurred_at.unwrap_or_else(Utc::now);
    let created_at = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO cash_flow_events (
            id, workspace_id, event_type, amount, currency, note, occurred_at, created_by
        )
        VALUES ($1, $2, $3, $4, 'USDC', $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(&event_type)
    .bind(req.amount)
    .bind(req.note.as_deref())
    .bind(occurred_at)
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    Ok(Json(CashFlowEventResponse {
        id: id.to_string(),
        event_type,
        amount: req.amount,
        currency: "USDC".to_string(),
        note: req.note,
        occurred_at,
        created_at,
    }))
}

async fn load_cash_flow_events(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    limit: i64,
) -> Result<Vec<CashFlowEventResponse>, sqlx::Error> {
    let rows: Vec<CashFlowRow> = sqlx::query_as(
        r#"
        SELECT id, event_type, amount, currency, note, occurred_at, created_at
        FROM cash_flow_events
        WHERE workspace_id = $1
        ORDER BY occurred_at DESC, created_at DESC
        LIMIT $2
        "#,
    )
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| CashFlowEventResponse {
            id: row.id.to_string(),
            event_type: row.event_type,
            amount: row.amount,
            currency: row.currency,
            note: row.note,
            occurred_at: row.occurred_at,
            created_at: row.created_at,
        })
        .collect())
}

async fn load_recent_trade_events(
    pool: &sqlx::PgPool,
    from: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<AccountTradeEventResponse>, sqlx::Error> {
    let rows: Vec<TradeEventRow> = sqlx::query_as(
        r#"
        SELECT
            id,
            occurred_at,
            strategy,
            source,
            event_type,
            execution_mode,
            market_id,
            position_id,
            reason,
            realized_pnl,
            unrealized_pnl
        FROM trade_events
        WHERE occurred_at >= $1
        ORDER BY occurred_at DESC, id DESC
        LIMIT $2
        "#,
    )
    .bind(from)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| AccountTradeEventResponse {
            id: row.id.to_string(),
            occurred_at: row.occurred_at,
            strategy: row.strategy,
            source: normalize_source_label(&row.source),
            event_type: row.event_type,
            execution_mode: row.execution_mode,
            market_id: row.market_id,
            position_id: row.position_id.map(|value| value.to_string()),
            reason: row.reason,
            realized_pnl: row.realized_pnl,
            unrealized_pnl: row.unrealized_pnl,
        })
        .collect())
}
