//! Position management handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use polymarket_core::db::positions::{
    SOURCE_ARBITRAGE, SOURCE_COPY_TRADE, SOURCE_MANUAL, SOURCE_RECOMMENDATION,
};
use polymarket_core::types::PositionState;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::position_service::EventContext;
use crate::state::AppState;
use crate::workspace_scope::{WorkspaceReadAccess, WorkspaceTradeAccess};

use crate::websocket::{PositionUpdate, PositionUpdateType};

/// Position response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PositionResponse {
    /// Position identifier.
    pub id: Uuid,
    /// Market identifier.
    pub market_id: String,
    /// Outcome (yes/no).
    pub outcome: String,
    /// Position side (long/short).
    pub side: String,
    /// Quantity held.
    pub quantity: Decimal,
    /// Average entry price.
    pub entry_price: Decimal,
    /// Current market price.
    pub current_price: Decimal,
    /// Unrealized P&L.
    pub unrealized_pnl: Decimal,
    /// Unrealized P&L percentage.
    pub unrealized_pnl_pct: Decimal,
    /// Stop loss price (if set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<Decimal>,
    /// Take profit price (if set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit: Option<Decimal>,
    /// Realized P&L (for closed positions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_pnl: Option<Decimal>,
    /// Full lifecycle state.
    pub state: String,
    /// Position opened timestamp.
    pub opened_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    /// YES tokens currently held (not yet exited).
    pub held_yes_qty: Decimal,
    /// NO tokens currently held (not yet exited).
    pub held_no_qty: Decimal,
    /// YES tokens sold or resolved out.
    pub exited_yes_qty: Decimal,
    /// NO tokens sold or resolved out.
    pub exited_no_qty: Decimal,
    /// YES exit fill price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yes_exit_price: Option<Decimal>,
    /// NO exit fill price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_exit_price: Option<Decimal>,
    /// Market resolution winner ("yes" or "no"), null while open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_winner: Option<String>,
    /// Exit strategy: "hold_to_resolution" or "exit_on_correction".
    pub exit_strategy: String,
    /// Whether the position still holds exposure the exit flow manages: every
    /// state but closed and entry_failed, plus a one-legged failed entry.
    pub is_open: bool,
}

/// Summary response for dashboard/overview metrics.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PositionsSummaryResponse {
    /// Deduplicated active position count (latest row per market/source).
    pub open_positions: i64,
    /// Distinct active markets after deduplication.
    pub open_markets: i64,
    /// Active markets with duplicate rows before deduplication.
    pub duplicate_open_markets: i64,
    /// Deduplicated marked position value based on current prices only.
    pub portfolio_value: Decimal,
    /// Deduplicated position count with a current mark.
    pub priced_open_positions: i64,
    /// Deduplicated position count missing a current mark.
    pub unpriced_open_positions: i64,
    /// Deduplicated cost basis for positions missing a current mark.
    pub unpriced_position_cost_basis: Decimal,
    /// Deduplicated unrealized P&L.
    pub unrealized_pnl: Decimal,
    /// Raw active row count before deduplication.
    pub raw_open_positions: i64,
    /// Raw marked position value before deduplication.
    pub raw_portfolio_value: Decimal,
    /// Raw active row count missing a current mark before deduplication.
    pub raw_unpriced_open_positions: i64,
    /// Raw cost basis for positions missing a current mark before deduplication.
    pub raw_unpriced_position_cost_basis: Decimal,
    /// Raw unrealized P&L before deduplication.
    pub raw_unrealized_pnl: Decimal,
    /// All closed positions, including flat closes.
    pub closed_positions: i64,
    /// Count of winning closed positions.
    pub wins: i64,
    /// Count of losing closed positions.
    pub losses: i64,
    /// Count of closed positions with zero realized P&L.
    pub flat_closes: i64,
    /// Win rate based on resolved non-flat closes only.
    pub win_rate: Decimal,
    /// Rows the closed list (`status=closed`) pages through: every position
    /// no longer open, in any state or mode.
    pub closed_list_count: i64,
}

/// Request to close a position.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ClosePositionRequest {
    /// Quantity to close (None = close all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<Decimal>,
    /// Limit price (None = market order).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<Decimal>,
}

