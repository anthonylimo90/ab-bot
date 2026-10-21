//! Backtesting operation handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::sync::{Arc, OnceLock};
use tokio::sync::Semaphore;
use tracing::{error, info};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use backtester::{
    ArbitrageStrategy, BacktestSimulator, DataQuery, GridStrategy, HistoricalDataStore,
    MeanReversionStrategy, MomentumStrategy, SimulatorConfig,
    SlippageModel as BacktesterSlippageModel, Strategy, TimeResolution,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::workspace_scope::{WorkspaceReadAccess, WorkspaceTradeAccess};

/// Request to run a backtest.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RunBacktestRequest {
    /// Strategy to backtest.
    pub strategy: StrategyConfig,
    /// Start date for backtest.
    pub start_date: DateTime<Utc>,
    /// End date for backtest.
    pub end_date: DateTime<Utc>,
    /// Initial capital.
    pub initial_capital: Decimal,
    /// Markets to include (None = all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markets: Option<Vec<String>>,
    /// Slippage model.
    #[serde(default)]
    pub slippage_model: SlippageModel,
    /// Trading fee percentage.
    #[serde(default = "default_fee")]
    pub fee_pct: Decimal,
}

fn default_fee() -> Decimal {
    Decimal::new(2, 2) // 2.0%
}

/// Strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StrategyConfig {
    /// Arbitrage strategy.
    Arbitrage {
        /// Minimum spread to trigger.
        min_spread: Decimal,
        /// Maximum position size.
        max_position: Decimal,
    },
    /// Momentum strategy.
    Momentum {
        /// Lookback period in hours.
        lookback_hours: i64,
        /// Momentum threshold.
        threshold: Decimal,
        /// Position size.
        position_size: Decimal,
    },
    /// Mean reversion strategy.
    MeanReversion {
        /// Moving average window in hours.
        window_hours: i64,
        /// Standard deviation threshold.
        std_threshold: Decimal,
        /// Position size.
        position_size: Decimal,
    },
    /// Grid trading strategy.
    Grid {
        /// Number of grid levels above and below center.
        grid_levels: usize,
        /// Spacing between levels as a fraction (e.g. 0.02 = 2%).
        grid_spacing_pct: Decimal,
        /// Order size per level as fraction of portfolio.
        order_size: Decimal,
    },
}

/// Slippage model configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlippageModel {
    /// No slippage.
    #[default]
    None,
    /// Fixed slippage percentage.
    Fixed {
        /// Slippage percentage.
        pct: Decimal,
    },
    /// Volume-based slippage.
    VolumeBased {
        /// Base slippage.
        base_pct: Decimal,
        /// Volume impact factor.
        volume_factor: Decimal,
    },
}

/// Backtest result response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BacktestResultResponse {
    /// Result identifier.
    pub id: Uuid,
    /// Strategy used.
    pub strategy: StrategyConfig,
    /// Backtest period start.
    pub start_date: DateTime<Utc>,
    /// Backtest period end.
    pub end_date: DateTime<Utc>,
    /// Initial capital.
    pub initial_capital: Decimal,
    /// Final portfolio value.
    pub final_value: Decimal,
    /// Total return.
    pub total_return: Decimal,
    /// Total return percentage.
    pub total_return_pct: Decimal,
    /// Annualized return (`null` when undefined or beyond the storable range).
    pub annualized_return: Option<Decimal>,
    /// Sharpe ratio (`null` when undefined).
    pub sharpe_ratio: Option<Decimal>,
    /// Sortino ratio (`null` when undefined, e.g. no losing period).
    pub sortino_ratio: Option<Decimal>,
    /// Maximum drawdown.
    pub max_drawdown: Decimal,
    /// Maximum drawdown percentage.
    pub max_drawdown_pct: Decimal,
    /// Total number of trades.
    pub total_trades: i64,
    /// Winning trades.
    pub winning_trades: i64,
    /// Losing trades.
    pub losing_trades: i64,
    /// Win rate percentage.
    pub win_rate: Decimal,
    /// Average profit per winning trade.
    pub avg_win: Decimal,
    /// Average loss per losing trade.
    pub avg_loss: Decimal,
    /// Profit factor (`null` when undefined, e.g. no losing trade).
    pub profit_factor: Option<Decimal>,
    /// Total fees paid.
    pub total_fees: Decimal,
    /// Backtest run timestamp.
    pub created_at: DateTime<Utc>,
    /// Status (pending, running, completed, failed).
    pub status: String,
    /// Whether the run was triggered manually or by automation.
    pub trigger_mode: String,
    /// Backtest schedule identifier for automated runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<Uuid>,
    /// Human-readable trigger label for scheduled runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_label: Option<String>,
    /// Optional market filter applied to this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markets: Option<Vec<String>>,
    /// Error message (if failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Equity curve (daily values).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equity_curve: Option<Vec<EquityPoint>>,
    /// Expectancy (average profit per trade).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expectancy: Option<Decimal>,
    /// Calmar ratio (annualized return / max drawdown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calmar_ratio: Option<Decimal>,
    /// Value at Risk at 95% confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_95: Option<Decimal>,
    /// Conditional VaR (Expected Shortfall) at 95%.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cvar_95: Option<Decimal>,
    /// Recovery factor (net profit / max drawdown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_factor: Option<Decimal>,
    /// Best single trade return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_trade_return: Option<Decimal>,
    /// Worst single trade return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_trade_return: Option<Decimal>,
    /// Maximum consecutive wins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_consecutive_wins: Option<i64>,
    /// Maximum consecutive losses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_consecutive_losses: Option<i64>,
    /// Average trade duration in hours.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_trade_duration_hours: Option<Decimal>,
    /// Full trade log (only included in detail view).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_log: Option<Vec<TradeLogEntry>>,
}

/// A single trade from the backtest log.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TradeLogEntry {
    /// Market ID.
    pub market_id: String,
    /// Outcome ID.
    pub outcome_id: String,
    /// Trade type (buy or close).
    pub trade_type: String,
    /// Entry timestamp.
    pub entry_time: DateTime<Utc>,
    /// Exit timestamp (if closed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_time: Option<DateTime<Utc>>,
    /// Entry price.
    pub entry_price: Decimal,
    /// Exit price (if closed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_price: Option<Decimal>,
    /// Quantity.
    pub quantity: Decimal,
    /// Fees paid.
    pub fees: Decimal,
    /// Realized P&L (if closed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pnl: Option<Decimal>,
    /// Return percentage (if closed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_pct: Option<f64>,
}

/// Point on equity curve.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EquityPoint {
    /// Timestamp.
    pub timestamp: DateTime<Utc>,
    /// Portfolio value.
    pub value: Decimal,
    /// Drawdown percentage.
    pub drawdown_pct: Decimal,
}

