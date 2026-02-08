//! Backtesting operation handlers.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use tracing::{error, info};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use backtester::{
    ArbitrageStrategy, BacktestSimulator, DataQuery, HistoricalDataStore, MeanReversionStrategy,
    MomentumStrategy, SimulatorConfig, SlippageModel as BacktesterSlippageModel, Strategy,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Request to run a backtest.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
    Decimal::new(1, 3) // 0.1%
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
    /// Copy trading strategy.
    CopyTrading {
        /// Wallets to copy.
        wallets: Vec<String>,
        /// Allocation percentage per wallet.
        allocation_pct: Decimal,
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
    /// Annualized return.
    pub annualized_return: Decimal,
    /// Sharpe ratio.
    pub sharpe_ratio: Decimal,
    /// Sortino ratio.
    pub sortino_ratio: Decimal,
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
    /// Profit factor.
    pub profit_factor: Decimal,
    /// Total fees paid.
    pub total_fees: Decimal,
    /// Backtest run timestamp.
    pub created_at: DateTime<Utc>,
    /// Status (pending, running, completed, failed).
    pub status: String,
    /// Error message (if failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Equity curve (daily values).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equity_curve: Option<Vec<EquityPoint>>,
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

#[derive(Debug, FromRow)]
struct BacktestRow {
    id: Uuid,
    strategy: serde_json::Value,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    initial_capital: Decimal,
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
    status: String,
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
    status: String,
    error: Option<String>,
    equity_curve: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
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
    Json(request): Json<RunBacktestRequest>,
) -> ApiResult<Json<BacktestResultResponse>> {
    // Validate request
    if request.end_date <= request.start_date {
        return Err(ApiError::BadRequest(
            "End date must be after start date".to_string(),
        ));
    }

    if request.initial_capital <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Initial capital must be positive".to_string(),
        ));
    }

    let result_id = Uuid::new_v4();
    let now = Utc::now();

    // Serialize strategy config
    let strategy_json =
        serde_json::to_value(&request.strategy).map_err(|e| ApiError::Internal(e.to_string()))?;
    let slippage_json = serde_json::to_value(&request.slippage_model)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Insert backtest record with 'running' status
    sqlx::query(
        r#"
        INSERT INTO backtest_results
        (id, strategy, start_date, end_date, initial_capital, slippage_model,
         fee_pct, status, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'running', $8)
        "#,
    )
    .bind(result_id)
    .bind(&strategy_json)
    .bind(request.start_date)
    .bind(request.end_date)
    .bind(request.initial_capital)
    .bind(&slippage_json)
    .bind(request.fee_pct)
    .bind(now)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Spawn background task to run the backtest
    let pool = state.pool.clone();
    let strategy_config = request.strategy.clone();
    let start_date = request.start_date;
    let end_date = request.end_date;
    let initial_capital = request.initial_capital;
    let fee_pct = request.fee_pct;
    let slippage_model = request.slippage_model.clone();

    tokio::spawn(async move {
        run_backtest_task(
            pool,
            result_id,
            strategy_config,
            start_date,
            end_date,
            initial_capital,
            fee_pct,
            slippage_model,
        )
        .await;
    });

    info!(backtest_id = %result_id, "Backtest task spawned");

    Ok(Json(BacktestResultResponse {
        id: result_id,
        strategy: request.strategy,
        start_date: request.start_date,
        end_date: request.end_date,
        initial_capital: request.initial_capital,
        final_value: Decimal::ZERO,
        total_return: Decimal::ZERO,
        total_return_pct: Decimal::ZERO,
        annualized_return: Decimal::ZERO,
        sharpe_ratio: Decimal::ZERO,
        sortino_ratio: Decimal::ZERO,
        max_drawdown: Decimal::ZERO,
        max_drawdown_pct: Decimal::ZERO,
        total_trades: 0,
        winning_trades: 0,
        losing_trades: 0,
        win_rate: Decimal::ZERO,
        avg_win: Decimal::ZERO,
        avg_loss: Decimal::ZERO,
        profit_factor: Decimal::ZERO,
        total_fees: Decimal::ZERO,
        created_at: now,
        status: "running".to_string(),
        error: None,
        equity_curve: None,
    }))
}