/// Query parameters for listing positions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListPositionsQuery {
    /// Filter by market.
    pub market_id: Option<String>,
    /// Filter by outcome.
    pub outcome: Option<String>,
    /// Filter by status (open/closed/all).
    #[serde(default = "default_status")]
    pub status: String,
    /// Maximum results.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_status() -> String {
    "open".to_string()
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, FromRow)]
struct PositionRow {
    id: Uuid,
    market_id: String,
    outcome: String,
    side: String,
    quantity: Decimal,
    entry_price: Decimal,
    current_price: Decimal,
    unrealized_pnl: Decimal,
    stop_loss: Option<Decimal>,
    take_profit: Option<Decimal>,
    realized_pnl: Option<Decimal>,
    state: i16,
    opened_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    held_yes_qty: Decimal,
    held_no_qty: Decimal,
    exited_yes_qty: Decimal,
    exited_no_qty: Decimal,
    yes_exit_price: Option<Decimal>,
    no_exit_price: Option<Decimal>,
    resolution_winner: Option<String>,
    exit_strategy: i16,
    is_open: bool,
}

/// The columns of a [`PositionRow`], ahead of a `WHERE` clause.
const POSITION_ROW_SELECT: &str = r#"
        SELECT id, market_id,
               COALESCE(outcome, 'both') AS outcome,
               COALESCE(side, 'long') AS side,
               quantity,
               COALESCE(entry_price, (yes_entry_price + no_entry_price)) AS entry_price,
               COALESCE(current_price, (yes_entry_price + no_entry_price)) AS current_price,
               unrealized_pnl, stop_loss, take_profit,
               realized_pnl,
               state,
               COALESCE(opened_at, entry_timestamp) AS opened_at,
               COALESCE(last_updated, updated_at, entry_timestamp) AS updated_at,
               held_yes_qty, held_no_qty, exited_yes_qty, exited_no_qty,
               yes_exit_price, no_exit_price, resolution_winner,
               COALESCE(exit_strategy, 0)::smallint AS exit_strategy,
               is_open
        FROM positions
"#;

impl From<PositionRow> for PositionResponse {
    fn from(row: PositionRow) -> Self {
        Self {
            unrealized_pnl_pct: unrealized_pct(row.unrealized_pnl, row.entry_price, row.quantity),
            id: row.id,
            market_id: row.market_id,
            outcome: row.outcome,
            side: row.side,
            quantity: row.quantity,
            entry_price: row.entry_price,
            current_price: row.current_price,
            unrealized_pnl: row.unrealized_pnl,
            stop_loss: row.stop_loss,
            take_profit: row.take_profit,
            realized_pnl: row.realized_pnl,
            state: position_state_name(row.state).to_string(),
            opened_at: row.opened_at,
            updated_at: row.updated_at,
            held_yes_qty: row.held_yes_qty,
            held_no_qty: row.held_no_qty,
            exited_yes_qty: row.exited_yes_qty,
            exited_no_qty: row.exited_no_qty,
            yes_exit_price: row.yes_exit_price,
            no_exit_price: row.no_exit_price,
            resolution_winner: row.resolution_winner,
            exit_strategy: exit_strategy_name(row.exit_strategy).to_string(),
            is_open: row.is_open,
        }
    }
}

#[derive(Debug, FromRow)]
struct CloseablePositionRow {
    id: Uuid,
    market_id: String,
    outcome: String,
    side: String,
    quantity: Decimal,
    entry_price: Decimal,
    current_price: Decimal,
    stop_loss: Option<Decimal>,
    take_profit: Option<Decimal>,
    realized_pnl: Option<Decimal>,
    source: i16,
    opened_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct PositionSummaryRow {
    open_positions: i64,
    open_markets: i64,
    duplicate_open_markets: i64,
    portfolio_value: Decimal,
    priced_open_positions: i64,
    unpriced_open_positions: i64,
    unpriced_position_cost_basis: Decimal,
    unrealized_pnl: Decimal,
    raw_open_positions: i64,
    raw_portfolio_value: Decimal,
    raw_unpriced_open_positions: i64,
    raw_unpriced_position_cost_basis: Decimal,
    raw_unrealized_pnl: Decimal,
    closed_positions: i64,
    wins: i64,
    losses: i64,
    flat_closes: i64,
    win_rate: Decimal,
    closed_list_count: i64,
}

fn exit_strategy_name(strategy: i16) -> &'static str {
    match strategy {
        0 => "hold_to_resolution",
        1 => "exit_on_correction",
        _ => "hold_to_resolution",
    }
}

