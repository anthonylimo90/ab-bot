//! Backtest simulator with slippage and fee models.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::data_store::{DataQuery, HistoricalDataStore, MarketSnapshot};
use crate::strategy::{Position, Signal, SignalType, Strategy, StrategyContext};

/// Configuration for the backtest simulator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatorConfig {
    /// Initial portfolio value.
    pub initial_capital: Decimal,
    /// Slippage model to use.
    pub slippage_model: SlippageModel,
    /// Fee model to use for trading costs.
    pub fee_model: FeeModel,
    /// Partial fill model to use.
    pub partial_fill_model: PartialFillModel,
    /// Whether to allow margin/leverage.
    pub allow_margin: bool,
    /// Maximum leverage if margin is allowed.
    pub max_leverage: Decimal,
    /// Whether to reinvest profits.
    pub reinvest_profits: bool,
    /// Minimum position size.
    pub min_position_size: Decimal,
    /// Maximum single position size as fraction of portfolio.
    pub max_position_pct: Decimal,
    /// Maximum spread to allow fills (wider spreads may be rejected).
    pub max_spread_for_fill: Decimal,
    /// Refuse to run if the query yields more snapshots than this, instead of
    /// loading an unbounded data set into memory or silently truncating it.
    /// `None` = unlimited.
    #[serde(default)]
    pub max_snapshots: Option<usize>,
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            initial_capital: Decimal::new(10000, 0),
            slippage_model: SlippageModel::Fixed(Decimal::new(1, 3)), // 0.1%
            fee_model: FeeModel::Fixed(Decimal::new(2, 2)),           // 2%
            partial_fill_model: PartialFillModel::FullFill,
            allow_margin: false,
            max_leverage: Decimal::ONE,
            reinvest_profits: true,
            min_position_size: Decimal::new(10, 0),
            max_position_pct: Decimal::new(20, 2),    // 20%
            max_spread_for_fill: Decimal::new(10, 2), // 10% max spread
            max_snapshots: None,
        }
    }
}

/// Slippage model for simulating execution impact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlippageModel {
    /// No slippage.
    None,
    /// Fixed percentage slippage.
    Fixed(Decimal),
    /// Volume-dependent slippage.
    VolumeBased {
        /// Base slippage percentage.
        base_pct: Decimal,
        /// Slippage increase per unit of size.
        size_impact: Decimal,
    },
    /// Spread-based slippage (uses actual orderbook spread).
    SpreadBased {
        /// Multiplier for spread.
        spread_multiplier: Decimal,
    },
    /// Depth-based slippage (uses orderbook depth).
    DepthBased {
        /// Base slippage percentage.
        base_pct: Decimal,
        /// Multiplier for depth impact.
        depth_multiplier: Decimal,
    },
    /// Time-of-day adjusted slippage.
    TimeAdjusted {
        /// Base slippage percentage.
        base_pct: Decimal,
        /// Prime time multiplier (lower = less slippage during active hours).
        prime_time_multiplier: Decimal,
        /// Off-hours multiplier (higher = more slippage during quiet hours).
        off_hours_multiplier: Decimal,
    },
}

impl SlippageModel {
    /// Calculate slippage amount.
    pub fn calculate(&self, price: Decimal, quantity: Decimal, spread: Decimal) -> Decimal {
        self.calculate_with_depth(price, quantity, spread, Decimal::ZERO, 12)
    }

    /// Calculate slippage with depth information.
    pub fn calculate_with_depth(
        &self,
        price: Decimal,
        quantity: Decimal,
        spread: Decimal,
        depth: Decimal,
        hour: u32,
    ) -> Decimal {
        match self {
            SlippageModel::None => Decimal::ZERO,
            SlippageModel::Fixed(pct) => price * pct,
            SlippageModel::VolumeBased {
                base_pct,
                size_impact,
            } => {
                let volume_impact = quantity * size_impact;
                price * (*base_pct + volume_impact)
            }
            SlippageModel::SpreadBased { spread_multiplier } => {
                spread * spread_multiplier / Decimal::TWO
            }
            SlippageModel::DepthBased {
                base_pct,
                depth_multiplier,
            } => {
                // More slippage when depth is low relative to quantity
                let depth_ratio = if depth > Decimal::ZERO {
                    quantity / depth
                } else {
                    Decimal::ONE // Max slippage if no depth
                };
                let impact = depth_ratio * *depth_multiplier;
                price * (*base_pct + impact)
            }
            SlippageModel::TimeAdjusted {
                base_pct,
                prime_time_multiplier,
                off_hours_multiplier,
            } => {
                // Prime time: 9am-5pm (hours 9-17)
                let time_mult = if (9..=17).contains(&hour) {
                    *prime_time_multiplier
                } else {
                    *off_hours_multiplier
                };
                price * *base_pct * time_mult
            }
        }
    }
}

/// Fee model for simulating trading costs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeModel {
    /// Fixed percentage fee.
    Fixed(Decimal),
    /// Maker/taker fee structure.
    MakerTaker {
        /// Fee for maker orders (add liquidity).
        maker_fee: Decimal,
        /// Fee for taker orders (remove liquidity).
        taker_fee: Decimal,
    },
    /// Volume-tiered fees.
    VolumeTiered {
        /// Volume thresholds and corresponding fees.
        /// Each tuple is (volume_threshold, fee_pct).
        tiers: Vec<(Decimal, Decimal)>,
        /// Default fee if no tier matches.
        default_fee: Decimal,
    },
}

impl Default for FeeModel {
    fn default() -> Self {
        FeeModel::Fixed(Decimal::new(2, 2)) // 2%
    }
}

impl FeeModel {
    /// Calculate fee for a trade.
    pub fn calculate(&self, value: Decimal, is_maker: bool, cumulative_volume: Decimal) -> Decimal {
        match self {
            FeeModel::Fixed(pct) => value * pct,
            FeeModel::MakerTaker {
                maker_fee,
                taker_fee,
            } => {
                let fee_pct = if is_maker { *maker_fee } else { *taker_fee };
                value * fee_pct
            }
            FeeModel::VolumeTiered { tiers, default_fee } => {
                // Find applicable tier based on cumulative volume
                let fee_pct = tiers
                    .iter()
                    .filter(|(threshold, _)| cumulative_volume >= *threshold)
                    .map(|(_, fee)| *fee)
                    .next_back()
                    .unwrap_or(*default_fee);
                value * fee_pct
            }
        }
    }
}

/// Model for simulating partial fills based on market liquidity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum PartialFillModel {
    /// Always fill the entire order (instant fill assumption).
    #[default]
    FullFill,
    /// Fill based on orderbook depth.
    DepthBased {
        /// Maximum fill as percentage of available depth (e.g., 0.5 = 50% of depth).
        max_depth_pct: Decimal,
    },
    /// Probabilistic fill based on order size relative to typical volume.
    Probabilistic {
        /// Base fill probability for small orders.
        base_fill_pct: Decimal,
        /// Size threshold above which fill probability decreases.
        size_threshold: Decimal,
        /// Decay rate for fill probability as size increases.
        decay_rate: Decimal,
    },
    /// Fixed percentage fill (useful for testing).
    Fixed(Decimal),
}

impl PartialFillModel {
    /// Calculate fill quantity based on the model.
    /// Returns the quantity that will be filled.
    pub fn calculate_fill(
        &self,
        requested_quantity: Decimal,
        available_depth: Decimal,
        _spread: Decimal,
    ) -> Decimal {
        match self {
            PartialFillModel::FullFill => requested_quantity,
            PartialFillModel::DepthBased { max_depth_pct } => {
                // Fill is limited by available depth
                let max_fill = available_depth * *max_depth_pct;
                requested_quantity.min(max_fill)
            }
            PartialFillModel::Probabilistic {
                base_fill_pct,
                size_threshold,
                decay_rate,
            } => {
                if requested_quantity <= *size_threshold {
                    // Small orders get full fill
                    requested_quantity * *base_fill_pct
                } else {
                    // Larger orders have reduced fill probability
                    let size_ratio = requested_quantity / *size_threshold;
                    let fill_pct = *base_fill_pct / (Decimal::ONE + size_ratio * *decay_rate);
                    requested_quantity * fill_pct
                }
            }
            PartialFillModel::Fixed(pct) => requested_quantity * *pct,
        }
    }

    /// Check if the order can be filled at all given current market conditions.
    pub fn can_fill(&self, spread: Decimal, max_spread: Decimal) -> bool {
        // Wide spreads indicate low liquidity - may not be able to fill
        spread <= max_spread
    }
}

/// Result of a backtest run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    /// Strategy name.
    pub strategy_name: String,
    /// Strategy parameters.
    pub strategy_params: HashMap<String, String>,
    /// Backtest start time.
    pub start_time: DateTime<Utc>,
    /// Backtest end time.
    pub end_time: DateTime<Utc>,
    /// Number of data points processed.
    pub data_points: usize,
    /// Initial capital.
    pub initial_capital: Decimal,
    /// Final portfolio value.
    pub final_value: Decimal,
    /// Total return (absolute).
    pub total_return: Decimal,
    /// Total return percentage.
    pub return_pct: f64,
    /// Annualized return.
    pub annualized_return: f64,
    /// Maximum drawdown percentage.
    pub max_drawdown: f64,
    /// Sharpe ratio (annualized).
    pub sharpe_ratio: f64,
    /// Sortino ratio (annualized).
    pub sortino_ratio: f64,
    /// Win rate.
    pub win_rate: f64,
    /// Profit factor.
    pub profit_factor: f64,
    /// Total trades executed.
    pub total_trades: usize,
    /// Winning trades.
    pub winning_trades: usize,
    /// Losing trades.
    pub losing_trades: usize,
    /// Total fees paid.
    pub total_fees: Decimal,
    /// Total slippage cost.
    pub total_slippage: Decimal,
    /// Average trade duration in hours.
    pub avg_trade_duration_hours: f64,
    /// Equity curve (timestamp, value).
    pub equity_curve: Vec<(DateTime<Utc>, Decimal)>,
    /// Trade log.
    pub trades: Vec<TradeRecord>,
    /// Computed at timestamp.
    pub computed_at: DateTime<Utc>,

    // New metrics (Phase 3)
    /// Calmar ratio (annualized return / max drawdown).
    pub calmar_ratio: f64,
    /// Value at Risk at 95% confidence level.
    pub var_95: f64,
    /// Conditional VaR (Expected Shortfall) at 95%.
    pub cvar_95: f64,
    /// Recovery factor (net profit / max drawdown).
    pub recovery_factor: f64,
    /// Best single trade return.
    pub best_trade_return: f64,
    /// Worst single trade return.
    pub worst_trade_return: f64,
    /// Maximum consecutive wins.
    pub max_consecutive_wins: usize,
    /// Maximum consecutive losses.
    pub max_consecutive_losses: usize,
    /// Average winning trade return.
    pub avg_win: f64,
    /// Average losing trade return.
    pub avg_loss: f64,
    /// Expectancy (average profit per trade).
    pub expectancy: f64,
}

