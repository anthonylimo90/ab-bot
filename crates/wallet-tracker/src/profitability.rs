//! Profitability analysis for wallet performance metrics.
//!
//! All P&L-derived metrics are computed from **realized** trade results, not
//! from raw cash flows. Fills from `wallet_trades` are replayed through a
//! per-asset FIFO lot engine ([`polymarket_core::feature_extractor::fifo`]):
//!
//! * a SELL closes the oldest open lots of the *same* asset and realizes
//!   `proceeds - cost_basis` for the matched quantity;
//! * SELLs (or parts of SELLs) with no matching BUY inside the fetched window
//!   have an unknown basis and are ignored, never counted as profit;
//! * open lots (buy-and-hold) realize nothing until sold;
//! * a **win/loss is counted per closing sale** (`realized_pnl > 0` / `< 0`);
//!   `win_rate = winning sales / closing sales` (breakeven sales are closed but
//!   not wins);
//! * `roi = total realized P&L / cost basis of the matched (closed) lots`;
//! * **capital at risk** (the denominator for daily returns) is the *peak
//!   concurrent open cost basis* seen while replaying the fills. Daily return =
//!   realized P&L on that UTC day / capital at risk, so Sharpe, Sortino,
//!   volatility and drawdown are unitless and independent of position size.
//!
//! Wallet fills carry no fees, and redemptions of resolved markets are not in
//! `wallet_trades`, so positions held to resolution stay "open" here.

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use polymarket_core::feature_extractor::fifo::{compute_fifo, ClosedSale, FifoSummary, Fill};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use statrs::statistics::{Data, Distribution};
use std::collections::{BTreeMap, HashMap};
use tracing::debug;

/// Maximum number of most-recent `wallet_trades` rows analysed per wallet.
///
/// Sufficient for accurate metrics while avoiding 11K+ row fetches that trigger
/// slow query warnings. The *newest* rows are always the ones kept.
const WALLET_TRADES_LIMIT: i64 = 2000;

/// Polymarket trades 24/7, so metrics are annualized with 365 daily periods.
const DAYS_PER_YEAR: f64 = 365.0;

/// Magnitude a Sharpe/Sortino ratio is capped at when stored.
///
/// A Sortino ratio is `+inf` when a wallet has no losing day. The `sortino_30d`
/// / `sharpe_30d` columns are `NOT NULL`, so rather than storing `0` (which
/// would rank the best wallets as the worst) the value is capped at this finite
/// ceiling. Consumers normalise these ratios against ~3, so 100 reads as "best".
pub const MAX_STORED_RATIO: f64 = 100.0;

/// Largest magnitude that fits `DECIMAL(20, 8)` (12 integer digits).
const MAX_DECIMAL_MAGNITUDE: f64 = 999_999_999_999.0;

/// Time period for metrics calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimePeriod {
    Day,
    Week,
    Month,
    Quarter,
    Year,
    AllTime,
}

impl TimePeriod {
    pub fn to_days(&self) -> Option<i64> {
        match self {
            TimePeriod::Day => Some(1),
            TimePeriod::Week => Some(7),
            TimePeriod::Month => Some(30),
            TimePeriod::Quarter => Some(90),
            TimePeriod::Year => Some(365),
            TimePeriod::AllTime => None,
        }
    }
}

/// Comprehensive wallet performance metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletMetrics {
    pub address: String,
    pub period: TimePeriod,

    // Return metrics
    pub total_return: Decimal,
    pub roi_percentage: f64,
    pub annualized_return: f64,

    // Risk metrics
    pub sharpe_ratio: f64,
    pub sortino_ratio: f64,
    pub max_drawdown: f64,
    pub max_drawdown_duration_days: i64,
    pub volatility: f64,
    pub downside_deviation: f64,

    // Trade metrics
    pub total_trades: u64,
    pub winning_trades: u64,
    pub losing_trades: u64,
    pub win_rate: f64,
    pub avg_win: Decimal,
    pub avg_loss: Decimal,
    pub profit_factor: f64,
    pub expectancy: Decimal,

    // Position metrics
    pub avg_position_size: Decimal,
    pub max_position_size: Decimal,
    pub avg_holding_period_hours: f64,

    // Consistency metrics
    pub consistency_score: f64,
    pub winning_streak: u32,
    pub losing_streak: u32,
    pub current_streak: i32,

    // Computed at timestamp
    pub computed_at: DateTime<Utc>,
}

impl WalletMetrics {
    /// Check if the wallet has good risk-adjusted returns.
    pub fn is_risk_adjusted_profitable(&self) -> bool {
        self.sharpe_ratio > 1.0 && self.roi_percentage > 0.0
    }

    /// Get a composite score (0-100) combining multiple factors.
    pub fn composite_score(&self) -> f64 {
        let roi_score = (self.roi_percentage * 100.0).clamp(0.0, 30.0);
        let sharpe_score = (self.sharpe_ratio * 10.0).clamp(0.0, 25.0);
        let win_rate_score = (self.win_rate * 25.0).clamp(0.0, 25.0);
        let consistency_score = (self.consistency_score * 20.0).clamp(0.0, 20.0);

        roi_score + sharpe_score + win_rate_score + consistency_score
    }
}

/// A single fill for profitability calculation.
///
/// `market_id` is the outcome-token id (`asset_id`); FIFO matching is per asset.
#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub timestamp: DateTime<Utc>,
    pub market_id: String,
    pub side: TradeSide,
    /// Fill price per share.
    pub entry_price: Decimal,
    pub exit_price: Option<Decimal>,
    pub quantity: Decimal,
    /// Not populated from `wallet_trades`: realized P&L is only defined after
    /// FIFO matching (see the module docs), never per raw fill.
    pub pnl: Option<Decimal>,
    pub fees: Decimal,
}