/// Query parameters for listing backtest results.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListBacktestQuery {
    /// Filter by strategy type.
    pub strategy_type: Option<String>,
    /// Filter by status.
    pub status: Option<String>,
    /// Maximum results.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ListBacktestScheduleQuery {
    /// Include disabled schedules.
    pub include_disabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BacktestScheduleResponse {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub strategy: StrategyConfig,
    pub lookback_days: i32,
    pub initial_capital: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markets: Option<Vec<String>>,
    pub slippage_model: SlippageModel,
    pub fee_pct: Decimal,
    pub interval_hours: i32,
    pub next_run_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateBacktestScheduleRequest {
    pub name: String,
    pub strategy: StrategyConfig,
    pub lookback_days: i32,
    pub initial_capital: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markets: Option<Vec<String>>,
    #[serde(default)]
    pub slippage_model: SlippageModel,
    #[serde(default = "default_fee")]
    pub fee_pct: Decimal,
    pub interval_hours: i32,
    #[serde(default = "default_schedule_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
pub struct UpdateBacktestScheduleRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<StrategyConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookback_days: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_capital: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markets: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slippage_model: Option<SlippageModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_pct: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_hours: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
}

fn default_schedule_enabled() -> bool {
    true
}

#[derive(Debug, FromRow)]
struct BacktestRow {
    id: Uuid,
    strategy: serde_json::Value,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    initial_capital: Decimal,
    markets: Option<Vec<String>>,
    final_value: Option<Decimal>,
    total_return: Option<Decimal>,
    total_return_pct: Option<Decimal>,
    annualized_return: Option<Decimal>,
    sharpe_ratio: Option<Decimal>,
    sortino_ratio: Option<Decimal>,
    max_drawdown: Option<Decimal>,
    max_drawdown_pct: Option<Decimal>,
    total_trades: Option<i64>,
    winning_trades: Option<i64>,
    losing_trades: Option<i64>,
    win_rate: Option<Decimal>,
    avg_win: Option<Decimal>,
    avg_loss: Option<Decimal>,
    profit_factor: Option<Decimal>,
    total_fees: Option<Decimal>,
    expectancy: Option<Decimal>,
    calmar_ratio: Option<Decimal>,
    var_95: Option<Decimal>,
    cvar_95: Option<Decimal>,
    recovery_factor: Option<Decimal>,
    best_trade_return: Option<Decimal>,
    worst_trade_return: Option<Decimal>,
    max_consecutive_wins: Option<i32>,
    max_consecutive_losses: Option<i32>,
    avg_trade_duration_hours: Option<Decimal>,
    status: String,
    trigger_mode: Option<String>,
    schedule_id: Option<Uuid>,
    trigger_label: Option<String>,
    error: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct BacktestRowWithCurve {
    id: Uuid,
    strategy: serde_json::Value,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    initial_capital: Decimal,
    markets: Option<Vec<String>>,
    final_value: Option<Decimal>,
    total_return: Option<Decimal>,
    total_return_pct: Option<Decimal>,
    annualized_return: Option<Decimal>,
    sharpe_ratio: Option<Decimal>,
    sortino_ratio: Option<Decimal>,
    max_drawdown: Option<Decimal>,
    max_drawdown_pct: Option<Decimal>,
    total_trades: Option<i64>,
    winning_trades: Option<i64>,
    losing_trades: Option<i64>,
    win_rate: Option<Decimal>,
    avg_win: Option<Decimal>,
    avg_loss: Option<Decimal>,
    profit_factor: Option<Decimal>,
    total_fees: Option<Decimal>,
    expectancy: Option<Decimal>,
    calmar_ratio: Option<Decimal>,
    var_95: Option<Decimal>,
    cvar_95: Option<Decimal>,
    recovery_factor: Option<Decimal>,
    best_trade_return: Option<Decimal>,
    worst_trade_return: Option<Decimal>,
    max_consecutive_wins: Option<i32>,
    max_consecutive_losses: Option<i32>,
    avg_trade_duration_hours: Option<Decimal>,
    status: String,
    trigger_mode: Option<String>,
    schedule_id: Option<Uuid>,
    trigger_label: Option<String>,
    error: Option<String>,
    equity_curve: Option<serde_json::Value>,
    trade_log: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct BacktestScheduleRow {
    id: Uuid,
    name: String,
    enabled: bool,
    strategy: serde_json::Value,
    lookback_days: i32,
    initial_capital: Decimal,
    markets: Option<Vec<String>>,
    slippage_model: serde_json::Value,
    fee_pct: Decimal,
    interval_hours: i32,
    next_run_at: DateTime<Utc>,
    last_run_at: Option<DateTime<Utc>>,
    last_result_id: Option<Uuid>,
    last_status: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn row_to_response(row: BacktestRow) -> BacktestResultResponse {
    let strategy: StrategyConfig =
        serde_json::from_value(row.strategy).unwrap_or(StrategyConfig::Arbitrage {
            min_spread: Decimal::new(2, 2),
            max_position: Decimal::new(1000, 0),
        });

    BacktestResultResponse {
        id: row.id,
        strategy,
        start_date: row.start_date,
        end_date: row.end_date,
        initial_capital: row.initial_capital,
        final_value: row.final_value.unwrap_or(Decimal::ZERO),
        total_return: row.total_return.unwrap_or(Decimal::ZERO),
        total_return_pct: row.total_return_pct.unwrap_or(Decimal::ZERO),
        annualized_return: row.annualized_return,
        sharpe_ratio: row.sharpe_ratio,
        sortino_ratio: row.sortino_ratio,
        max_drawdown: row.max_drawdown.unwrap_or(Decimal::ZERO),
        max_drawdown_pct: row.max_drawdown_pct.unwrap_or(Decimal::ZERO),
        total_trades: row.total_trades.unwrap_or(0),
        winning_trades: row.winning_trades.unwrap_or(0),
        losing_trades: row.losing_trades.unwrap_or(0),
        win_rate: row.win_rate.unwrap_or(Decimal::ZERO),
        avg_win: row.avg_win.unwrap_or(Decimal::ZERO),
        avg_loss: row.avg_loss.unwrap_or(Decimal::ZERO),
        profit_factor: row.profit_factor,
        total_fees: row.total_fees.unwrap_or(Decimal::ZERO),
        created_at: row.created_at,
        status: row.status,
        trigger_mode: row.trigger_mode.unwrap_or_else(|| "manual".to_string()),
        schedule_id: row.schedule_id,
        trigger_label: row.trigger_label,
        markets: row.markets,
        error: row.error,
        equity_curve: None,
        expectancy: row.expectancy,
        calmar_ratio: row.calmar_ratio,
        var_95: row.var_95,
        cvar_95: row.cvar_95,
        recovery_factor: row.recovery_factor,
        best_trade_return: row.best_trade_return,
        worst_trade_return: row.worst_trade_return,
        max_consecutive_wins: row.max_consecutive_wins.map(|v| v as i64),
        max_consecutive_losses: row.max_consecutive_losses.map(|v| v as i64),
        avg_trade_duration_hours: row.avg_trade_duration_hours,
        trade_log: None,
    }
}

fn schedule_row_to_response(row: BacktestScheduleRow) -> BacktestScheduleResponse {
    let strategy: StrategyConfig =
        serde_json::from_value(row.strategy).unwrap_or(StrategyConfig::Arbitrage {
            min_spread: Decimal::new(2, 2),
            max_position: Decimal::new(1000, 0),
        });
    let slippage_model: SlippageModel =
        serde_json::from_value(row.slippage_model).unwrap_or_default();

    BacktestScheduleResponse {
        id: row.id,
        name: row.name,
        enabled: row.enabled,
        strategy,
        lookback_days: row.lookback_days,
        initial_capital: row.initial_capital,
        markets: row.markets,
        slippage_model,
        fee_pct: row.fee_pct,
        interval_hours: row.interval_hours,
        next_run_at: row.next_run_at,
        last_run_at: row.last_run_at,
        last_result_id: row.last_result_id,
        last_status: row.last_status,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Run a backtest.
#[utoipa::path(
    post,
    path = "/api/v1/backtest",
    tag = "backtest",
    request_body = RunBacktestRequest,
    responses(
        (status = 202, description = "Backtest started", body = BacktestResultResponse),
        (status = 400, description = "Invalid request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn run_backtest(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceTradeAccess,
    Json(request): Json<RunBacktestRequest>,
) -> ApiResult<Json<BacktestResultResponse>> {
    let response = enqueue_backtest(
        state.pool.clone(),
        request,
        "manual",
        None,
        Some("manual".to_string()),
    )
    .await?;

    Ok(Json(response))
}

pub(crate) async fn enqueue_backtest(
    pool: PgPool,
    request: RunBacktestRequest,
    trigger_mode: &str,
    schedule_id: Option<Uuid>,
    trigger_label: Option<String>,
) -> ApiResult<BacktestResultResponse> {
    validate_backtest_request(&request)?;

    let result_id = Uuid::new_v4();
    let now = Utc::now();

    let strategy_json =
        serde_json::to_value(&request.strategy).map_err(|e| ApiError::Internal(e.to_string()))?;
    let slippage_json = serde_json::to_value(&request.slippage_model)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let strategy_name = strategy_name(&request.strategy);

    sqlx::query(
        r#"
        INSERT INTO backtest_results
        (id, strategy_name, strategy_params, start_time, end_time, data_points,
         initial_capital, final_value, total_return, return_pct, annualized_return,
         max_drawdown, sharpe_ratio, sortino_ratio, win_rate, profit_factor,
         total_trades, winning_trades, losing_trades, total_fees, total_slippage,
         avg_trade_duration_hours, computed_at, created_at,
         strategy, start_date, end_date, initial_capital_api, slippage_model,
         fee_pct, status, schedule_id, trigger_mode, trigger_label, markets)
        VALUES ($1, $2, $3, $4, $5, $6,
                $7, $8, $9, $10, $11,
                $12, $13, $14, $15, $16,
                $17, $18, $19, $20, $21,
                $22, $23, $24,
                $25, $26, $27, $28, $29,
                $30, 'running', $31, $32, $33, $34)
        "#,
    )
    .bind(result_id)
    .bind(strategy_name)
    .bind(&strategy_json)
    .bind(request.start_date)
    .bind(request.end_date)
    .bind(0_i32)
    .bind(request.initial_capital)
    .bind(request.initial_capital)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(0_i32)
    .bind(0_i32)
    .bind(0_i32)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(Decimal::ZERO)
    .bind(now)
    .bind(now)
    .bind(&strategy_json)
    .bind(request.start_date)
    .bind(request.end_date)
    .bind(request.initial_capital)
    .bind(&slippage_json)
    .bind(request.fee_pct)
    .bind(schedule_id)
    .bind(trigger_mode)
    .bind(&trigger_label)
    .bind(request.markets.clone())
    .execute(&pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(schedule_id) = schedule_id {
        sqlx::query(
            r#"
            UPDATE backtest_schedules
            SET last_result_id = $2,
                last_status = 'running',
                updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(schedule_id)
        .bind(result_id)
        .execute(&pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    let task_request = request.clone();
    let task_pool = pool.clone();
    tokio::spawn(async move {
        run_backtest_task(task_pool, result_id, task_request, schedule_id).await;
    });

    info!(backtest_id = %result_id, trigger_mode, schedule_id = ?schedule_id, "Backtest task spawned");

    Ok(BacktestResultResponse {
        id: result_id,
        strategy: request.strategy,
        start_date: request.start_date,
        end_date: request.end_date,
        initial_capital: request.initial_capital,
        final_value: Decimal::ZERO,
        total_return: Decimal::ZERO,
        total_return_pct: Decimal::ZERO,
        annualized_return: Some(Decimal::ZERO),
        sharpe_ratio: Some(Decimal::ZERO),
        sortino_ratio: Some(Decimal::ZERO),
        max_drawdown: Decimal::ZERO,
        max_drawdown_pct: Decimal::ZERO,
        total_trades: 0,
        winning_trades: 0,
        losing_trades: 0,
        win_rate: Decimal::ZERO,
        avg_win: Decimal::ZERO,
        avg_loss: Decimal::ZERO,
        profit_factor: Some(Decimal::ZERO),
        total_fees: Decimal::ZERO,
        created_at: now,
        status: "running".to_string(),
        trigger_mode: trigger_mode.to_string(),
        schedule_id,
        trigger_label,
        markets: request.markets,
        error: None,
        equity_curve: None,
        expectancy: None,
        calmar_ratio: None,
        var_95: None,
        cvar_95: None,
        recovery_factor: None,
        best_trade_return: None,
        worst_trade_return: None,
        max_consecutive_wins: None,
        max_consecutive_losses: None,
        avg_trade_duration_hours: None,
        trade_log: None,
    })
}

fn strategy_name(strategy: &StrategyConfig) -> &'static str {
    match strategy {
        StrategyConfig::Arbitrage { .. } => "arb",
        StrategyConfig::Momentum { .. } => "momentum",
        StrategyConfig::MeanReversion { .. } => "mean_reversion",
        StrategyConfig::Grid { .. } => "grid",
    }
}

/// Resource limits for backtest runs. Read from the environment once.
///
/// | variable | default | meaning |
/// |---|---|---|
/// | `BACKTEST_MAX_CONCURRENT_RUNS` | 1 | simulations running at once (manual and scheduled share them) |
/// | `BACKTEST_MAX_RANGE_DAYS` | 90 | longest `end_date - start_date` / schedule lookback |
/// | `BACKTEST_MAX_MARKETS` | 100 | longest explicit `markets` filter |
/// | `BACKTEST_MAX_SNAPSHOTS` | 1,000,000 | largest data set a run may load; larger runs fail |
///
/// Non-numeric or non-positive values fall back to the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BacktestLimits {
    pub max_concurrent_runs: usize,
    pub max_range_days: i64,
    pub max_markets: usize,
    pub max_snapshots: usize,
}

impl BacktestLimits {
    pub(crate) const DEFAULT_MAX_CONCURRENT_RUNS: usize = 1;
    pub(crate) const DEFAULT_MAX_RANGE_DAYS: i64 = 90;
    pub(crate) const DEFAULT_MAX_MARKETS: usize = 100;
    pub(crate) const DEFAULT_MAX_SNAPSHOTS: usize = 1_000_000;

    /// Build the limits from a variable lookup (the environment in production).
    pub(crate) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        fn positive<T: std::str::FromStr + PartialOrd + Default>(
            raw: Option<String>,
            default: T,
        ) -> T {
            raw.and_then(|v| v.trim().parse::<T>().ok())
                .filter(|v| *v > T::default())
                .unwrap_or(default)
        }
        Self {
            max_concurrent_runs: positive(
                get("BACKTEST_MAX_CONCURRENT_RUNS"),
                Self::DEFAULT_MAX_CONCURRENT_RUNS,
            ),
            max_range_days: positive(get("BACKTEST_MAX_RANGE_DAYS"), Self::DEFAULT_MAX_RANGE_DAYS),
            max_markets: positive(get("BACKTEST_MAX_MARKETS"), Self::DEFAULT_MAX_MARKETS),
            max_snapshots: positive(get("BACKTEST_MAX_SNAPSHOTS"), Self::DEFAULT_MAX_SNAPSHOTS),
        }
    }

    pub(crate) fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }
}

fn limits() -> &'static BacktestLimits {
    static LIMITS: OnceLock<BacktestLimits> = OnceLock::new();
    LIMITS.get_or_init(BacktestLimits::from_env)
}

/// Permits shared by every backtest run in the process, manual or scheduled.
fn run_permits() -> &'static Semaphore {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| Semaphore::new(limits().max_concurrent_runs))
}

/// Run `work` once a permit of `permits` is available, holding it until done.
async fn with_run_permit<F, T>(permits: &Semaphore, work: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // The semaphore is never closed, so acquiring cannot fail.
    let _permit = permits.acquire().await.expect("backtest semaphore closed");
    work.await
}

/// Run CPU-bound `work` on a blocking thread so a long simulation can never
/// starve the async runtime (a panic inside it becomes an error, not a crash of
/// the task that must record the outcome).
async fn run_on_blocking_thread<T, F>(work: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| anyhow::anyhow!("Backtest worker terminated unexpectedly: {e}"))
}

/// Check the size limits of a run's date range and market filter.
fn validate_run_limits(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    markets: Option<&[String]>,
    limits: &BacktestLimits,
) -> Result<(), String> {
    if end - start > chrono::Duration::days(limits.max_range_days) {
        return Err(format!(
            "Backtest range exceeds the maximum of {} days",
            limits.max_range_days
        ));
    }
    validate_markets_limit(markets, limits)
}

/// Check the size of an explicit market filter.
fn validate_markets_limit(
    markets: Option<&[String]>,
    limits: &BacktestLimits,
) -> Result<(), String> {
    match markets {
        Some(markets) if markets.len() > limits.max_markets => Err(format!(
            "Backtest market filter exceeds the maximum of {} markets",
            limits.max_markets
        )),
        _ => Ok(()),
    }
}

/// Bar size of the data every backtest runs on. Strategy lookbacks are given in
/// hours and converted to bars with it, and the query is built with it, so the
/// two cannot drift apart.
const BACKTEST_RESOLUTION: TimeResolution = TimeResolution::Minute5;

/// Longest lookback/window accepted, in hours (14 days = 4,032 five-minute
/// bars, below the simulator's per-market history cap).
const MAX_LOOKBACK_HOURS: i64 = 24 * 14;

/// Convert a lookback in hours to a number of bars of `resolution`, rounding up
/// so the window covers at least the requested time. `None` for non-positive
/// hours.
fn hours_to_bars(hours: i64, resolution: TimeResolution) -> Option<usize> {
    if hours <= 0 {
        return None;
    }
    let bar_seconds = resolution.to_duration().num_seconds().max(1);
    let seconds = hours.checked_mul(3600)?;
    usize::try_from((seconds + bar_seconds - 1) / bar_seconds).ok()
}

/// Convert a dollar amount to a fraction of the initial capital in `[0, 1]`.
///
/// The simulator sizes positions as a fraction of portfolio value, while the
/// arbitrage `max_position` is entered in dollars. It is converted against the
/// initial capital (the portfolio value when the run starts); amounts at or
/// above the capital mean "all of it" (the simulator's per-position cap still
/// applies).
fn dollars_to_portfolio_fraction(dollars: Decimal, initial_capital: Decimal) -> Decimal {
    if initial_capital <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    (dollars / initial_capital).clamp(Decimal::ZERO, Decimal::ONE)
}

/// Check strategy parameters that would otherwise silently produce a strategy
/// that never trades (or a nonsensical size).
fn validate_strategy_config(config: &StrategyConfig) -> Result<(), String> {
    let check_hours = |name: &str, hours: i64| -> Result<(), String> {
        if !(1..=MAX_LOOKBACK_HOURS).contains(&hours) {
            return Err(format!(
                "{name} must be between 1 and {MAX_LOOKBACK_HOURS} hours"
            ));
        }
        Ok(())
    };
    match config {
        StrategyConfig::Arbitrage { max_position, .. } => {
            if *max_position <= Decimal::ZERO {
                return Err("Arbitrage max_position (in dollars) must be positive".to_string());
            }
        }
        StrategyConfig::Momentum { lookback_hours, .. } => {
            check_hours("Momentum lookback_hours", *lookback_hours)?;
        }
        StrategyConfig::MeanReversion { window_hours, .. } => {
            check_hours("Mean reversion window_hours", *window_hours)?;
        }
        StrategyConfig::Grid { .. } => {}
    }
    Ok(())
}

/// Build the backtester strategy for an API strategy config.
///
/// Parameter units are normalized here:
/// * `lookback_hours` / `window_hours` become bars of [`BACKTEST_RESOLUTION`];
/// * the arbitrage `max_position` (dollars) becomes a fraction of
///   `initial_capital`;
/// * `position_size` / `order_size` are already fractions of the portfolio.
fn build_strategy(
    config: &StrategyConfig,
    initial_capital: Decimal,
    fee_pct: Decimal,
) -> Result<Box<dyn Strategy>, String> {
    validate_strategy_config(config)?;
    let bars = |hours: i64| {
        hours_to_bars(hours, BACKTEST_RESOLUTION)
            .ok_or_else(|| "lookback must be a positive number of hours".to_string())
    };

    Ok(match config {
        StrategyConfig::Arbitrage {
            min_spread,
            max_position,
        } => Box::new(
            ArbitrageStrategy::new(
                *min_spread,
                dollars_to_portfolio_fraction(*max_position, initial_capital),
                10,
            )
            .with_fee(fee_pct),
        ),
        StrategyConfig::Momentum {
            lookback_hours,
            threshold,
            position_size,
        } => Box::new(MomentumStrategy::new(
            bars(*lookback_hours)?,
            *threshold,
            *position_size,
        )),
        StrategyConfig::MeanReversion {
            window_hours,
            std_threshold,
            position_size,
        } => Box::new(MeanReversionStrategy::new(
            bars(*window_hours)?,
            std_threshold.to_f64().unwrap_or(2.0),
            *position_size,
        )),
        StrategyConfig::Grid {
            grid_levels,
            grid_spacing_pct,
            order_size,
        } => Box::new(GridStrategy::new(
            *grid_levels,
            *grid_spacing_pct,
            *order_size,
        )),
    })
}

fn validate_backtest_request(request: &RunBacktestRequest) -> ApiResult<()> {
    validate_backtest_request_with(request, limits())
}

fn validate_backtest_request_with(
    request: &RunBacktestRequest,
    limits: &BacktestLimits,
) -> ApiResult<()> {
    validate_strategy_config(&request.strategy).map_err(ApiError::BadRequest)?;

    if request.end_date <= request.start_date {
        return Err(ApiError::BadRequest(
            "End date must be after start date".to_string(),
        ));
    }

    validate_run_limits(
        request.start_date,
        request.end_date,
        request.markets.as_deref(),
        limits,
    )
    .map_err(ApiError::BadRequest)?;

    if request.initial_capital <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Initial capital must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Background task to run the backtest.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_backtest_task(
    pool: PgPool,
    result_id: Uuid,
    request: RunBacktestRequest,
    schedule_id: Option<Uuid>,
) {
    // Manual and scheduled runs share the permits; later runs wait here (their
    // row stays `running`) instead of all simulating at once.
    with_run_permit(run_permits(), async {
        run_backtest_inner(pool, result_id, request, schedule_id).await;
    })
    .await;
}

async fn run_backtest_inner(
    pool: PgPool,
    result_id: Uuid,
    request: RunBacktestRequest,
    schedule_id: Option<Uuid>,
) {
    info!(backtest_id = %result_id, "Starting backtest execution");

    // Create data store
    let data_store = HistoricalDataStore::new(pool.clone());

    // Convert slippage model
    let backtester_slippage = match request.slippage_model.clone() {
        SlippageModel::None => BacktesterSlippageModel::None,
        SlippageModel::Fixed { pct } => BacktesterSlippageModel::Fixed(pct),
        SlippageModel::VolumeBased {
            base_pct,
            volume_factor,
        } => BacktesterSlippageModel::VolumeBased {
            base_pct,
            size_impact: volume_factor,
        },
    };

    // Configure simulator
    let simulator_config = SimulatorConfig {
        initial_capital: request.initial_capital,
        slippage_model: backtester_slippage,
        fee_model: backtester::simulator::FeeModel::Fixed(request.fee_pct),
        max_snapshots: Some(limits().max_snapshots),
        ..Default::default()
    };

    let simulator = BacktestSimulator::new(data_store, simulator_config);

    // Create strategy from config
    let result = match build_strategy(&request.strategy, request.initial_capital, request.fee_pct) {
        Ok(strategy) => {
            run_simulation(
                simulator,
                strategy,
                request.start_date,
                request.end_date,
                request.markets.clone(),
            )
            .await
        }
        Err(message) => Err(anyhow::anyhow!(message)),
    };

    finish_backtest(&pool, result_id, schedule_id, result).await;
}

/// Record the outcome of a run: store a successful result and mark it (and its
/// schedule) `completed`, or mark both `failed` with the reason.
///
/// If storing a successful result itself fails, the run is marked `failed` with
/// that error rather than being left `running`, and the schedule is only marked
/// `completed` after the result was actually written.
async fn finish_backtest(
    pool: &PgPool,
    result_id: Uuid,
    schedule_id: Option<Uuid>,
    result: anyhow::Result<backtester::BacktestResult>,
) {
    match result {
        Ok(backtest_result) => {
            match persist_backtest_result(pool, result_id, &backtest_result).await {
                Ok(()) => {
                    set_schedule_status(pool, schedule_id, result_id, "completed").await;
                    info!(
                        backtest_id = %result_id,
                        final_value = %backtest_result.final_value,
                        return_pct = %backtest_result.return_pct,
                        "Backtest completed successfully"
                    );
                }
                Err(e) => {
                    error!(backtest_id = %result_id, error = %e, "Failed to update backtest results");
                    fail_backtest(
                        pool,
                        result_id,
                        schedule_id,
                        &format!("Failed to store backtest results: {e}"),
                    )
                    .await;
                }
            }
        }
        Err(e) => {
            let error_msg = e.to_string();
            fail_backtest(pool, result_id, schedule_id, &error_msg).await;
            error!(backtest_id = %result_id, error = %error_msg, "Backtest failed");
        }
    }
}

/// Largest magnitude (exclusive) written to the `NUMERIC(24, 8)` metric columns.
/// The columns hold |value| < 1e16; the margin keeps rounding well inside it.
const MAX_STORED_METRIC: f64 = 1e15;

/// Convert a float metric into a storable decimal.
///
/// NaN, +/-infinity (e.g. profit factor with no losing trade) and magnitudes the
/// column cannot hold map to `None`, i.e. SQL `NULL` = "undefined". They are
/// never stored as `0`, which would read as a real, terrible result.
fn metric(val: f64) -> Option<Decimal> {
    if !val.is_finite() || val.abs() >= MAX_STORED_METRIC {
        return None;
    }
    Decimal::try_from(val).ok()
}

/// Write the finished result and mark the row `completed`.
///
/// Any database error (including a numeric overflow) is returned to the caller,
/// which must mark the run failed so the row never stays `running`.
async fn persist_backtest_result(
    pool: &PgPool,
    result_id: Uuid,
    backtest_result: &backtester::BacktestResult,
) -> Result<(), sqlx::Error> {
    // Build equity curve with running-peak drawdown
    let mut peak = Decimal::ZERO;
    let equity_curve: Vec<EquityPoint> = backtest_result
        .equity_curve
        .iter()
        .map(|(timestamp, value)| {
            if *value > peak {
                peak = *value;
            }
            let drawdown_pct = if peak > Decimal::ZERO {
                ((peak - *value) / peak) * Decimal::from(100u32)
            } else {
                Decimal::ZERO
            };
            EquityPoint {
                timestamp: *timestamp,
                value: *value,
                drawdown_pct,
            }
        })
        .collect();
    let equity_json = serde_json::to_value(&equity_curve).ok();

    // Serialize trade log
    let trade_log: Vec<TradeLogEntry> = backtest_result
        .trades
        .iter()
        .map(|t| TradeLogEntry {
            market_id: t.market_id.clone(),
            outcome_id: t.outcome_id.clone(),
            trade_type: format!("{:?}", t.trade_type).to_lowercase(),
            entry_time: t.entry_time,
            exit_time: t.exit_time,
            entry_price: t.entry_price,
            exit_price: t.exit_price,
            quantity: t.quantity,
            fees: t.fees,
            pnl: t.pnl,
            return_pct: t.return_pct,
        })
        .collect();
    let trade_log_json = serde_json::to_value(&trade_log).ok();

    sqlx::query(
        r#"
        UPDATE backtest_results SET
            status = 'completed',
            completed_at = NOW(),
            final_value = $2,
            total_return = $3,
            total_return_pct = $4,
            annualized_return = $5,
            sharpe_ratio = $6,
            sortino_ratio = $7,
            max_drawdown = $8,
            max_drawdown_pct = $9,
            total_trades = $10,
            winning_trades = $11,
            losing_trades = $12,
            win_rate = $13,
            profit_factor = $14,
            total_fees = $15,
            avg_win = $16,
            avg_loss = $17,
            equity_curve = $18,
            trade_log = $19,
            expectancy = $20,
            calmar_ratio = $21,
            var_95 = $22,
            cvar_95 = $23,
            recovery_factor = $24,
            best_trade_return = $25,
            worst_trade_return = $26,
            max_consecutive_wins = $27,
            max_consecutive_losses = $28,
            avg_trade_duration_hours = $29
        WHERE id = $1
        "#,
    )
    .bind(result_id) // $1
    .bind(backtest_result.final_value) // $2
    .bind(backtest_result.total_return) // $3
    .bind(metric(backtest_result.return_pct)) // $4
    .bind(metric(backtest_result.annualized_return)) // $5
    .bind(metric(backtest_result.sharpe_ratio)) // $6
    .bind(metric(backtest_result.sortino_ratio)) // $7
    .bind(metric(backtest_result.max_drawdown).unwrap_or(Decimal::ZERO)) // $8 (NOT NULL, finite)
    .bind(metric(backtest_result.max_drawdown * 100.0)) // $9
    .bind(backtest_result.total_trades as i64) // $10
    .bind(backtest_result.winning_trades as i64) // $11
    .bind(backtest_result.losing_trades as i64) // $12
    .bind(metric(backtest_result.win_rate).unwrap_or(Decimal::ZERO)) // $13 (NOT NULL, in [0, 1])
    .bind(metric(backtest_result.profit_factor)) // $14
    .bind(backtest_result.total_fees) // $15
    .bind(metric(backtest_result.avg_win)) // $16
    .bind(metric(backtest_result.avg_loss)) // $17
    .bind(equity_json) // $18
    .bind(trade_log_json) // $19
    .bind(metric(backtest_result.expectancy)) // $20
    .bind(metric(backtest_result.calmar_ratio)) // $21
    .bind(metric(backtest_result.var_95)) // $22
    .bind(metric(backtest_result.cvar_95)) // $23
    .bind(metric(backtest_result.recovery_factor)) // $24
    .bind(metric(backtest_result.best_trade_return)) // $25
    .bind(metric(backtest_result.worst_trade_return)) // $26
    .bind(backtest_result.max_consecutive_wins as i32) // $27
    .bind(backtest_result.max_consecutive_losses as i32) // $28
    .bind(metric(backtest_result.avg_trade_duration_hours).unwrap_or(Decimal::ZERO)) // $29 (NOT NULL, finite)
    .execute(pool)
    .await?;

    Ok(())
}

/// Record the final status of the run on its schedule (no-op for manual runs).
async fn set_schedule_status(
    pool: &PgPool,
    schedule_id: Option<Uuid>,
    result_id: Uuid,
    status: &str,
) {
    let Some(schedule_id) = schedule_id else {
        return;
    };
    if let Err(e) = sqlx::query(
        r#"
        UPDATE backtest_schedules
        SET last_status = $3,
            last_result_id = $2,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .bind(result_id)
    .bind(status)
    .execute(pool)
    .await
    {
        error!(backtest_id = %result_id, schedule_id = %schedule_id, error = %e, "Failed to update backtest schedule status");
    }
}

/// Mark a run `failed` with `message` (truncated to a sane length) and reflect
/// that on its schedule.
async fn fail_backtest(pool: &PgPool, result_id: Uuid, schedule_id: Option<Uuid>, message: &str) {
    let message: String = message.chars().take(2000).collect();
    if let Err(db_err) = sqlx::query(
        "UPDATE backtest_results SET status = 'failed', error = $2, completed_at = NOW() WHERE id = $1",
    )
    .bind(result_id)
    .bind(&message)
    .execute(pool)
    .await
    {
        error!(backtest_id = %result_id, error = %db_err, "Failed to update backtest error");
    }
    set_schedule_status(pool, schedule_id, result_id, "failed").await;
}

/// Fail runs that have been `running` for longer than `max_age`.
///
/// A run only stays `running` that long if its worker died (process restart,
/// panic, lost database connection) before it could record an outcome. Their
/// schedules are marked failed too. Returns the number of runs reaped.
pub(crate) async fn reap_stale_running_backtests(
    pool: &PgPool,
    max_age: chrono::Duration,
) -> Result<u64, sqlx::Error> {
    let message = format!(
        "Timed out: no result was recorded within {} minutes (the worker likely stopped)",
        max_age.num_minutes()
    );
    let reaped: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        UPDATE backtest_results
        SET status = 'failed',
            error = $2,
            completed_at = NOW()
        WHERE status = 'running'
          AND created_at < NOW() - make_interval(secs => $1)
        RETURNING id
        "#,
    )
    .bind(max_age.num_seconds() as f64)
    .bind(&message)
    .fetch_all(pool)
    .await?;

    if !reaped.is_empty() {
        let ids: Vec<Uuid> = reaped.iter().map(|(id,)| *id).collect();
        sqlx::query(
            r#"
            UPDATE backtest_schedules
            SET last_status = 'failed',
                updated_at = NOW()
            WHERE last_result_id = ANY($1)
              AND last_status = 'running'
            "#,
        )
        .bind(&ids)
        .execute(pool)
        .await?;
    }

    Ok(reaped.len() as u64)
}

/// Run the simulation on a blocking thread.
///
/// The simulator's step loop is CPU-bound and almost never awaits, so running
/// it as an ordinary task would monopolize an async worker for the whole run.
/// The data fetch inside still uses the runtime (via `Handle::block_on`).
async fn run_simulation(
    simulator: BacktestSimulator,
    mut strategy: Box<dyn Strategy>,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    markets: Option<Vec<String>>,
) -> anyhow::Result<backtester::BacktestResult> {
    let handle = tokio::runtime::Handle::current();
    run_on_blocking_thread(move || {
        handle.block_on(run_strategy(
            &simulator,
            strategy.as_mut(),
            start_date,
            end_date,
            markets,
        ))
    })
    .await?
}

/// Run a strategy through the simulator.
async fn run_strategy<S: Strategy + ?Sized>(
    simulator: &BacktestSimulator,
    strategy: &mut S,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    markets: Option<Vec<String>>,
) -> anyhow::Result<backtester::BacktestResult> {
    let mut query = match markets {
        Some(markets) if !markets.is_empty() => {
            DataQuery::range(start_date, end_date).markets(markets)
        }
        _ => DataQuery::range(start_date, end_date),
    };
    query.resolution = BACKTEST_RESOLUTION;
    simulator.run(strategy, query).await
}

const LIST_QUERY: &str = r#"
    SELECT id, strategy, start_date, end_date, initial_capital, markets,
           final_value, total_return, total_return_pct, annualized_return,
           sharpe_ratio, sortino_ratio, max_drawdown, max_drawdown_pct,
           total_trades, winning_trades, losing_trades, win_rate,
           avg_win, avg_loss, profit_factor, total_fees,
           expectancy, calmar_ratio, var_95, cvar_95,
           recovery_factor, best_trade_return, worst_trade_return,
            max_consecutive_wins, max_consecutive_losses,
           avg_trade_duration_hours,
           status, trigger_mode, schedule_id, trigger_label, error, created_at
    FROM backtest_results
    WHERE ($1::text IS NULL OR status = $1)
      AND ($2::text IS NULL OR strategy->>'type' = $2)
    ORDER BY created_at DESC
    LIMIT $3 OFFSET $4
"#;

/// List backtest results.
#[utoipa::path(
    get,
    path = "/api/v1/backtest/results",
    tag = "backtest",
    params(ListBacktestQuery),
    responses(
        (status = 200, description = "List of backtest results", body = Vec<BacktestResultResponse>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_backtest_results(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceReadAccess,
    Query(query): Query<ListBacktestQuery>,
) -> ApiResult<Json<Vec<BacktestResultResponse>>> {
    let rows: Vec<BacktestRow> = sqlx::query_as(LIST_QUERY)
        .bind(&query.status)
        .bind(&query.strategy_type)
        .bind(query.limit)
        .bind(query.offset)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let results: Vec<BacktestResultResponse> = rows.into_iter().map(row_to_response).collect();

    Ok(Json(results))
}

/// Get a specific backtest result.
#[utoipa::path(
    get,
    path = "/api/v1/backtest/results/{result_id}",
    tag = "backtest",
    params(
        ("result_id" = Uuid, Path, description = "Backtest result identifier")
    ),
    responses(
        (status = 200, description = "Backtest result details", body = BacktestResultResponse),
        (status = 404, description = "Result not found")
    )
)]
pub async fn get_backtest_result(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceReadAccess,
    Path(result_id): Path<Uuid>,
) -> ApiResult<Json<BacktestResultResponse>> {
    let row: Option<BacktestRowWithCurve> = sqlx::query_as(
        r#"
        SELECT id, strategy, start_date, end_date, initial_capital, markets,
               final_value, total_return, total_return_pct, annualized_return,
               sharpe_ratio, sortino_ratio, max_drawdown, max_drawdown_pct,
               total_trades, winning_trades, losing_trades, win_rate,
               avg_win, avg_loss, profit_factor, total_fees,
               expectancy, calmar_ratio, var_95, cvar_95,
               recovery_factor, best_trade_return, worst_trade_return,
               max_consecutive_wins, max_consecutive_losses,
                avg_trade_duration_hours,
               status, trigger_mode, schedule_id, trigger_label, error, equity_curve, trade_log, created_at
        FROM backtest_results
        WHERE id = $1
        "#,
    )
    .bind(result_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(row) => {
            let strategy: StrategyConfig =
                serde_json::from_value(row.strategy).unwrap_or(StrategyConfig::Arbitrage {
                    min_spread: Decimal::new(2, 2),
                    max_position: Decimal::new(1000, 0),
                });

            let equity_curve: Option<Vec<EquityPoint>> = row
                .equity_curve
                .and_then(|v| serde_json::from_value(v).ok());

            let trade_log: Option<Vec<TradeLogEntry>> =
                row.trade_log.and_then(|v| serde_json::from_value(v).ok());

            Ok(Json(BacktestResultResponse {
                id: row.id,
                strategy,
                start_date: row.start_date,
                end_date: row.end_date,
                initial_capital: row.initial_capital,
                final_value: row.final_value.unwrap_or(Decimal::ZERO),
                total_return: row.total_return.unwrap_or(Decimal::ZERO),
                total_return_pct: row.total_return_pct.unwrap_or(Decimal::ZERO),
                annualized_return: row.annualized_return,
                sharpe_ratio: row.sharpe_ratio,
                sortino_ratio: row.sortino_ratio,
                max_drawdown: row.max_drawdown.unwrap_or(Decimal::ZERO),
                max_drawdown_pct: row.max_drawdown_pct.unwrap_or(Decimal::ZERO),
                total_trades: row.total_trades.unwrap_or(0),
                winning_trades: row.winning_trades.unwrap_or(0),
                losing_trades: row.losing_trades.unwrap_or(0),
                win_rate: row.win_rate.unwrap_or(Decimal::ZERO),
                avg_win: row.avg_win.unwrap_or(Decimal::ZERO),
                avg_loss: row.avg_loss.unwrap_or(Decimal::ZERO),
                profit_factor: row.profit_factor,
                total_fees: row.total_fees.unwrap_or(Decimal::ZERO),
                created_at: row.created_at,
                status: row.status,
                trigger_mode: row.trigger_mode.unwrap_or_else(|| "manual".to_string()),
                schedule_id: row.schedule_id,
                trigger_label: row.trigger_label,
                markets: row.markets,
                error: row.error,
                equity_curve,
                expectancy: row.expectancy,
                calmar_ratio: row.calmar_ratio,
                var_95: row.var_95,
                cvar_95: row.cvar_95,
                recovery_factor: row.recovery_factor,
                best_trade_return: row.best_trade_return,
                worst_trade_return: row.worst_trade_return,
                max_consecutive_wins: row.max_consecutive_wins.map(|v| v as i64),
                max_consecutive_losses: row.max_consecutive_losses.map(|v| v as i64),
                avg_trade_duration_hours: row.avg_trade_duration_hours,
                trade_log,
            }))
        }
        None => Err(ApiError::NotFound(format!(
            "Backtest result {} not found",
            result_id
        ))),
    }
}

/// List automated backtest schedules.
#[utoipa::path(
    get,
    path = "/api/v1/backtest/schedules",
    tag = "backtest",
    params(ListBacktestScheduleQuery),
    responses(
        (status = 200, description = "Backtest schedules", body = Vec<BacktestScheduleResponse>)
    )
)]
pub async fn list_backtest_schedules(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceReadAccess,
    Query(query): Query<ListBacktestScheduleQuery>,
) -> ApiResult<Json<Vec<BacktestScheduleResponse>>> {
    let include_disabled = query.include_disabled.unwrap_or(false);
    let rows: Vec<BacktestScheduleRow> = sqlx::query_as(
        r#"
        SELECT id, name, enabled, strategy, lookback_days, initial_capital,
               markets, slippage_model, fee_pct, interval_hours, next_run_at,
               last_run_at, last_result_id, last_status, created_at, updated_at
        FROM backtest_schedules
        WHERE ($1::bool = true OR enabled = true)
        ORDER BY enabled DESC, next_run_at ASC, name ASC
        "#,
    )
    .bind(include_disabled)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(
        rows.into_iter().map(schedule_row_to_response).collect(),
    ))
}

/// Create an automated backtest schedule.
#[utoipa::path(
    post,
    path = "/api/v1/backtest/schedules",
    tag = "backtest",
    request_body = CreateBacktestScheduleRequest,
    responses(
        (status = 201, description = "Backtest schedule created", body = BacktestScheduleResponse)
    )
)]
pub async fn create_backtest_schedule(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceTradeAccess,
    Json(request): Json<CreateBacktestScheduleRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<BacktestScheduleResponse>)> {
    validate_schedule_request(
        request.lookback_days,
        request.initial_capital,
        request.interval_hours,
    )?;
    validate_strategy_config(&request.strategy).map_err(ApiError::BadRequest)?;
    validate_markets_limit(request.markets.as_deref(), limits()).map_err(ApiError::BadRequest)?;

    let strategy_json =
        serde_json::to_value(&request.strategy).map_err(|e| ApiError::Internal(e.to_string()))?;
    let slippage_json = serde_json::to_value(&request.slippage_model)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let row: BacktestScheduleRow = sqlx::query_as(
        r#"
        INSERT INTO backtest_schedules (
            name, enabled, strategy, lookback_days, initial_capital, markets,
            slippage_model, fee_pct, interval_hours, next_run_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
        RETURNING id, name, enabled, strategy, lookback_days, initial_capital,
                  markets, slippage_model, fee_pct, interval_hours, next_run_at,
                  last_run_at, last_result_id, last_status, created_at, updated_at
        "#,
    )
    .bind(&request.name)
    .bind(request.enabled)
    .bind(strategy_json)
    .bind(request.lookback_days)
    .bind(request.initial_capital)
    .bind(request.markets.clone())
    .bind(slippage_json)
    .bind(request.fee_pct)
    .bind(request.interval_hours)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(schedule_row_to_response(row)),
    ))
}

