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
use crate::wallet_inventory::load_canonical_inventory_summary;
use crate::workspace_scope::resolve_canonical_workspace_membership;

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountSummaryResponse {
    pub workspace_id: String,
    pub snapshot_time: DateTime<Utc>,
    pub wallet_address: Option<String>,
    /// `None` when the wallet's balance could not be read.
    pub cash_balance: Option<Decimal>,
    pub position_value: Decimal,
    /// Cash plus positions; `None` when the cash could not be read.
    pub total_equity: Option<Decimal>,
    /// Whether `total_equity` is a complete live valuation: live trading, the
    /// cash read and the wallet's inventory scanned.
    pub equity_complete: bool,
    pub unrealized_pnl: Decimal,
    pub realized_pnl_24h: Decimal,
    pub net_cash_flows_24h: Decimal,
    pub open_positions: i64,
    pub open_markets: i64,
    pub unpriced_open_positions: i64,
    pub unpriced_position_cost_basis: Decimal,
    pub orphan_positions: i64,
    pub orphan_marked_value: Decimal,
    pub inventory_last_synced_at: Option<DateTime<Utc>>,
    pub inventory_last_scanned_block: Option<i64>,
    pub inventory_backfill_cursor_block: Option<i64>,
    pub inventory_backfill_completed_at: Option<DateTime<Utc>>,
    pub inventory_backfill_in_progress: bool,
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
    /// The entry's id, chosen by the client: a request sent again with the
    /// same key records nothing more and answers the entry already stored.
    #[serde(default)]
    pub idempotency_key: Option<Uuid>,
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

fn inventory_backfill_in_progress(
    summary: Option<&polymarket_core::db::inventory::WalletInventorySummary>,
) -> bool {
    summary
        .map(|summary| {
            summary.inventory_last_scanned_block.is_some()
                && summary.inventory_backfill_completed_at.is_none()
        })
        .unwrap_or(false)
}

/// How old an inventory scan the polled account summary may show. The
/// background reconciler rescans every three minutes (by default), so a
/// dashboard poll reuses its scan instead of starting one per request; the
/// response says when the inventory was last synced.
const SUMMARY_INVENTORY_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(240);

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

    let snapshot = load_live_account_snapshot(state.as_ref(), SUMMARY_INVENTORY_MAX_AGE)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Canonical workspace not found".into()))?;
    let inventory_summary = load_canonical_inventory_summary(state.as_ref())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(AccountSummaryResponse {
        workspace_id: snapshot.workspace_id.to_string(),
        snapshot_time: snapshot.snapshot_time,
        wallet_address: snapshot.wallet_address,
        cash_balance: snapshot.cash_observed.then_some(snapshot.cash_balance),
        position_value: snapshot.position_value,
        total_equity: snapshot.cash_observed.then_some(snapshot.total_equity),
        equity_complete: snapshot.equity_complete,
        unrealized_pnl: snapshot.unrealized_pnl,
        realized_pnl_24h: snapshot.realized_pnl_24h,
        net_cash_flows_24h: snapshot.net_cash_flows_24h,
        open_positions: snapshot.open_positions,
        open_markets: snapshot.open_markets,
        unpriced_open_positions: snapshot.unpriced_open_positions,
        unpriced_position_cost_basis: snapshot.unpriced_position_cost_basis,
        orphan_positions: inventory_summary
            .as_ref()
            .map(|summary| summary.orphan_positions)
            .unwrap_or(0),
        orphan_marked_value: inventory_summary
            .as_ref()
            .map(|summary| summary.orphan_marked_value)
            .unwrap_or(Decimal::ZERO),
        inventory_last_synced_at: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_last_synced_at),
        inventory_last_scanned_block: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_last_scanned_block),
        inventory_backfill_cursor_block: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_backfill_cursor_block),
        inventory_backfill_completed_at: inventory_summary
            .as_ref()
            .and_then(|summary| summary.inventory_backfill_completed_at),
        inventory_backfill_in_progress: inventory_backfill_in_progress(inventory_summary.as_ref()),
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

    // The current valuation joins the curve only with its cash, as the
    // stored snapshots do.
    let newer = equity_curve
        .last()
        .map(|point| point.snapshot_time < summary.snapshot_time)
        .unwrap_or(true);
    if let (true, Some(cash_balance), Some(total_equity)) =
        (newer, summary.cash_balance, summary.total_equity)
    {
        equity_curve.push(AccountEquityPointResponse {
            snapshot_time: summary.snapshot_time,
            cash_balance,
            position_value: summary.position_value,
            total_equity,
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
    // The circuit breaker reads outflows by their sign: a withdrawal entered
    // as positive would read as a trading loss.
    if let Some(error) = cash_flow_sign_error(&event_type, req.amount) {
        return Err(ApiError::BadRequest(error.into()));
    }

    let entry = NewCashFlow {
        id: req.idempotency_key.unwrap_or_else(Uuid::new_v4),
        workspace_id,
        event_type,
        amount: req.amount,
        note: req.note,
        occurred_at: req.occurred_at.unwrap_or_else(Utc::now),
        created_by: user_id,
    };
    let recorded = record_cash_flow(&state.pool, entry).await?;
    // A withdrawal recorded here moves the breaker's drawdown references.
    super::audit_change(
        &state,
        user_id,
        ::auth::AuditAction::Custom("cash_flow_recorded".into()),
        format!("cash_flow/{}", recorded.id),
        serde_json::json!({
            "event_type": recorded.event_type,
            "amount": recorded.amount,
            "occurred_at": recorded.occurred_at,
        }),
    )
    .await;
    Ok(Json(recorded))
}

/// Why a cash flow's sign contradicts its type: money in is positive, money
/// out negative. Transfers, fees and adjustments may go either way.
fn cash_flow_sign_error(event_type: &str, amount: Decimal) -> Option<&'static str> {
    match event_type {
        "deposit" if amount < Decimal::ZERO => Some("a deposit's amount must be positive"),
        "withdrawal" if amount > Decimal::ZERO => {
            Some("a withdrawal's amount must be negative (money leaving the wallet)")
        }
        _ => None,
    }
}

/// A manually recorded cash flow.
struct NewCashFlow {
    id: Uuid,
    workspace_id: Uuid,
    event_type: String,
    amount: Decimal,
    note: Option<String>,
    occurred_at: DateTime<Utc>,
    created_by: Uuid,
}

/// Record a cash flow once. Each entry moves the circuit breaker's
/// references, so one sent again under its id (a retried request) records
/// nothing more and answers the entry stored; an id already naming another
/// entry is a conflict.
async fn record_cash_flow(
    pool: &sqlx::PgPool,
    entry: NewCashFlow,
) -> ApiResult<CashFlowEventResponse> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO cash_flow_events (
            id, workspace_id, event_type, amount, currency, note, occurred_at, created_by
        )
        VALUES ($1, $2, $3, $4, 'USDC', $5, $6, $7)
        ON CONFLICT (id) DO NOTHING
        "#,
    )
    .bind(entry.id)
    .bind(entry.workspace_id)
    .bind(&entry.event_type)
    .bind(entry.amount)
    .bind(entry.note.as_deref())
    .bind(entry.occurred_at)
    .bind(entry.created_by)
    .execute(pool)
    .await?;
    let stored = load_cash_flow_event(pool, entry.workspace_id, entry.id).await?;
    match stored {
        Some(stored) if inserted.rows_affected() == 1 => Ok(stored),
        // A replay: the same entry (its amount as stored: to 8 places,
        // halves away from zero as the database rounds them).
        Some(stored)
            if stored.event_type == entry.event_type
                && stored.amount
                    == entry.amount.round_dp_with_strategy(
                        8,
                        rust_decimal::RoundingStrategy::MidpointAwayFromZero,
                    ) =>
        {
            Ok(stored)
        }
        _ => Err(ApiError::Conflict(
            "This idempotency key already names another cash flow".into(),
        )),
    }
}