impl BacktestResult {
    /// Check if the backtest was profitable.
    pub fn is_profitable(&self) -> bool {
        self.final_value > self.initial_capital
    }

    /// Get risk-adjusted return (Sharpe > 1 is good).
    pub fn is_risk_adjusted_profitable(&self) -> bool {
        self.sharpe_ratio > 1.0 && self.return_pct > 0.0
    }
}

/// Record of a trade executed during backtest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    /// Trade ID.
    pub id: uuid::Uuid,
    /// Signal that triggered the trade.
    pub signal_id: uuid::Uuid,
    /// Market ID.
    pub market_id: String,
    /// Outcome ID.
    pub outcome_id: String,
    /// Trade type.
    pub trade_type: TradeType,
    /// Entry timestamp.
    pub entry_time: DateTime<Utc>,
    /// Exit timestamp (if closed).
    pub exit_time: Option<DateTime<Utc>>,
    /// Entry price.
    pub entry_price: Decimal,
    /// Exit price (if closed).
    pub exit_price: Option<Decimal>,
    /// Quantity.
    pub quantity: Decimal,
    /// Fees paid.
    pub fees: Decimal,
    /// Slippage cost.
    pub slippage: Decimal,
    /// Realized P&L (if closed).
    pub pnl: Option<Decimal>,
    /// Return percentage (if closed).
    pub return_pct: Option<f64>,
}

/// Type of trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeType {
    Buy,
    Sell,
    Close,
}

/// The backtest simulator engine.
pub struct BacktestSimulator {
    config: SimulatorConfig,
    data_store: HistoricalDataStore,
}

/// One owned dataset with lightweight timestamp ranges. No snapshot clones or
/// hash/sort pass are needed: the data store already returns chronological rows.
struct PreparedBacktest {
    snapshots: Vec<MarketSnapshot>,
    batches: Vec<std::ops::Range<usize>>,
}
impl PreparedBacktest {
    fn new(snapshots: Vec<MarketSnapshot>) -> Result<Self> {
        if snapshots.is_empty() {
            return Err(anyhow!("No data available for the specified query"));
        }
        let mut batches = Vec::new();
        let mut start = 0;
        for index in 1..snapshots.len() {
            if snapshots[index].timestamp < snapshots[index - 1].timestamp {
                return Err(anyhow!("Backtest snapshots must be chronological"));
            }
            if snapshots[index].timestamp != snapshots[start].timestamp {
                batches.push(start..index);
                start = index;
            }
        }
        batches.push(start..snapshots.len());
        Ok(Self { snapshots, batches })
    }
}

impl BacktestSimulator {
    /// Create a new backtest simulator.
    pub fn new(data_store: HistoricalDataStore, config: SimulatorConfig) -> Self {
        Self { config, data_store }
    }

    /// Run a backtest with the given strategy and data query.
    pub async fn run<S: Strategy + ?Sized>(
        &self,
        strategy: &mut S,
        query: DataQuery,
    ) -> Result<BacktestResult> {
        info!(
            strategy = strategy.name(),
            start = %query.start_time,
            end = %query.end_time,
            "Starting backtest"
        );

        let prepared = self.prepare(&query).await?;
        self.run_prepared(strategy, &query, &prepared).await
    }

    async fn prepare(&self, query: &DataQuery) -> Result<PreparedBacktest> {
        let mut query = query.clone();
        query.limit = effective_query_limit(query.limit, self.config.max_snapshots);
        let snapshots = self.data_store.query_snapshots(&query).await?;
        enforce_snapshot_limit(snapshots.len(), self.config.max_snapshots)?;
        PreparedBacktest::new(snapshots)
    }

    async fn run_prepared<S: Strategy + ?Sized>(
        &self,
        strategy: &mut S,
        query: &DataQuery,
        prepared: &PreparedBacktest,
    ) -> Result<BacktestResult> {
        // Initialize simulation state
        let mut state = SimulationState::new(self.config.initial_capital);
        state
            .equity_curve
            .push((query.start_time, self.config.initial_capital));
        let mut context = StrategyContext::new(self.config.initial_capital);

        // Initialize strategy
        strategy.initialize(&context).await?;

        // Retain as much history per market as the strategy's longest lookback
        // needs (bounded), so lookbacks beyond the old fixed 100 can trade.
        let history_limit = history_limit(strategy.required_history());

        // Process each time step
        for range in &prepared.batches {
            let market_snapshots = &prepared.snapshots[range.clone()];
            let timestamp = market_snapshots[0].timestamp;
            context.timestamp = timestamp;

            // Update market data in context
            for snapshot in market_snapshots {
                let data = context
                    .market_data
                    .entry(snapshot.market_id.clone())
                    .or_default();
                data.push(snapshot.clone());
                trim_history(data, history_limit);
            }

            // Update position prices
            self.update_positions(&mut context, &mut state, market_snapshots);

            // Get signals from strategy
            let signals = strategy.on_data(&context).await?;

            // Execute signals
            for signal in signals {
                if let Some(trade) = self
                    .execute_signal(&signal, &mut context, &mut state, market_snapshots)
                    .await?
                {
                    strategy
                        .on_fill(
                            &signal,
                            trade.exit_price.unwrap_or(trade.entry_price),
                            trade.quantity,
                        )
                        .await?;
                }
            }

            // Record equity curve
            let portfolio_value = state.portfolio_value();
            state.equity_curve.push((timestamp, portfolio_value));

            // Update context portfolio value
            context.portfolio_value = portfolio_value;
            context.available_cash = state.cash;
        }

        // Liquidation costs belong to the terminal observation and risk metrics.
        self.close_all_positions(&mut state, &context);
        anyhow::ensure!(
            state.positions.is_empty(),
            "Unable to liquidate all positions at the end of the backtest"
        );
        if let Some((_, equity)) = state.equity_curve.last_mut() {
            *equity = state.cash;
        }
        context.portfolio_value = state.cash;
        context.available_cash = state.cash;
        context.positions.clear();
        strategy.finalize(&context).await?;

        // Calculate final metrics
        let result = self.calculate_results(strategy, &state, query, prepared.snapshots.len());

        info!(
            strategy = strategy.name(),
            return_pct = result.return_pct,
            sharpe = result.sharpe_ratio,
            trades = result.total_trades,
            "Backtest completed"
        );

        Ok(result)
    }