/// Update an automated backtest schedule.
#[utoipa::path(
    patch,
    path = "/api/v1/backtest/schedules/{schedule_id}",
    tag = "backtest",
    request_body = UpdateBacktestScheduleRequest,
    params(
        ("schedule_id" = Uuid, Path, description = "Backtest schedule identifier")
    ),
    responses(
        (status = 200, description = "Backtest schedule updated", body = BacktestScheduleResponse),
        (status = 404, description = "Backtest schedule not found")
    )
)]
pub async fn update_backtest_schedule(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceTradeAccess,
    Path(schedule_id): Path<Uuid>,
    Json(request): Json<UpdateBacktestScheduleRequest>,
) -> ApiResult<Json<BacktestScheduleResponse>> {
    if let Some(lookback_days) = request.lookback_days {
        validate_schedule_request(
            lookback_days,
            request.initial_capital.unwrap_or(Decimal::ONE),
            request.interval_hours.unwrap_or(1),
        )?;
    } else if let Some(interval_hours) = request.interval_hours {
        validate_schedule_request(
            1,
            request.initial_capital.unwrap_or(Decimal::ONE),
            interval_hours,
        )?;
    } else if let Some(initial_capital) = request.initial_capital {
        validate_schedule_request(1, initial_capital, 1)?;
    }
    if let Some(strategy) = &request.strategy {
        validate_strategy_config(strategy).map_err(ApiError::BadRequest)?;
    }
    validate_markets_limit(request.markets.as_deref(), limits()).map_err(ApiError::BadRequest)?;

    let strategy_json = request
        .strategy
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let slippage_json = request
        .slippage_model
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let row: Option<BacktestScheduleRow> = sqlx::query_as(
        r#"
        UPDATE backtest_schedules
        SET name = COALESCE($2, name),
            enabled = COALESCE($3, enabled),
            strategy = COALESCE($4, strategy),
            lookback_days = COALESCE($5, lookback_days),
            initial_capital = COALESCE($6, initial_capital),
            markets = COALESCE($7, markets),
            slippage_model = COALESCE($8, slippage_model),
            fee_pct = COALESCE($9, fee_pct),
            interval_hours = COALESCE($10, interval_hours),
            next_run_at = COALESCE($11, next_run_at),
            updated_at = NOW()
        WHERE id = $1
        RETURNING id, name, enabled, strategy, lookback_days, initial_capital,
                  markets, slippage_model, fee_pct, interval_hours, next_run_at,
                  last_run_at, last_result_id, last_status, created_at, updated_at
        "#,
    )
    .bind(schedule_id)
    .bind(request.name)
    .bind(request.enabled)
    .bind(strategy_json)
    .bind(request.lookback_days)
    .bind(request.initial_capital)
    .bind(request.markets)
    .bind(slippage_json)
    .bind(request.fee_pct)
    .bind(request.interval_hours)
    .bind(request.next_run_at)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(row) => Ok(Json(schedule_row_to_response(row))),
        None => Err(ApiError::NotFound(format!(
            "Backtest schedule {} not found",
            schedule_id
        ))),
    }
}