fn position_state_name(state: i16) -> &'static str {
    match state {
        0 => "pending",
        1 => "open",
        2 => "exit_ready",
        3 => "closing",
        4 => "closed",
        5 => "entry_failed",
        6 => "exit_failed",
        7 => "stalled",
        _ => "closed",
    }
}

pub(crate) fn position_state_label(state: PositionState) -> &'static str {
    match state {
        PositionState::Pending => "pending",
        PositionState::Open => "open",
        PositionState::ExitReady => "exit_ready",
        PositionState::Closing => "closing",
        PositionState::Closed => "closed",
        PositionState::EntryFailed => "entry_failed",
        PositionState::ExitFailed => "exit_failed",
        PositionState::Stalled => "stalled",
    }
}

fn trade_event_labels(source: i16) -> (&'static str, &'static str) {
    match source {
        SOURCE_ARBITRAGE => ("arb", "arb"),
        SOURCE_COPY_TRADE => ("copy_trade", "copy_trade"),
        SOURCE_RECOMMENDATION => ("quant", "quant"),
        SOURCE_MANUAL => ("manual", "manual"),
        _ => ("manual", "manual"),
    }
}

async fn current_execution_mode(state: &AppState) -> &'static str {
    if state.order_executor.is_live_ready().await {
        "live"
    } else {
        "paper"
    }
}

fn validate_status_filter(status: &str) -> ApiResult<&str> {
    match status {
        "open" | "closed" | "all" => Ok(status),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid status '{}'. Expected open, closed, or all",
            status
        ))),
    }
}