    /// Run multiple strategies and compare results.
    pub async fn compare_strategies(
        &self,
        strategies: &mut [Box<dyn Strategy>],
        query: DataQuery,
    ) -> Result<Vec<BacktestResult>> {
        let mut results = Vec::new();
        if strategies.is_empty() {
            return Ok(results);
        }
        let prepared = self.prepare(&query).await?;
        for strategy in strategies.iter_mut() {
            match self
                .run_prepared(strategy.as_mut(), &query, &prepared)
                .await
            {
                Ok(result) => results.push(result),
                Err(e) => {
                    warn!(strategy = strategy.name(), error = %e, "Strategy backtest failed");
                }
            }
        }

        // Sort by Sharpe ratio descending
        results.sort_by(|a, b| {
            b.sharpe_ratio
                .partial_cmp(&a.sharpe_ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    // Private methods

    fn update_positions(
        &self,
        context: &mut StrategyContext,
        state: &mut SimulationState,
        snapshots: &[MarketSnapshot],
    ) {
        let prices: HashMap<&str, &MarketSnapshot> = snapshots
            .iter()
            .map(|snapshot| (snapshot.market_id.as_str(), snapshot))
            .collect();
        for position in state.positions.values_mut() {
            if let Some(snapshot) = prices.get(position.market_id.as_str()) {
                let price = if position.outcome_id == "yes" {
                    snapshot.yes_bid
                } else {
                    snapshot.no_bid
                };
                position.current_price = price;
                position.unrealized_pnl = (price - position.entry_price) * position.quantity;
            }
        }
        context.positions = state.positions.clone();
    }

    async fn execute_signal(
        &self,
        signal: &Signal,
        context: &mut StrategyContext,
        state: &mut SimulationState,
        snapshots: &[MarketSnapshot],
    ) -> Result<Option<TradeRecord>> {
        let snapshot = snapshots.iter().find(|s| s.market_id == signal.market_id);

        let snapshot = match snapshot {
            Some(s) => s,
            None => return Ok(None),
        };

        match signal.signal_type {
            SignalType::Buy => self.execute_buy(signal, context, state, snapshot),
            SignalType::Sell => self.execute_sell(signal, state, snapshot),
            SignalType::Close => self.execute_close(signal, state, snapshot),
            SignalType::Hold => Ok(None),
        }
    }

    fn execute_buy(
        &self,
        signal: &Signal,
        context: &StrategyContext,
        state: &mut SimulationState,
        snapshot: &MarketSnapshot,
    ) -> Result<Option<TradeRecord>> {
        // Calculate position size
        let existing_value = state
            .positions
            .get(&position_key(&signal.market_id, &signal.outcome_id))
            .map(|p| p.quantity * p.current_price)
            .unwrap_or(Decimal::ZERO);
        let max_position_value = (context.portfolio_value * self.config.max_position_pct
            - existing_value)
            .max(Decimal::ZERO);
        let target_value = context.portfolio_value * signal.position_size;
        let position_value = target_value.min(max_position_value).min(state.cash);

        if position_value < self.config.min_position_size {
            debug!(
                market = %signal.market_id,
                "Position size too small, skipping"
            );
            return Ok(None);
        }

        // Get execution price with slippage
        let base_price = if signal.outcome_id == "yes" {
            snapshot.yes_ask
        } else {
            snapshot.no_ask
        };

        let spread = if signal.outcome_id == "yes" {
            snapshot.yes_spread
        } else {
            snapshot.no_spread
        };

        // For buying, we use ask depth (liquidity available to buy)
        let depth = if signal.outcome_id == "yes" {
            snapshot.yes_ask_depth
        } else {
            snapshot.no_ask_depth
        };

        // Check if spread is acceptable for filling
        if !self
            .config
            .partial_fill_model
            .can_fill(spread, self.config.max_spread_for_fill)
        {
            debug!(
                market = %signal.market_id,
                spread = %spread,
                max_spread = %self.config.max_spread_for_fill,
                "Spread too wide, skipping trade"
            );
            return Ok(None);
        }

        // Calculate requested quantity
        let requested_quantity = position_value / base_price;

        // Apply partial fill model
        let fill_quantity =
            self.config
                .partial_fill_model
                .calculate_fill(requested_quantity, depth, spread);

        // Skip if fill is too small
        if fill_quantity < self.config.min_position_size / base_price {
            debug!(
                market = %signal.market_id,
                requested = %requested_quantity,
                fill = %fill_quantity,
                "Partial fill too small, skipping"
            );
            return Ok(None);
        }

        // Calculate slippage with depth and time information
        let hour = snapshot.timestamp.hour();
        let slippage = self.config.slippage_model.calculate_with_depth(
            base_price,
            fill_quantity,
            spread,
            depth,
            hour,
        );
        let execution_price = base_price + slippage;

        // Calculate fees using the new fee model
        let trade_value = fill_quantity * execution_price;
        let fees = self.config.fee_model.calculate(
            trade_value,
            false,               // taker order
            state.traded_volume, // cumulative volume for tiered fees
        );

        // Check if we have enough cash
        let total_cost = trade_value + fees;
        if total_cost > state.cash {
            debug!(
                market = %signal.market_id,
                cost = %total_cost,
                cash = %state.cash,
                "Insufficient cash for trade"
            );
            return Ok(None);
        }

        // Execute trade
        state.cash -= total_cost;
        state.total_fees += fees;
        state.traded_volume += trade_value;
        state.total_slippage += slippage * fill_quantity;

        let key = position_key(&signal.market_id, &signal.outcome_id);
        state
            .positions
            .entry(key.clone())
            .and_modify(|position| {
                let total_quantity = position.quantity + fill_quantity;
                position.entry_price = (position.entry_price * position.quantity
                    + execution_price * fill_quantity)
                    / total_quantity;
                position.quantity = total_quantity;
                position.current_price = execution_price;
                position.unrealized_pnl = (execution_price - position.entry_price) * total_quantity;
            })
            .or_insert_with(|| Position {
                market_id: signal.market_id.clone(),
                outcome_id: signal.outcome_id.clone(),
                quantity: fill_quantity,
                entry_price: execution_price,
                opened_at: context.timestamp,
                unrealized_pnl: Decimal::ZERO,
                current_price: execution_price,
            });

        // Record trade
        let trade = TradeRecord {
            id: uuid::Uuid::new_v4(),
            signal_id: signal.id,
            market_id: signal.market_id.clone(),
            outcome_id: signal.outcome_id.clone(),
            trade_type: TradeType::Buy,
            entry_time: context.timestamp,
            exit_time: None,
            entry_price: execution_price,
            exit_price: None,
            quantity: fill_quantity,
            fees,
            slippage: slippage * fill_quantity,
            pnl: None,
            return_pct: None,
        };

        state.trades.push(trade.clone());
        state
            .open_lots
            .entry(key)
            .or_default()
            .push(state.trades.len() - 1);
        Ok(Some(trade))
    }

    fn execute_sell(
        &self,
        signal: &Signal,
        state: &mut SimulationState,
        snapshot: &MarketSnapshot,
    ) -> Result<Option<TradeRecord>> {
        // Partial close not implemented - use Close instead
        self.execute_close(signal, state, snapshot)
    }

    fn execute_close(
        &self,
        signal: &Signal,
        state: &mut SimulationState,
        snapshot: &MarketSnapshot,
    ) -> Result<Option<TradeRecord>> {
        let key = position_key(&signal.market_id, &signal.outcome_id);
        let position = match state.positions.remove(&key) {
            Some(p) => p,
            None => return Ok(None),
        };
        // Entry lots of this position that this close will settle.
        let lot_indices = state.open_lots.remove(&key).unwrap_or_default();

        // Get execution price with slippage
        let base_price = if position.outcome_id == "yes" {
            snapshot.yes_bid
        } else {
            snapshot.no_bid
        };

        let spread = if position.outcome_id == "yes" {
            snapshot.yes_spread
        } else {
            snapshot.no_spread
        };

        // For selling/closing, we use bid depth (liquidity available to sell into)
        let depth = if position.outcome_id == "yes" {
            snapshot.yes_bid_depth
        } else {
            snapshot.no_bid_depth
        };

        // Calculate slippage with depth and time information
        let hour = snapshot.timestamp.hour();
        let slippage = self.config.slippage_model.calculate_with_depth(
            base_price,
            position.quantity,
            spread,
            depth,
            hour,
        );
        let execution_price = base_price - slippage;

        // Calculate proceeds and fees using the new fee model
        let trade_value = position.quantity * execution_price;
        let fees = self.config.fee_model.calculate(
            trade_value,
            false,               // taker order
            state.traded_volume, // cumulative volume for tiered fees
        );
        let proceeds = trade_value - fees;

        // Calculate P&L
        let entry_fees: Decimal = lot_indices.iter().map(|&i| state.trades[i].fees).sum();
        let cost_basis = position.quantity * position.entry_price + entry_fees;
        let pnl = proceeds - cost_basis;
        let return_pct = if cost_basis > Decimal::ZERO {
            ((proceeds / cost_basis) - Decimal::ONE)
                .to_f64()
                .unwrap_or(0.0)
        } else {
            0.0
        };

        // Update state
        state.cash += proceeds;
        state.total_fees += fees;
        state.traded_volume += trade_value;
        state.total_slippage += slippage * position.quantity;
        state.realized_pnl += pnl;

        if pnl > Decimal::ZERO {
            state.winning_trades += 1;
            state.total_wins += pnl;
        } else {
            state.losing_trades += 1;
            state.total_losses += pnl.abs();
        }

        // Close every entry lot, allocating exit fees in proportion to quantity.
        for &index in &lot_indices {
            let entry = &mut state.trades[index];
            let allocated_exit_fee = fees * entry.quantity / position.quantity;
            let entry_cost = entry.quantity * entry.entry_price + entry.fees;
            let lot_proceeds = entry.quantity * execution_price - allocated_exit_fee;
            entry.exit_time = Some(snapshot.timestamp);
            entry.exit_price = Some(execution_price);
            entry.pnl = Some(lot_proceeds - entry_cost);
            entry.return_pct = Some(
                ((lot_proceeds / entry_cost) - Decimal::ONE)
                    .to_f64()
                    .unwrap_or(0.0),
            );
        }

        let trade = TradeRecord {
            id: uuid::Uuid::new_v4(),
            signal_id: signal.id,
            market_id: signal.market_id.clone(),
            outcome_id: position.outcome_id,
            trade_type: TradeType::Close,
            entry_time: position.opened_at,
            exit_time: Some(snapshot.timestamp),
            entry_price: position.entry_price,
            exit_price: Some(execution_price),
            quantity: position.quantity,
            fees,
            slippage: slippage * position.quantity,
            pnl: Some(pnl),
            return_pct: Some(return_pct),
        };

        state.trades.push(trade.clone());
        Ok(Some(trade))
    }

    fn close_all_positions(&self, state: &mut SimulationState, context: &StrategyContext) {
        let positions: Vec<_> = state.positions.values().cloned().collect();

        for position in positions {
            if let Some(snapshots) = context.market_data.get(&position.market_id) {
                if let Some(snapshot) = snapshots.last() {
                    let signal = Signal::close(&position.market_id, &position.outcome_id);
                    let _ = self.execute_close(&signal, state, snapshot);
                }
            }
        }
    }

    fn calculate_results<S: Strategy + ?Sized>(
        &self,
        strategy: &S,
        state: &SimulationState,
        query: &DataQuery,
        data_points: usize,
    ) -> BacktestResult {
        let final_value = state.portfolio_value();
        let total_return = final_value - self.config.initial_capital;
        let return_pct = ((final_value / self.config.initial_capital) - Decimal::ONE)
            .to_f64()
            .unwrap_or(0.0);

        // Calculate trading days for annualization
        let end_time = state
            .equity_curve
            .last()
            .map(|(time, _)| *time)
            .unwrap_or(query.start_time);
        let days = (end_time - query.start_time).num_seconds() as f64 / 86_400.0;
        let years = days / 365.0;
        let annualized_return = if years > 0.0 {
            (1.0 + return_pct).powf(1.0 / years) - 1.0
        } else {
            0.0
        };

        // Calculate drawdown
        let max_drawdown = self.calculate_max_drawdown(&state.equity_curve);

        // Calculate Sharpe and Sortino ratios
        let (sharpe, sortino) = self.calculate_risk_metrics(&state.equity_curve);

        // Calculate win rate and profit factor
        let total_trades = state.winning_trades + state.losing_trades;
        let win_rate = if total_trades > 0 {
            state.winning_trades as f64 / total_trades as f64
        } else {
            0.0
        };

        let profit_factor = if state.total_losses > Decimal::ZERO {
            (state.total_wins / state.total_losses)
                .to_f64()
                .unwrap_or(0.0)
        } else if state.total_wins > Decimal::ZERO {
            f64::INFINITY
        } else {
            0.0
        };

        // Calculate average trade duration
        let avg_duration = self.calculate_avg_trade_duration(&state.trades);

        // Calculate new metrics (Phase 3)

        // Calmar ratio
        let calmar_ratio = if max_drawdown > 0.0 {
            annualized_return / max_drawdown
        } else if annualized_return > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };

        // Recovery factor
        let recovery_factor = if max_drawdown > 0.0 {
            return_pct / max_drawdown
        } else if return_pct > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };

        // VaR and CVaR
        let (var_95, cvar_95) = self.calculate_var_cvar(&state.equity_curve);

        // Trade-level metrics
        let trade_returns: Vec<f64> = state
            .trades
            .iter()
            .filter(|t| t.trade_type == TradeType::Close)
            .filter_map(|t| t.return_pct)
            .collect();

        let best_trade_return = trade_returns
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        let worst_trade_return = trade_returns.iter().cloned().fold(f64::INFINITY, f64::min);

        let best_trade_return = if best_trade_return == f64::NEG_INFINITY {
            0.0
        } else {
            best_trade_return
        };
        let worst_trade_return = if worst_trade_return == f64::INFINITY {
            0.0
        } else {
            worst_trade_return
        };

        // Consecutive wins/losses
        let (max_consecutive_wins, max_consecutive_losses) =
            self.calculate_consecutive_streaks(&state.trades);

        // Average win/loss
        let winning_returns: Vec<f64> = trade_returns
            .iter()
            .filter(|&&r| r > 0.0)
            .copied()
            .collect();
        let losing_returns: Vec<f64> = trade_returns
            .iter()
            .filter(|&&r| r < 0.0)
            .copied()
            .collect();

        let avg_win = if !winning_returns.is_empty() {
            winning_returns.iter().sum::<f64>() / winning_returns.len() as f64
        } else {
            0.0
        };

        let avg_loss = if !losing_returns.is_empty() {
            losing_returns.iter().sum::<f64>() / losing_returns.len() as f64
        } else {
            0.0
        };

        // Expectancy
        let expectancy = (win_rate * avg_win) + ((1.0 - win_rate) * avg_loss);

        BacktestResult {
            strategy_name: strategy.name().to_string(),
            strategy_params: strategy.parameters(),
            start_time: query.start_time,
            end_time,
            data_points,
            initial_capital: self.config.initial_capital,
            final_value,
            total_return,
            return_pct,
            annualized_return,
            max_drawdown,
            sharpe_ratio: sharpe,
            sortino_ratio: sortino,
            win_rate,
            profit_factor,
            total_trades,
            winning_trades: state.winning_trades,
            losing_trades: state.losing_trades,
            total_fees: state.total_fees,
            total_slippage: state.total_slippage,
            avg_trade_duration_hours: avg_duration,
            equity_curve: state.equity_curve.clone(),
            trades: state.trades.clone(),
            computed_at: Utc::now(),
            // New metrics
            calmar_ratio,
            var_95,
            cvar_95,
            recovery_factor,
            best_trade_return,
            worst_trade_return,
            max_consecutive_wins,
            max_consecutive_losses,
            avg_win,
            avg_loss,
            expectancy,
        }
    }

    /// Calculate VaR and CVaR at 95% confidence from daily returns.
    fn calculate_var_cvar(&self, equity_curve: &[(DateTime<Utc>, Decimal)]) -> (f64, f64) {
        var_cvar_95(&daily_returns(equity_curve))
    }

    /// Calculate maximum consecutive wins and losses.
    fn calculate_consecutive_streaks(&self, trades: &[TradeRecord]) -> (usize, usize) {
        let mut max_wins = 0;
        let mut max_losses = 0;
        let mut current_wins = 0;
        let mut current_losses = 0;

        for trade in trades.iter().filter(|t| t.trade_type == TradeType::Close) {
            if let Some(pnl) = trade.pnl {
                if pnl > Decimal::ZERO {
                    current_wins += 1;
                    current_losses = 0;
                    max_wins = max_wins.max(current_wins);
                } else if pnl < Decimal::ZERO {
                    current_losses += 1;
                    current_wins = 0;
                    max_losses = max_losses.max(current_losses);
                }
            }
        }

        (max_wins, max_losses)
    }

    fn calculate_max_drawdown(&self, equity_curve: &[(DateTime<Utc>, Decimal)]) -> f64 {
        if equity_curve.is_empty() {
            return 0.0;
        }

        let mut peak = equity_curve[0].1;
        let mut max_drawdown: f64 = 0.0;

        for (_, value) in equity_curve {
            if *value > peak {
                peak = *value;
            }
            let drawdown = ((peak - *value) / peak).to_f64().unwrap_or(0.0);
            max_drawdown = max_drawdown.max(drawdown);
        }

        max_drawdown
    }

    /// Sharpe and Sortino ratios from UTC-day-close returns, annualized with
    /// [`ANNUALIZATION_DAYS`].
    fn calculate_risk_metrics(&self, equity_curve: &[(DateTime<Utc>, Decimal)]) -> (f64, f64) {
        sharpe_sortino(&daily_returns(equity_curve))
    }

    fn calculate_avg_trade_duration(&self, trades: &[TradeRecord]) -> f64 {
        let closed_trades: Vec<_> = trades
            .iter()
            .filter(|t| t.trade_type == TradeType::Close && t.exit_time.is_some())
            .collect();

        if closed_trades.is_empty() {
            return 0.0;
        }

        let total_hours: i64 = closed_trades
            .iter()
            .map(|t| {
                t.exit_time
                    .unwrap()
                    .signed_duration_since(t.entry_time)
                    .num_hours()
            })
            .sum();

        total_hours as f64 / closed_trades.len() as f64
    }
}

/// Row limit to send with the query so an oversized data set is detected.
///
/// One row more than `max` is requested: if it comes back, the data set is too
/// large and the run is refused (see [`enforce_snapshot_limit`]) rather than
/// being silently cut off at `max` rows (the query is ordered by time, so a
/// truncated run would just miss its most recent period).
fn effective_query_limit(
    query_limit: Option<usize>,
    max_snapshots: Option<usize>,
) -> Option<usize> {
    match max_snapshots {
        Some(max) => {
            let cap = max.saturating_add(1);
            Some(query_limit.map_or(cap, |limit| limit.min(cap)))
        }
        None => query_limit,
    }
}

/// Fail if more than `max_snapshots` rows were loaded.
fn enforce_snapshot_limit(loaded: usize, max_snapshots: Option<usize>) -> Result<()> {
    if let Some(max) = max_snapshots {
        anyhow::ensure!(
            loaded <= max,
            "Backtest data set exceeds {max} snapshots; narrow the date range or select fewer markets"
        );
    }
    Ok(())
}

/// Snapshots per market always retained for strategies (the previous fixed
/// window), whatever lookback they declare.
pub const MIN_HISTORY_BARS: usize = crate::strategy::DEFAULT_REQUIRED_HISTORY;

/// Upper bound on snapshots retained per market, to bound memory when a
/// strategy declares a very long lookback.
pub const MAX_HISTORY_BARS: usize = 5_000;

/// Snapshots per market to retain for a strategy that reads `required` of them.
fn history_limit(required: usize) -> usize {
    required.clamp(MIN_HISTORY_BARS, MAX_HISTORY_BARS)
}

/// Drop the oldest snapshots once the history exceeds `keep` by a slack margin,
/// leaving exactly `keep` (so it always holds at least `keep`, and trimming is
/// amortized over `slack` pushes rather than done every step).
fn trim_history(data: &mut Vec<MarketSnapshot>, keep: usize) {
    let slack = (keep / 4).max(50);
    if data.len() > keep + slack {
        let excess = data.len() - keep;
        data.drain(0..excess);
    }
}

/// Observations per year used to annualize *daily* returns.
///
/// Polymarket trades around the clock, so every calendar day is an observation
/// (365, not the 252 trading days of equities).
const ANNUALIZATION_DAYS: f64 = 365.0;

/// Resample an equity curve to UTC-day closes and return the simple daily
/// returns.
///
/// The curve gets one point per market-data step (minutes apart), so annualizing
/// step-to-step returns with a daily factor overstates risk by roughly
/// `sqrt(steps per day)`. Here the first point is the opening equity and every
/// later UTC day contributes its last observation (the day close):
/// `r_1 = close_1 / opening - 1` and `r_k = close_k / close_(k-1) - 1`. A day
/// without observations contributes nothing (the next return then spans the
/// gap); a zero previous equity yields a `0.0` return.
fn daily_returns(equity_curve: &[(DateTime<Utc>, Decimal)]) -> Vec<f64> {
    let Some(((_, opening), rest)) = equity_curve.split_first() else {
        return Vec::new();
    };

    let mut closes: Vec<(chrono::NaiveDate, Decimal)> = Vec::new();
    for (time, value) in rest {
        let day = time.date_naive();
        match closes.last_mut() {
            Some((last_day, last_value)) if *last_day == day => *last_value = *value,
            _ => closes.push((day, *value)),
        }
    }

    let mut previous = *opening;
    closes
        .into_iter()
        .map(|(_, close)| {
            let ret = if previous.is_zero() {
                0.0
            } else {
                ((close - previous) / previous).to_f64().unwrap_or(0.0)
            };
            previous = close;
            ret
        })
        .collect()
}

/// Annualized Sharpe (0% risk-free rate) and Sortino ratios of daily returns.
///
/// Fewer than two daily observations cannot describe dispersion, so both ratios
/// are reported as `0.0` instead of an artefact of a single day.
fn sharpe_sortino(returns: &[f64]) -> (f64, f64) {
    if returns.len() < 2 {
        return (0.0, 0.0);
    }

    let annualization = ANNUALIZATION_DAYS.sqrt();
    let mean_return: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance: f64 = returns
        .iter()
        .map(|r| (r - mean_return).powi(2))
        .sum::<f64>()
        / returns.len() as f64;
    let std_dev = variance.sqrt();

    // Sharpe ratio (assuming 0% risk-free rate for simplicity)
    let sharpe = if std_dev > 0.0 {
        (mean_return / std_dev) * annualization
    } else {
        0.0
    };

    // Sortino ratio (only downside deviation)
    let downside_returns: Vec<f64> = returns
        .iter()
        .filter(|&&r| r < 0.0)
        .map(|&r| r * r)
        .collect();
    // Over every day, not only the losing ones.
    let downside_dev = if !downside_returns.is_empty() {
        (downside_returns.iter().sum::<f64>() / returns.len() as f64).sqrt()
    } else {
        0.0
    };

    let sortino = if downside_dev > 0.0 {
        (mean_return / downside_dev) * annualization
    } else if mean_return > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };

    (sharpe, sortino)
}

/// Historical 95% VaR and CVaR (expected shortfall) of daily returns, as
/// positive loss figures.
///
/// Both use the same tail: the worst `ceil(5% * n)` returns (at least one).
/// VaR is the loss at the boundary of that tail (its least severe member) and
/// CVaR is the average loss over the whole tail, so `CVaR >= VaR` always holds.
fn var_cvar_95(returns: &[f64]) -> (f64, f64) {
    if returns.is_empty() {
        return (0.0, 0.0);
    }

    let mut sorted = returns.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let tail_len = ((sorted.len() as f64 * 0.05).ceil() as usize).clamp(1, sorted.len());
    let tail = &sorted[..tail_len];

    let var_95 = -tail[tail_len - 1];
    let cvar_95 = -(tail.iter().sum::<f64>() / tail_len as f64);

    (var_95, cvar_95)
}

/// Internal simulation state.
struct SimulationState {
    cash: Decimal,
    positions: HashMap<String, Position>,
    trades: Vec<TradeRecord>,
    /// Indices into `trades` of the still-open entry lots of each position key,
    /// so closing a position touches only its own lots instead of scanning every
    /// trade ever made.
    open_lots: HashMap<String, Vec<usize>>,
    equity_curve: Vec<(DateTime<Utc>, Decimal)>,
    total_fees: Decimal,
    /// Value traded so far, buys and sells: the volume fee tiers are set by.
    traded_volume: Decimal,
    total_slippage: Decimal,
    realized_pnl: Decimal,
    winning_trades: usize,
    losing_trades: usize,
    total_wins: Decimal,
    total_losses: Decimal,
}

fn position_key(market_id: &str, outcome_id: &str) -> String {
    format!("{market_id}:{outcome_id}")
}

impl SimulationState {
    fn new(initial_capital: Decimal) -> Self {
        Self {
            cash: initial_capital,
            positions: HashMap::new(),
            trades: Vec::new(),
            open_lots: HashMap::new(),
            equity_curve: Vec::new(),
            total_fees: Decimal::ZERO,
            traded_volume: Decimal::ZERO,
            total_slippage: Decimal::ZERO,
            realized_pnl: Decimal::ZERO,
            winning_trades: 0,
            losing_trades: 0,
            total_wins: Decimal::ZERO,
            total_losses: Decimal::ZERO,
        }
    }