/// Delete an automated backtest schedule.
#[utoipa::path(
    delete,
    path = "/api/v1/backtest/schedules/{schedule_id}",
    tag = "backtest",
    params(
        ("schedule_id" = Uuid, Path, description = "Backtest schedule identifier")
    ),
    responses(
        (status = 204, description = "Backtest schedule deleted"),
        (status = 404, description = "Backtest schedule not found")
    )
)]
pub async fn delete_backtest_schedule(
    State(state): State<Arc<AppState>>,
    _access: WorkspaceTradeAccess,
    Path(schedule_id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode> {
    let deleted = sqlx::query(
        r#"
        DELETE FROM backtest_schedules
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .rows_affected();

    if deleted == 0 {
        return Err(ApiError::NotFound(format!(
            "Backtest schedule {} not found",
            schedule_id
        )));
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

fn validate_schedule_request(
    lookback_days: i32,
    initial_capital: Decimal,
    interval_hours: i32,
) -> ApiResult<()> {
    if lookback_days <= 0 {
        return Err(ApiError::BadRequest(
            "Lookback days must be positive".to_string(),
        ));
    }
    if i64::from(lookback_days) > limits().max_range_days {
        return Err(ApiError::BadRequest(format!(
            "Lookback days exceeds the maximum of {} days",
            limits().max_range_days
        )));
    }
    if initial_capital <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Initial capital must be positive".to_string(),
        ));
    }
    if interval_hours <= 0 {
        return Err(ApiError::BadRequest(
            "Interval hours must be positive".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strategy_config_serialization() {
        let arb = StrategyConfig::Arbitrage {
            min_spread: Decimal::new(2, 2),
            max_position: Decimal::new(5000, 0),
        };
        let json = serde_json::to_string(&arb).unwrap();
        assert!(json.contains("arbitrage"));
        assert!(json.contains("min_spread"));

        let momentum = StrategyConfig::Momentum {
            lookback_hours: 24,
            threshold: Decimal::new(5, 2),
            position_size: Decimal::new(1000, 0),
        };
        let json = serde_json::to_string(&momentum).unwrap();
        assert!(json.contains("momentum"));
        assert!(json.contains("lookback_hours"));

        let grid = StrategyConfig::Grid {
            grid_levels: 5,
            grid_spacing_pct: Decimal::new(2, 2),
            order_size: Decimal::new(5, 2),
        };
        let json = serde_json::to_string(&grid).unwrap();
        assert!(json.contains("grid"));
        assert!(json.contains("grid_levels"));
    }

    #[test]
    fn test_slippage_model_serialization() {
        let none = SlippageModel::None;
        let json = serde_json::to_string(&none).unwrap();
        assert!(json.contains("none"));

        let fixed = SlippageModel::Fixed {
            pct: Decimal::new(1, 3),
        };
        let json = serde_json::to_string(&fixed).unwrap();
        assert!(json.contains("fixed"));
    }

    #[test]
    fn test_backtest_result_response() {
        let result = BacktestResultResponse {
            id: Uuid::new_v4(),
            strategy: StrategyConfig::Arbitrage {
                min_spread: Decimal::new(1, 2),
                max_position: Decimal::new(1000, 0),
            },
            start_date: Utc::now() - chrono::Duration::days(30),
            end_date: Utc::now(),
            initial_capital: Decimal::new(10000, 0),
            final_value: Decimal::new(12500, 0),
            total_return: Decimal::new(2500, 0),
            total_return_pct: Decimal::new(25, 0),
            annualized_return: Some(Decimal::new(300, 0)),
            sharpe_ratio: Some(Decimal::new(185, 2)),
            sortino_ratio: Some(Decimal::new(225, 2)),
            max_drawdown: Decimal::new(500, 0),
            max_drawdown_pct: Decimal::new(5, 0),
            total_trades: 50,
            winning_trades: 35,
            losing_trades: 15,
            win_rate: Decimal::new(70, 0),
            avg_win: Decimal::new(100, 0),
            avg_loss: Decimal::new(50, 0),
            profit_factor: Some(Decimal::new(467, 2)),
            total_fees: Decimal::new(25, 0),
            created_at: Utc::now(),
            status: "completed".to_string(),
            trigger_mode: "manual".to_string(),
            schedule_id: None,
            trigger_label: None,
            markets: None,
            error: None,
            equity_curve: Some(vec![
                EquityPoint {
                    timestamp: Utc::now() - chrono::Duration::days(30),
                    value: Decimal::new(10000, 0),
                    drawdown_pct: Decimal::ZERO,
                },
                EquityPoint {
                    timestamp: Utc::now(),
                    value: Decimal::new(12500, 0),
                    drawdown_pct: Decimal::ZERO,
                },
            ]),
            expectancy: Some(Decimal::new(50, 0)),
            calmar_ratio: Some(Decimal::new(600, 2)),
            var_95: Some(Decimal::new(3, 2)),
            cvar_95: Some(Decimal::new(5, 2)),
            recovery_factor: Some(Decimal::new(5, 0)),
            best_trade_return: Some(Decimal::new(15, 2)),
            worst_trade_return: Some(Decimal::new(-8, 2)),
            max_consecutive_wins: Some(7),
            max_consecutive_losses: Some(3),
            avg_trade_duration_hours: Some(Decimal::new(48, 1)),
            trade_log: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("completed"));
        assert!(json.contains("sharpe_ratio"));
        assert!(json.contains("equity_curve"));
        assert!(json.contains("calmar_ratio"));
        assert!(json.contains("expectancy"));
    }

    #[test]
    fn test_run_backtest_request() {
        let request = RunBacktestRequest {
            strategy: StrategyConfig::MeanReversion {
                window_hours: 48,
                std_threshold: Decimal::new(2, 0),
                position_size: Decimal::new(500, 0),
            },
            start_date: Utc::now() - chrono::Duration::days(90),
            end_date: Utc::now(),
            initial_capital: Decimal::new(50000, 0),
            markets: Some(vec!["market1".to_string(), "market2".to_string()]),
            slippage_model: SlippageModel::VolumeBased {
                base_pct: Decimal::new(5, 4),
                volume_factor: Decimal::new(1, 3),
            },
            fee_pct: Decimal::new(1, 3),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("mean_reversion"));
        assert!(json.contains("window_hours"));
        assert!(json.contains("volume_based"));
    }

    #[test]
    fn test_trade_log_entry_serialization() {
        let entry = TradeLogEntry {
            market_id: "market-123".to_string(),
            outcome_id: "outcome-456".to_string(),
            trade_type: "buy".to_string(),
            entry_time: Utc::now(),
            exit_time: Some(Utc::now()),
            entry_price: Decimal::new(45, 2),
            exit_price: Some(Decimal::new(55, 2)),
            quantity: Decimal::new(100, 0),
            fees: Decimal::new(2, 0),
            pnl: Some(Decimal::new(8, 0)),
            return_pct: Some(0.222),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("market-123"));
        assert!(json.contains("buy"));
    }
}

/// Persistence of backtest outcomes: numeric range, NULL for undefined metrics,
/// failure handling and the stale-run reaper (sqlx::test).
#[cfg(test)]
mod persistence_tests {
    use super::*;
    use sqlx::PgPool;
    use std::collections::HashMap;

    const MIGRATION_085: &str =
        include_str!("../../../../migrations/085_backtest_results_metric_range.sql");

    /// `backtest_results` / `backtest_schedules` as created by migrations
    /// 004, 005, 025 and 050 (before migration 085).
    pub(super) async fn create_pre_085_schema(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE backtest_results (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                strategy_name VARCHAR(100) NOT NULL,
                strategy_params JSONB NOT NULL DEFAULT '{}',
                start_time TIMESTAMPTZ NOT NULL,
                end_time TIMESTAMPTZ NOT NULL,
                data_points INTEGER NOT NULL,
                initial_capital DECIMAL(18, 8) NOT NULL,
                final_value DECIMAL(18, 8) NOT NULL,
                total_return DECIMAL(18, 8) NOT NULL,
                return_pct DECIMAL(10, 6) NOT NULL,
                annualized_return DECIMAL(10, 6) NOT NULL,
                max_drawdown DECIMAL(10, 6) NOT NULL,
                sharpe_ratio DECIMAL(10, 6) NOT NULL,
                sortino_ratio DECIMAL(10, 6) NOT NULL,
                win_rate DECIMAL(5, 4) NOT NULL,
                profit_factor DECIMAL(10, 4) NOT NULL,
                total_trades INTEGER NOT NULL,
                winning_trades INTEGER NOT NULL,
                losing_trades INTEGER NOT NULL,
                total_fees DECIMAL(18, 8) NOT NULL,
                total_slippage DECIMAL(18, 8) NOT NULL,
                avg_trade_duration_hours DECIMAL(10, 2) NOT NULL,
                equity_curve JSONB,
                trade_log JSONB,
                computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                notes TEXT,
                strategy JSONB,
                start_date TIMESTAMPTZ,
                end_date TIMESTAMPTZ,
                initial_capital_api DECIMAL(18, 8),
                slippage_model JSONB,
                fee_pct DECIMAL(8, 6) DEFAULT 0.001,
                total_return_pct DECIMAL(10, 4),
                max_drawdown_pct DECIMAL(10, 4),
                avg_win DECIMAL(18, 8),
                avg_loss DECIMAL(18, 8),
                status VARCHAR(20) DEFAULT 'completed',
                error TEXT,
                completed_at TIMESTAMPTZ,
                expectancy DECIMAL(10, 6),
                calmar_ratio DECIMAL(10, 4),
                var_95 DECIMAL(10, 6),
                cvar_95 DECIMAL(10, 6),
                recovery_factor DECIMAL(10, 4),
                best_trade_return DECIMAL(10, 6),
                worst_trade_return DECIMAL(10, 6),
                max_consecutive_wins INTEGER,
                max_consecutive_losses INTEGER,
                schedule_id UUID,
                trigger_mode VARCHAR(20) NOT NULL DEFAULT 'manual',
                trigger_label TEXT,
                markets TEXT[]
            );
            CREATE INDEX idx_backtest_sharpe ON backtest_results(sharpe_ratio DESC);
            CREATE INDEX idx_backtest_return ON backtest_results(return_pct DESC);
            CREATE TABLE backtest_schedules (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                name TEXT NOT NULL UNIQUE,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                strategy JSONB NOT NULL DEFAULT
                    '{"type":"grid","grid_levels":3,"grid_spacing_pct":"0.02","order_size":"0.05"}',
                lookback_days INTEGER NOT NULL DEFAULT 7,
                initial_capital DECIMAL(18, 8) NOT NULL DEFAULT 10000,
                markets TEXT[],
                slippage_model JSONB NOT NULL DEFAULT '{"type":"none"}',
                fee_pct DECIMAL(8, 6) NOT NULL DEFAULT 0.02,
                interval_hours INTEGER NOT NULL DEFAULT 24,
                next_run_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                last_run_at TIMESTAMPTZ,
                last_result_id UUID REFERENCES backtest_results(id) ON DELETE SET NULL,
                last_status VARCHAR(20),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );
            "#,
        )
        .execute(pool)
        .await
        .expect("create schema");
    }

    pub(super) async fn apply_migration_085(pool: &PgPool) {
        sqlx::raw_sql(MIGRATION_085)
            .execute(pool)
            .await
            .expect("migration 085");
    }

    /// Insert a `running` row like `enqueue_backtest` does, created `age` ago.
    async fn insert_running(pool: &PgPool, age: chrono::Duration) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO backtest_results
            (id, strategy_name, start_time, end_time, data_points, initial_capital,
             final_value, total_return, return_pct, annualized_return, max_drawdown,
             sharpe_ratio, sortino_ratio, win_rate, profit_factor, total_trades,
             winning_trades, losing_trades, total_fees, total_slippage,
             avg_trade_duration_hours, status, created_at)
            VALUES ($1, 'arb', NOW(), NOW(), 0, 10000, 10000, 0, 0, 0, 0, 0, 0, 0, 0,
                    0, 0, 0, 0, 0, 0, 'running', $2)
            "#,
        )
        .bind(id)
        .bind(Utc::now() - age)
        .execute(pool)
        .await
        .expect("insert running row");
        id
    }

    async fn insert_schedule(pool: &PgPool, name: &str, result_id: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO backtest_schedules (name, last_result_id, last_status) \
             VALUES ($1, $2, 'running') RETURNING id",
        )
        .bind(name)
        .bind(result_id)
        .fetch_one(pool)
        .await
        .expect("insert schedule")
    }

    fn sample_result() -> backtester::BacktestResult {
        let now = Utc::now();
        backtester::BacktestResult {
            strategy_name: "arb".to_string(),
            strategy_params: HashMap::new(),
            start_time: now - chrono::Duration::days(1),
            end_time: now,
            data_points: 10,
            initial_capital: Decimal::new(10_000, 0),
            final_value: Decimal::new(10_500, 0),
            total_return: Decimal::new(500, 0),
            return_pct: 0.05,
            annualized_return: 3.5,
            max_drawdown: 0.02,
            sharpe_ratio: 1.8,
            sortino_ratio: 2.4,
            win_rate: 0.6,
            profit_factor: 1.7,
            total_trades: 10,
            winning_trades: 6,
            losing_trades: 4,
            total_fees: Decimal::new(25, 0),
            total_slippage: Decimal::new(5, 0),
            avg_trade_duration_hours: 3.0,
            equity_curve: vec![(now, Decimal::new(10_500, 0))],
            trades: vec![],
            computed_at: now,
            calmar_ratio: 1.2,
            var_95: 0.01,
            cvar_95: 0.015,
            recovery_factor: 2.0,
            best_trade_return: 0.1,
            worst_trade_return: -0.05,
            max_consecutive_wins: 3,
            max_consecutive_losses: 2,
            avg_win: 0.04,
            avg_loss: -0.02,
            expectancy: 0.01,
        }
    }

    #[allow(clippy::type_complexity)]
    async fn read_metrics(
        pool: &PgPool,
        id: Uuid,
    ) -> (
        String,
        Option<String>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) {
        sqlx::query_as(
            "SELECT status, error, annualized_return::float8, sharpe_ratio::float8, \
             sortino_ratio::float8, profit_factor::float8, calmar_ratio::float8, \
             recovery_factor::float8 FROM backtest_results WHERE id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("row")
    }

    async fn schedule_status(pool: &PgPool, id: Uuid) -> Option<String> {
        sqlx::query_scalar("SELECT last_status FROM backtest_schedules WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("schedule")
    }

    #[test]
    fn metric_maps_undefined_values_to_null() {
        assert_eq!(metric(1.5), Some(Decimal::new(15, 1)));
        assert_eq!(metric(-0.25), Some(Decimal::new(-25, 2)));
        assert_eq!(metric(0.0), Some(Decimal::ZERO));
        assert_eq!(metric(f64::NAN), None);
        assert_eq!(metric(f64::INFINITY), None);
        assert_eq!(metric(f64::NEG_INFINITY), None);
        // Beyond what NUMERIC(24, 8) holds.
        assert_eq!(metric(1e15), None);
        assert_eq!(metric(-1e20), None);
        assert_eq!(metric(1e300), None);
        // Large but storable.
        assert!(metric(123_456_789_012.5).is_some());
    }

    #[sqlx::test(migrations = false)]
    async fn old_schema_overflows_and_migration_085_fixes_it(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        let mut result = sample_result();
        // 1-day backtest annualized: far beyond DECIMAL(10, 6).
        result.annualized_return = 123_456.789;

        let id = insert_running(&pool, chrono::Duration::minutes(1)).await;
        let err = persist_backtest_result(&pool, id, &result)
            .await
            .expect_err("the narrow column must overflow");
        assert!(err.to_string().contains("overflow"), "got {err}");

        apply_migration_085(&pool).await;
        apply_migration_085(&pool).await; // idempotent

        persist_backtest_result(&pool, id, &result)
            .await
            .expect("widened columns accept the value");
        let (status, _, annualized, ..) = read_metrics(&pool, id).await;
        assert_eq!(status, "completed");
        assert_eq!(annualized, Some(123_456.789));
    }

    #[sqlx::test(migrations = false)]
    async fn non_finite_metrics_are_stored_as_null_not_zero(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;

        let mut result = sample_result();
        result.profit_factor = f64::INFINITY;
        result.sortino_ratio = f64::INFINITY;
        result.calmar_ratio = f64::INFINITY;
        result.recovery_factor = f64::INFINITY;
        result.sharpe_ratio = f64::NAN;
        result.annualized_return = 1e30;

        let id = insert_running(&pool, chrono::Duration::minutes(1)).await;
        persist_backtest_result(&pool, id, &result).await.unwrap();

        let (status, error, annualized, sharpe, sortino, profit_factor, calmar, recovery) =
            read_metrics(&pool, id).await;
        assert_eq!(status, "completed");
        assert_eq!(error, None);
        assert_eq!(annualized, None);
        assert_eq!(sharpe, None);
        assert_eq!(sortino, None);
        assert_eq!(profit_factor, None);
        assert_eq!(calmar, None);
        assert_eq!(recovery, None);

        // Finite metrics are still stored.
        let (win_rate, expectancy): (f64, Option<f64>) = sqlx::query_as(
            "SELECT win_rate::float8, expectancy::float8 FROM backtest_results WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(win_rate, 0.6);
        assert_eq!(expectancy, Some(0.01));
    }

    #[sqlx::test(migrations = false)]
    async fn rows_with_null_metrics_map_to_none_in_the_response(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;
        let mut result = sample_result();
        result.sortino_ratio = f64::INFINITY;
        result.profit_factor = f64::INFINITY;
        let id = insert_running(&pool, chrono::Duration::minutes(1)).await;
        persist_backtest_result(&pool, id, &result).await.unwrap();

        // Same columns and row mapping the list endpoint uses.
        let row: BacktestRow = sqlx::query_as(
            "SELECT id, COALESCE(strategy, '{}'::jsonb) AS strategy, \
             COALESCE(start_date, NOW()) AS start_date, COALESCE(end_date, NOW()) AS end_date, \
             initial_capital, markets, final_value, total_return, total_return_pct, \
             annualized_return, sharpe_ratio, sortino_ratio, max_drawdown, max_drawdown_pct, \
             total_trades::bigint, winning_trades::bigint, losing_trades::bigint, win_rate, \
             avg_win, avg_loss, profit_factor, total_fees, expectancy, calmar_ratio, var_95, \
             cvar_95, recovery_factor, best_trade_return, worst_trade_return, \
             max_consecutive_wins, max_consecutive_losses, avg_trade_duration_hours, \
             COALESCE(status, 'completed') AS status, trigger_mode, schedule_id, trigger_label, \
             error, created_at FROM backtest_results WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let response = row_to_response(row);
        assert_eq!(response.sortino_ratio, None);
        assert_eq!(response.profit_factor, None);
        assert!(response.sharpe_ratio.is_some());
        let json = serde_json::to_value(&response).unwrap();
        assert!(json["sortino_ratio"].is_null());
        assert!(json["profit_factor"].is_null());
    }

    #[sqlx::test(migrations = false)]
    async fn failed_result_write_marks_the_run_failed_not_running(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;

        // win_rate is DECIMAL(5, 4): 1e6 cannot be stored, so the UPDATE fails.
        let mut result = sample_result();
        result.win_rate = 1e6;

        let id = insert_running(&pool, chrono::Duration::minutes(1)).await;
        let schedule = insert_schedule(&pool, "nightly", id).await;
        finish_backtest(&pool, id, Some(schedule), Ok(result)).await;

        let (status, error, ..) = read_metrics(&pool, id).await;
        assert_eq!(status, "failed", "must not stay running");
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("Failed to store backtest results"),
            "got {error:?}"
        );
        // The schedule must not claim a completed run.
        assert_eq!(
            schedule_status(&pool, schedule).await.as_deref(),
            Some("failed")
        );
    }

    #[sqlx::test(migrations = false)]
    async fn schedule_completes_only_after_the_result_is_written(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;

        let id = insert_running(&pool, chrono::Duration::minutes(1)).await;
        let schedule = insert_schedule(&pool, "hourly", id).await;
        finish_backtest(&pool, id, Some(schedule), Ok(sample_result())).await;

        let (status, error, annualized, ..) = read_metrics(&pool, id).await;
        assert_eq!((status.as_str(), error), ("completed", None));
        assert_eq!(annualized, Some(3.5));
        assert_eq!(
            schedule_status(&pool, schedule).await.as_deref(),
            Some("completed")
        );
    }

    #[sqlx::test(migrations = false)]
    async fn simulation_errors_mark_run_and_schedule_failed(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;

        let id = insert_running(&pool, chrono::Duration::minutes(1)).await;
        let schedule = insert_schedule(&pool, "daily", id).await;
        finish_backtest(
            &pool,
            id,
            Some(schedule),
            Err(anyhow::anyhow!("No data available for the specified query")),
        )
        .await;

        let (status, error, ..) = read_metrics(&pool, id).await;
        assert_eq!(status, "failed");
        assert_eq!(
            error.as_deref(),
            Some("No data available for the specified query")
        );
        assert_eq!(
            schedule_status(&pool, schedule).await.as_deref(),
            Some("failed")
        );
    }

    #[sqlx::test(migrations = false)]
    async fn reaper_fails_only_stale_running_rows(pool: PgPool) {
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;

        let stale = insert_running(&pool, chrono::Duration::hours(3)).await;
        let recent = insert_running(&pool, chrono::Duration::minutes(10)).await;
        let done = insert_running(&pool, chrono::Duration::hours(5)).await;
        sqlx::query("UPDATE backtest_results SET status = 'completed' WHERE id = $1")
            .bind(done)
            .execute(&pool)
            .await
            .unwrap();
        let stale_schedule = insert_schedule(&pool, "stale", stale).await;
        let recent_schedule = insert_schedule(&pool, "recent", recent).await;

        let reaped = reap_stale_running_backtests(&pool, chrono::Duration::hours(2))
            .await
            .unwrap();
        assert_eq!(reaped, 1);

        let (status, error, ..) = read_metrics(&pool, stale).await;
        assert_eq!(status, "failed");
        assert!(error.unwrap().contains("Timed out"));
        assert_eq!(read_metrics(&pool, recent).await.0, "running");
        assert_eq!(read_metrics(&pool, done).await.0, "completed");
        assert_eq!(
            schedule_status(&pool, stale_schedule).await.as_deref(),
            Some("failed")
        );
        assert_eq!(
            schedule_status(&pool, recent_schedule).await.as_deref(),
            Some("running")
        );

        // Nothing left to reap; the call is repeatable.
        assert_eq!(
            reap_stale_running_backtests(&pool, chrono::Duration::hours(2))
                .await
                .unwrap(),
            0
        );
    }
}

/// Strategy parameters are converted to the units the backtester expects.
#[cfg(test)]
mod scaling_tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn param(strategy: &dyn Strategy, key: &str) -> String {
        strategy
            .parameters()
            .get(key)
            .cloned()
            .unwrap_or_else(|| panic!("missing parameter {key}"))
    }

    #[test]
    fn hours_become_bars_of_the_data_resolution() {
        assert_eq!(hours_to_bars(24, TimeResolution::Minute5), Some(288));
        assert_eq!(hours_to_bars(48, TimeResolution::Minute5), Some(576));
        assert_eq!(hours_to_bars(1, TimeResolution::Minute5), Some(12));
        assert_eq!(hours_to_bars(1, TimeResolution::Minute15), Some(4));
        assert_eq!(hours_to_bars(6, TimeResolution::Hour), Some(6));
        // Rounds up so the window covers at least the requested time.
        assert_eq!(hours_to_bars(1, TimeResolution::Day), Some(1));
        assert_eq!(hours_to_bars(25, TimeResolution::Day), Some(2));
        // Non-positive or overflowing input is rejected, never wrapped.
        assert_eq!(hours_to_bars(0, TimeResolution::Minute5), None);
        assert_eq!(hours_to_bars(-3, TimeResolution::Minute5), None);
        assert_eq!(hours_to_bars(i64::MAX, TimeResolution::Minute5), None);
    }

    #[test]
    fn momentum_lookback_hours_are_converted_to_five_minute_bars() {
        // The old code passed `lookback_hours as usize` (24) as a bar count,
        // i.e. a two-hour window.
        let config = StrategyConfig::Momentum {
            lookback_hours: 24,
            threshold: d("0.05"),
            position_size: d("0.1"),
        };
        let strategy = build_strategy(&config, d("10000"), d("0.02")).unwrap();
        assert_eq!(param(strategy.as_ref(), "lookback_periods"), "288");
        assert_eq!(strategy.required_history(), 288);
    }

    #[test]
    fn mean_reversion_window_hours_are_converted_to_bars() {
        let config = StrategyConfig::MeanReversion {
            window_hours: 48,
            std_threshold: d("2.5"),
            position_size: d("0.1"),
        };
        let strategy = build_strategy(&config, d("10000"), d("0.02")).unwrap();
        assert_eq!(param(strategy.as_ref(), "lookback_periods"), "576");
        assert_eq!(strategy.required_history(), 576);
    }

    #[test]
    fn arbitrage_max_position_dollars_become_a_portfolio_fraction() {
        let build = |max_position: &str, capital: &str| {
            let config = StrategyConfig::Arbitrage {
                min_spread: d("0.02"),
                max_position: d(max_position),
            };
            let strategy = build_strategy(&config, d(capital), d("0.02")).unwrap();
            d(&param(strategy.as_ref(), "position_size"))
        };
        // $1,000 of a $10,000 portfolio is 10%, not 1000x the portfolio.
        assert_eq!(build("1000", "10000"), d("0.1"));
        assert_eq!(build("2500", "50000"), d("0.05"));
        // At or above the capital: all of it (the simulator caps positions).
        assert_eq!(build("20000", "10000"), Decimal::ONE);
        assert_eq!(build("10000", "10000"), Decimal::ONE);
    }

    #[test]
    fn dollar_conversion_is_clamped_and_safe() {
        assert_eq!(
            dollars_to_portfolio_fraction(d("500"), d("2000")),
            d("0.25")
        );
        assert_eq!(
            dollars_to_portfolio_fraction(d("500"), Decimal::ZERO),
            Decimal::ZERO
        );
        assert_eq!(
            dollars_to_portfolio_fraction(d("-5"), d("2000")),
            Decimal::ZERO
        );
        assert_eq!(
            dollars_to_portfolio_fraction(d("5000"), d("2000")),
            Decimal::ONE
        );
    }

    #[test]
    fn fraction_parameters_are_passed_through_unchanged() {
        let config = StrategyConfig::Momentum {
            lookback_hours: 24,
            threshold: d("0.05"),
            position_size: d("0.1"),
        };
        let strategy = build_strategy(&config, d("10000"), d("0.02")).unwrap();
        assert_eq!(d(&param(strategy.as_ref(), "position_size")), d("0.1"));
    }

    #[test]
    fn invalid_lookbacks_and_positions_are_rejected() {
        let momentum = |hours: i64| StrategyConfig::Momentum {
            lookback_hours: hours,
            threshold: d("0.05"),
            position_size: d("0.1"),
        };
        for hours in [0, -1, MAX_LOOKBACK_HOURS + 1] {
            assert!(
                validate_strategy_config(&momentum(hours)).is_err(),
                "{hours}"
            );
            assert!(build_strategy(&momentum(hours), d("10000"), d("0.02")).is_err());
        }
        for hours in [1, 24, MAX_LOOKBACK_HOURS] {
            assert!(
                validate_strategy_config(&momentum(hours)).is_ok(),
                "{hours}"
            );
        }

        let mean_reversion = StrategyConfig::MeanReversion {
            window_hours: 0,
            std_threshold: d("2"),
            position_size: d("0.1"),
        };
        assert!(validate_strategy_config(&mean_reversion).is_err());

        let arbitrage = StrategyConfig::Arbitrage {
            min_spread: d("0.02"),
            max_position: Decimal::ZERO,
        };
        assert!(validate_strategy_config(&arbitrage).is_err());

        // The request-level validation reports it as a bad request.
        let request = RunBacktestRequest {
            strategy: momentum(0),
            start_date: Utc::now() - chrono::Duration::days(7),
            end_date: Utc::now(),
            initial_capital: d("10000"),
            markets: None,
            slippage_model: SlippageModel::None,
            fee_pct: d("0.02"),
        };
        assert!(matches!(
            validate_backtest_request(&request),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn the_longest_allowed_lookback_fits_the_simulator_history_cap() {
        let bars = hours_to_bars(MAX_LOOKBACK_HOURS, BACKTEST_RESOLUTION).unwrap();
        assert!(bars <= backtester::simulator::MAX_HISTORY_BARS);
    }
}

/// Resource limits, the shared run permits, blocking execution and workspace
/// access control of the backtest endpoints.
#[cfg(test)]
mod limits_tests {
    use super::persistence_tests::{apply_migration_085, create_pre_085_schema};
    use super::*;
    use crate::middleware;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn grid_request(start: DateTime<Utc>, end: DateTime<Utc>) -> RunBacktestRequest {
        RunBacktestRequest {
            strategy: StrategyConfig::Grid {
                grid_levels: 3,
                grid_spacing_pct: d("0.02"),
                order_size: d("0.05"),
            },
            start_date: start,
            end_date: end,
            initial_capital: d("10000"),
            markets: None,
            slippage_model: SlippageModel::None,
            fee_pct: d("0.02"),
        }
    }

    // -- configuration --------------------------------------------------------

    #[test]
    fn limits_default_to_conservative_values() {
        let limits = BacktestLimits::from_lookup(lookup(&[]));
        assert_eq!(limits.max_concurrent_runs, 1);
        assert_eq!(limits.max_range_days, 90);
        assert_eq!(limits.max_markets, 100);
        assert_eq!(limits.max_snapshots, 1_000_000);
    }

    #[test]
    fn limits_are_configurable_and_ignore_invalid_values() {
        let limits = BacktestLimits::from_lookup(lookup(&[
            ("BACKTEST_MAX_CONCURRENT_RUNS", "2"),
            ("BACKTEST_MAX_RANGE_DAYS", " 30 "),
            ("BACKTEST_MAX_MARKETS", "25"),
            ("BACKTEST_MAX_SNAPSHOTS", "250000"),
        ]));
        assert_eq!(limits.max_concurrent_runs, 2);
        assert_eq!(limits.max_range_days, 30);
        assert_eq!(limits.max_markets, 25);
        assert_eq!(limits.max_snapshots, 250_000);

        // Zero, negative and garbage values fall back to the defaults (a zero
        // permit count would deadlock every run).
        let limits = BacktestLimits::from_lookup(lookup(&[
            ("BACKTEST_MAX_CONCURRENT_RUNS", "0"),
            ("BACKTEST_MAX_RANGE_DAYS", "-5"),
            ("BACKTEST_MAX_MARKETS", "many"),
            ("BACKTEST_MAX_SNAPSHOTS", ""),
        ]));
        assert_eq!(limits, BacktestLimits::from_lookup(lookup(&[])));
    }

    // -- request validation ----------------------------------------------------

    #[test]
    fn date_range_is_limited() {
        let limits = BacktestLimits::from_lookup(lookup(&[]));
        let end = Utc::now();
        let ok = grid_request(end - chrono::Duration::days(90), end);
        assert!(validate_backtest_request_with(&ok, &limits).is_ok());

        let too_long = grid_request(end - chrono::Duration::days(91), end);
        match validate_backtest_request_with(&too_long, &limits) {
            Err(ApiError::BadRequest(message)) => assert!(message.contains("90 days"), "{message}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // A stricter configured limit applies.
        let strict = BacktestLimits::from_lookup(lookup(&[("BACKTEST_MAX_RANGE_DAYS", "7")]));
        assert!(validate_backtest_request_with(
            &grid_request(end - chrono::Duration::days(8), end),
            &strict
        )
        .is_err());
    }

    #[test]
    fn market_filter_is_limited() {
        let limits = BacktestLimits::from_lookup(lookup(&[("BACKTEST_MAX_MARKETS", "3")]));
        let end = Utc::now();
        let mut request = grid_request(end - chrono::Duration::days(7), end);

        request.markets = None; // "all markets" is bounded by the snapshot limit
        assert!(validate_backtest_request_with(&request, &limits).is_ok());
        request.markets = Some(vec!["a".into(), "b".into(), "c".into()]);
        assert!(validate_backtest_request_with(&request, &limits).is_ok());
        request.markets = Some(vec!["a".into(), "b".into(), "c".into(), "d".into()]);
        assert!(matches!(
            validate_backtest_request_with(&request, &limits),
            Err(ApiError::BadRequest(_))
        ));
        assert!(validate_markets_limit(Some(&["a".to_string()]), &limits).is_ok());
        assert!(validate_markets_limit(None, &limits).is_ok());
    }

    #[test]
    fn schedule_lookback_is_limited_by_the_same_range() {
        // Uses the process-wide limits (defaults unless overridden by env).
        let max = limits().max_range_days;
        assert!(validate_schedule_request(max as i32, d("1000"), 24).is_ok());
        assert!(matches!(
            validate_schedule_request(max as i32 + 1, d("1000"), 24),
            Err(ApiError::BadRequest(_))
        ));
        assert!(validate_schedule_request(0, d("1000"), 24).is_err());
    }

    // -- permits and blocking execution -----------------------------------------

    #[tokio::test]
    async fn run_permits_bound_concurrency() {
        for permits in [1usize, 2] {
            let semaphore = Arc::new(Semaphore::new(permits));
            let running = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));

            let mut tasks = Vec::new();
            for _ in 0..8 {
                let (semaphore, running, peak) = (semaphore.clone(), running.clone(), peak.clone());
                tasks.push(tokio::spawn(async move {
                    with_run_permit(&semaphore, async {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                        running.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
                }));
            }
            for task in tasks {
                task.await.unwrap();
            }

            assert_eq!(peak.load(Ordering::SeqCst), permits, "permits = {permits}");
            assert_eq!(running.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn a_failing_run_releases_its_permit() {
        let semaphore = Semaphore::new(1);
        let outcome: Result<(), &str> = with_run_permit(&semaphore, async { Err("boom") }).await;
        assert!(outcome.is_err());
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_does_not_starve_the_runtime() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = {
            let ticks = ticks.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    ticks.fetch_add(1, Ordering::SeqCst);
                }
            })
        };

        // 150ms of CPU-bound-style blocking work. Run inline on this
        // single-threaded runtime it would stop the ticker completely.
        let value = run_on_blocking_thread(|| {
            std::thread::sleep(std::time::Duration::from_millis(150));
            42
        })
        .await
        .unwrap();
        ticker.abort();

        assert_eq!(value, 42);
        assert!(
            ticks.load(Ordering::SeqCst) >= 5,
            "runtime made no progress while the simulation ran: {} ticks",
            ticks.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn a_panicking_simulation_becomes_an_error() {
        let result: anyhow::Result<()> =
            run_on_blocking_thread(|| panic!("strategy exploded")).await;
        let message = result.unwrap_err().to_string();
        assert!(message.contains("terminated unexpectedly"), "{message}");
    }

    // -- workspace access control ------------------------------------------------

    async fn auth_setup(pool: &PgPool) -> (Uuid, Uuid) {
        sqlx::raw_sql(
            "CREATE TABLE users(id UUID PRIMARY KEY, role SMALLINT);
             CREATE TABLE workspaces(id UUID PRIMARY KEY, created_at TIMESTAMPTZ DEFAULT NOW(),
                live_trading_enabled BOOLEAN DEFAULT false, exit_handler_enabled BOOLEAN DEFAULT false,
                arb_auto_execute BOOLEAN DEFAULT false, trading_wallet_address TEXT);
             CREATE TABLE workspace_members(workspace_id UUID, user_id UUID, role TEXT)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../../migrations/072_persistent_sessions.sql"
        ))
        .execute(pool)
        .await
        .unwrap();
        let workspace = Uuid::new_v4();
        sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
            .bind(workspace)
            .execute(pool)
            .await
            .unwrap();
        (workspace, Uuid::new_v4())
    }

    async fn add_user(pool: &PgPool, workspace: Option<(Uuid, &str)>) -> Uuid {
        let user = Uuid::new_v4();
        sqlx::query("INSERT INTO users VALUES($1, 1, 0)")
            .bind(user)
            .execute(pool)
            .await
            .unwrap();
        if let Some((workspace, role)) = workspace {
            sqlx::query("INSERT INTO workspace_members VALUES($1, $2, $3)")
                .bind(workspace)
                .bind(user)
                .bind(role)
                .execute(pool)
                .await
                .unwrap();
        }
        user
    }

    fn app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/backtest", post(run_backtest))
            .route("/results", get(list_backtest_results))
            .route("/results/:id", get(get_backtest_result))
            .route(
                "/schedules",
                get(list_backtest_schedules).post(create_backtest_schedule),
            )
            .route(
                "/schedules/:id",
                axum::routing::patch(update_backtest_schedule).delete(delete_backtest_schedule),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                middleware::require_auth,
            ))
            .with_state(state)
    }

    async fn call(state: &Arc<AppState>, token: &str, method: &str, path: &str) -> StatusCode {
        app(state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    fn endpoints(id: Uuid) -> Vec<(&'static str, String, bool)> {
        // (method, path, mutating)
        vec![
            ("GET", "/results".to_string(), false),
            ("GET", format!("/results/{id}"), false),
            ("GET", "/schedules".to_string(), false),
            ("POST", "/backtest".to_string(), true),
            ("POST", "/schedules".to_string(), true),
            ("PATCH", format!("/schedules/{id}"), true),
            ("DELETE", format!("/schedules/{id}"), true),
        ]
    }

    #[sqlx::test(migrations = false)]
    async fn backtest_endpoints_require_workspace_membership_and_trading_permission(pool: PgPool) {
        let (workspace, missing) = auth_setup(&pool).await;
        create_pre_085_schema(&pool).await;
        apply_migration_085(&pool).await;

        let outsider = add_user(&pool, None).await;
        let viewer = add_user(&pool, Some((workspace, "viewer"))).await;
        let member = add_user(&pool, Some((workspace, "member"))).await;

        let state = AppState::for_tests(pool.clone());
        let token = |user| {
            let state = state.clone();
            async move {
                middleware::issue_session_token(&state, user, ::auth::UserRole::Trader)
                    .await
                    .unwrap()
            }
        };

        // Not a member of the canonical workspace: nothing is reachable.
        let outsider_token = token(outsider).await;
        for (method, path, _) in endpoints(missing) {
            assert_eq!(
                call(&state, &outsider_token, method, &path).await,
                StatusCode::FORBIDDEN,
                "{method} {path}"
            );
        }

        // A viewer may read but cannot create runs or change schedules.
        let viewer_token = token(viewer).await;
        for (method, path, mutating) in endpoints(missing) {
            let status = call(&state, &viewer_token, method, &path).await;
            if mutating {
                assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
            } else {
                assert_ne!(status, StatusCode::FORBIDDEN, "{method} {path}");
                assert!(
                    status == StatusCode::OK || status == StatusCode::NOT_FOUND,
                    "{method} {path}: {status}"
                );
            }
        }

        // A trading member passes authorization on every endpoint (the empty
        // payloads are then rejected or not found by the handlers themselves).
        let member_token = token(member).await;
        for (method, path, _) in endpoints(missing) {
            let status = call(&state, &member_token, method, &path).await;
            assert_ne!(status, StatusCode::FORBIDDEN, "{method} {path}");
            assert_ne!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        }
        assert_eq!(
            call(
                &state,
                &member_token,
                "DELETE",
                &format!("/schedules/{missing}")
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }
}
