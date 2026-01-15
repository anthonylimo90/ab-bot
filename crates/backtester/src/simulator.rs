//! Backtest simulator with slippage and fee models.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
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
    /// Trading fee percentage (e.g., 0.02 for 2%). Deprecated: use fee_model instead.
    pub trading_fee_pct: Decimal,
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
}

impl Default for SimulatorConfig {
    fn default() -> Self {
        Self {
            initial_capital: Decimal::new(10000, 0),
            trading_fee_pct: Decimal::new(2, 2), // 2% (deprecated)
            slippage_model: SlippageModel::Fixed(Decimal::new(1, 3)), // 0.1%
            fee_model: FeeModel::Fixed(Decimal::new(2, 2)), // 2%
            partial_fill_model: PartialFillModel::FullFill,
            allow_margin: false,
            max_leverage: Decimal::ONE,
            reinvest_profits: true,
            min_position_size: Decimal::new(10, 0),
            max_position_pct: Decimal::new(20, 2), // 20%
            max_spread_for_fill: Decimal::new(10, 2), // 10% max spread
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
            SlippageModel::VolumeBased { base_pct, size_impact } => {
                let volume_impact = quantity * size_impact;
                price * (*base_pct + volume_impact)
            }
            SlippageModel::SpreadBased { spread_multiplier } => {
                spread * spread_multiplier / Decimal::TWO
            }
            SlippageModel::DepthBased { base_pct, depth_multiplier } => {
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
                let time_mult = if hour >= 9 && hour <= 17 {
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
            FeeModel::MakerTaker { maker_fee, taker_fee } => {
                let fee_pct = if is_maker { *maker_fee } else { *taker_fee };
                value * fee_pct
            }
            FeeModel::VolumeTiered { tiers, default_fee } => {
                // Find applicable tier based on cumulative volume
                let fee_pct = tiers
                    .iter()
                    .filter(|(threshold, _)| cumulative_volume >= *threshold)
                    .map(|(_, fee)| *fee)
                    .last()
                    .unwrap_or(*default_fee);
                value * fee_pct
            }
        }
    }
}

/// Model for simulating partial fills based on market liquidity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialFillModel {
    /// Always fill the entire order (instant fill assumption).
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

impl Default for PartialFillModel {
    fn default() -> Self {
        PartialFillModel::FullFill
    }
}

impl PartialFillModel {
    /// Calculate fill quantity based on the model.
    /// Returns the quantity that will be filled.
    pub fn calculate_fill(
        &self,
        requested_quantity: Decimal,
        available_depth: Decimal,
        spread: Decimal,
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

        // Fetch historical data
        let snapshots = self.data_store.query_snapshots(&query).await?;
        if snapshots.is_empty() {
            return Err(anyhow!("No data available for the specified query"));
        }

        // Group data by timestamp for sequential processing
        let timeline = self.build_timeline(&snapshots);

        // Initialize simulation state
        let mut state = SimulationState::new(self.config.initial_capital);
        let mut context = StrategyContext::new(self.config.initial_capital);

        // Initialize strategy
        strategy.initialize(&context).await?;

        // Process each time step
        for (timestamp, market_snapshots) in timeline {
            context.timestamp = timestamp;

            // Update market data in context
            for snapshot in &market_snapshots {
                context
                    .market_data
                    .entry(snapshot.market_id.clone())
                    .or_default()
                    .push(snapshot.clone());

                // Limit history size
                if let Some(data) = context.market_data.get_mut(&snapshot.market_id) {
                    if data.len() > 100 {
                        data.drain(0..50);
                    }
                }
            }

            // Update position prices
            self.update_positions(&mut context, &mut state, &market_snapshots);

            // Get signals from strategy
            let signals = strategy.on_data(&context).await?;

            // Execute signals
            for signal in signals {
                if let Some(trade) = self.execute_signal(&signal, &mut context, &mut state, &market_snapshots).await? {
                    strategy.on_fill(&signal, trade.entry_price, trade.quantity).await?;
                }
            }

            // Record equity curve
            state.equity_curve.push((timestamp, state.portfolio_value()));

            // Update context portfolio value
            context.portfolio_value = state.portfolio_value();
            context.available_cash = state.cash;
        }

        // Finalize strategy
        strategy.finalize(&context).await?;

        // Close any remaining positions at last price
        self.close_all_positions(&mut state, &context);

        // Calculate final metrics
        let result = self.calculate_results(strategy, &state, &query, snapshots.len());

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

        for strategy in strategies.iter_mut() {
            match self.run(strategy.as_mut(), query.clone()).await {
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

    fn build_timeline(&self, snapshots: &[MarketSnapshot]) -> Vec<(DateTime<Utc>, Vec<MarketSnapshot>)> {
        let mut timeline: HashMap<DateTime<Utc>, Vec<MarketSnapshot>> = HashMap::new();

        for snapshot in snapshots {
            timeline
                .entry(snapshot.timestamp)
                .or_default()
                .push(snapshot.clone());
        }

        let mut sorted: Vec<_> = timeline.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted
    }

    fn update_positions(
        &self,
        context: &mut StrategyContext,
        state: &mut SimulationState,
        snapshots: &[MarketSnapshot],
    ) {
        for snapshot in snapshots {
            if let Some(pos) = state.positions.get_mut(&snapshot.market_id) {
                let price = if pos.outcome_id == "yes" {
                    snapshot.yes_bid
                } else {
                    snapshot.no_bid
                };

                pos.current_price = price;
                pos.unrealized_pnl = (price - pos.entry_price) * pos.quantity;
            }
        }

        // Update context positions
        context.positions = state
            .positions
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    }

    async fn execute_signal(
        &self,
        signal: &Signal,
        context: &mut StrategyContext,
        state: &mut SimulationState,
        snapshots: &[MarketSnapshot],
    ) -> Result<Option<TradeRecord>> {
        let snapshot = snapshots
            .iter()
            .find(|s| s.market_id == signal.market_id);

        let snapshot = match snapshot {
            Some(s) => s,
            None => return Ok(None),
        };

        match signal.signal_type {
            SignalType::Buy => {
                self.execute_buy(signal, context, state, snapshot)
            }
            SignalType::Sell => {
                self.execute_sell(signal, state, snapshot)
            }
            SignalType::Close => {
                self.execute_close(signal, state, snapshot)
            }
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
        let max_position_value = context.portfolio_value * self.config.max_position_pct;
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
        if !self.config.partial_fill_model.can_fill(spread, self.config.max_spread_for_fill) {
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
        let fill_quantity = self.config.partial_fill_model.calculate_fill(
            requested_quantity,
            depth,
            spread,
        );

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
            false, // taker order
            state.total_fees, // cumulative volume for tiered fees
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
        state.total_slippage += slippage * fill_quantity;

        // Create or add to position
        let position = Position {
            market_id: signal.market_id.clone(),
            outcome_id: signal.outcome_id.clone(),
            quantity: fill_quantity,
            entry_price: execution_price,
            opened_at: context.timestamp,
            unrealized_pnl: Decimal::ZERO,
            current_price: execution_price,
        };

        state.positions.insert(signal.market_id.clone(), position);

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
        let position = match state.positions.remove(&signal.market_id) {
            Some(p) => p,
            None => return Ok(None),
        };

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
            false, // taker order
            state.total_fees, // cumulative volume for tiered fees
        );
        let proceeds = trade_value - fees;

        // Calculate P&L
        let cost_basis = position.quantity * position.entry_price;
        let pnl = proceeds - cost_basis;
        let return_pct = if cost_basis > Decimal::ZERO {
            ((proceeds / cost_basis) - Decimal::ONE).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };

        // Update state
        state.cash += proceeds;
        state.total_fees += fees;
        state.total_slippage += slippage * position.quantity;
        state.realized_pnl += pnl;

        if pnl > Decimal::ZERO {
            state.winning_trades += 1;
            state.total_wins += pnl;
        } else {
            state.losing_trades += 1;
            state.total_losses += pnl.abs();
        }

        // Find and update the entry trade
        let entry_trade_idx = state.trades.iter().position(|t| {
            t.market_id == signal.market_id && t.exit_time.is_none()
        });

        if let Some(idx) = entry_trade_idx {
            state.trades[idx].exit_time = Some(snapshot.timestamp);
            state.trades[idx].exit_price = Some(execution_price);
            state.trades[idx].pnl = Some(pnl);
            state.trades[idx].return_pct = Some(return_pct);
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

        Ok(Some(trade))
    }

    fn close_all_positions(&self, state: &mut SimulationState, context: &StrategyContext) {
        let positions: Vec<_> = state.positions.keys().cloned().collect();

        for market_id in positions {
            if let Some(snapshots) = context.market_data.get(&market_id) {
                if let Some(snapshot) = snapshots.last() {
                    let signal = Signal::close(&market_id, "");
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
        let days = (query.end_time - query.start_time).num_days() as f64;
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
            (state.total_wins / state.total_losses).to_f64().unwrap_or(0.0)
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
        let trade_returns: Vec<f64> = state.trades
            .iter()
            .filter_map(|t| t.return_pct)
            .collect();

        let best_trade_return = trade_returns.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let worst_trade_return = trade_returns.iter().cloned().fold(f64::INFINITY, f64::min);

        let best_trade_return = if best_trade_return == f64::NEG_INFINITY { 0.0 } else { best_trade_return };
        let worst_trade_return = if worst_trade_return == f64::INFINITY { 0.0 } else { worst_trade_return };

        // Consecutive wins/losses
        let (max_consecutive_wins, max_consecutive_losses) = self.calculate_consecutive_streaks(&state.trades);

        // Average win/loss
        let winning_returns: Vec<f64> = trade_returns.iter().filter(|&&r| r > 0.0).copied().collect();
        let losing_returns: Vec<f64> = trade_returns.iter().filter(|&&r| r < 0.0).copied().collect();

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
            end_time: query.end_time,
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

    /// Calculate VaR and CVaR at 95% confidence.
    fn calculate_var_cvar(&self, equity_curve: &[(DateTime<Utc>, Decimal)]) -> (f64, f64) {
        if equity_curve.len() < 2 {
            return (0.0, 0.0);
        }

        // Calculate daily returns
        let mut returns: Vec<f64> = equity_curve
            .windows(2)
            .map(|w| {
                let prev = w[0].1;
                let curr = w[1].1;
                if prev == Decimal::ZERO {
                    0.0
                } else {
                    ((curr - prev) / prev).to_f64().unwrap_or(0.0)
                }
            })
            .collect();

        if returns.is_empty() {
            return (0.0, 0.0);
        }

        // Sort returns for percentile calculation
        returns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // VaR at 95% (5th percentile of returns)
        let var_index = (returns.len() as f64 * 0.05).ceil() as usize;
        let var_95 = if var_index < returns.len() {
            -returns[var_index] // Negate because losses are negative
        } else {
            0.0
        };

        // CVaR (average of returns below VaR)
        let tail_returns: Vec<f64> = returns.iter().take(var_index.max(1)).copied().collect();
        let cvar_95 = if !tail_returns.is_empty() {
            -(tail_returns.iter().sum::<f64>() / tail_returns.len() as f64)
        } else {
            0.0
        };

        (var_95, cvar_95)
    }

    /// Calculate maximum consecutive wins and losses.
    fn calculate_consecutive_streaks(&self, trades: &[TradeRecord]) -> (usize, usize) {
        let mut max_wins = 0;
        let mut max_losses = 0;
        let mut current_wins = 0;
        let mut current_losses = 0;

        for trade in trades {
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

    fn calculate_risk_metrics(&self, equity_curve: &[(DateTime<Utc>, Decimal)]) -> (f64, f64) {
        if equity_curve.len() < 2 {
            return (0.0, 0.0);
        }

        // Calculate daily returns
        let returns: Vec<f64> = equity_curve
            .windows(2)
            .map(|w| {
                let prev = w[0].1;
                let curr = w[1].1;
                if prev == Decimal::ZERO {
                    0.0
                } else {
                    ((curr - prev) / prev).to_f64().unwrap_or(0.0)
                }
            })
            .collect();

        if returns.is_empty() {
            return (0.0, 0.0);
        }

        let mean_return: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter().map(|r| (r - mean_return).powi(2)).sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        // Sharpe ratio (assuming 0% risk-free rate for simplicity)
        let sharpe = if std_dev > 0.0 {
            (mean_return / std_dev) * (252.0_f64).sqrt() // Annualized
        } else {
            0.0
        };

        // Sortino ratio (only downside deviation)
        let downside_returns: Vec<f64> = returns.iter().filter(|&&r| r < 0.0).map(|&r| r * r).collect();
        let downside_dev = if !downside_returns.is_empty() {
            (downside_returns.iter().sum::<f64>() / downside_returns.len() as f64).sqrt()
        } else {
            0.0
        };

        let sortino = if downside_dev > 0.0 {
            (mean_return / downside_dev) * (252.0_f64).sqrt()
        } else if mean_return > 0.0 {
            f64::INFINITY
        } else {
            0.0
        };

        (sharpe, sortino)
    }

    fn calculate_avg_trade_duration(&self, trades: &[TradeRecord]) -> f64 {
        let closed_trades: Vec<_> = trades
            .iter()
            .filter(|t| t.exit_time.is_some())
            .collect();

        if closed_trades.is_empty() {
            return 0.0;
        }

        let total_hours: i64 = closed_trades
            .iter()
            .map(|t| {
                t.exit_time.unwrap().signed_duration_since(t.entry_time).num_hours()
            })
            .sum();

        total_hours as f64 / closed_trades.len() as f64
    }
}

/// Internal simulation state.
struct SimulationState {
    cash: Decimal,
    positions: HashMap<String, Position>,
    trades: Vec<TradeRecord>,
    equity_curve: Vec<(DateTime<Utc>, Decimal)>,
    total_fees: Decimal,
    total_slippage: Decimal,
    realized_pnl: Decimal,
    winning_trades: usize,
    losing_trades: usize,
    total_wins: Decimal,
    total_losses: Decimal,
}

impl SimulationState {
    fn new(initial_capital: Decimal) -> Self {
        Self {
            cash: initial_capital,
            positions: HashMap::new(),
            trades: Vec::new(),
            equity_curve: Vec::new(),
            total_fees: Decimal::ZERO,
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

    #[test]
    fn test_slippage_model_fixed() {
        let model = SlippageModel::Fixed(Decimal::new(1, 2)); // 1%
        let slippage = model.calculate(
            Decimal::new(100, 0),
            Decimal::new(10, 0),
            Decimal::ZERO,
        );
        assert_eq!(slippage, Decimal::ONE);
    }

    #[test]
    fn test_slippage_model_volume() {
        let model = SlippageModel::VolumeBased {
            base_pct: Decimal::new(5, 3), // 0.5%
            size_impact: Decimal::new(1, 4), // 0.01% per unit
        };
        let slippage = model.calculate(
            Decimal::new(100, 0),
            Decimal::new(100, 0),
            Decimal::ZERO,
        );
        // 100 * (0.005 + 100 * 0.0001) = 100 * 0.015 = 1.5
        assert_eq!(slippage, Decimal::new(15, 1));
    }

    #[test]
    fn test_simulator_config_default() {
        let config = SimulatorConfig::default();
        assert_eq!(config.initial_capital, Decimal::new(10000, 0));
        assert_eq!(config.trading_fee_pct, Decimal::new(2, 2));
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
            calmar_ratio: 2.0,        // 10% return / 5% drawdown
            var_95: 0.02,             // 2% VaR
            cvar_95: 0.03,            // 3% CVaR
            recovery_factor: 2.0,     // 10% profit / 5% drawdown
            best_trade_return: 0.15,  // 15% best trade
            worst_trade_return: -0.05, // 5% worst trade
            max_consecutive_wins: 4,
            max_consecutive_losses: 2,
            avg_win: 0.08,            // 8% avg win
            avg_loss: 0.03,           // 3% avg loss
            expectancy: 0.03,         // 3% expected return per trade
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
                (Decimal::new(10000, 0), Decimal::new(15, 3)),   // 1.5% above $10k
                (Decimal::new(100000, 0), Decimal::new(10, 3)),  // 1.0% above $100k
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
            base_pct: Decimal::new(5, 3),      // 0.5%
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
            base_pct: Decimal::new(1, 2), // 1%
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
        let fill = model.calculate_fill(
            Decimal::new(100, 0),
            Decimal::new(1000, 0),
            Decimal::ZERO,
        );
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
        let fill_small = model.calculate_fill(
            Decimal::new(50, 0),
            Decimal::new(1000, 0),
            Decimal::ZERO,
        );
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

        let fill = model.calculate_fill(
            Decimal::new(100, 0),
            Decimal::new(1000, 0),
            Decimal::ZERO,
        );
        assert_eq!(fill, Decimal::new(75, 0)); // 100 * 0.75 = 75
    }

    #[test]
    fn test_partial_fill_can_fill() {
        let model = PartialFillModel::FullFill;

        // Narrow spread - can fill
        assert!(model.can_fill(
            Decimal::new(2, 2),  // 2% spread
            Decimal::new(10, 2) // 10% max
        ));

        // Wide spread - cannot fill
        assert!(!model.can_fill(
            Decimal::new(15, 2), // 15% spread
            Decimal::new(10, 2) // 10% max
        ));
    }
}