    fn portfolio_value(&self) -> Decimal {
        let position_value: Decimal = self
            .positions
            .values()
            .map(|p| p.quantity * p.current_price)
            .sum();

        self.cash + position_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FixtureStrategy {
        observations: Vec<(DateTime<Utc>, usize, usize)>,
        fills: Vec<(Decimal, Decimal)>,
    }
    #[async_trait::async_trait]
    impl Strategy for FixtureStrategy {
        fn name(&self) -> &str {
            "fixed-input-fixture"
        }
        async fn on_data(&mut self, context: &StrategyContext) -> Result<Vec<Signal>> {
            let a = &context.market_data["a"];
            let b = &context.market_data["b"];
            assert_eq!(a.last().unwrap().timestamp, context.timestamp);
            assert_eq!(b.last().unwrap().timestamp, context.timestamp);
            self.observations
                .push((context.timestamp, a.len(), b.len()));
            Ok(if self.observations.len() == 1 {
                vec![
                    Signal::buy("a", "yes", Decimal::new(4, 2)),
                    Signal::buy("b", "no", Decimal::new(4, 2)),
                ]
            } else {
                vec![Signal::close("a", "yes"), Signal::close("b", "no")]
            })
        }
        async fn on_fill(&mut self, _: &Signal, price: Decimal, quantity: Decimal) -> Result<()> {
            self.fills.push((price, quantity));
            Ok(())
        }
    }

    fn legacy_timeline(snapshots: &[MarketSnapshot]) -> Vec<(DateTime<Utc>, Vec<MarketSnapshot>)> {
        let mut grouped = HashMap::<DateTime<Utc>, Vec<MarketSnapshot>>::new();
        for snapshot in snapshots {
            grouped
                .entry(snapshot.timestamp)
                .or_default()
                .push(snapshot.clone());
        }
        let mut result: Vec<_> = grouped.into_iter().collect();
        result.sort_by_key(|row| row.0);
        result
    }

    #[tokio::test]
    async fn prepared_input_preserves_same_timestamp_context_fills_fees_and_fresh_strategy_state() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        let simulator = BacktestSimulator::new(
            HistoricalDataStore::new(pool),
            SimulatorConfig {
                initial_capital: Decimal::from(1000),
                slippage_model: SlippageModel::None,
                fee_model: FeeModel::Fixed(Decimal::new(2, 2)),
                min_position_size: Decimal::ONE,
                max_position_pct: Decimal::ONE,
                ..SimulatorConfig::default()
            },
        );
        let start = Utc::now();
        let end = start + chrono::Duration::minutes(1);
        let mut input = Vec::new();
        for (time, price) in [(start, Decimal::new(4, 1)), (end, Decimal::new(6, 1))] {
            for market in ["a", "b"] {
                input.push(MarketSnapshot::new(
                    market, time, price, price, price, price,
                ));
            }
        }
        let legacy = legacy_timeline(&input);
        let prepared = PreparedBacktest::new(input).unwrap();
        for (range, (timestamp, expected)) in prepared.batches.iter().zip(&legacy) {
            let batch = &prepared.snapshots[range.clone()];
            assert_eq!(batch[0].timestamp, *timestamp);
            assert_eq!(
                serde_json::to_value(batch).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
        for _ in 0..2 {
            let mut strategy = FixtureStrategy::default();
            let result = simulator
                .run_prepared(&mut strategy, &DataQuery::range(start, end), &prepared)
                .await
                .unwrap();
            assert_eq!(result.data_points, 4);
            assert_eq!(result.total_trades, 2);
            assert_eq!(result.final_value, Decimal::from(1036));
            assert_eq!(result.total_fees, Decimal::from(4));
            assert_eq!(strategy.observations, vec![(start, 1, 1), (end, 2, 2)]);
            assert_eq!(
                strategy.fills,
                vec![
                    (Decimal::new(4, 1), Decimal::from(100)),
                    (Decimal::new(4, 1), Decimal::from(100)),
                    (Decimal::new(6, 1), Decimal::from(100)),
                    (Decimal::new(6, 1), Decimal::from(100))
                ]
            );
        }
        let mut reversed = prepared.snapshots;
        reversed.reverse();
        assert!(PreparedBacktest::new(reversed).is_err());
        assert!(PreparedBacktest::new(Vec::new()).is_err());
    }

    #[test]
    #[ignore = "manual synthetic preparation benchmark"]
    fn benchmark_sorted_preparation() {
        let start = Utc::now();
        let input: Vec<_> = (0..100_000)
            .map(|index| {
                MarketSnapshot::new(
                    &format!("market-{}", index % 200),
                    start + chrono::Duration::seconds(index / 200),
                    Decimal::ONE,
                    Decimal::ONE,
                    Decimal::ONE,
                    Decimal::ONE,
                )
            })
            .collect();
        let mut old = Vec::new();
        let mut new = Vec::new();
        for _ in 0..5 {
            let source = input.clone();
            let time = std::time::Instant::now();
            let timeline = legacy_timeline(&source);
            std::hint::black_box(&timeline);
            old.push(time.elapsed().as_secs_f64() * 1000.0);
            drop(timeline);
            drop(source);
            let source = input.clone();
            let time = std::time::Instant::now();
            let prepared = PreparedBacktest::new(source).unwrap();
            std::hint::black_box(&prepared);
            new.push(time.elapsed().as_secs_f64() * 1000.0);
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        println!("100000 snapshots: old median {:.3} ms; prepared median {:.3} ms; removed cloned snapshot structs {} bytes plus market IDs",old[2],new[2],std::mem::size_of::<MarketSnapshot>()*input.len());
    }

    #[tokio::test]
    async fn repeated_buys_preserve_inventory_cost_fees_and_all_entry_lots() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        let simulator = BacktestSimulator::new(
            HistoricalDataStore::new(pool),
            SimulatorConfig {
                initial_capital: Decimal::from(1000),
                slippage_model: SlippageModel::None,
                fee_model: FeeModel::Fixed(Decimal::new(2, 2)),
                min_position_size: Decimal::ONE,
                max_position_pct: Decimal::ONE,
                ..SimulatorConfig::default()
            },
        );
        for outcome in ["yes", "no"] {
            let mut state = SimulationState::new(Decimal::from(1000));
            let mut context = StrategyContext::new(Decimal::from(1000));
            let first_time = context.timestamp;
            for (price, fraction) in [
                (Decimal::new(4, 1), Decimal::new(4, 2)),
                (Decimal::new(6, 1), Decimal::new(6, 2)),
            ] {
                let snapshot =
                    MarketSnapshot::new("market", context.timestamp, price, price, price, price);
                simulator
                    .execute_buy(
                        &Signal::buy("market", outcome, fraction),
                        &context,
                        &mut state,
                        &snapshot,
                    )
                    .unwrap()
                    .unwrap();
                context.timestamp += chrono::Duration::minutes(1);
            }
            let position = &state.positions[&position_key("market", outcome)];
            assert_eq!(position.quantity, Decimal::from(200));
            assert_eq!(position.entry_price, Decimal::new(5, 1));
            assert_eq!(position.opened_at, first_time);
            assert_eq!(state.cash, Decimal::from(898));
            assert_eq!(state.portfolio_value(), Decimal::from(1018));
            let price = Decimal::new(7, 1);
            let snapshot =
                MarketSnapshot::new("market", context.timestamp, price, price, price, price);
            let closed = simulator
                .execute_close(&Signal::close("market", outcome), &mut state, &snapshot)
                .unwrap()
                .unwrap();
            assert!(state.positions.is_empty());
            assert_eq!(state.cash, Decimal::new(10352, 1));
            assert_eq!(state.realized_pnl, Decimal::new(352, 1));
            assert_eq!(closed.quantity, Decimal::from(200));
            let entries: Vec<_> = state
                .trades
                .iter()
                .filter(|t| t.trade_type == TradeType::Buy)
                .collect();
            assert_eq!(entries.len(), 2);
            assert!(entries.iter().all(|t| t.exit_time.is_some()));
            assert_eq!(
                entries.iter().map(|t| t.pnl.unwrap()).sum::<Decimal>(),
                closed.pnl.unwrap()
            );
            assert_eq!(
                simulator.calculate_consecutive_streaks(&state.trades),
                (1, 0)
            );
            assert_eq!(state.winning_trades, 1);
        }
    }

    #[test]
    fn test_slippage_model_fixed() {
        let model = SlippageModel::Fixed(Decimal::new(1, 2)); // 1%
        let slippage = model.calculate(Decimal::new(100, 0), Decimal::new(10, 0), Decimal::ZERO);
        assert_eq!(slippage, Decimal::ONE);
    }

    #[test]
    fn test_slippage_model_volume() {
        let model = SlippageModel::VolumeBased {
            base_pct: Decimal::new(5, 3),    // 0.5%
            size_impact: Decimal::new(1, 4), // 0.01% per unit
        };
        let slippage = model.calculate(Decimal::new(100, 0), Decimal::new(100, 0), Decimal::ZERO);
        // 100 * (0.005 + 100 * 0.0001) = 100 * 0.015 = 1.5
        assert_eq!(slippage, Decimal::new(15, 1));
    }

    #[test]
    fn test_simulator_config_default() {
        let config = SimulatorConfig::default();
        assert_eq!(config.initial_capital, Decimal::new(10000, 0));
        assert!(matches!(config.fee_model, FeeModel::Fixed(f) if f == Decimal::new(2, 2)));
    }

    #[test]
    fn test_simulation_state() {
        let mut state = SimulationState::new(Decimal::new(10000, 0));
        assert_eq!(state.portfolio_value(), Decimal::new(10000, 0));

        state.positions.insert(
            "market1".to_string(),
            Position {
                market_id: "market1".to_string(),
                outcome_id: "yes".to_string(),
                quantity: Decimal::new(100, 0),
                entry_price: Decimal::new(50, 2),
                opened_at: Utc::now(),
                unrealized_pnl: Decimal::ZERO,
                current_price: Decimal::new(60, 2),
            },
        );

        state.cash = Decimal::new(9000, 0);
        // Portfolio = 9000 cash + 100 * 0.60 = 9000 + 60 = 9060
        assert_eq!(state.portfolio_value(), Decimal::new(9060, 0));
    }

    #[test]
    fn test_backtest_result_profitability() {
        let result = BacktestResult {
            strategy_name: "test".to_string(),
            strategy_params: HashMap::new(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            data_points: 0,
            initial_capital: Decimal::new(10000, 0),
            final_value: Decimal::new(11000, 0),
            total_return: Decimal::new(1000, 0),
            return_pct: 0.10,
            annualized_return: 0.0,
            max_drawdown: 0.05,
            sharpe_ratio: 1.5,
            sortino_ratio: 2.0,
            win_rate: 0.6,
            profit_factor: 1.5,
            total_trades: 10,
            winning_trades: 6,
            losing_trades: 4,
            total_fees: Decimal::ZERO,
            total_slippage: Decimal::ZERO,
            avg_trade_duration_hours: 24.0,
            equity_curve: vec![],
            trades: vec![],
            computed_at: Utc::now(),
            // New Phase 3 metrics
            calmar_ratio: 2.0,         // 10% return / 5% drawdown
            var_95: 0.02,              // 2% VaR
            cvar_95: 0.03,             // 3% CVaR
            recovery_factor: 2.0,      // 10% profit / 5% drawdown
            best_trade_return: 0.15,   // 15% best trade
            worst_trade_return: -0.05, // 5% worst trade
            max_consecutive_wins: 4,
            max_consecutive_losses: 2,
            avg_win: 0.08,    // 8% avg win
            avg_loss: 0.03,   // 3% avg loss
            expectancy: 0.03, // 3% expected return per trade
        };

        assert!(result.is_profitable());
        assert!(result.is_risk_adjusted_profitable());
    }

    #[test]
    fn test_fee_model_fixed() {
        let model = FeeModel::Fixed(Decimal::new(2, 2)); // 2%
        let fee = model.calculate(Decimal::new(100, 0), false, Decimal::ZERO);
        assert_eq!(fee, Decimal::new(2, 0)); // $2 fee on $100
    }

    #[test]
    fn test_fee_model_maker_taker() {
        let model = FeeModel::MakerTaker {
            maker_fee: Decimal::new(1, 3), // 0.1%
            taker_fee: Decimal::new(3, 3), // 0.3%
        };

        // Maker fee
        let maker_fee = model.calculate(Decimal::new(1000, 0), true, Decimal::ZERO);
        assert_eq!(maker_fee, Decimal::ONE); // $1 on $1000

        // Taker fee
        let taker_fee = model.calculate(Decimal::new(1000, 0), false, Decimal::ZERO);
        assert_eq!(taker_fee, Decimal::new(3, 0)); // $3 on $1000
    }

    #[test]
    fn test_fee_model_volume_tiered() {
        let model = FeeModel::VolumeTiered {
            tiers: vec![
                (Decimal::new(10000, 0), Decimal::new(15, 3)), // 1.5% above $10k
                (Decimal::new(100000, 0), Decimal::new(10, 3)), // 1.0% above $100k
            ],
            default_fee: Decimal::new(2, 2), // 2% default
        };

        // Below first tier
        let fee_low = model.calculate(Decimal::new(100, 0), false, Decimal::new(5000, 0));
        assert_eq!(fee_low, Decimal::new(2, 0)); // 2% = $2

        // Above first tier
        let fee_mid = model.calculate(Decimal::new(100, 0), false, Decimal::new(50000, 0));
        assert_eq!(fee_mid, Decimal::new(15, 1)); // 1.5% = $1.5

        // Above second tier
        let fee_high = model.calculate(Decimal::new(100, 0), false, Decimal::new(200000, 0));
        assert_eq!(fee_high, Decimal::ONE); // 1.0% = $1
    }

    #[test]
    fn test_slippage_model_depth_based() {
        let model = SlippageModel::DepthBased {
            base_pct: Decimal::new(5, 3),         // 0.5%
            depth_multiplier: Decimal::new(2, 2), // 2% per depth ratio
        };

        // Low depth (high slippage)
        let slippage_low_depth = model.calculate_with_depth(
            Decimal::new(100, 0),
            Decimal::new(100, 0),
            Decimal::ZERO,
            Decimal::new(50, 0), // 100/50 = 2.0 ratio
            12,
        );
        // 100 * (0.005 + 2.0 * 0.02) = 100 * 0.045 = 4.5
        assert_eq!(slippage_low_depth, Decimal::new(45, 1));

        // High depth (low slippage)
        let slippage_high_depth = model.calculate_with_depth(
            Decimal::new(100, 0),
            Decimal::new(100, 0),
            Decimal::ZERO,
            Decimal::new(1000, 0), // 100/1000 = 0.1 ratio
            12,
        );
        // 100 * (0.005 + 0.1 * 0.02) = 100 * 0.007 = 0.7
        assert_eq!(slippage_high_depth, Decimal::new(7, 1));
    }

    #[test]
    fn test_slippage_model_time_adjusted() {
        let model = SlippageModel::TimeAdjusted {
            base_pct: Decimal::new(1, 2),              // 1%
            prime_time_multiplier: Decimal::new(5, 1), // 0.5x during prime time
            off_hours_multiplier: Decimal::new(15, 1), // 1.5x during off hours
        };

        // Prime time (hour 12)
        let slippage_prime = model.calculate_with_depth(
            Decimal::new(100, 0),
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            12,
        );
        // 100 * 0.01 * 0.5 = 0.5
        assert_eq!(slippage_prime, Decimal::new(5, 1));

        // Off hours (hour 3)
        let slippage_off = model.calculate_with_depth(
            Decimal::new(100, 0),
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
            3,
        );
        // 100 * 0.01 * 1.5 = 1.5
        assert_eq!(slippage_off, Decimal::new(15, 1));
    }

    #[test]
    fn test_partial_fill_full() {
        let model = PartialFillModel::FullFill;
        let fill = model.calculate_fill(Decimal::new(100, 0), Decimal::new(1000, 0), Decimal::ZERO);
        assert_eq!(fill, Decimal::new(100, 0)); // Full fill
    }

    #[test]
    fn test_partial_fill_depth_based() {
        let model = PartialFillModel::DepthBased {
            max_depth_pct: Decimal::new(5, 1), // 50% of depth
        };

        // Order smaller than available depth
        let fill_small = model.calculate_fill(
            Decimal::new(100, 0),  // request 100
            Decimal::new(1000, 0), // 1000 depth available
            Decimal::ZERO,
        );
        assert_eq!(fill_small, Decimal::new(100, 0)); // Full fill (100 < 500)

        // Order larger than max fill from depth
        let fill_large = model.calculate_fill(
            Decimal::new(1000, 0), // request 1000
            Decimal::new(500, 0),  // 500 depth available
            Decimal::ZERO,
        );
        // Max fill = 500 * 0.5 = 250
        assert_eq!(fill_large, Decimal::new(250, 0)); // Partial fill
    }

    #[test]
    fn test_partial_fill_probabilistic() {
        let model = PartialFillModel::Probabilistic {
            base_fill_pct: Decimal::ONE,          // 100% base fill for small orders
            size_threshold: Decimal::new(100, 0), // Threshold at 100 units
            decay_rate: Decimal::ONE,             // Linear decay
        };

        // Small order (below threshold) - full fill
        let fill_small =
            model.calculate_fill(Decimal::new(50, 0), Decimal::new(1000, 0), Decimal::ZERO);
        assert_eq!(fill_small, Decimal::new(50, 0)); // 50 * 1.0 = 50

        // Large order (above threshold) - reduced fill
        let fill_large = model.calculate_fill(
            Decimal::new(200, 0), // request 200
            Decimal::new(1000, 0),
            Decimal::ZERO,
        );
        // size_ratio = 200/100 = 2.0
        // fill_pct = 1.0 / (1.0 + 2.0 * 1.0) = 1/3 = 0.333...
        // fill = 200 * 0.333... ≈ 66.67
        // Use range check due to decimal precision
        assert!(fill_large > Decimal::new(66, 0)); // > 66
        assert!(fill_large < Decimal::new(67, 0)); // < 67
    }

    #[test]
    fn test_partial_fill_fixed() {
        let model = PartialFillModel::Fixed(Decimal::new(75, 2)); // 75%

        let fill = model.calculate_fill(Decimal::new(100, 0), Decimal::new(1000, 0), Decimal::ZERO);
        assert_eq!(fill, Decimal::new(75, 0)); // 100 * 0.75 = 75
    }

    #[test]
    fn test_partial_fill_can_fill() {
        let model = PartialFillModel::FullFill;

        // Narrow spread - can fill
        assert!(model.can_fill(
            Decimal::new(2, 2),  // 2% spread
            Decimal::new(10, 2)  // 10% max
        ));

        // Wide spread - cannot fill
        assert!(!model.can_fill(
            Decimal::new(15, 2), // 15% spread
            Decimal::new(10, 2)  // 10% max
        ));
    }
}

#[cfg(test)]
#[path = "simulator/correctness_tests.rs"]
mod correctness_tests;

/// Risk metrics are computed from UTC-day closes, not from every sample.
#[cfg(test)]
mod risk_metric_tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn day0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 3, 1, 0, 0, 0).unwrap()
    }

