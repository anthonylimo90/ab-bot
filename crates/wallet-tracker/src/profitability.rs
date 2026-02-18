//! Profitability analysis for wallet performance metrics.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use statrs::statistics::{Data, Distribution};
use std::collections::HashMap;
use tracing::debug;

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

/// A single trade for profitability calculation.
#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub timestamp: DateTime<Utc>,
    pub market_id: String,
    pub side: TradeSide,
    pub entry_price: Decimal,
    pub exit_price: Option<Decimal>,
    pub quantity: Decimal,
    pub pnl: Option<Decimal>,
    pub fees: Decimal,
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
    pub async fn calculate_metrics(
        &self,
        address: &str,
        period: TimePeriod,
    ) -> Result<WalletMetrics> {
        let trades = self.fetch_trades(address, period).await?;

        if trades.is_empty() {
            return Ok(self.empty_metrics(address, period));
        }

        let daily_returns = self.calculate_daily_returns(&trades);
        let trade_pnls: Vec<f64> = trades
            .iter()
            .filter_map(|t| t.pnl.map(|p| p.to_f64().unwrap_or(0.0)))
            .collect();

        // Calculate return metrics
        let total_pnl: Decimal = trades.iter().filter_map(|t| t.pnl).sum();
        let initial_capital = self.estimate_initial_capital(&trades);
        let roi = if initial_capital > Decimal::ZERO {
            (total_pnl / initial_capital).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };

        // Calculate risk metrics
        let sharpe = self.calculate_sharpe_ratio(&daily_returns);
        let sortino = self.calculate_sortino_ratio(&daily_returns);
        let (max_dd, dd_duration) = self.calculate_max_drawdown(&daily_returns);
        let volatility = self.calculate_volatility(&daily_returns);
        let downside_dev = self.calculate_downside_deviation(&daily_returns);

        // Calculate trade metrics
        let winning_trades: Vec<&TradeRecord> = trades
            .iter()
            .filter(|t| t.pnl.map(|p| p > Decimal::ZERO).unwrap_or(false))
            .collect();
        let losing_trades: Vec<&TradeRecord> = trades
            .iter()
            .filter(|t| t.pnl.map(|p| p < Decimal::ZERO).unwrap_or(false))
            .collect();

        let win_count = winning_trades.len() as u64;
        let loss_count = losing_trades.len() as u64;
        let total_trades = trades.len() as u64;

        let win_rate = if total_trades > 0 {
            win_count as f64 / total_trades as f64
        } else {
            0.0
        };

        let avg_win = if !winning_trades.is_empty() {
            winning_trades.iter().filter_map(|t| t.pnl).sum::<Decimal>()
                / Decimal::from(winning_trades.len())
        } else {
            Decimal::ZERO
        };

        let avg_loss = if !losing_trades.is_empty() {
            losing_trades
                .iter()
                .filter_map(|t| t.pnl)
                .sum::<Decimal>()
                .abs()
                / Decimal::from(losing_trades.len())
        } else {
            Decimal::ZERO
        };

        let profit_factor = if avg_loss > Decimal::ZERO {
            (avg_win * Decimal::from(win_count as i64)
                / (avg_loss * Decimal::from(loss_count.max(1) as i64)))
            .to_f64()
            .unwrap_or(0.0)
        } else {
            f64::INFINITY
        };

        let expectancy = (avg_win * Decimal::try_from(win_rate).unwrap_or_default())
            - (avg_loss * Decimal::try_from(1.0 - win_rate).unwrap_or_default());

        // Calculate position metrics
        let position_sizes: Vec<Decimal> =
            trades.iter().map(|t| t.quantity * t.entry_price).collect();
        let avg_position = if !position_sizes.is_empty() {
            position_sizes.iter().sum::<Decimal>() / Decimal::from(position_sizes.len())
        } else {
            Decimal::ZERO
        };
        let max_position = position_sizes
            .iter()
            .max()
            .cloned()
            .unwrap_or(Decimal::ZERO);

        // Calculate consistency
        let consistency = self.calculate_consistency_score(&trade_pnls);
        let (win_streak, lose_streak, current) = self.calculate_streaks(&trades);

        // Annualized return
        let days = period.to_days().unwrap_or(365) as f64;
        let annualized = ((1.0 + roi).powf(365.0 / days)) - 1.0;

        Ok(WalletMetrics {
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
            total_trades,
            winning_trades: win_count,
            losing_trades: loss_count,
            win_rate,
            avg_win,
            avg_loss,
            profit_factor,
            expectancy,
            avg_position_size: avg_position,
            max_position_size: max_position,
            avg_holding_period_hours: 24.0, // Simplified
            consistency_score: consistency,
            winning_streak: win_streak,
            losing_streak: lose_streak,
            current_streak: current,
            computed_at: Utc::now(),
        })
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

    /// Clamp an f64 to a range safe for PostgreSQL DECIMAL(20,8).
    /// Replaces NaN and Infinity with 0.0, then clamps to [-999_999_999_999.0, 999_999_999_999.0].
    fn clamp_for_decimal(value: f64) -> f64 {
        if value.is_nan() || value.is_infinite() {
            return 0.0;
        }
        value.clamp(-999_999_999_999.0, 999_999_999_999.0)
    }

    /// Store metrics in the database.
    pub async fn store_metrics(&self, metrics: &WalletMetrics) -> Result<()> {
        let total_trades = i32::try_from(metrics.total_trades).unwrap_or(i32::MAX);
        let winning_trades = i32::try_from(metrics.winning_trades).unwrap_or(i32::MAX);
        let losing_trades = i32::try_from(metrics.losing_trades).unwrap_or(i32::MAX);

        let roi = Self::clamp_for_decimal(metrics.roi_percentage);
        let annualized = Self::clamp_for_decimal(metrics.annualized_return);
        let sharpe = Self::clamp_for_decimal(metrics.sharpe_ratio);
        let sortino = Self::clamp_for_decimal(metrics.sortino_ratio);
        let max_drawdown = Self::clamp_for_decimal(metrics.max_drawdown);
        let volatility = Self::clamp_for_decimal(metrics.volatility);
        let consistency = Self::clamp_for_decimal(metrics.consistency_score);
        let win_rate = Self::clamp_for_decimal(metrics.win_rate);
        let composite = Self::clamp_for_decimal(metrics.composite_score() / 100.0);

        sqlx::query(
            r#"
            INSERT INTO wallet_success_metrics (
                address, wallet_address, roi_30d, roi_90d, roi_all_time, annualized_return,
                sharpe_30d, sortino_30d, max_drawdown_30d, volatility_30d, consistency_score,
                win_rate_30d, trades_30d, winning_trades_30d, losing_trades_30d,
                predicted_success_prob, last_computed, calculated_at, roi
            ) VALUES (
                $1, $1, $2, $2, $2, $3,
                $4, $5, $6, $7, $8,
                $9, $10, $11, $12,
                $13, $14, $14, $2
            )
            ON CONFLICT (address) DO UPDATE SET
                wallet_address = EXCLUDED.wallet_address,
                roi_30d = EXCLUDED.roi_30d,
                roi_90d = EXCLUDED.roi_90d,
                roi_all_time = EXCLUDED.roi_all_time,
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

    // Private calculation methods

    async fn fetch_trades(&self, address: &str, period: TimePeriod) -> Result<Vec<TradeRecord>> {
        let cutoff = period.to_days().map(|d| Utc::now() - Duration::days(d));

        // Try copy_trade_history first (for actual copy trading performance)
        let query = if let Some(cutoff_date) = cutoff {
            sqlx::query(
                r#"
                SELECT source_market_id AS market_id,
                       source_direction AS side,
                       source_price AS entry_price,
                       source_quantity AS quantity,
                       pnl,
                       source_timestamp AS timestamp
                FROM copy_trade_history
                WHERE LOWER(source_wallet) = LOWER($1)
                  AND source_timestamp >= $2
                ORDER BY source_timestamp
                "#,
            )
            .bind(address)
            .bind(cutoff_date)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT source_market_id AS market_id,
                       source_direction AS side,
                       source_price AS entry_price,
                       source_quantity AS quantity,
                       pnl,
                       source_timestamp AS timestamp
                FROM copy_trade_history
                WHERE LOWER(source_wallet) = LOWER($1)
                ORDER BY source_timestamp
                "#,
            )
            .bind(address)
            .fetch_all(&self.pool)
            .await?
        };

        let mut trades: Vec<TradeRecord> = query
            .iter()
            .map(|row| {
                use sqlx::Row;
                let side_raw: i16 = row.get("side");
                let side = if side_raw == 0 {
                    TradeSide::Buy
                } else {
                    TradeSide::Sell
                };
                let entry_price: Decimal = row.get("entry_price");
                let quantity: Decimal = row.get("quantity");
                let recorded_pnl: Option<Decimal> = row.try_get("pnl").unwrap_or(None);

                // Some rows may not have realized PnL yet; fall back to signed cashflow.
                let signed_cashflow = match side {
                    TradeSide::Buy => -(quantity * entry_price),
                    TradeSide::Sell => quantity * entry_price,
                };

                TradeRecord {
                    timestamp: row.get("timestamp"),
                    market_id: row.get("market_id"),
                    side,
                    entry_price,
                    exit_price: None,
                    quantity,
                    pnl: recorded_pnl.or(Some(signed_cashflow)),
                    fees: Decimal::ZERO,
                }
            })
            .collect();

        // If no copy trades found, fall back to wallet_trades (for discovery/evaluation)
        // Using sqlx::query (not query!) to avoid offline mode requirement for wallet_trades
        if trades.is_empty() {
            let wallet_query = if let Some(cutoff_date) = cutoff {
                sqlx::query(
                    r#"
                    SELECT asset_id,
                           side,
                           price,
                           quantity,
                           value,
                           timestamp
                    FROM wallet_trades
                    WHERE LOWER(wallet_address) = LOWER($1)
                      AND timestamp >= $2
                    ORDER BY timestamp
                    "#,
                )
                .bind(address)
                .bind(cutoff_date)
                .fetch_all(&self.pool)
                .await?
            } else {
                sqlx::query(
                    r#"
                    SELECT asset_id,
                           side,
                           price,
                           quantity,
                           value,
                           timestamp
                    FROM wallet_trades
                    WHERE LOWER(wallet_address) = LOWER($1)
                    ORDER BY timestamp
                    "#,
                )
                .bind(address)
                .fetch_all(&self.pool)
                .await?
            };

            trades = wallet_query
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

                    // Use signed cashflow approach for PnL calculation
                    let pnl = match side {
                        TradeSide::Buy => -(quantity * price), // Spending money
                        TradeSide::Sell => quantity * price,   // Receiving money
                    };

                    TradeRecord {
                        timestamp: row.get("timestamp"),
                        market_id: row.get("asset_id"),
                        side,
                        entry_price: price,
                        exit_price: None,
                        quantity,
                        pnl: Some(pnl),
                        fees: Decimal::ZERO,
                    }
                })
                .collect();
        }

        Ok(trades)
    }

    fn calculate_daily_returns(&self, trades: &[TradeRecord]) -> Vec<DailyReturn> {
        if trades.is_empty() {
            return vec![];
        }

        let mut daily_pnl: HashMap<String, f64> = HashMap::new();

        for trade in trades {
            let date_key = trade.timestamp.format("%Y-%m-%d").to_string();
            let pnl = trade.pnl.map(|p| p.to_f64().unwrap_or(0.0)).unwrap_or(0.0);
            *daily_pnl.entry(date_key).or_insert(0.0) += pnl;
        }

        let mut dates: Vec<_> = daily_pnl.keys().cloned().collect();
        dates.sort();

        let mut cumulative: f64 = 1.0;
        let mut returns = vec![];

        for date in dates {
            let pnl = daily_pnl[&date];
            let return_pct = pnl / cumulative.max(1.0);
            cumulative += pnl;

            returns.push(DailyReturn {
                date: date.parse().unwrap_or_else(|_| Utc::now()),
                return_pct,
                cumulative_value: cumulative,
            });
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

    fn calculate_max_drawdown(&self, returns: &[DailyReturn]) -> (f64, i64) {
        if returns.is_empty() {
            return (0.0, 0);
        }

        let mut peak = returns[0].cumulative_value;
        let mut max_drawdown: f64 = 0.0;
        let mut max_dd_duration = 0i64;
        let mut current_dd_start: Option<DateTime<Utc>> = None;

        for ret in returns {
            if ret.cumulative_value > peak {
                peak = ret.cumulative_value;
                if let Some(start) = current_dd_start.take() {
                    let duration = (ret.date - start).num_days();
                    max_dd_duration = max_dd_duration.max(duration);
                }
            } else {
                let drawdown = (peak - ret.cumulative_value) / peak;
                max_drawdown = max_drawdown.max(drawdown);

                if current_dd_start.is_none() {
                    current_dd_start = Some(ret.date);
                }
            }
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

    fn calculate_streaks(&self, trades: &[TradeRecord]) -> (u32, u32, i32) {
        let mut max_win_streak = 0u32;
        let mut max_lose_streak = 0u32;
        let mut current_streak = 0i32;
        let mut current_win = 0u32;
        let mut current_lose = 0u32;

        for trade in trades {
            let is_win = trade.pnl.map(|p| p > Decimal::ZERO).unwrap_or(false);

            if is_win {
                current_win += 1;
                current_lose = 0;
                current_streak = current_win as i32;
                max_win_streak = max_win_streak.max(current_win);
            } else {
                current_lose += 1;
                current_win = 0;
                current_streak = -(current_lose as i32);
                max_lose_streak = max_lose_streak.max(current_lose);
            }
        }

        (max_win_streak, max_lose_streak, current_streak)
    }

    fn estimate_initial_capital(&self, trades: &[TradeRecord]) -> Decimal {
        // Estimate based on first few trades
        trades
            .iter()
            .take(5)
            .map(|t| t.quantity * t.entry_price)
            .max()
            .unwrap_or(Decimal::new(1000, 0))
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

    fn make_trade(pnl: f64, day_offset: i64) -> TradeRecord {
        TradeRecord {
            timestamp: Utc::now() - Duration::days(day_offset),
            market_id: "market-1".to_string(),
            side: TradeSide::Buy,
            entry_price: Decimal::new(50, 2),
            exit_price: Some(Decimal::new(60, 2)),
            quantity: Decimal::new(100, 0),
            pnl: Some(Decimal::from_f64_retain(pnl).unwrap_or(Decimal::ZERO)),
            fees: Decimal::ZERO,
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
        let trades = vec![
            make_trade(10.0, 7),
            make_trade(5.0, 6),
            make_trade(-3.0, 5),
            make_trade(8.0, 4),
            make_trade(-2.0, 3),
            make_trade(-4.0, 2),
            make_trade(-1.0, 1),
        ];

        let (win_streak, lose_streak, current) = analyzer.calculate_streaks(&trades);
        assert_eq!(win_streak, 2, "Max winning streak should be 2");
        assert_eq!(lose_streak, 3, "Max losing streak should be 3");
        assert_eq!(current, -3, "Current streak should be -3 (3 losses)");
    }

    #[tokio::test]
    async fn test_estimate_initial_capital() {
        let analyzer = test_analyzer();

        let trades = vec![
            TradeRecord {
                timestamp: Utc::now() - Duration::days(3),
                market_id: "m1".to_string(),
                side: TradeSide::Buy,
                entry_price: Decimal::new(50, 2), // 0.50
                quantity: Decimal::new(200, 0),   // 200
                exit_price: None,
                pnl: Some(Decimal::new(10, 0)),
                fees: Decimal::ZERO,
            },
            TradeRecord {
                timestamp: Utc::now() - Duration::days(2),
                market_id: "m2".to_string(),
                side: TradeSide::Buy,
                entry_price: Decimal::new(80, 2), // 0.80
                quantity: Decimal::new(500, 0),   // 500
                exit_price: None,
                pnl: Some(Decimal::new(20, 0)),
                fees: Decimal::ZERO,
            },
        ];

        let capital = analyzer.estimate_initial_capital(&trades);
        // max(200*0.50, 500*0.80) = max(100, 400) = 400
        assert_eq!(capital, Decimal::new(400, 0));
    }

    #[tokio::test]
    async fn test_consistency_score_few_trades() {
        let analyzer = test_analyzer();

        // Less than 5 PnLs => 0.0
        let pnls = vec![1.0, 2.0, -1.0];
        assert_eq!(analyzer.calculate_consistency_score(&pnls), 0.0);
    }

    #[test]
    fn test_clamp_for_decimal() {
        // Normal values pass through
        assert_eq!(ProfitabilityAnalyzer::clamp_for_decimal(1.5), 1.5);
        assert_eq!(ProfitabilityAnalyzer::clamp_for_decimal(-0.5), -0.5);

        // NaN becomes 0
        assert_eq!(ProfitabilityAnalyzer::clamp_for_decimal(f64::NAN), 0.0);

        // Infinity becomes 0
        assert_eq!(ProfitabilityAnalyzer::clamp_for_decimal(f64::INFINITY), 0.0);
        assert_eq!(
            ProfitabilityAnalyzer::clamp_for_decimal(f64::NEG_INFINITY),
            0.0
        );

        // Extreme values are clamped
        assert_eq!(
            ProfitabilityAnalyzer::clamp_for_decimal(1e15),
            999_999_999_999.0
        );
        assert_eq!(
            ProfitabilityAnalyzer::clamp_for_decimal(-1e15),
            -999_999_999_999.0
        );
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