impl TradeRecord {
    fn to_fill(&self) -> Fill {
        Fill {
            timestamp: self.timestamp,
            asset_id: self.market_id.clone(),
            is_buy: self.side == TradeSide::Buy,
            price: self.entry_price,
            quantity: self.quantity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeSide {
    Buy,
    Sell,
}

/// Daily return record for time series analysis.
#[derive(Debug, Clone)]
struct DailyReturn {
    date: DateTime<Utc>,
    return_pct: f64,
    cumulative_value: f64,
}

/// Analyzer for calculating wallet profitability metrics.
pub struct ProfitabilityAnalyzer {
    pool: PgPool,
    /// Risk-free rate for Sharpe calculation (annualized).
    risk_free_rate: f64,
}

impl ProfitabilityAnalyzer {
    /// Create a new profitability analyzer.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            risk_free_rate: 0.05, // 5% default risk-free rate
        }
    }

    /// Set the risk-free rate for Sharpe ratio calculation.
    pub fn with_risk_free_rate(mut self, rate: f64) -> Self {
        self.risk_free_rate = rate;
        self
    }

    /// Calculate metrics for a wallet over a time period.
    ///
    /// Fetches the newest [`WALLET_TRADES_LIMIT`] fills inside the period and
    /// derives every metric from FIFO-realized results (see the module docs).
    pub async fn calculate_metrics(
        &self,
        address: &str,
        period: TimePeriod,
    ) -> Result<WalletMetrics> {
        let trades = self
            .fetch_trades(address, period, WALLET_TRADES_LIMIT)
            .await?;
        Ok(self.compute_metrics(address, period, &trades))
    }

    /// Compute metrics from fills sorted by ascending timestamp.
    ///
    /// Pure (no I/O) so it can be unit-tested with synthetic fills.
    fn compute_metrics(
        &self,
        address: &str,
        period: TimePeriod,
        trades: &[TradeRecord],
    ) -> WalletMetrics {
        let (Some(first_fill), Some(last_fill)) = (
            trades.iter().map(|t| t.timestamp).min(),
            trades.iter().map(|t| t.timestamp).max(),
        ) else {
            return self.empty_metrics(address, period);
        };

        let fills: Vec<Fill> = trades.iter().map(TradeRecord::to_fill).collect();
        let fifo = compute_fifo(&fills);
        let closed = &fifo.closed_sales;

        // Return metrics. ROI is realized P&L over the cost of the lots that were
        // actually closed; sells without a known basis never enter either side.
        let total_pnl = fifo.total_realized_pnl();
        let matched_cost = fifo.matched_cost_basis();
        let capital_at_risk = fifo.peak_open_cost_basis;
        let roi = if matched_cost > Decimal::ZERO {
            (total_pnl / matched_cost).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };
        let return_on_capital = if capital_at_risk > Decimal::ZERO {
            (total_pnl / capital_at_risk).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };
        // Fixed periods annualize over the period length; AllTime over the span
        // actually observed in the fills.
        let days = period
            .to_days()
            .map(|d| d as f64)
            .unwrap_or_else(|| ((last_fill - first_fill).num_seconds() as f64 / 86_400.0).max(1.0));
        let annualized = Self::annualize(return_on_capital, days);

        // Risk metrics from unitless daily returns (P&L / capital at risk). Prefer
        // daily returns for Sharpe when there are >= 2 daily observations,
        // otherwise fall back to per-closing-sale returns, which work even when
        // all activity is concentrated in one day.
        let daily_returns =
            self.calculate_daily_returns(&fifo, first_fill.date_naive(), capital_at_risk);
        let daily_sharpe = self.calculate_sharpe_ratio(&daily_returns);
        let sharpe = if daily_sharpe == 0.0 {
            let sale_returns: Vec<f64> = closed.iter().filter_map(|c| c.return_on_cost()).collect();
            self.calculate_trade_sharpe(&sale_returns)
        } else {
            daily_sharpe
        };
        let sortino = self.calculate_sortino_ratio(&daily_returns);
        let (max_dd, dd_duration) = self.calculate_max_drawdown(&daily_returns);
        let volatility = self.calculate_volatility(&daily_returns);
        let downside_dev = self.calculate_downside_deviation(&daily_returns);

        // Trade metrics: one result per closing sale.
        let win_count = fifo.winning_sales() as u64;
        let loss_count = fifo.losing_sales() as u64;
        let win_rate = fifo.win_rate().unwrap_or(0.0);

        let gross_win: Decimal = closed
            .iter()
            .filter(|c| c.realized_pnl > Decimal::ZERO)
            .map(|c| c.realized_pnl)
            .sum();
        let gross_loss: Decimal = closed
            .iter()
            .filter(|c| c.realized_pnl < Decimal::ZERO)
            .map(|c| -c.realized_pnl)
            .sum();
        let avg_win = if win_count > 0 {
            gross_win / Decimal::from(win_count)
        } else {
            Decimal::ZERO
        };
        let avg_loss = if loss_count > 0 {
            gross_loss / Decimal::from(loss_count)
        } else {
            Decimal::ZERO
        };
        let profit_factor = if gross_loss > Decimal::ZERO {
            (gross_win / gross_loss).to_f64().unwrap_or(0.0)
        } else if gross_win > Decimal::ZERO {
            f64::INFINITY
        } else {
            0.0
        };
        // Mean realized P&L per closing sale.
        let expectancy = if closed.is_empty() {
            Decimal::ZERO
        } else {
            total_pnl / Decimal::from(closed.len())
        };

        // Position metrics (notional of every fill).
        let position_sizes: Vec<Decimal> =
            trades.iter().map(|t| t.quantity * t.entry_price).collect();
        let avg_position = position_sizes.iter().sum::<Decimal>() / Decimal::from(trades.len());
        let max_position = position_sizes
            .iter()
            .max()
            .cloned()
            .unwrap_or(Decimal::ZERO);

        // Consistency and streaks over closing sales.
        let sale_pnls: Vec<f64> = closed
            .iter()
            .map(|c| c.realized_pnl.to_f64().unwrap_or(0.0))
            .collect();
        let consistency = self.calculate_consistency_score(&sale_pnls);
        let (win_streak, lose_streak, current) = self.calculate_streaks(closed);

        // Average holding period from BUY→SELL pairing per market
        let avg_hold_hours = Self::compute_avg_hold_time_hours(trades);

        WalletMetrics {
            address: address.to_string(),
            period,
            total_return: total_pnl,
            roi_percentage: roi,
            annualized_return: annualized,
            sharpe_ratio: sharpe,
            sortino_ratio: sortino,
            max_drawdown: max_dd,
            max_drawdown_duration_days: dd_duration,
            volatility,
            downside_deviation: downside_dev,
            // Number of fills analysed; win/loss counts below are closing sales.
            total_trades: trades.len() as u64,
            winning_trades: win_count,
            losing_trades: loss_count,
            win_rate,
            avg_win,
            avg_loss,
            profit_factor,
            expectancy,
            avg_position_size: avg_position,
            max_position_size: max_position,
            avg_holding_period_hours: avg_hold_hours,
            consistency_score: consistency,
            winning_streak: win_streak,
            losing_streak: lose_streak,
            current_streak: current,
            computed_at: Utc::now(),
        }
    }

    /// Compound a period return on capital to a yearly figure.
    ///
    /// A return of -100% or worse (growth factor <= 0) is a total loss: `-1.0`.
    /// The result is capped to what `DECIMAL(20, 8)` can hold.
    fn annualize(return_on_capital: f64, days: f64) -> f64 {
        let growth = 1.0 + return_on_capital;
        if growth <= 0.0 {
            return -1.0;
        }
        (growth.powf(DAYS_PER_YEAR / days.max(1.0)) - 1.0).min(MAX_DECIMAL_MAGNITUDE)
    }

    /// Calculate metrics for multiple wallets.
    pub async fn calculate_batch_metrics(
        &self,
        addresses: &[String],
        period: TimePeriod,
    ) -> Result<HashMap<String, WalletMetrics>> {
        let mut results = HashMap::new();

        for address in addresses {
            match self.calculate_metrics(address, period).await {
                Ok(metrics) => {
                    results.insert(address.clone(), metrics);
                }
                Err(e) => {
                    debug!(address = %address, error = %e, "Failed to calculate metrics");
                }
            }
        }

        Ok(results)
    }

    /// Saturate an `f64` into the range of a `DECIMAL(20, 8)` column.
    ///
    /// `NaN` (undefined) maps to `0.0`; `+/-inf` and out-of-range values saturate
    /// at `+/-999_999_999_999` so an infinite value is never stored as `0`.
    fn sanitize_for_decimal(value: f64) -> f64 {
        if value.is_nan() {
            return 0.0;
        }
        value.clamp(-MAX_DECIMAL_MAGNITUDE, MAX_DECIMAL_MAGNITUDE)
    }

    /// Prepare a Sharpe/Sortino ratio for a `NOT NULL` column.
    ///
    /// `NaN` (undefined) maps to `0.0`; `+inf` (e.g. Sortino with no losing day)
    /// and any value above the ceiling map to [`MAX_STORED_RATIO`], `-inf` to
    /// `-MAX_STORED_RATIO`.
    fn sanitize_ratio(value: f64) -> f64 {
        if value.is_nan() {
            return 0.0;
        }
        value.clamp(-MAX_STORED_RATIO, MAX_STORED_RATIO)
    }

    /// Store metrics in the database.
    ///
    /// Each [`WalletMetrics`] covers exactly one window, and only the columns of
    /// that window are written:
    ///
    /// * `Month`: the `*_30d` columns, `trades_30d` counters, `roi_30d`, the
    ///   headline `roi` (mirrors `roi_30d`), `annualized_return`,
    ///   `consistency_score`, `predicted_success_prob` and the freshness
    ///   timestamps.
    /// * `Quarter`: only `roi_90d`.
    /// * `AllTime`: only `roi_all_time`.
    /// * Other periods have no storage columns and return an error.
    ///
    /// For `Quarter`/`AllTime` the freshness timestamps are left untouched (a new
    /// row starts at the epoch) so a stale 30-day row is never made to look fresh.
    pub async fn store_metrics(&self, metrics: &WalletMetrics) -> Result<()> {
        let roi = Self::sanitize_for_decimal(metrics.roi_percentage);

        match metrics.period {
            TimePeriod::Month => {}
            TimePeriod::Quarter | TimePeriod::AllTime => {
                let column = if metrics.period == TimePeriod::Quarter {
                    "roi_90d"
                } else {
                    "roi_all_time"
                };
                let sql = format!(
                    r#"
                    INSERT INTO wallet_success_metrics (
                        address, wallet_address, {column}, last_computed, calculated_at
                    ) VALUES ($1, $1, $2, to_timestamp(0), to_timestamp(0))
                    ON CONFLICT (address) DO UPDATE SET
                        wallet_address = EXCLUDED.wallet_address,
                        {column} = EXCLUDED.{column}
                    "#
                );
                sqlx::query(&sql)
                    .bind(&metrics.address)
                    .bind(roi)
                    .execute(&self.pool)
                    .await?;
                return Ok(());
            }
            other => anyhow::bail!(
                "wallet_success_metrics has no columns for the {:?} window; \
                 only Month, Quarter and AllTime can be stored",
                other
            ),
        }

        let total_trades = i32::try_from(metrics.total_trades).unwrap_or(i32::MAX);
        let winning_trades = i32::try_from(metrics.winning_trades).unwrap_or(i32::MAX);
        let losing_trades = i32::try_from(metrics.losing_trades).unwrap_or(i32::MAX);

        let annualized = Self::sanitize_for_decimal(metrics.annualized_return);
        let sharpe = Self::sanitize_ratio(metrics.sharpe_ratio);
        let sortino = Self::sanitize_ratio(metrics.sortino_ratio);
        let max_drawdown = Self::sanitize_for_decimal(metrics.max_drawdown);
        let volatility = Self::sanitize_for_decimal(metrics.volatility);
        let consistency = Self::sanitize_for_decimal(metrics.consistency_score);
        let win_rate = Self::sanitize_for_decimal(metrics.win_rate);
        let composite = Self::sanitize_for_decimal(metrics.composite_score() / 100.0);

        sqlx::query(
            r#"
            INSERT INTO wallet_success_metrics (
                address, wallet_address, roi_30d, annualized_return,
                sharpe_30d, sortino_30d, max_drawdown_30d, volatility_30d, consistency_score,
                win_rate_30d, trades_30d, winning_trades_30d, losing_trades_30d,
                predicted_success_prob, last_computed, calculated_at, roi
            ) VALUES (
                $1, $1, $2, $3,
                $4, $5, $6, $7, $8,
                $9, $10, $11, $12,
                $13, $14, $14, $2
            )
            ON CONFLICT (address) DO UPDATE SET
                wallet_address = EXCLUDED.wallet_address,
                roi_30d = EXCLUDED.roi_30d,
                annualized_return = EXCLUDED.annualized_return,
                sharpe_30d = EXCLUDED.sharpe_30d,
                sortino_30d = EXCLUDED.sortino_30d,
                max_drawdown_30d = EXCLUDED.max_drawdown_30d,
                volatility_30d = EXCLUDED.volatility_30d,
                consistency_score = EXCLUDED.consistency_score,
                win_rate_30d = EXCLUDED.win_rate_30d,
                trades_30d = EXCLUDED.trades_30d,
                winning_trades_30d = EXCLUDED.winning_trades_30d,
                losing_trades_30d = EXCLUDED.losing_trades_30d,
                predicted_success_prob = EXCLUDED.predicted_success_prob,
                last_computed = EXCLUDED.last_computed,
                calculated_at = EXCLUDED.calculated_at,
                roi = EXCLUDED.roi
            "#,
        )
        .bind(&metrics.address)
        .bind(roi)
        .bind(annualized)
        .bind(sharpe)
        .bind(sortino)
        .bind(max_drawdown)
        .bind(volatility)
        .bind(consistency)
        .bind(win_rate)
        .bind(total_trades)
        .bind(winning_trades)
        .bind(losing_trades)
        .bind(composite)
        .bind(metrics.computed_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Compute the average holding period in hours by pairing BUY→SELL trades
    /// on the same market. Uses a FIFO (first-in-first-out) matching strategy:
    /// the earliest unmatched BUY for a given `market_id` is paired with the
    /// next SELL on that market. Unmatched trades (open positions) are excluded.
    ///
    /// Falls back to 24.0 hours if no matched pairs are found.
    fn compute_avg_hold_time_hours(trades: &[TradeRecord]) -> f64 {
        // Collect earliest buy timestamps per market (FIFO queue)
        let mut buy_queues: HashMap<String, Vec<DateTime<Utc>>> = HashMap::new();
        let mut hold_durations: Vec<f64> = Vec::new();

        for trade in trades {
            match trade.side {
                TradeSide::Buy => {
                    buy_queues
                        .entry(trade.market_id.clone())
                        .or_default()
                        .push(trade.timestamp);
                }
                TradeSide::Sell => {
                    if let Some(queue) = buy_queues.get_mut(&trade.market_id) {
                        if let Some(buy_time) = queue.first().copied() {
                            let hold_hours = trade
                                .timestamp
                                .signed_duration_since(buy_time)
                                .num_seconds()
                                .max(0) as f64
                                / 3600.0;
                            hold_durations.push(hold_hours);
                            queue.remove(0);
                        }
                    }
                }
            }
        }

        if hold_durations.is_empty() {
            24.0 // Fallback when no matched pairs
        } else {
            let sum: f64 = hold_durations.iter().sum();
            sum / hold_durations.len() as f64
        }
    }

    // Private calculation methods

    /// Fetch the **newest** `limit` fills of `address` inside `period`.
    ///
    /// The query orders newest-first so `LIMIT` keeps the most recent rows (the
    /// ones that matter for current performance); the result is then re-sorted
    /// oldest-first, which FIFO matching and the daily series require.
    async fn fetch_trades(
        &self,
        address: &str,
        period: TimePeriod,
        limit: i64,
    ) -> Result<Vec<TradeRecord>> {
        let cutoff = period.to_days().map(|d| Utc::now() - Duration::days(d));

        // copy_trade_history has been dropped; fetch directly from wallet_trades.
        let wallet_query = if let Some(cutoff_date) = cutoff {
            sqlx::query(
                r#"
                SELECT asset_id,
                       side,
                       price,
                       quantity,
                       timestamp
                FROM wallet_trades
                WHERE LOWER(wallet_address) = LOWER($1)
                  AND timestamp >= $2
                ORDER BY timestamp DESC
                LIMIT $3
                "#,
            )
            .bind(address)
            .bind(cutoff_date)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT asset_id,
                       side,
                       price,
                       quantity,
                       timestamp
                FROM wallet_trades
                WHERE LOWER(wallet_address) = LOWER($1)
                ORDER BY timestamp DESC
                LIMIT $2
                "#,
            )
            .bind(address)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        let mut trades: Vec<TradeRecord> = wallet_query
            .iter()
            .map(|row| {
                use sqlx::Row;
                let side_str: String = row.get("side");
                let side = if side_str.to_uppercase() == "BUY" {
                    TradeSide::Buy
                } else {
                    TradeSide::Sell
                };
                let price: Decimal = row.get("price");
                let quantity: Decimal = row.get("quantity");

                TradeRecord {
                    timestamp: row.get("timestamp"),
                    market_id: row.get("asset_id"),
                    side,
                    entry_price: price,
                    exit_price: None,
                    quantity,
                    // Realized P&L only exists once FIFO matching has paired a
                    // sell with earlier buys; it is not a per-row property.
                    pnl: None,
                    fees: Decimal::ZERO,
                }
            })
            .collect();

        // Stable sort: oldest first for chronological FIFO replay.
        trades.sort_by_key(|t| t.timestamp);

        Ok(trades)
    }

    /// Build the unitless daily return series from realized results.
    ///
    /// * One entry per UTC calendar day from the day of the first fill through
    ///   the day of the last closing sale; days without a closing sale carry a
    ///   `0.0` return (no P&L was realized).
    /// * `return_pct = realized P&L closed that day / capital_at_risk`, where
    ///   `capital_at_risk` is the peak concurrent open cost basis, so the series
    ///   does not depend on position size.
    /// * `cumulative_value` is the equity curve in units of capital at risk,
    ///   starting from `1.0`.
    ///
    /// Returns an empty series when nothing was closed or capital is zero.
    fn calculate_daily_returns(
        &self,
        fifo: &FifoSummary,
        first_day: NaiveDate,
        capital_at_risk: Decimal,
    ) -> Vec<DailyReturn> {
        let capital = capital_at_risk.to_f64().unwrap_or(0.0);
        let Some(last_sale) = fifo.closed_sales.last() else {
            return vec![];
        };
        if capital <= 0.0 {
            return vec![];
        }
        let last_day = last_sale.timestamp.date_naive();

        let mut pnl_by_day: BTreeMap<NaiveDate, f64> = BTreeMap::new();
        for sale in &fifo.closed_sales {
            *pnl_by_day.entry(sale.timestamp.date_naive()).or_insert(0.0) +=
                sale.realized_pnl.to_f64().unwrap_or(0.0);
        }

        let mut returns = vec![];
        let mut cumulative = 1.0_f64;
        let mut day = first_day;
        while day <= last_day {
            let return_pct = pnl_by_day.get(&day).copied().unwrap_or(0.0) / capital;
            cumulative += return_pct;
            returns.push(DailyReturn {
                date: day.and_time(NaiveTime::MIN).and_utc(),
                return_pct,
                cumulative_value: cumulative,
            });
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }

        returns
    }

    fn calculate_sharpe_ratio(&self, returns: &[DailyReturn]) -> f64 {
        if returns.len() < 2 {
            return 0.0;
        }

        let return_values: Vec<f64> = returns.iter().map(|r| r.return_pct).collect();
        let data = Data::new(return_values.clone());

        let mean_return = data.mean().unwrap_or(0.0);
        let std_dev = data.std_dev().unwrap_or(1.0);

        if std_dev == 0.0 {
            return 0.0;
        }

        // Annualize: 365 days (Polymarket trades 24/7, not stock market hours)
        let daily_rf = self.risk_free_rate / 365.0;
        let excess_return = mean_return - daily_rf;

        (excess_return / std_dev) * (365.0_f64).sqrt()
    }

    /// Calculate Sharpe ratio from per-closing-sale returns (`pnl / cost basis`).
    ///
    /// Uses individual sale returns instead of daily aggregation, making it meaningful
    /// even when all sales happen on 1-2 calendar days. Requires at least 5 sales
    /// for statistical significance.
    fn calculate_trade_sharpe(&self, trade_returns: &[f64]) -> f64 {
        if trade_returns.len() < 5 {
            return 0.0;
        }

        let data = Data::new(trade_returns.to_vec());
        let mean_return = data.mean().unwrap_or(0.0);
        let std_dev = data.std_dev().unwrap_or(1.0);

        if std_dev == 0.0 {
            return 0.0;
        }

        // Scale to annualized assuming ~2 trades/day average
        let trades_per_year = 365.0 * 2.0;
        let rf_per_trade = self.risk_free_rate / trades_per_year;
        let excess = mean_return - rf_per_trade;

        (excess / std_dev) * trades_per_year.sqrt()
    }

    fn calculate_sortino_ratio(&self, returns: &[DailyReturn]) -> f64 {
        if returns.len() < 2 {
            return 0.0;
        }

        let return_values: Vec<f64> = returns.iter().map(|r| r.return_pct).collect();
        let data = Data::new(return_values.clone());
        let mean_return = data.mean().unwrap_or(0.0);

        let downside_returns: Vec<f64> = return_values
            .iter()
            .filter(|&&r| r < 0.0)
            .map(|&r| r * r)
            .collect();

        if downside_returns.is_empty() {
            return f64::INFINITY;
        }

        let downside_dev =
            (downside_returns.iter().sum::<f64>() / downside_returns.len() as f64).sqrt();

        if downside_dev == 0.0 {
            return f64::INFINITY;
        }

        let daily_rf = self.risk_free_rate / 365.0;
        let excess_return = mean_return - daily_rf;

        (excess_return / downside_dev) * (365.0_f64).sqrt()
    }

    /// Maximum peak-to-trough decline of the equity curve and its longest duration
    /// in days.
    ///
    /// The curve is in units of capital at risk and starts at `1.0` *before* the
    /// first day, so a loss on the very first day is a drawdown. The decline is
    /// capped at `1.0` (100%): equity below zero means the capital at risk was
    /// wiped out. A drawdown still open at the end of the series counts towards
    /// the duration.
    fn calculate_max_drawdown(&self, returns: &[DailyReturn]) -> (f64, i64) {
        let Some(last) = returns.last() else {
            return (0.0, 0);
        };

        let mut peak = 1.0_f64;
        let mut max_drawdown: f64 = 0.0;
        let mut max_dd_duration = 0i64;
        let mut current_dd_start: Option<DateTime<Utc>> = None;

        for ret in returns {
            if ret.cumulative_value >= peak {
                peak = ret.cumulative_value;
                if let Some(start) = current_dd_start.take() {
                    let duration = (ret.date - start).num_days();
                    max_dd_duration = max_dd_duration.max(duration);
                }
            } else {
                let drawdown = ((peak - ret.cumulative_value) / peak).min(1.0);
                max_drawdown = max_drawdown.max(drawdown);

                if current_dd_start.is_none() {
                    current_dd_start = Some(ret.date);
                }
            }
        }

        if let Some(start) = current_dd_start {
            max_dd_duration = max_dd_duration.max((last.date - start).num_days());
        }

        (max_drawdown, max_dd_duration)
    }

    fn calculate_volatility(&self, returns: &[DailyReturn]) -> f64 {
        if returns.len() < 2 {
            return 0.0;
        }

        let return_values: Vec<f64> = returns.iter().map(|r| r.return_pct).collect();
        let data = Data::new(return_values);

        // Annualized volatility
        data.std_dev().unwrap_or(0.0) * (365.0_f64).sqrt()
    }

    fn calculate_downside_deviation(&self, returns: &[DailyReturn]) -> f64 {
        let negative_returns: Vec<f64> = returns
            .iter()
            .filter(|r| r.return_pct < 0.0)
            .map(|r| r.return_pct * r.return_pct)
            .collect();

        if negative_returns.is_empty() {
            return 0.0;
        }

        (negative_returns.iter().sum::<f64>() / negative_returns.len() as f64).sqrt()
            * (365.0_f64).sqrt()
    }

    fn calculate_consistency_score(&self, pnls: &[f64]) -> f64 {
        if pnls.len() < 5 {
            return 0.0;
        }

        // Consistency = percentage of profitable periods / volatility of returns
        let profitable = pnls.iter().filter(|&&p| p > 0.0).count() as f64;
        let total = pnls.len() as f64;
        let profit_rate = profitable / total;

        let data = Data::new(pnls.to_vec());
        let cv = if let (Some(mean), Some(std)) = (data.mean(), data.std_dev()) {
            if mean != 0.0 {
                (std / mean.abs()).min(2.0)
            } else {
                1.0
            }
        } else {
            1.0
        };

        // Higher profit rate and lower CV = higher consistency
        profit_rate / (1.0 + cv)
    }

    /// Winning/losing streaks over closing sales in chronological order.
    ///
    /// Returns `(longest win streak, longest loss streak, current streak)` where
    /// the current streak is positive for wins and negative for losses. A
    /// breakeven sale ends both streaks (current streak `0`).
    fn calculate_streaks(&self, closed: &[ClosedSale]) -> (u32, u32, i32) {
        let mut max_win_streak = 0u32;
        let mut max_lose_streak = 0u32;
        let mut current_streak = 0i32;
        let mut current_win = 0u32;
        let mut current_lose = 0u32;

        for sale in closed {
            if sale.realized_pnl > Decimal::ZERO {
                current_win += 1;
                current_lose = 0;
                current_streak = current_win as i32;
                max_win_streak = max_win_streak.max(current_win);
            } else if sale.realized_pnl < Decimal::ZERO {
                current_lose += 1;
                current_win = 0;
                current_streak = -(current_lose as i32);
                max_lose_streak = max_lose_streak.max(current_lose);
            } else {
                current_win = 0;
                current_lose = 0;
                current_streak = 0;
            }
        }

        (max_win_streak, max_lose_streak, current_streak)
    }

    fn empty_metrics(&self, address: &str, period: TimePeriod) -> WalletMetrics {
        WalletMetrics {
            address: address.to_string(),
            period,
            total_return: Decimal::ZERO,
            roi_percentage: 0.0,
            annualized_return: 0.0,
            sharpe_ratio: 0.0,
            sortino_ratio: 0.0,
            max_drawdown: 0.0,
            max_drawdown_duration_days: 0,
            volatility: 0.0,
            downside_deviation: 0.0,
            total_trades: 0,
            winning_trades: 0,
            losing_trades: 0,
            win_rate: 0.0,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: 0.0,
            expectancy: Decimal::ZERO,
            avg_position_size: Decimal::ZERO,
            max_position_size: Decimal::ZERO,
            avg_holding_period_hours: 0.0,
            consistency_score: 0.0,
            winning_streak: 0,
            losing_streak: 0,
            current_streak: 0,
            computed_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    /// Create a test analyzer with a lazy (never-connected) pool.
    /// The pool is never actually queried in these pure-calc unit tests.
    fn test_analyzer() -> ProfitabilityAnalyzer {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://fake:fake@localhost/fake")
            .expect("lazy pool");
        ProfitabilityAnalyzer::new(pool)
    }

    fn make_sale(pnl: f64, day_offset: i64) -> ClosedSale {
        let pnl = Decimal::from_f64_retain(pnl).unwrap_or(Decimal::ZERO);
        ClosedSale {
            timestamp: Utc::now() - Duration::days(day_offset),
            asset_id: "market-1".to_string(),
            matched_quantity: Decimal::new(100, 0),
            unmatched_quantity: Decimal::ZERO,
            cost_basis: Decimal::new(50, 0),
            proceeds: Decimal::new(50, 0) + pnl,
            realized_pnl: pnl,
        }
    }

    #[test]
    fn test_time_period_to_days() {
        assert_eq!(TimePeriod::Day.to_days(), Some(1));
        assert_eq!(TimePeriod::Week.to_days(), Some(7));
        assert_eq!(TimePeriod::Month.to_days(), Some(30));
        assert_eq!(TimePeriod::Quarter.to_days(), Some(90));
        assert_eq!(TimePeriod::Year.to_days(), Some(365));
        assert_eq!(TimePeriod::AllTime.to_days(), None);
    }

    #[test]
    fn test_composite_score() {
        let metrics = WalletMetrics {
            address: "0x1234".to_string(),
            period: TimePeriod::Month,
            total_return: Decimal::new(1000, 0),
            roi_percentage: 0.20, // 20% ROI -> 20 points (capped at 30)
            annualized_return: 0.0,
            sharpe_ratio: 2.0, // 2.0 Sharpe -> 20 points (capped at 25)
            sortino_ratio: 0.0,
            max_drawdown: 0.1,
            max_drawdown_duration_days: 5,
            volatility: 0.2,
            downside_deviation: 0.0,
            total_trades: 100,
            winning_trades: 60,
            losing_trades: 40,
            win_rate: 0.60, // 60% win rate -> 15 points
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: 1.5,
            expectancy: Decimal::ZERO,
            avg_position_size: Decimal::ZERO,
            max_position_size: Decimal::ZERO,
            avg_holding_period_hours: 0.0,
            consistency_score: 0.70, // 70% consistency -> 14 points
            winning_streak: 5,
            losing_streak: 3,
            current_streak: 2,
            computed_at: Utc::now(),
        };

        let score = metrics.composite_score();
        // 20 + 20 + 15 + 14 = 69
        assert!(score > 65.0 && score < 75.0);
    }

    #[test]
    fn test_is_risk_adjusted_profitable() {
        let mut metrics = WalletMetrics {
            address: "0x1234".to_string(),
            period: TimePeriod::Month,
            total_return: Decimal::ZERO,
            roi_percentage: 0.10,
            annualized_return: 0.0,
            sharpe_ratio: 1.5,
            sortino_ratio: 0.0,
            max_drawdown: 0.0,
            max_drawdown_duration_days: 0,
            volatility: 0.0,
            downside_deviation: 0.0,
            total_trades: 0,
            winning_trades: 0,
            losing_trades: 0,
            win_rate: 0.0,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: 0.0,
            expectancy: Decimal::ZERO,
            avg_position_size: Decimal::ZERO,
            max_position_size: Decimal::ZERO,
            avg_holding_period_hours: 0.0,
            consistency_score: 0.0,
            winning_streak: 0,
            losing_streak: 0,
            current_streak: 0,
            computed_at: Utc::now(),
        };

        assert!(metrics.is_risk_adjusted_profitable());

        metrics.sharpe_ratio = 0.5;
        assert!(!metrics.is_risk_adjusted_profitable());
    }

    #[tokio::test]
    async fn test_sharpe_ratio_basic() {
        let analyzer = test_analyzer();
        // Create returns with known mean and std dev
        let returns = vec![
            DailyReturn {
                date: Utc::now() - Duration::days(5),
                return_pct: 0.02,
                cumulative_value: 1.02,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(4),
                return_pct: -0.01,
                cumulative_value: 1.01,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(3),
                return_pct: 0.03,
                cumulative_value: 1.04,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(2),
                return_pct: 0.01,
                cumulative_value: 1.05,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(1),
                return_pct: 0.015,
                cumulative_value: 1.065,
            },
        ];

        let sharpe = analyzer.calculate_sharpe_ratio(&returns);
        // Positive returns should produce a positive Sharpe ratio
        assert!(
            sharpe > 0.0,
            "Sharpe should be positive for net-positive returns, got {}",
            sharpe
        );
    }

    #[tokio::test]
    async fn test_sharpe_ratio_zero_volatility() {
        let analyzer = test_analyzer();
        // All same returns => zero standard deviation => Sharpe = 0
        let returns = vec![
            DailyReturn {
                date: Utc::now() - Duration::days(3),
                return_pct: 0.01,
                cumulative_value: 1.01,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(2),
                return_pct: 0.01,
                cumulative_value: 1.02,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(1),
                return_pct: 0.01,
                cumulative_value: 1.03,
            },
        ];

        let sharpe = analyzer.calculate_sharpe_ratio(&returns);
        assert_eq!(sharpe, 0.0, "Sharpe should be 0 for zero volatility");
    }

    #[tokio::test]
    async fn test_sharpe_ratio_insufficient_data() {
        let analyzer = test_analyzer();
        // Less than 2 returns => 0.0
        let returns = vec![DailyReturn {
            date: Utc::now(),
            return_pct: 0.05,
            cumulative_value: 1.05,
        }];

        assert_eq!(analyzer.calculate_sharpe_ratio(&returns), 0.0);
        assert_eq!(analyzer.calculate_sharpe_ratio(&[]), 0.0);
    }

    #[tokio::test]
    async fn test_sortino_ratio_no_downside() {
        let analyzer = test_analyzer();
        // Only positive returns => no downside deviation => infinity
        let returns = vec![
            DailyReturn {
                date: Utc::now() - Duration::days(3),
                return_pct: 0.02,
                cumulative_value: 1.02,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(2),
                return_pct: 0.03,
                cumulative_value: 1.05,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(1),
                return_pct: 0.01,
                cumulative_value: 1.06,
            },
        ];

        let sortino = analyzer.calculate_sortino_ratio(&returns);
        assert!(
            sortino.is_infinite() && sortino > 0.0,
            "Sortino should be +infinity with no downside"
        );
    }

    #[tokio::test]
    async fn test_max_drawdown_monotonic_up() {
        let analyzer = test_analyzer();
        // Always rising => drawdown should be 0
        let returns = vec![
            DailyReturn {
                date: Utc::now() - Duration::days(3),
                return_pct: 0.01,
                cumulative_value: 1.01,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(2),
                return_pct: 0.02,
                cumulative_value: 1.03,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(1),
                return_pct: 0.01,
                cumulative_value: 1.04,
            },
        ];

        let (max_dd, _dd_duration) = analyzer.calculate_max_drawdown(&returns);
        assert_eq!(
            max_dd, 0.0,
            "No drawdown for monotonically increasing values"
        );
    }

    #[tokio::test]
    async fn test_max_drawdown_known_case() {
        let analyzer = test_analyzer();
        // Peak at 1.10, trough at 0.88 => drawdown = (1.10 - 0.88) / 1.10 = 0.2
        let returns = vec![
            DailyReturn {
                date: Utc::now() - Duration::days(4),
                return_pct: 0.10,
                cumulative_value: 1.10,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(3),
                return_pct: -0.10,
                cumulative_value: 1.00,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(2),
                return_pct: -0.12,
                cumulative_value: 0.88,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(1),
                return_pct: 0.25,
                cumulative_value: 1.13,
            },
        ];

        let (max_dd, _dd_duration) = analyzer.calculate_max_drawdown(&returns);
        assert!(
            (max_dd - 0.2).abs() < 0.001,
            "Expected ~20% drawdown, got {}",
            max_dd
        );
    }

    #[tokio::test]
    async fn test_volatility_constant_returns() {
        let analyzer = test_analyzer();
        // Constant returns => zero std dev => zero volatility
        let returns = vec![
            DailyReturn {
                date: Utc::now() - Duration::days(3),
                return_pct: 0.01,
                cumulative_value: 1.01,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(2),
                return_pct: 0.01,
                cumulative_value: 1.02,
            },
            DailyReturn {
                date: Utc::now() - Duration::days(1),
                return_pct: 0.01,
                cumulative_value: 1.03,
            },
        ];

        let vol = analyzer.calculate_volatility(&returns);
        assert_eq!(vol, 0.0, "Zero volatility for constant returns");
    }

    #[tokio::test]
    async fn test_calculate_streaks() {
        let analyzer = test_analyzer();

        // Win, Win, Loss, Win, Loss, Loss, Loss
        let sales = vec![
            make_sale(10.0, 7),
            make_sale(5.0, 6),
            make_sale(-3.0, 5),
            make_sale(8.0, 4),
            make_sale(-2.0, 3),
            make_sale(-4.0, 2),
            make_sale(-1.0, 1),
        ];

        let (win_streak, lose_streak, current) = analyzer.calculate_streaks(&sales);
        assert_eq!(win_streak, 2, "Max winning streak should be 2");
        assert_eq!(lose_streak, 3, "Max losing streak should be 3");
        assert_eq!(current, -3, "Current streak should be -3 (3 losses)");
    }

    #[tokio::test]
    async fn test_calculate_streaks_breakeven_ends_streaks() {
        let analyzer = test_analyzer();
        let sales = vec![make_sale(1.0, 3), make_sale(0.0, 2), make_sale(-1.0, 1)];
        let (win_streak, lose_streak, current) = analyzer.calculate_streaks(&sales);
        assert_eq!((win_streak, lose_streak, current), (1, 1, -1));

        let sales = vec![make_sale(1.0, 2), make_sale(0.0, 1)];
        assert_eq!(analyzer.calculate_streaks(&sales), (1, 0, 0));
    }

    #[tokio::test]
    async fn test_consistency_score_few_trades() {
        let analyzer = test_analyzer();

        // Less than 5 PnLs => 0.0
        let pnls = vec![1.0, 2.0, -1.0];
        assert_eq!(analyzer.calculate_consistency_score(&pnls), 0.0);
    }

    #[test]
    fn test_sanitize_for_decimal() {
        // Normal values pass through
        assert_eq!(ProfitabilityAnalyzer::sanitize_for_decimal(1.5), 1.5);
        assert_eq!(ProfitabilityAnalyzer::sanitize_for_decimal(-0.5), -0.5);

        // NaN is undefined -> 0
        assert_eq!(ProfitabilityAnalyzer::sanitize_for_decimal(f64::NAN), 0.0);

        // Infinity saturates, it is never stored as 0
        assert_eq!(
            ProfitabilityAnalyzer::sanitize_for_decimal(f64::INFINITY),
            999_999_999_999.0
        );
        assert_eq!(
            ProfitabilityAnalyzer::sanitize_for_decimal(f64::NEG_INFINITY),
            -999_999_999_999.0
        );

        // Extreme values are clamped
        assert_eq!(
            ProfitabilityAnalyzer::sanitize_for_decimal(1e15),
            999_999_999_999.0
        );
        assert_eq!(
            ProfitabilityAnalyzer::sanitize_for_decimal(-1e15),
            -999_999_999_999.0
        );
    }

    #[test]
    fn test_sanitize_ratio_never_turns_infinity_into_zero() {
        assert_eq!(ProfitabilityAnalyzer::sanitize_ratio(1.7), 1.7);
        assert_eq!(ProfitabilityAnalyzer::sanitize_ratio(f64::NAN), 0.0);
        assert_eq!(
            ProfitabilityAnalyzer::sanitize_ratio(f64::INFINITY),
            MAX_STORED_RATIO
        );
        assert_eq!(
            ProfitabilityAnalyzer::sanitize_ratio(f64::NEG_INFINITY),
            -MAX_STORED_RATIO
        );
        assert_eq!(ProfitabilityAnalyzer::sanitize_ratio(1e9), MAX_STORED_RATIO);
    }

    #[test]
    fn test_composite_score_edge_cases() {
        // All zeros
        let metrics = WalletMetrics {
            address: "0x0".to_string(),
            period: TimePeriod::Month,
            total_return: Decimal::ZERO,
            roi_percentage: 0.0,
            annualized_return: 0.0,
            sharpe_ratio: 0.0,
            sortino_ratio: 0.0,
            max_drawdown: 0.0,
            max_drawdown_duration_days: 0,
            volatility: 0.0,
            downside_deviation: 0.0,
            total_trades: 0,
            winning_trades: 0,
            losing_trades: 0,
            win_rate: 0.0,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: 0.0,
            expectancy: Decimal::ZERO,
            avg_position_size: Decimal::ZERO,
            max_position_size: Decimal::ZERO,
            avg_holding_period_hours: 0.0,
            consistency_score: 0.0,
            winning_streak: 0,
            losing_streak: 0,
            current_streak: 0,
            computed_at: Utc::now(),
        };
        assert_eq!(metrics.composite_score(), 0.0);

        // All maxed out
        let maxed = WalletMetrics {
            roi_percentage: 10.0,   // 10 * 100 = 1000, capped at 30
            sharpe_ratio: 5.0,      // 5 * 10 = 50, capped at 25
            win_rate: 1.0,          // 1.0 * 25 = 25, capped at 25
            consistency_score: 1.0, // 1.0 * 20 = 20, capped at 20
            ..metrics.clone()
        };
        assert_eq!(maxed.composite_score(), 100.0);

        // Negative ROI should be clamped at 0
        let negative = WalletMetrics {
            roi_percentage: -0.50,
            ..metrics
        };
        assert_eq!(negative.composite_score(), 0.0);
    }
}

/// Regression tests for realized-P&L based metrics (FIFO), the daily series and
/// the storage mapping. Pure tests use synthetic fills with fixed timestamps.
#[cfg(test)]
mod realized_pnl_tests {
    use super::*;
    use chrono::TimeZone;
    use sqlx::postgres::PgPoolOptions;

    fn analyzer() -> ProfitabilityAnalyzer {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://fake:fake@localhost/fake")
            .expect("lazy pool");
        ProfitabilityAnalyzer::new(pool)
    }

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap()
    }

    fn rec(day: i64, asset: &str, side: TradeSide, price: &str, qty: &str) -> TradeRecord {
        TradeRecord {
            timestamp: t0() + Duration::days(day),
            market_id: asset.to_string(),
            side,
            entry_price: price.parse().unwrap(),
            exit_price: None,
            quantity: qty.parse().unwrap(),
            pnl: None,
            fees: Decimal::ZERO,
        }
    }

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn metrics_for(trades: &[TradeRecord]) -> WalletMetrics {
        analyzer().compute_metrics("0xabc", TimePeriod::Month, trades)
    }

    /// Wins and losses over seven days across three assets.
    /// Realized sales: +5, -2.5, +12, -10. Peak concurrent open cost basis: 70.
    fn mixed_history(scale: &str) -> Vec<TradeRecord> {
        let q = |n: &str| -> String {
            (n.parse::<Decimal>().unwrap() * scale.parse::<Decimal>().unwrap()).to_string()
        };
        vec![
            rec(0, "A", TradeSide::Buy, "0.50", &q("100")),
            rec(1, "A", TradeSide::Sell, "0.60", &q("50")),
            rec(2, "A", TradeSide::Sell, "0.45", &q("50")),
            rec(3, "B", TradeSide::Buy, "0.30", &q("200")),
            rec(4, "B", TradeSide::Sell, "0.36", &q("200")),
            rec(5, "C", TradeSide::Buy, "0.70", &q("100")),
            rec(6, "C", TradeSide::Sell, "0.60", &q("100")),
        ]
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[tokio::test]
    async fn buy_and_hold_is_not_a_loss() {
        // The old cash-flow model reported -40 here.
        let m = metrics_for(&[rec(0, "A", TradeSide::Buy, "0.40", "100")]);
        assert_eq!(m.total_return, Decimal::ZERO);
        assert_eq!(m.roi_percentage, 0.0);
        assert_eq!(m.win_rate, 0.0);
        assert_eq!((m.winning_trades, m.losing_trades), (0, 0));
        assert_eq!(m.total_trades, 1);
        assert_eq!(
            (m.sharpe_ratio, m.sortino_ratio, m.max_drawdown),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(m.avg_position_size, d("40"));
    }

    #[tokio::test]
    async fn round_trip_win() {
        let m = metrics_for(&[
            rec(0, "A", TradeSide::Buy, "0.40", "100"),
            rec(1, "A", TradeSide::Sell, "0.60", "100"),
        ]);
        assert_eq!(m.total_return, d("20"));
        assert!(approx(m.roi_percentage, 0.5));
        assert_eq!(m.win_rate, 1.0);
        assert_eq!((m.winning_trades, m.losing_trades), (1, 0));
        assert_eq!(m.total_trades, 2);
        assert_eq!(m.avg_win, d("20"));
        assert_eq!(m.avg_loss, Decimal::ZERO);
        assert_eq!(m.expectancy, d("20"));
        assert!(m.profit_factor.is_infinite() && m.profit_factor > 0.0);
        assert_eq!(m.current_streak, 1);
        assert_eq!(m.max_drawdown, 0.0);
        // Daily returns are [0, +0.5]: no downside day -> Sortino is +inf.
        assert!(m.sortino_ratio.is_infinite() && m.sortino_ratio > 0.0);
    }

    #[tokio::test]
    async fn round_trip_loss() {
        let m = metrics_for(&[
            rec(0, "A", TradeSide::Buy, "0.60", "100"),
            rec(1, "A", TradeSide::Sell, "0.45", "100"),
        ]);
        assert_eq!(m.total_return, d("-15"));
        assert!(approx(m.roi_percentage, -0.25));
        assert_eq!(m.win_rate, 0.0);
        assert_eq!((m.winning_trades, m.losing_trades), (0, 1));
        assert_eq!(m.avg_loss, d("15"));
        assert_eq!(m.profit_factor, 0.0);
        assert_eq!(m.current_streak, -1);
        // Capital at risk 60; the day-1 loss of 15 is a 25% drawdown.
        assert!(approx(m.max_drawdown, 0.25), "got {}", m.max_drawdown);
        assert!(approx(
            m.annualized_return,
            0.75_f64.powf(365.0 / 30.0) - 1.0
        ));
    }

    #[tokio::test]
    async fn partial_sells_use_fifo_lots() {
        let m = metrics_for(&[
            rec(0, "A", TradeSide::Buy, "0.40", "100"),
            rec(1, "A", TradeSide::Buy, "0.50", "100"),
            rec(2, "A", TradeSide::Sell, "0.60", "150"), // +25 on cost 65
            rec(3, "A", TradeSide::Sell, "0.45", "50"),  // -2.5 on cost 25
        ]);
        assert_eq!(m.total_return, d("22.5"));
        assert!(approx(m.roi_percentage, 22.5 / 90.0));
        assert_eq!((m.winning_trades, m.losing_trades), (1, 1));
        assert!(approx(m.win_rate, 0.5));
        assert_eq!(m.avg_win, d("25"));
        assert_eq!(m.avg_loss, d("2.5"));
        assert_eq!(m.expectancy, d("11.25"));
        assert!(approx(m.profit_factor, 10.0));
        assert_eq!(m.total_trades, 4);
        assert_eq!(
            (m.winning_streak, m.losing_streak, m.current_streak),
            (1, 1, -1)
        );
    }

    #[tokio::test]
    async fn sells_without_basis_are_ignored() {
        // The old model booked +90 of "profit" for this lone sell.
        let m = metrics_for(&[rec(0, "A", TradeSide::Sell, "0.90", "100")]);
        assert_eq!(m.total_return, Decimal::ZERO);
        assert_eq!(m.roi_percentage, 0.0);
        assert_eq!(m.win_rate, 0.0);
        assert_eq!((m.winning_trades, m.losing_trades), (0, 0));
        assert_eq!(m.total_trades, 1);

        // Selling a different outcome token than the one bought realizes nothing.
        let m = metrics_for(&[
            rec(0, "YES", TradeSide::Buy, "0.40", "100"),
            rec(1, "NO", TradeSide::Sell, "0.90", "100"),
        ]);
        assert_eq!(m.total_return, Decimal::ZERO);
        assert_eq!((m.winning_trades, m.losing_trades), (0, 0));

        // A sell larger than the known holdings only counts the matched part.
        let m = metrics_for(&[
            rec(0, "A", TradeSide::Buy, "0.50", "40"),
            rec(1, "A", TradeSide::Sell, "0.70", "100"),
        ]);
        assert_eq!(m.total_return, d("8"));
        assert!(approx(m.roi_percentage, 0.4));
    }

    #[tokio::test]
    async fn ratios_do_not_depend_on_position_size() {
        let small = metrics_for(&mixed_history("1"));
        let large = metrics_for(&mixed_history("1000"));

        assert_eq!(small.total_return, d("4.5"));
        assert_eq!(large.total_return, d("4500"));
        assert!(small.sharpe_ratio != 0.0 && small.sortino_ratio.is_finite());
        for (a, b) in [
            (small.roi_percentage, large.roi_percentage),
            (small.sharpe_ratio, large.sharpe_ratio),
            (small.sortino_ratio, large.sortino_ratio),
            (small.volatility, large.volatility),
            (small.downside_deviation, large.downside_deviation),
            (small.max_drawdown, large.max_drawdown),
            (small.annualized_return, large.annualized_return),
            (small.win_rate, large.win_rate),
        ] {
            assert!(approx(a, b), "{a} != {b}");
        }
        assert!(small.sharpe_ratio.abs() < MAX_STORED_RATIO);
    }

    #[tokio::test]
    async fn daily_returns_are_pnl_over_peak_capital_with_flat_days() {
        let a = analyzer();
        let trades = mixed_history("1");
        let fills: Vec<Fill> = trades.iter().map(TradeRecord::to_fill).collect();
        let fifo = compute_fifo(&fills);
        assert_eq!(fifo.peak_open_cost_basis, d("70"));

        let series = a.calculate_daily_returns(&fifo, trades[0].timestamp.date_naive(), d("70"));
        let returns: Vec<f64> = series.iter().map(|r| r.return_pct).collect();
        let expected = [
            0.0,
            5.0 / 70.0,
            -2.5 / 70.0,
            0.0,
            12.0 / 70.0,
            0.0,
            -10.0 / 70.0,
        ];
        assert_eq!(returns.len(), expected.len());
        for (got, want) in returns.iter().zip(expected) {
            assert!(approx(*got, want), "{got} != {want}");
        }
        let last = series.last().unwrap();
        assert!(approx(last.cumulative_value, 1.0 + 4.5 / 70.0));
        // One entry per consecutive UTC day with real dates (not "now").
        assert_eq!(series[0].date.date_naive(), t0().date_naive());
        assert_eq!(
            (series[6].date - series[0].date).num_days(),
            6,
            "dates must advance one calendar day per entry"
        );
    }

    #[tokio::test]
    async fn daily_returns_empty_without_closings() {
        let a = analyzer();
        let fills = [rec(0, "A", TradeSide::Buy, "0.5", "10").to_fill()];
        let fifo = compute_fifo(&fills);
        assert!(a
            .calculate_daily_returns(&fifo, t0().date_naive(), d("5"))
            .is_empty());
    }

    #[tokio::test]
    async fn same_day_sales_fall_back_to_per_sale_sharpe() {
        // Six round trips on one day -> one daily observation, so the daily
        // Sharpe is undefined and the per-sale series (>= 5 sales) is used.
        let mut trades = vec![];
        for (i, sell) in ["0.60", "0.45", "0.65", "0.48", "0.70", "0.40"]
            .into_iter()
            .enumerate()
        {
            let mut buy = rec(0, &format!("M{i}"), TradeSide::Buy, "0.50", "100");
            buy.timestamp += Duration::minutes(2 * i as i64);
            let mut s = rec(0, &format!("M{i}"), TradeSide::Sell, sell, "100");
            s.timestamp += Duration::minutes(2 * i as i64 + 1);
            trades.push(buy);
            trades.push(s);
        }
        let m = metrics_for(&trades);
        assert_eq!((m.winning_trades, m.losing_trades), (3, 3));
        assert!(m.sharpe_ratio != 0.0, "per-sale fallback should apply");
    }

    #[tokio::test]
    async fn empty_fills_give_empty_metrics() {
        let m = metrics_for(&[]);
        assert_eq!(m.total_trades, 0);
        assert_eq!(m.total_return, Decimal::ZERO);
    }

    #[tokio::test]
    async fn drawdown_counts_first_day_loss_and_open_drawdown() {
        let a = analyzer();
        let day = |n: i64, cum: f64| DailyReturn {
            date: t0() + Duration::days(n),
            return_pct: 0.0,
            cumulative_value: cum,
        };
        // Starts at 1.0, drops to 0.9 on day 0 and never recovers by day 3.
        let (dd, dur) = a.calculate_max_drawdown(&[day(0, 0.9), day(1, 0.95), day(3, 0.97)]);
        assert!(approx(dd, 0.1), "got {dd}");
        assert_eq!(
            dur, 3,
            "an unrecovered drawdown counts towards the duration"
        );

        // Flat days at the peak do not start a drawdown.
        let (dd, dur) = a.calculate_max_drawdown(&[day(0, 1.1), day(1, 1.1), day(2, 1.1)]);
        assert_eq!((dd, dur), (0.0, 0));

        // Equity below zero is a wipe-out: capped at 100%.
        let (dd, _) = a.calculate_max_drawdown(&[day(0, -0.5)]);
        assert_eq!(dd, 1.0);
    }

    #[test]
    fn annualize_handles_total_loss_and_overflow() {
        assert!(approx(
            ProfitabilityAnalyzer::annualize(0.5, 30.0),
            1.5_f64.powf(365.0 / 30.0) - 1.0
        ));
        assert_eq!(ProfitabilityAnalyzer::annualize(-1.0, 30.0), -1.0);
        assert_eq!(ProfitabilityAnalyzer::annualize(-2.5, 30.0), -1.0);
        let huge = ProfitabilityAnalyzer::annualize(1e6, 1.0);
        assert!(huge.is_finite());
        assert_eq!(huge, MAX_DECIMAL_MAGNITUDE);
    }

    // ---------------------------------------------------------------------
    // Database-backed tests
    // ---------------------------------------------------------------------

    async fn create_tables(pool: &PgPool) {
        sqlx::raw_sql(
            r#"
            CREATE TABLE wallet_trades (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                transaction_hash VARCHAR NOT NULL UNIQUE,
                wallet_address VARCHAR NOT NULL,
                asset_id VARCHAR NOT NULL,
                condition_id VARCHAR,
                side VARCHAR NOT NULL CHECK (side IN ('BUY', 'SELL')),
                price DECIMAL(20, 10) NOT NULL,
                quantity DECIMAL(20, 10) NOT NULL,
                value DECIMAL(20, 10) NOT NULL,
                timestamp TIMESTAMPTZ NOT NULL,
                outcome TEXT
            );
            CREATE TABLE wallet_success_metrics (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                address VARCHAR(42) NOT NULL UNIQUE,
                roi_30d DECIMAL(20, 8) NOT NULL DEFAULT 0,
                roi_90d DECIMAL(20, 8) NOT NULL DEFAULT 0,
                roi_all_time DECIMAL(20, 8) NOT NULL DEFAULT 0,
                annualized_return DECIMAL(20, 8) NOT NULL DEFAULT 0,
                sharpe_30d DECIMAL(20, 8) NOT NULL DEFAULT 0,
                sortino_30d DECIMAL(20, 8) NOT NULL DEFAULT 0,
                max_drawdown_30d DECIMAL(20, 8) NOT NULL DEFAULT 0,
                volatility_30d DECIMAL(20, 8) NOT NULL DEFAULT 0,
                consistency_score DECIMAL(5, 4) NOT NULL DEFAULT 0,
                win_rate_30d DECIMAL(5, 4) NOT NULL DEFAULT 0,
                profit_factor DECIMAL(10, 4) NOT NULL DEFAULT 0,
                trades_30d INTEGER NOT NULL DEFAULT 0,
                winning_trades_30d INTEGER NOT NULL DEFAULT 0,
                losing_trades_30d INTEGER NOT NULL DEFAULT 0,
                predicted_success_prob DECIMAL(5, 4) NOT NULL DEFAULT 0,
                prediction_confidence DECIMAL(5, 4) NOT NULL DEFAULT 0,
                prediction_category SMALLINT NOT NULL DEFAULT 3,
                last_computed TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                wallet_address VARCHAR(42),
                roi DECIMAL(20, 8),
                calculated_at TIMESTAMPTZ
            );
            "#,
        )
        .execute(pool)
        .await
        .expect("create tables");
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_trade(
        pool: &PgPool,
        n: u32,
        wallet: &str,
        asset: &str,
        side: &str,
        price: &str,
        qty: &str,
        ts: DateTime<Utc>,
    ) {
        sqlx::query(
            "INSERT INTO wallet_trades \
             (transaction_hash, wallet_address, asset_id, side, price, quantity, value, timestamp) \
             VALUES ($1, $2, $3, $4, $5, $6, $5 * $6, $7)",
        )
        .bind(format!("0xtx{n}"))
        .bind(wallet)
        .bind(asset)
        .bind(side)
        .bind(d(price))
        .bind(d(qty))
        .bind(ts)
        .execute(pool)
        .await
        .expect("insert trade");
    }

    #[sqlx::test(migrations = false)]
    async fn fetch_trades_keeps_newest_rows_in_ascending_order(pool: PgPool) {
        create_tables(&pool).await;
        let now = Utc::now();
        // Days ago: 45 (outside the month), 20, 15, 10, 5, 1.
        for (i, days) in [45i64, 20, 15, 10, 5, 1].into_iter().enumerate() {
            insert_trade(
                &pool,
                i as u32,
                "0xABCDEF",
                "A",
                "BUY",
                "0.5",
                "10",
                now - Duration::days(days),
            )
            .await;
        }
        // Another wallet must never leak in.
        insert_trade(&pool, 99, "0xother", "A", "BUY", "0.5", "10", now).await;

        let a = ProfitabilityAnalyzer::new(pool.clone());
        let ages = |rows: &[TradeRecord]| -> Vec<i64> {
            rows.iter()
                .map(|r| (now - r.timestamp).num_days())
                .collect()
        };

        // LIMIT must keep the NEWEST rows (10, 5, 1 days ago), oldest first.
        let rows = a
            .fetch_trades("0xabcdef", TimePeriod::Month, 3)
            .await
            .unwrap();
        assert_eq!(ages(&rows), vec![10, 5, 1]);

        // Month excludes the 45-day-old row; AllTime includes it, ascending too.
        let month = a
            .fetch_trades("0xABCDEF", TimePeriod::Month, 100)
            .await
            .unwrap();
        assert_eq!(ages(&month), vec![20, 15, 10, 5, 1]);
        let all = a
            .fetch_trades("0xABCDEF", TimePeriod::AllTime, 100)
            .await
            .unwrap();
        assert_eq!(ages(&all), vec![45, 20, 15, 10, 5, 1]);

        // AllTime with a small limit still keeps the newest rows, ascending.
        let newest_two = a
            .fetch_trades("0xABCDEF", TimePeriod::AllTime, 2)
            .await
            .unwrap();
        assert_eq!(ages(&newest_two), vec![5, 1]);
    }

    #[sqlx::test(migrations = false)]
    async fn calculate_metrics_uses_realized_pnl_from_the_database(pool: PgPool) {
        create_tables(&pool).await;
        let now = Utc::now();
        // Insert out of chronological order on purpose.
        insert_trade(
            &pool,
            1,
            "0xw",
            "A",
            "SELL",
            "0.60",
            "100",
            now - Duration::days(2),
        )
        .await;
        insert_trade(
            &pool,
            2,
            "0xw",
            "A",
            "BUY",
            "0.40",
            "100",
            now - Duration::days(3),
        )
        .await;
        // A sell whose buy is outside the window has no basis and is ignored.
        insert_trade(
            &pool,
            3,
            "0xw",
            "Z",
            "SELL",
            "0.99",
            "500",
            now - Duration::days(1),
        )
        .await;

        let a = ProfitabilityAnalyzer::new(pool);
        let m = a.calculate_metrics("0xw", TimePeriod::Month).await.unwrap();
        assert_eq!(m.total_trades, 3);
        assert_eq!(m.total_return, d("20"));
        assert!(approx(m.roi_percentage, 0.5));
        assert_eq!((m.winning_trades, m.losing_trades), (1, 0));
        assert_eq!(m.win_rate, 1.0);
    }

    #[allow(clippy::type_complexity)]
    async fn read_row(pool: &PgPool, address: &str) -> (f64, f64, f64, f64, f64, f64, i32, f64) {
        sqlx::query_as(
            "SELECT roi_30d::float8, roi_90d::float8, roi_all_time::float8, COALESCE(roi::float8, 0), \
             sharpe_30d::float8, sortino_30d::float8, trades_30d, win_rate_30d::float8 \
             FROM wallet_success_metrics WHERE address = $1",
        )
        .bind(address)
        .fetch_one(pool)
        .await
        .expect("row")
    }

    fn sample_metrics(period: TimePeriod, roi: f64) -> WalletMetrics {
        WalletMetrics {
            address: "0xw".to_string(),
            period,
            total_return: Decimal::ZERO,
            roi_percentage: roi,
            annualized_return: f64::INFINITY,
            sharpe_ratio: 1.25,
            sortino_ratio: f64::INFINITY,
            max_drawdown: 0.1,
            max_drawdown_duration_days: 2,
            volatility: 0.3,
            downside_deviation: 0.0,
            total_trades: 12,
            winning_trades: 9,
            losing_trades: 1,
            win_rate: 0.9,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: f64::INFINITY,
            expectancy: Decimal::ZERO,
            avg_position_size: Decimal::ZERO,
            max_position_size: Decimal::ZERO,
            avg_holding_period_hours: 0.0,
            consistency_score: 0.5,
            winning_streak: 3,
            losing_streak: 1,
            current_streak: 1,
            computed_at: Utc::now(),
        }
    }

    #[sqlx::test(migrations = false)]
    async fn store_metrics_writes_only_the_computed_window(pool: PgPool) {
        create_tables(&pool).await;
        let a = ProfitabilityAnalyzer::new(pool.clone());

        // Month: roi_30d/roi set; the other windows are NOT filled with it.
        a.store_metrics(&sample_metrics(TimePeriod::Month, 0.25))
            .await
            .unwrap();
        let (r30, r90, rall, roi, sharpe, sortino, trades, wr) = read_row(&pool, "0xw").await;
        assert!(approx(r30, 0.25) && approx(roi, 0.25));
        assert_eq!(
            (r90, rall),
            (0.0, 0.0),
            "uncomputed windows must stay at default"
        );
        assert_eq!(sharpe, 1.25);
        assert_eq!(
            sortino, MAX_STORED_RATIO,
            "+inf Sortino must not be stored as 0"
        );
        assert_eq!(trades, 12);
        assert!(approx(wr, 0.9));
        let annualized: f64 =
            sqlx::query_scalar("SELECT annualized_return::float8 FROM wallet_success_metrics")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(annualized, MAX_DECIMAL_MAGNITUDE);

        let before: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_computed FROM wallet_success_metrics")
                .fetch_one(&pool)
                .await
                .unwrap();

        // Quarter and AllTime only touch their own roi column.
        a.store_metrics(&sample_metrics(TimePeriod::Quarter, 0.4))
            .await
            .unwrap();
        a.store_metrics(&sample_metrics(TimePeriod::AllTime, 0.9))
            .await
            .unwrap();
        let (r30, r90, rall, roi, sharpe, sortino, trades, _) = read_row(&pool, "0xw").await;
        assert!(
            approx(r30, 0.25) && approx(roi, 0.25),
            "30d values untouched"
        );
        assert!(approx(r90, 0.4) && approx(rall, 0.9));
        assert_eq!((sharpe, sortino, trades), (1.25, MAX_STORED_RATIO, 12));
        let after: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_computed FROM wallet_success_metrics")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before, after, "freshness must not change for other windows");

        // Windows without storage columns are rejected instead of mislabelled.
        assert!(a
            .store_metrics(&sample_metrics(TimePeriod::Week, 0.1))
            .await
            .is_err());
    }

    #[sqlx::test(migrations = false)]
    async fn quarter_only_row_is_created_stale(pool: PgPool) {
        create_tables(&pool).await;
        let a = ProfitabilityAnalyzer::new(pool.clone());
        a.store_metrics(&sample_metrics(TimePeriod::Quarter, 0.4))
            .await
            .unwrap();
        let (r30, r90, ..) = read_row(&pool, "0xw").await;
        assert_eq!(r30, 0.0);
        assert!(approx(r90, 0.4));
        let last: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_computed FROM wallet_success_metrics")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last.timestamp(), 0, "must not look freshly computed");
    }

    #[sqlx::test(migrations = false)]
    async fn migration_087_resets_mislabelled_window_rois(pool: PgPool) {
        create_tables(&pool).await;
        sqlx::query(
            "INSERT INTO wallet_success_metrics \
             (address, roi_30d, roi_90d, roi_all_time, roi) VALUES ('0xold', 0.3, 0.3, 0.3, 0.3)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let migration = include_str!("../../../migrations/087_reset_wallet_window_rois.sql");
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        // Safe to rerun.
        sqlx::raw_sql(migration).execute(&pool).await.unwrap();

        let (r30, r90, rall, roi, ..) = read_row(&pool, "0xold").await;
        assert!(approx(r30, 0.3) && approx(roi, 0.3), "30d values are kept");
        assert_eq!((r90, rall), (0.0, 0.0));
    }
}