    fn dec(v: f64) -> Decimal {
        Decimal::try_from(v).unwrap()
    }

    /// One equity point per day close, preceded by the opening equity.
    fn daily_curve(opening: f64, daily_returns: &[f64]) -> Vec<(DateTime<Utc>, Decimal)> {
        let mut curve = vec![(day0(), dec(opening))];
        let mut equity = opening;
        for (i, r) in daily_returns.iter().enumerate() {
            equity *= 1.0 + r;
            // Close at 23:55 of each day.
            curve.push((
                day0() + Duration::days(i as i64) + Duration::minutes(23 * 60 + 55),
                dec(equity),
            ));
        }
        curve
    }

    /// The same daily closes, but sampled every 5 minutes with large intraday
    /// swings that net out by the close of each day.
    fn five_minute_curve(opening: f64, daily_returns: &[f64]) -> Vec<(DateTime<Utc>, Decimal)> {
        let mut curve = vec![(day0(), dec(opening))];
        let mut previous_close = opening;
        for (i, r) in daily_returns.iter().enumerate() {
            let close = previous_close * (1.0 + r);
            let start = day0() + Duration::days(i as i64);
            // 287 noisy samples (+/-3%), then the exact close at 23:55.
            for step in 1..288 {
                let swing = if step % 2 == 0 { 0.03 } else { -0.03 };
                curve.push((
                    start + Duration::minutes(5 * step),
                    dec(previous_close * (1.0 + swing)),
                ));
            }
            curve.push((start + Duration::minutes(23 * 60 + 55), dec(close)));
            previous_close = close;
        }
        curve
    }