/// List positions.
#[utoipa::path(
    get,
    path = "/api/v1/positions",
    tag = "positions",
    params(ListPositionsQuery),
    responses(
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 200, description = "List of positions", body = Vec<PositionResponse>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_positions(
    _access: WorkspaceReadAccess,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListPositionsQuery>,
) -> ApiResult<Json<Vec<PositionResponse>>> {
    let rows = load_position_rows(&state.pool, &query).await?;

    let positions: Vec<PositionResponse> = rows.into_iter().map(PositionResponse::from).collect();

    Ok(Json(positions))
}

async fn load_position_rows(
    pool: &sqlx::PgPool,
    query: &ListPositionsQuery,
) -> ApiResult<Vec<PositionRow>> {
    if query.offset < 0 || !(1..=500).contains(&query.limit) {
        return Err(ApiError::BadRequest(
            "limit must be 1–500 and offset nonnegative".into(),
        ));
    }
    let status_filter = validate_status_filter(&query.status)?;

    let mut qb: QueryBuilder<Postgres> =
        QueryBuilder::new(format!("{POSITION_ROW_SELECT} WHERE 1 = 1"));

    if let Some(market_id) = &query.market_id {
        qb.push(" AND market_id = ");
        qb.push_bind(market_id);
    }

    if let Some(outcome) = &query.outcome {
        qb.push(" AND COALESCE(outcome, 'both') = ");
        qb.push_bind(outcome);
    }

    match status_filter {
        "open" => {
            qb.push(" AND is_open = TRUE");
        }
        "closed" => {
            qb.push(" AND is_open = FALSE");
        }
        "all" => {}
        _ => unreachable!("status_filter already validated"),
    }

    qb.push(" ORDER BY COALESCE(opened_at, entry_timestamp) DESC, id DESC");
    qb.push(" LIMIT ");
    qb.push_bind(query.limit);
    qb.push(" OFFSET ");
    qb.push_bind(query.offset);

    let rows: Vec<PositionRow> = qb
        .build_query_as()
        .fetch_all(pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(rows)
}

/// Get aggregate position metrics for dashboards.
#[utoipa::path(
    get,
    path = "/api/v1/positions/summary",
    tag = "positions",
    responses(
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 200, description = "Aggregate position metrics", body = PositionsSummaryResponse),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_positions_summary(
    _access: WorkspaceReadAccess,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<PositionsSummaryResponse>> {
    // A live account leaves paper positions out, as the account valuation does.
    let live = current_execution_mode(state.as_ref()).await == "live";
    let row: PositionSummaryRow = sqlx::query_as(
        r#"
        WITH active_positions AS (
            SELECT
                id,
                market_id,
                COALESCE(source, 0)::smallint AS source,
                quantity,
                current_price,
                COALESCE(entry_price, (yes_entry_price + no_entry_price), 0) AS entry_price,
                (quantity * COALESCE(entry_price, (yes_entry_price + no_entry_price), 0)) AS entry_value,
                unrealized_pnl,
                COALESCE(last_updated, updated_at, entry_timestamp) AS sort_updated
            FROM positions
            WHERE is_open = TRUE
              AND ($1 OR execution_mode <> 'paper')
        ),
        ranked_active AS (
            SELECT
                *,
                ROW_NUMBER() OVER (
                    PARTITION BY market_id, source
                    ORDER BY sort_updated DESC, id DESC
                ) AS rn
            FROM active_positions
        ),
        effective_active AS (
            SELECT *
            FROM ranked_active
            WHERE rn = 1
        ),
        duplicate_active_markets AS (
            SELECT COUNT(*)::bigint AS duplicate_open_markets
            FROM (
                SELECT market_id, source
                FROM active_positions
                GROUP BY market_id, source
                HAVING COUNT(*) > 1
            ) dup
        ),
        closed_stats AS (
            SELECT
                COUNT(*) FILTER (WHERE state = 4)::bigint AS closed_positions,
                COUNT(*) FILTER (WHERE state = 4 AND COALESCE(realized_pnl, 0) > 0)::bigint AS wins,
                COUNT(*) FILTER (WHERE state = 4 AND COALESCE(realized_pnl, 0) < 0)::bigint AS losses,
                COUNT(*) FILTER (WHERE state = 4 AND COALESCE(realized_pnl, 0) = 0)::bigint AS flat_closes
            FROM positions
            WHERE $1 OR execution_mode <> 'paper'
        )
        SELECT
            COALESCE((SELECT COUNT(*) FROM effective_active), 0)::bigint AS open_positions,
            COALESCE((SELECT COUNT(DISTINCT market_id) FROM effective_active), 0)::bigint AS open_markets,
            COALESCE((SELECT duplicate_open_markets FROM duplicate_active_markets), 0)::bigint AS duplicate_open_markets,
            COALESCE((SELECT SUM(CASE
                WHEN current_price IS NOT NULL THEN quantity * current_price
                ELSE GREATEST(entry_value + COALESCE(unrealized_pnl, 0), 0)
            END) FROM effective_active), 0) AS portfolio_value,
            COALESCE((SELECT COUNT(*) FROM effective_active WHERE current_price IS NOT NULL), 0)::bigint AS priced_open_positions,
            COALESCE((SELECT COUNT(*) FROM effective_active WHERE current_price IS NULL), 0)::bigint AS unpriced_open_positions,
            COALESCE((SELECT SUM(quantity * entry_price) FROM effective_active WHERE current_price IS NULL), 0) AS unpriced_position_cost_basis,
            COALESCE((SELECT SUM(unrealized_pnl) FROM effective_active), 0) AS unrealized_pnl,
            COALESCE((SELECT COUNT(*) FROM active_positions), 0)::bigint AS raw_open_positions,
            COALESCE((SELECT SUM(CASE
                WHEN current_price IS NOT NULL THEN quantity * current_price
                ELSE GREATEST(entry_value + COALESCE(unrealized_pnl, 0), 0)
            END) FROM active_positions), 0) AS raw_portfolio_value,
            COALESCE((SELECT COUNT(*) FROM active_positions WHERE current_price IS NULL), 0)::bigint AS raw_unpriced_open_positions,
            COALESCE((SELECT SUM(quantity * entry_price) FROM active_positions WHERE current_price IS NULL), 0) AS raw_unpriced_position_cost_basis,
            COALESCE((SELECT SUM(unrealized_pnl) FROM active_positions), 0) AS raw_unrealized_pnl,
            closed_positions,
            wins,
            losses,
            flat_closes,
            CASE
                WHEN (wins + losses) > 0
                    THEN (wins::numeric / (wins + losses)::numeric) * 100
                ELSE 0
            END AS win_rate,
            (SELECT COUNT(*) FROM positions WHERE is_open = FALSE)::bigint AS closed_list_count
        FROM closed_stats
        "#,
    )
    .bind(!live)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(PositionsSummaryResponse {
        open_positions: row.open_positions,
        open_markets: row.open_markets,
        duplicate_open_markets: row.duplicate_open_markets,
        portfolio_value: row.portfolio_value,
        priced_open_positions: row.priced_open_positions,
        unpriced_open_positions: row.unpriced_open_positions,
        unpriced_position_cost_basis: row.unpriced_position_cost_basis,
        unrealized_pnl: row.unrealized_pnl,
        raw_open_positions: row.raw_open_positions,
        raw_portfolio_value: row.raw_portfolio_value,
        raw_unpriced_open_positions: row.raw_unpriced_open_positions,
        raw_unpriced_position_cost_basis: row.raw_unpriced_position_cost_basis,
        raw_unrealized_pnl: row.raw_unrealized_pnl,
        closed_positions: row.closed_positions,
        wins: row.wins,
        losses: row.losses,
        flat_closes: row.flat_closes,
        win_rate: row.win_rate,
        closed_list_count: row.closed_list_count,
    }))
}

/// Get a specific position.
#[utoipa::path(
    get,
    path = "/api/v1/positions/{position_id}",
    tag = "positions",
    params(
        ("position_id" = Uuid, Path, description = "Position identifier")
    ),
    responses(
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 200, description = "Position details", body = PositionResponse),
        (status = 404, description = "Position not found")
    )
)]
pub async fn get_position(
    _access: WorkspaceReadAccess,
    State(state): State<Arc<AppState>>,
    Path(position_id): Path<Uuid>,
) -> ApiResult<Json<PositionResponse>> {
    let sql = format!("{POSITION_ROW_SELECT} WHERE id = $1");
    let row: Option<PositionRow> = sqlx::query_as(&sql)
        .bind(position_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(row) => Ok(Json(PositionResponse::from(row))),
        None => Err(ApiError::NotFound(format!(
            "Position {} not found",
            position_id
        ))),
    }
}