async fn load_cash_flow_event(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<CashFlowEventResponse>, sqlx::Error> {
    let row: Option<CashFlowRow> = sqlx::query_as(
        r#"
        SELECT id, event_type, amount, currency, note, occurred_at, created_at
        FROM cash_flow_events
        WHERE id = $1 AND workspace_id = $2
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(cash_flow_response))
}

fn cash_flow_response(row: CashFlowRow) -> CashFlowEventResponse {
    CashFlowEventResponse {
        id: row.id.to_string(),
        event_type: row.event_type,
        amount: row.amount,
        currency: row.currency,
        note: row.note,
        occurred_at: row.occurred_at,
        created_at: row.created_at,
    }
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

    Ok(rows.into_iter().map(cash_flow_response).collect())
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

#[cfg(test)]
mod cash_flow_tests {
    use super::*;
    use sqlx::Executor;

    #[test]
    fn money_in_is_positive_and_money_out_negative() {
        let amount = |v: i64| Decimal::from(v);
        assert!(cash_flow_sign_error("withdrawal", amount(8000)).is_some());
        assert!(cash_flow_sign_error("withdrawal", amount(-8000)).is_none());
        assert!(cash_flow_sign_error("deposit", amount(-5)).is_some());
        assert!(cash_flow_sign_error("deposit", amount(5)).is_none());
        for either_way in ["transfer", "fee", "adjustment"] {
            assert!(cash_flow_sign_error(either_way, amount(5)).is_none());
            assert!(cash_flow_sign_error(either_way, amount(-5)).is_none());
        }
    }

    #[sqlx::test(migrations = false)]
    async fn a_cash_flow_sent_again_is_recorded_once(pool: sqlx::PgPool) {
        pool.execute(
            "CREATE TABLE workspaces (id UUID PRIMARY KEY); \
             CREATE TABLE cash_flow_events (id UUID PRIMARY KEY, workspace_id UUID NOT NULL, \
                event_type TEXT NOT NULL, amount DECIMAL(18, 8) NOT NULL, \
                currency TEXT NOT NULL DEFAULT 'USDC', note TEXT, occurred_at TIMESTAMPTZ NOT NULL, \
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), created_by UUID)",
        )
        .await
        .unwrap();
        let workspace = Uuid::new_v4();
        let entry = |amount: &str| NewCashFlow {
            id: Uuid::nil(),
            workspace_id: workspace,
            event_type: "withdrawal".into(),
            amount: amount.parse().unwrap(),
            note: Some("off-app transfer".into()),
            occurred_at: Utc::now(),
            created_by: Uuid::new_v4(),
        };
        let first = record_cash_flow(&pool, entry("-12.5")).await.unwrap();
        // Sent again under its key: answered, not recorded twice.
        let again = record_cash_flow(&pool, entry("-12.5")).await.unwrap();
        assert_eq!(first.id, again.id);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cash_flow_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        // The key naming another entry is refused.
        assert!(matches!(
            record_cash_flow(&pool, entry("-20")).await,
            Err(ApiError::Conflict(_))
        ));
        // An amount with more places than stored is still the same entry.
        let mut fine = entry("-1.123456789");
        fine.id = Uuid::new_v4();
        let stored = record_cash_flow(&pool, fine).await.unwrap();
        assert_eq!(stored.amount, "-1.12345679".parse::<Decimal>().unwrap());
    }
}