    #[test]
    fn daily_returns_use_day_closes_and_ignore_intraday_noise() {
        let returns = [0.02, -0.01, 0.03, 0.0, -0.02];
        let curve = five_minute_curve(1000.0, &returns);
        assert!(curve.len() > 1400, "fixture is 5-minute resolution");

        let got = daily_returns(&curve);
        assert_eq!(got.len(), returns.len());
        for (g, want) in got.iter().zip(returns) {
            assert!(approx(*g, want), "{g} != {want}");
        }
    }

    #[test]
    fn sharpe_and_sortino_match_hand_computed_daily_values() {
        // mean 1%, population std 2%, downside deviation over all four days
        // sqrt(2 x 1%^2 / 4) = 1% / sqrt(2).
        let returns = [0.03, -0.01, 0.03, -0.01];
        let annualization = 365.0_f64.sqrt();

        for curve in [
            daily_curve(1000.0, &returns),
            five_minute_curve(1000.0, &returns),
        ] {
            let (sharpe, sortino) = sharpe_sortino(&daily_returns(&curve));
            assert!(approx(sharpe, 0.5 * annualization), "sharpe {sharpe}");
            assert!(
                approx(sortino, 2.0_f64.sqrt() * annualization),
                "sortino {sortino}"
            );
        }
    }