/// Close a position.
#[utoipa::path(
    post,
    path = "/api/v1/positions/{position_id}/close",
    tag = "positions",
    params(
        ("position_id" = Uuid, Path, description = "Position identifier")
    ),
    request_body = ClosePositionRequest,
    responses(
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Insufficient canonical workspace permissions"),
        (status = 200, description = "Position closed", body = PositionResponse),
        (status = 404, description = "Position not found"),
        (status = 400, description = "Invalid request")
    )
)]
pub async fn close_position(
    _access: WorkspaceTradeAccess,
    State(state): State<Arc<AppState>>,
    axum::Extension(claims): axum::Extension<auth::Claims>,
    Path(position_id): Path<Uuid>,
    Json(request): Json<ClosePositionRequest>,
) -> ApiResult<Json<PositionResponse>> {
    if request.limit_price.is_some() {
        return Err(ApiError::BadRequest(
            "Manual close queues a market exit; limit_price is not supported".to_string(),
        ));
    }

    // First, fetch the position
    let row = load_closeable_position(&state.pool, position_id).await?;

    let row = match row {
        Some(r) => r,
        None => {
            return Err(ApiError::NotFound(format!(
                "Open position {} not found",
                position_id
            )))
        }
    };

    let close_quantity = request.quantity.unwrap_or(row.quantity);

    if close_quantity > row.quantity {
        return Err(ApiError::BadRequest(
            "Close quantity exceeds position size".to_string(),
        ));
    }
    if close_quantity != row.quantity {
        return Err(ApiError::BadRequest(
            "Partial manual close is not supported; queueing exits is full-position only"
                .to_string(),
        ));
    }

    let mut position = state
        .position_service
        .repo()
        .get(position_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("Open position {} not found", position_id)))?;

    // A queued exit runs only while the exit handler is on: say so rather
    // than accept an exit that would wait unseen.
    let exit_handler_on =
        crate::runtime_sync::canonical_workspace_exit_handler_enabled(&state.pool).await?
            || crate::runtime_sync::env_enabled("EXIT_HANDLER_ENABLED");
    if !exit_handler_on
        && !matches!(
            position.state,
            PositionState::ExitReady | PositionState::Closing
        )
    {
        return Err(ApiError::Conflict(
            "The exit handler is off, so a queued exit would not run; turn it on first".into(),
        ));
    }

    let execution_mode = current_execution_mode(state.as_ref()).await;
    let (strategy, source_label) = trade_event_labels(row.source);
    let ctx = EventContext {
        wallet_address: None,
        execution_mode: execution_mode.to_string(),
        strategy: strategy.to_string(),
        source_label: source_label.to_string(),
    };

    match position.state {
        PositionState::Open => {
            // The exit handler rewrites an open position's marks every pass,
            // which moves its version on: a request that loses that race
            // reads the position again and retries.
            let mut attempts = 0;
            loop {
                match state
                    .position_service
                    .mark_exit_ready(&mut position, "manual_close_request", &ctx)
                    .await
                {
                    Ok(()) => break,
                    Err(e) if attempts < 3 && e.to_string().contains("Stale position update") => {
                        attempts += 1;
                        position = state
                            .position_service
                            .repo()
                            .get(position_id)
                            .await
                            .map_err(|e| ApiError::Internal(e.to_string()))?
                            .ok_or_else(|| {
                                ApiError::NotFound(format!("Open position {position_id} not found"))
                            })?;
                        if position.state != PositionState::Open {
                            // Moved on meanwhile (the exit handler queued it).
                            break;
                        }
                    }
                    Err(e) => return Err(ApiError::BadRequest(e.to_string())),
                }
            }
        }
        // Already in the exit flow. Nothing is written: a write would bump
        // the row's version under a sale in progress and fail its booking.
        PositionState::ExitReady | PositionState::Closing => {}
        PositionState::ExitFailed => {
            let recovered = state
                .position_service
                .attempt_exit_recovery(&mut position, &ctx)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            if !recovered {
                return Err(ApiError::BadRequest(
                    "Position could not be re-queued for exit".to_string(),
                ));
            }
        }
        PositionState::Stalled => {
            // Validate before mutating: only held shares can be queued for sale.
            let requeued = state
                .position_service
                .requeue_stalled_for_exit(&mut position, &ctx)
                .await
                .map_err(|e| ApiError::BadRequest(e.to_string()))?;
            if !requeued {
                return Err(ApiError::BadRequest(
                    "Stalled position holds no shares to close".to_string(),
                ));
            }
        }
        PositionState::EntryFailed if position.is_one_legged_entry_fail() => {
            let held_leg = position.one_legged_held_leg();
            state
                .position_service
                .transition_one_legged_to_exit_ready(
                    &mut position,
                    held_leg,
                    "manual_close_one_legged",
                    &ctx,
                )
                .await
                .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        }
        PositionState::Pending => {
            return Err(ApiError::BadRequest(
                "Position is still entering and cannot be manually closed yet".to_string(),
            ))
        }
        PositionState::EntryFailed => {
            return Err(ApiError::BadRequest(
                "Position failed to enter and has no exit flow to queue".to_string(),
            ))
        }
        PositionState::Closed => {
            return Err(ApiError::NotFound(format!(
                "Open position {} not found",
                position_id
            )))
        }
    }

    super::audit_change(
        &state,
        Uuid::parse_str(&claims.sub).unwrap_or_default(),
        ::auth::AuditAction::ManualExit,
        format!("position/{position_id}"),
        serde_json::json!({
            "market_id": row.market_id,
            "state": position_state_label(position.state),
            "execution_mode": position.execution_mode,
        }),
    )
    .await;

    // Publish position update via WebSocket
    let update = PositionUpdate {
        position_id,
        market_id: row.market_id.clone(),
        update_type: PositionUpdateType::Updated,
        quantity: row.quantity,
        current_price: row.current_price,
        unrealized_pnl: position.unrealized_pnl,
        timestamp: Utc::now(),
    };
    let _ = state.publish_position(update);

    let pnl_pct = unrealized_pct(position.unrealized_pnl, row.entry_price, row.quantity);

    Ok(Json(PositionResponse {
        id: row.id,
        market_id: row.market_id,
        outcome: row.outcome,
        side: row.side,
        quantity: row.quantity,
        entry_price: row.entry_price,
        current_price: row.current_price,
        unrealized_pnl: position.unrealized_pnl,
        unrealized_pnl_pct: pnl_pct,
        stop_loss: row.stop_loss,
        take_profit: row.take_profit,
        realized_pnl: row.realized_pnl,
        state: position_state_label(position.state).to_string(),
        opened_at: row.opened_at,
        updated_at: position.last_updated,
        held_yes_qty: position.held_yes_qty,
        held_no_qty: position.held_no_qty,
        exited_yes_qty: position.exited_yes_qty,
        exited_no_qty: position.exited_no_qty,
        yes_exit_price: position.yes_exit_price,
        no_exit_price: position.no_exit_price,
        resolution_winner: position.resolution_winner.clone(),
        exit_strategy: match position.exit_strategy {
            polymarket_core::types::ExitStrategy::HoldToResolution => "hold_to_resolution",
            polymarket_core::types::ExitStrategy::ExitOnCorrection => "exit_on_correction",
        }
        .to_string(),
        is_open: position.should_persist_as_open(),
    }))
}