/// Background task to run the backtest.
async fn run_backtest_task(
    pool: PgPool,
    result_id: Uuid,
    strategy_config: StrategyConfig,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    initial_capital: Decimal,
    fee_pct: Decimal,
    slippage_model: SlippageModel,
) {
    info!(backtest_id = %result_id, "Starting backtest execution");

    // Create data store
    let data_store = HistoricalDataStore::new(pool.clone());

    // Convert slippage model
    let backtester_slippage = match slippage_model {
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
        initial_capital,
        slippage_model: backtester_slippage,
        fee_model: backtester::simulator::FeeModel::Fixed(fee_pct),
        ..Default::default()
    };

    let simulator = BacktestSimulator::new(data_store, simulator_config);

    // Create strategy from config
    let result = match strategy_config {
        StrategyConfig::Arbitrage {
            min_spread,
            max_position,
        } => {
            // ArbitrageStrategy::new takes (min_spread, position_size, max_positions)
            let mut strategy = ArbitrageStrategy::new(min_spread, max_position, 10);
            run_strategy(&simulator, &mut strategy, start_date, end_date).await
        }
        StrategyConfig::Momentum {
            lookback_hours,
            threshold,
            position_size,
        } => {
            let mut strategy =
                MomentumStrategy::new(lookback_hours as usize, threshold, position_size);
            run_strategy(&simulator, &mut strategy, start_date, end_date).await
        }
        StrategyConfig::MeanReversion {
            window_hours,
            std_threshold,
            position_size,
        } => {
            let mut strategy = MeanReversionStrategy::new(
                window_hours as usize,
                std_threshold.to_string().parse().unwrap_or(2.0),
                position_size,
            );
            run_strategy(&simulator, &mut strategy, start_date, end_date).await
        }
        StrategyConfig::CopyTrading { .. } => {
            // Copy trading strategy requires wallet tracking - not supported in backtest yet
            Err(anyhow::anyhow!(
                "Copy trading strategy not supported in backtests"
            ))
        }
    };

    // Update database with results
    match result {
        Ok(backtest_result) => {
            // Serialize equity curve
            let equity_curve: Vec<EquityPoint> = backtest_result
                .equity_curve
                .iter()
                .map(|(timestamp, value)| EquityPoint {
                    timestamp: *timestamp,
                    value: *value,
                    drawdown_pct: Decimal::ZERO, // Simplified
                })
                .collect();
            let equity_json = serde_json::to_value(&equity_curve).ok();

            let update_result = sqlx::query(
                r#"
                UPDATE backtest_results SET
                    status = 'completed',
                    final_value = $2,
                    total_return = $3,
                    total_return_pct = $4,
                    annualized_return = $5,
                    sharpe_ratio = $6,
                    sortino_ratio = $7,
                    max_drawdown = $8,
                    max_drawdown_pct = $8,
                    total_trades = $9,
                    winning_trades = $10,
                    losing_trades = $11,
                    win_rate = $12,
                    profit_factor = $13,
                    total_fees = $14,
                    equity_curve = $15
                WHERE id = $1
                "#,
            )
            .bind(result_id)
            .bind(backtest_result.final_value)
            .bind(backtest_result.total_return)
            .bind(Decimal::try_from(backtest_result.return_pct).unwrap_or(Decimal::ZERO))
            .bind(Decimal::try_from(backtest_result.annualized_return).unwrap_or(Decimal::ZERO))
            .bind(Decimal::try_from(backtest_result.sharpe_ratio).unwrap_or(Decimal::ZERO))
            .bind(Decimal::try_from(backtest_result.sortino_ratio).unwrap_or(Decimal::ZERO))
            .bind(Decimal::try_from(backtest_result.max_drawdown).unwrap_or(Decimal::ZERO))
            .bind(backtest_result.total_trades as i64)
            .bind(backtest_result.winning_trades as i64)
            .bind(backtest_result.losing_trades as i64)
            .bind(Decimal::try_from(backtest_result.win_rate).unwrap_or(Decimal::ZERO))
            .bind(Decimal::try_from(backtest_result.profit_factor).unwrap_or(Decimal::ZERO))
            .bind(backtest_result.total_fees)
            .bind(equity_json)
            .execute(&pool)
            .await;

            match update_result {
                Ok(_) => {
                    info!(
                        backtest_id = %result_id,
                        final_value = %backtest_result.final_value,
                        return_pct = %backtest_result.return_pct,
                        "Backtest completed successfully"
                    );
                }
                Err(e) => {
                    error!(backtest_id = %result_id, error = %e, "Failed to update backtest results");
                }
            }
        }
        Err(e) => {
            let error_msg = e.to_string();
            let update_result = sqlx::query(
                "UPDATE backtest_results SET status = 'failed', error = $2 WHERE id = $1",
            )
            .bind(result_id)
            .bind(&error_msg)
            .execute(&pool)
            .await;

            if let Err(db_err) = update_result {
                error!(backtest_id = %result_id, error = %db_err, "Failed to update backtest error");
            }

            error!(backtest_id = %result_id, error = %error_msg, "Backtest failed");
        }
    }
}