    #[test]
    fn five_minute_sampling_no_longer_inflates_the_ratios() {
        // A drifting curve whose 5-minute path is dominated by +/-3% noise. The
        // daily series only sees the closes, so both samplings agree.
        let returns = [0.01, 0.02, 0.005, 0.015, 0.01, 0.02, 0.0, 0.01];
        let from_daily = sharpe_sortino(&daily_returns(&daily_curve(5000.0, &returns)));
        let from_noisy = sharpe_sortino(&daily_returns(&five_minute_curve(5000.0, &returns)));
        assert!(approx(from_daily.0, from_noisy.0));
        assert!(from_daily.0 > 3.0, "clear drift, got {}", from_daily.0);
    }

    #[tokio::test]
    async fn simulator_methods_use_the_daily_series() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        let simulator =
            BacktestSimulator::new(HistoricalDataStore::new(pool), SimulatorConfig::default());
        let returns = [0.03, -0.01, 0.03, -0.01];
        let curve = five_minute_curve(1000.0, &returns);
        let (sharpe, sortino) = simulator.calculate_risk_metrics(&curve);
        let annualization = 365.0_f64.sqrt();
        assert!(approx(sharpe, 0.5 * annualization));
        assert!(approx(sortino, 2.0_f64.sqrt() * annualization));

        let (var_95, cvar_95) = simulator.calculate_var_cvar(&curve);
        // Four daily returns -> tail of one: the worst day (-1%).
        assert!(approx(var_95, 0.01) && approx(cvar_95, 0.01));
    }

    #[test]
    fn first_day_is_measured_against_opening_equity_and_gaps_are_skipped() {
        let curve = vec![
            (day0(), dec(1000.0)),
            // Same UTC day as the opening point: its last value is the day close.
            (day0() + Duration::hours(3), dec(1100.0)),
            (day0() + Duration::hours(20), dec(1050.0)),
            // Days 2 and 3 have no observations; day 4 closes at 1155.
            (day0() + Duration::days(3) + Duration::hours(1), dec(1155.0)),
        ];
        let got = daily_returns(&curve);
        assert_eq!(got.len(), 2);
        assert!(approx(got[0], 0.05));
        assert!(approx(got[1], 0.10));
    }

    #[test]
    fn degenerate_curves_yield_no_ratios() {
        assert!(daily_returns(&[]).is_empty());
        assert!(daily_returns(&[(day0(), dec(1000.0))]).is_empty());
        // Zero opening equity: no division by zero.
        let curve = vec![
            (day0(), Decimal::ZERO),
            (day0() + Duration::days(1), dec(10.0)),
        ];
        assert_eq!(daily_returns(&curve), vec![0.0]);

        assert_eq!(sharpe_sortino(&[]), (0.0, 0.0));
        // One daily observation is not enough to measure dispersion.
        assert_eq!(sharpe_sortino(&[0.05]), (0.0, 0.0));
        assert_eq!(var_cvar_95(&[]), (0.0, 0.0));
    }

    #[test]
    fn no_downside_gives_infinite_sortino_and_flat_curve_gives_zero() {
        let (sharpe, sortino) = sharpe_sortino(&[0.01, 0.02, 0.03]);
        assert!(sharpe > 0.0);
        assert!(sortino.is_infinite() && sortino > 0.0);

        assert_eq!(sharpe_sortino(&[0.0, 0.0, 0.0]), (0.0, 0.0));
    }

    #[test]
    fn var_and_cvar_use_the_same_tail() {
        // 100 returns from -4.9% to +5.0%; the 5% tail is the worst five.
        let returns: Vec<f64> = (1..=100).map(|i| (i as f64 - 50.0) / 1000.0).collect();
        let (var_95, cvar_95) = var_cvar_95(&returns);
        // Tail: -4.9%, -4.8%, -4.7%, -4.6%, -4.5%.
        assert!(approx(var_95, 0.045), "var {var_95}");
        assert!(approx(cvar_95, 0.047), "cvar {cvar_95}");
        assert!(cvar_95 >= var_95);

        // Input order is irrelevant.
        let mut shuffled = returns.clone();
        shuffled.reverse();
        assert_eq!(var_cvar_95(&shuffled), (var_95, cvar_95));
    }

    #[test]
    fn var_and_cvar_for_small_samples_are_consistent() {
        // One return: both are that loss (the old code reported VaR 0 here).
        let (var_95, cvar_95) = var_cvar_95(&[-0.02]);
        assert!(approx(var_95, 0.02) && approx(cvar_95, 0.02));

        // Ten returns: tail of one, so VaR equals CVaR (the worst day). The old
        // code took VaR from the second-worst return and CVaR from the worst.
        let returns = [-0.05, -0.04, -0.01, 0.0, 0.01, 0.02, 0.02, 0.03, 0.03, 0.04];
        let (var_95, cvar_95) = var_cvar_95(&returns);
        assert!(approx(var_95, 0.05) && approx(cvar_95, 0.05));

        // Twenty-one returns: tail of two.
        let mut returns: Vec<f64> = vec![0.01; 19];
        returns.extend([-0.03, -0.07]);
        let (var_95, cvar_95) = var_cvar_95(&returns);
        assert!(approx(var_95, 0.03), "var {var_95}");
        assert!(approx(cvar_95, 0.05), "cvar {cvar_95}");
    }
}