async fn load_closeable_position(
    pool: &sqlx::PgPool,
    position_id: Uuid,
) -> ApiResult<Option<CloseablePositionRow>> {
    let row: Option<CloseablePositionRow> = sqlx::query_as(
        r#"
        SELECT id, market_id,
               COALESCE(outcome, 'both') AS outcome,
               COALESCE(side, 'long') AS side,
               quantity,
               COALESCE(entry_price, (yes_entry_price + no_entry_price)) AS entry_price,
               COALESCE(current_price, (yes_entry_price + no_entry_price)) AS current_price,
               stop_loss, take_profit,
               realized_pnl,
               COALESCE(source, 0)::smallint AS source,
               COALESCE(opened_at, entry_timestamp) AS opened_at
        FROM positions
        WHERE id = $1
          AND is_open = TRUE
        "#,
    )
    .bind(position_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(row)
}

/// Unrealized P&L as a percentage of the position's cost: the stored P&L,
/// net of the fees its buys paid, so the two figures always agree.
fn unrealized_pct(unrealized_pnl: Decimal, entry_price: Decimal, quantity: Decimal) -> Decimal {
    let cost = entry_price * quantity;
    if cost > Decimal::ZERO {
        unrealized_pnl / cost * Decimal::new(100, 0)
    } else {
        Decimal::ZERO
    }
}

#[cfg(test)]
mod unrealized_pct_tests {
    use super::*;

    #[test]
    fn the_percentage_is_the_stored_pnl_over_cost() {
        // 10 shares bought at 0.40 ($4), marked at 0.50, less 0.168 of fees.
        let pct = unrealized_pct(Decimal::new(832, 3), Decimal::new(40, 2), Decimal::TEN);
        assert_eq!(pct, Decimal::new(208, 1));
        assert_eq!(
            unrealized_pct(Decimal::ONE, Decimal::ZERO, Decimal::TEN),
            Decimal::ZERO
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = false)]
    async fn history_pages_cover_more_than_500_tied_timestamps_without_duplicates(
        pool: sqlx::PgPool,
    ) {
        use sqlx::Executor;
        pool.execute(
            "CREATE VIEW positions AS SELECT md5(g::text)::uuid AS id,
            'market'::text AS market_id,'yes'::text AS outcome,'long'::text AS side,
            10::numeric AS quantity,0.4::numeric AS entry_price,0.5::numeric AS current_price,
            0.4::numeric AS yes_entry_price,0::numeric AS no_entry_price,
            0::numeric AS unrealized_pnl, NULL::numeric AS stop_loss,NULL::numeric AS take_profit,
            1::numeric AS realized_pnl,4::smallint AS state,NULL::timestamptz AS opened_at,
            '2026-10-19T00:00:00Z'::timestamptz AS entry_timestamp,
            NOW() AS updated_at,NULL::timestamptz AS last_updated,
            0::numeric AS held_yes_qty,0::numeric AS held_no_qty,
            10::numeric AS exited_yes_qty,0::numeric AS exited_no_qty,
            0.5::numeric AS yes_exit_price,NULL::numeric AS no_exit_price,
            NULL::text AS resolution_winner,0::smallint AS exit_strategy,0::smallint AS source,(g>600) AS is_open
            FROM generate_series(1,605) g",
        )
        .await
        .unwrap();
        let mut query = ListPositionsQuery {
            market_id: None,
            outcome: None,
            status: "closed".into(),
            limit: 25,
            offset: 0,
        };
        let mut seen = Vec::new();
        for page in 0..24 {
            query.offset = page * 25;
            let rows = load_position_rows(&pool, &query).await.unwrap();
            assert_eq!(rows.len(), 25);
            seen.extend(rows.into_iter().map(|row| row.id));
        }
        let expected: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM positions WHERE is_open=FALSE ORDER BY id DESC")
                .fetch_all(&pool)
                .await
                .unwrap();
        let open: Uuid = sqlx::query_scalar("SELECT id FROM positions WHERE is_open=TRUE LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            load_closeable_position(&pool, open)
                .await
                .unwrap()
                .unwrap()
                .source,
            0
        );
        assert!(load_closeable_position(&pool, seen[0])
            .await
            .unwrap()
            .is_none());
        assert_eq!(seen, expected);
        assert_eq!(
            seen.iter().collect::<std::collections::HashSet<_>>().len(),
            600
        );
        query.offset = 600;
        assert!(load_position_rows(&pool, &query).await.unwrap().is_empty());
        query.offset = -1;
        assert!(load_position_rows(&pool, &query).await.is_err());
    }

    #[test]
    fn test_position_response_serialization() {
        let position = PositionResponse {
            id: Uuid::new_v4(),
            market_id: "market1".to_string(),
            outcome: "yes".to_string(),
            side: "long".to_string(),
            quantity: Decimal::new(100, 0),
            entry_price: Decimal::new(50, 2),
            current_price: Decimal::new(55, 2),
            unrealized_pnl: Decimal::new(5, 0),
            unrealized_pnl_pct: Decimal::new(10, 0),
            stop_loss: Some(Decimal::new(45, 2)),
            take_profit: None,
            realized_pnl: None,
            state: "open".to_string(),
            opened_at: Utc::now(),
            updated_at: Utc::now(),
            held_yes_qty: Decimal::new(100, 0),
            held_no_qty: Decimal::ZERO,
            exited_yes_qty: Decimal::ZERO,
            exited_no_qty: Decimal::ZERO,
            yes_exit_price: None,
            no_exit_price: None,
            resolution_winner: None,
            exit_strategy: "hold_to_resolution".to_string(),
            is_open: true,
        };

        let json = serde_json::to_string(&position).unwrap();
        assert!(json.contains("market1"));
        assert!(json.contains("yes"));
        assert!(json.contains("stop_loss"));
        assert!(!json.contains("take_profit")); // skipped when None
    }

    #[test]
    fn test_close_position_request() {
        let request = ClosePositionRequest {
            quantity: Some(Decimal::new(50, 0)),
            limit_price: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("50"));
    }

    #[test]
    fn test_position_state_name() {
        assert_eq!(position_state_name(1), "open");
        assert_eq!(position_state_name(4), "closed");
        assert_eq!(position_state_name(5), "entry_failed");
    }

    #[test]
    fn test_validate_status_filter() {
        assert!(validate_status_filter("open").is_ok());
        assert!(validate_status_filter("closed").is_ok());
        assert!(validate_status_filter("all").is_ok());
        assert!(validate_status_filter("bad").is_err());
    }
}