/// Run a strategy through the simulator.
async fn run_strategy<S: Strategy>(
    simulator: &BacktestSimulator,
    strategy: &mut S,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
) -> anyhow::Result<backtester::BacktestResult> {
    let query = DataQuery::range(start_date, end_date);
    simulator.run(strategy, query).await
}

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
    Query(query): Query<ListBacktestQuery>,
) -> ApiResult<Json<Vec<BacktestResultResponse>>> {
    let rows: Vec<BacktestRow> = sqlx::query_as(
        r#"
        SELECT id, strategy, start_date, end_date, initial_capital,
               final_value, total_return, total_return_pct, annualized_return,
               sharpe_ratio, sortino_ratio, max_drawdown, max_drawdown_pct,
               total_trades, winning_trades, losing_trades, win_rate,
               avg_win, avg_loss, profit_factor, total_fees,
               status, error, created_at
        FROM backtest_results
        WHERE ($1::text IS NULL OR status = $1)
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&query.status)
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let results: Vec<BacktestResultResponse> = rows
        .into_iter()
        .map(|row| {
            let strategy: StrategyConfig =
                serde_json::from_value(row.strategy).unwrap_or(StrategyConfig::Arbitrage {
                    min_spread: Decimal::new(1, 2),
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
                annualized_return: row.annualized_return.unwrap_or(Decimal::ZERO),
                sharpe_ratio: row.sharpe_ratio.unwrap_or(Decimal::ZERO),
                sortino_ratio: row.sortino_ratio.unwrap_or(Decimal::ZERO),
                max_drawdown: row.max_drawdown.unwrap_or(Decimal::ZERO),
                max_drawdown_pct: row.max_drawdown_pct.unwrap_or(Decimal::ZERO),
                total_trades: row.total_trades.unwrap_or(0),
                winning_trades: row.winning_trades.unwrap_or(0),
                losing_trades: row.losing_trades.unwrap_or(0),
                win_rate: row.win_rate.unwrap_or(Decimal::ZERO),
                avg_win: row.avg_win.unwrap_or(Decimal::ZERO),
                avg_loss: row.avg_loss.unwrap_or(Decimal::ZERO),
                profit_factor: row.profit_factor.unwrap_or(Decimal::ZERO),
                total_fees: row.total_fees.unwrap_or(Decimal::ZERO),
                created_at: row.created_at,
                status: row.status,
                error: row.error,
                equity_curve: None, // Not included in list view
            }
        })
        .collect();

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
    Path(result_id): Path<Uuid>,
) -> ApiResult<Json<BacktestResultResponse>> {
    let row: Option<BacktestRowWithCurve> = sqlx::query_as(
        r#"
        SELECT id, strategy, start_date, end_date, initial_capital,
               final_value, total_return, total_return_pct, annualized_return,
               sharpe_ratio, sortino_ratio, max_drawdown, max_drawdown_pct,
               total_trades, winning_trades, losing_trades, win_rate,
               avg_win, avg_loss, profit_factor, total_fees,
               status, error, equity_curve, created_at
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
                    min_spread: Decimal::new(1, 2),
                    max_position: Decimal::new(1000, 0),
                });

            let equity_curve: Option<Vec<EquityPoint>> = row
                .equity_curve
                .and_then(|v| serde_json::from_value(v).ok());

            Ok(Json(BacktestResultResponse {
                id: row.id,
                strategy,
                start_date: row.start_date,
                end_date: row.end_date,
                initial_capital: row.initial_capital,
                final_value: row.final_value.unwrap_or(Decimal::ZERO),
                total_return: row.total_return.unwrap_or(Decimal::ZERO),
                total_return_pct: row.total_return_pct.unwrap_or(Decimal::ZERO),
                annualized_return: row.annualized_return.unwrap_or(Decimal::ZERO),
                sharpe_ratio: row.sharpe_ratio.unwrap_or(Decimal::ZERO),
                sortino_ratio: row.sortino_ratio.unwrap_or(Decimal::ZERO),
                max_drawdown: row.max_drawdown.unwrap_or(Decimal::ZERO),
                max_drawdown_pct: row.max_drawdown_pct.unwrap_or(Decimal::ZERO),
                total_trades: row.total_trades.unwrap_or(0),
                winning_trades: row.winning_trades.unwrap_or(0),
                losing_trades: row.losing_trades.unwrap_or(0),
                win_rate: row.win_rate.unwrap_or(Decimal::ZERO),
                avg_win: row.avg_win.unwrap_or(Decimal::ZERO),
                avg_loss: row.avg_loss.unwrap_or(Decimal::ZERO),
                profit_factor: row.profit_factor.unwrap_or(Decimal::ZERO),
                total_fees: row.total_fees.unwrap_or(Decimal::ZERO),
                created_at: row.created_at,
                status: row.status,
                error: row.error,
                equity_curve,
            }))
        }
        None => Err(ApiError::NotFound(format!(
            "Backtest result {} not found",
            result_id
        ))),
    }
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
            annualized_return: Decimal::new(300, 0),
            sharpe_ratio: Decimal::new(185, 2),
            sortino_ratio: Decimal::new(225, 2),
            max_drawdown: Decimal::new(500, 0),
            max_drawdown_pct: Decimal::new(5, 0),
            total_trades: 50,
            winning_trades: 35,
            losing_trades: 15,
            win_rate: Decimal::new(70, 0),
            avg_win: Decimal::new(100, 0),
            avg_loss: Decimal::new(50, 0),
            profit_factor: Decimal::new(467, 2),
            total_fees: Decimal::new(25, 0),
            created_at: Utc::now(),
            status: "completed".to_string(),
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
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("completed"));
        assert!(json.contains("sharpe_ratio"));
        assert!(json.contains("equity_curve"));
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
}