/// Per-market history retention follows the strategy's declared lookback.
#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::strategy::{ArbitrageStrategy, MomentumStrategy};
    use chrono::Duration;

    /// Records the history length it sees every step.
    struct Recorder {
        required: usize,
        seen: Vec<usize>,
    }

    #[async_trait::async_trait]
    impl Strategy for Recorder {
        fn name(&self) -> &str {
            "history-recorder"
        }
        async fn on_data(&mut self, context: &StrategyContext) -> Result<Vec<Signal>> {
            self.seen.push(context.market_data["m"].len());
            Ok(vec![])
        }
        fn required_history(&self) -> usize {
            self.required
        }
    }

    fn simulator(initial: i64) -> BacktestSimulator {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        BacktestSimulator::new(
            HistoricalDataStore::new(pool),
            SimulatorConfig {
                initial_capital: Decimal::from(initial),
                slippage_model: SlippageModel::None,
                fee_model: FeeModel::Fixed(Decimal::ZERO),
                min_position_size: Decimal::ONE,
                ..SimulatorConfig::default()
            },
        )
    }

    /// `n` five-minute snapshots whose yes_bid rises by `step` per bar.
    fn rising(n: usize, start: &str, step: &str) -> (Vec<MarketSnapshot>, DateTime<Utc>) {
        let t0 = Utc::now();
        let mut price: Decimal = start.parse().unwrap();
        let step: Decimal = step.parse().unwrap();
        let snapshots = (0..n)
            .map(|i| {
                let ask = price + Decimal::new(5, 3);
                let snapshot = MarketSnapshot::new(
                    "m",
                    t0 + Duration::minutes(5 * i as i64),
                    price,
                    ask,
                    Decimal::ONE - ask,
                    Decimal::ONE - price,
                );
                price += step;
                snapshot
            })
            .collect();
        (snapshots, t0)
    }

    async fn run_recorder(required: usize, bars: usize) -> Vec<usize> {
        let (snapshots, t0) = rising(bars, "0.30", "0.0001");
        let prepared = PreparedBacktest::new(snapshots).unwrap();
        let mut strategy = Recorder {
            required,
            seen: vec![],
        };
        simulator(10_000)
            .run_prepared(
                &mut strategy,
                &DataQuery::range(t0, t0 + Duration::days(30)),
                &prepared,
            )
            .await
            .unwrap();
        strategy.seen
    }

    #[test]
    fn history_limit_is_clamped() {
        assert_eq!(history_limit(0), MIN_HISTORY_BARS);
        assert_eq!(history_limit(1), MIN_HISTORY_BARS);
        assert_eq!(history_limit(288), 288);
        assert_eq!(history_limit(1_000_000), MAX_HISTORY_BARS);
    }

    #[tokio::test]
    async fn default_strategies_keep_at_least_the_old_window() {
        let seen = run_recorder(0, 600).await;
        // Never trimmed below the default window once it has filled...
        assert!(seen[MIN_HISTORY_BARS..]
            .iter()
            .all(|len| *len >= MIN_HISTORY_BARS));
        // ...and bounded: limit plus max(limit / 4, 50) slack.
        assert!(seen.iter().all(|len| *len <= MIN_HISTORY_BARS + 50));
    }

    #[tokio::test]
    async fn long_lookbacks_see_their_full_window() {
        // The old fixed cap trimmed history to 51-100 snapshots, so a 288-bar
        // (24h at 5 minutes) lookback never saw 288.
        let seen = run_recorder(288, 1_200).await;
        assert!(seen.contains(&288));
        assert!(seen[288..].iter().all(|len| *len >= 288));
        // Retention stays bounded: limit + max(limit/4, 50) slack.
        assert!(seen.iter().all(|len| *len <= 288 + 72));
    }

    #[tokio::test]
    async fn retention_is_capped_for_absurd_lookbacks() {
        let seen = run_recorder(10_000_000, MAX_HISTORY_BARS + 2_000).await;
        let max = *seen.iter().max().unwrap();
        assert!(max <= MAX_HISTORY_BARS + MAX_HISTORY_BARS / 4);
    }

    #[tokio::test]
    async fn a_24_hour_momentum_lookback_now_trades() {
        // 24h of 5-minute bars = 288. The price climbs ~0.0001 per bar from
        // 0.30, i.e. well over the 5% threshold across any 288-bar window, so
        // the strategy must enter once 288 bars of history exist.
        let (snapshots, t0) = rising(500, "0.30", "0.0005");
        let prepared = PreparedBacktest::new(snapshots).unwrap();
        let mut strategy = MomentumStrategy::new(288, Decimal::new(5, 2), Decimal::new(10, 2));
        let result = simulator(10_000)
            .run_prepared(
                &mut strategy,
                &DataQuery::range(t0, t0 + Duration::days(3)),
                &prepared,
            )
            .await
            .unwrap();
        assert!(
            result.total_trades > 0,
            "a lookback beyond the old 100-bar cap must be able to trade"
        );
    }

    #[tokio::test]
    async fn arbitrage_legs_fill_equal_share_counts() {
        let t0 = Utc::now();
        let snapshots: Vec<MarketSnapshot> = (0..3)
            .map(|i| {
                MarketSnapshot::new(
                    "m",
                    t0 + Duration::minutes(5 * i),
                    Decimal::new(39, 2),
                    Decimal::new(40, 2),
                    Decimal::new(54, 2),
                    Decimal::new(55, 2),
                )
                .with_depth(
                    Decimal::from(1000),
                    Decimal::from(1000),
                    Decimal::from(1000),
                    Decimal::from(1000),
                )
            })
            .collect();
        let prepared = PreparedBacktest::new(snapshots).unwrap();
        let mut strategy =
            ArbitrageStrategy::new(Decimal::ZERO, Decimal::new(20, 2), 10).with_fee(Decimal::ZERO);
        let result = simulator(10_000)
            .run_prepared(
                &mut strategy,
                &DataQuery::range(t0, t0 + Duration::days(1)),
                &prepared,
            )
            .await
            .unwrap();

        let buy_quantity = |outcome: &str| -> Decimal {
            result
                .trades
                .iter()
                .find(|t| t.trade_type == TradeType::Buy && t.outcome_id == outcome)
                .map(|t| t.quantity)
                .expect("buy fill")
        };
        let yes = buy_quantity("yes");
        let no = buy_quantity("no");
        assert!(
            (yes - no).abs() < Decimal::new(1, 6),
            "{yes} YES vs {no} NO shares"
        );
    }
}

/// Data-set size guard and indexed closing of entry lots.
#[cfg(test)]
mod limits_tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn query_limit_requests_one_extra_row_to_detect_oversized_data() {
        assert_eq!(effective_query_limit(None, None), None);
        assert_eq!(effective_query_limit(Some(50), None), Some(50));
        assert_eq!(effective_query_limit(None, Some(1_000)), Some(1_001));
        // A stricter caller limit is respected.
        assert_eq!(effective_query_limit(Some(10), Some(1_000)), Some(10));
        // A looser caller limit is tightened.
        assert_eq!(effective_query_limit(Some(5_000), Some(1_000)), Some(1_001));
        assert_eq!(
            effective_query_limit(None, Some(usize::MAX)),
            Some(usize::MAX)
        );
    }

    #[test]
    fn oversized_data_is_refused_not_truncated() {
        assert!(enforce_snapshot_limit(1_000, Some(1_000)).is_ok());
        assert!(enforce_snapshot_limit(0, Some(1_000)).is_ok());
        assert!(enforce_snapshot_limit(10_000_000, None).is_ok());
        let err = enforce_snapshot_limit(1_001, Some(1_000)).unwrap_err();
        assert!(err.to_string().contains("exceeds 1000 snapshots"), "{err}");
    }

    fn simulator() -> BacktestSimulator {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        BacktestSimulator::new(
            HistoricalDataStore::new(pool),
            SimulatorConfig {
                initial_capital: Decimal::from(1_000_000),
                slippage_model: SlippageModel::None,
                fee_model: FeeModel::Fixed(Decimal::new(2, 2)),
                min_position_size: Decimal::ONE,
                ..SimulatorConfig::default()
            },
        )
    }

    fn snapshot(market: &str, minute: i64, bid: &str) -> MarketSnapshot {
        let bid: Decimal = bid.parse().unwrap();
        MarketSnapshot::new(
            market,
            Utc::now() + Duration::minutes(minute),
            bid,
            bid + Decimal::new(1, 2),
            Decimal::ONE - bid - Decimal::new(1, 2),
            Decimal::ONE - bid,
        )
    }

    fn context() -> StrategyContext {
        let mut context = StrategyContext::new(Decimal::from(1_000_000));
        context.timestamp = Utc::now();
        context
    }

    #[tokio::test]
    async fn closing_settles_only_the_positions_own_lots() {
        let simulator = simulator();
        let context = context();
        let mut state = SimulationState::new(Decimal::from(1_000_000));

        let a_open = snapshot("a", 0, "0.40");
        let b_open = snapshot("b", 0, "0.40");
        let size = Decimal::new(1, 3);
        // Two adds to a/yes (one position, two lots) and one lot in b/yes.
        for (market, snap) in [("a", &a_open), ("b", &b_open), ("a", &a_open)] {
            simulator
                .execute_buy(
                    &Signal::buy(market, "yes", size),
                    &context,
                    &mut state,
                    snap,
                )
                .unwrap()
                .expect("buy fills");
        }
        assert_eq!(state.open_lots["a:yes"].len(), 2);
        assert_eq!(state.open_lots["b:yes"].len(), 1);

        let a_close = snapshot("a", 60, "0.55");
        let close = simulator
            .execute_close(&Signal::close("a", "yes"), &mut state, &a_close)
            .unwrap()
            .expect("close fills");

        // a's lots are settled and unregistered; b's lot is untouched.
        assert!(!state.open_lots.contains_key("a:yes"));
        assert_eq!(state.open_lots["b:yes"].len(), 1);
        let lots = |market: &str| -> Vec<&TradeRecord> {
            state
                .trades
                .iter()
                .filter(|t| t.trade_type == TradeType::Buy && t.market_id == market)
                .collect()
        };
        assert_eq!(lots("a").len(), 2);
        assert!(lots("a")
            .iter()
            .all(|t| t.exit_time.is_some() && t.pnl.is_some() && t.return_pct.is_some()));
        assert!(lots("b")
            .iter()
            .all(|t| t.exit_time.is_none() && t.pnl.is_none()));

        // Per-lot P&L adds up to the close trade's P&L (entry and exit fees
        // included), exactly as with the previous full scan.
        let lot_pnl: Decimal = lots("a").iter().filter_map(|t| t.pnl).sum();
        let close_pnl = close.pnl.unwrap();
        assert!(
            (lot_pnl - close_pnl).abs() < Decimal::new(1, 12),
            "{lot_pnl} vs {close_pnl}"
        );
    }

    #[tokio::test]
    async fn many_round_trips_leave_no_open_lots() {
        let simulator = simulator();
        let context = context();
        let mut state = SimulationState::new(Decimal::from(1_000_000));
        let size = Decimal::new(1, 4);

        for round in 0..2_000_i64 {
            let market = format!("m{}", round % 7);
            let open = snapshot(&market, round, "0.40");
            let close = snapshot(&market, round + 1, "0.45");
            simulator
                .execute_buy(
                    &Signal::buy(&market, "yes", size),
                    &context,
                    &mut state,
                    &open,
                )
                .unwrap()
                .expect("buy fills");
            simulator
                .execute_close(&Signal::close(&market, "yes"), &mut state, &close)
                .unwrap()
                .expect("close fills");
        }

        assert!(state.open_lots.is_empty());
        assert!(state.positions.is_empty());
        assert!(state
            .trades
            .iter()
            .filter(|t| t.trade_type == TradeType::Buy)
            .all(|t| t.exit_time.is_some() && t.pnl.is_some()));
        assert_eq!(state.winning_trades + state.losing_trades, 2_000);
    }
}
