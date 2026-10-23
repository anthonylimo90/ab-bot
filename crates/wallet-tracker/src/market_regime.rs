//! Market regime detection: whether recent prices trend and how volatile
//! they are, so wallet metrics and discovery thresholds can adapt.

use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::debug;

/// Closed trades (wins plus losses) a wallet needs in 30 days to count
/// towards the regime's average win rate and ROI spread.
const MIN_CLOSED_TRADES_FOR_REGIME: i32 = 5;

/// Market regime for context-aware prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketRegime {
    /// High volatility, trending up.
    BullVolatile,
    /// Low volatility, trending up.
    BullCalm,
    /// High volatility, trending down.
    BearVolatile,
    /// Low volatility, trending down.
    BearCalm,
    /// Ranging/sideways market.
    Ranging,
    /// Unknown/transitioning.
    Uncertain,
}

/// Convert Decimal to f64.
fn rust_decimal_to_f64(d: Decimal) -> f64 {
    d.to_string().parse().unwrap_or(0.0)
}

/// Market condition analyzer for regime detection.
pub struct MarketConditionAnalyzer {
    pool: PgPool,
    volatility_threshold_high: f64,
    volatility_threshold_low: f64,
    trend_threshold: f64,
}

impl MarketConditionAnalyzer {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            volatility_threshold_high: 0.3,
            volatility_threshold_low: 0.1,
            trend_threshold: 0.02, // 2% trend threshold
        }
    }

    /// Detect current market regime.
    pub async fn detect_regime(&self) -> Result<MarketRegime> {
        // Get recent market data
        let volatility = self.calculate_market_volatility().await?;
        let trend = self.calculate_market_trend().await?;

        let is_volatile = volatility > self.volatility_threshold_high;
        let is_calm = volatility < self.volatility_threshold_low;
        let is_bullish = trend > self.trend_threshold;
        let is_bearish = trend < -self.trend_threshold;

        let regime = if is_bullish && is_volatile {
            MarketRegime::BullVolatile
        } else if is_bullish && is_calm {
            MarketRegime::BullCalm
        } else if is_bearish && is_volatile {
            MarketRegime::BearVolatile
        } else if is_bearish && is_calm {
            MarketRegime::BearCalm
        } else if !is_bullish && !is_bearish {
            MarketRegime::Ranging
        } else {
            MarketRegime::Uncertain
        };

        Ok(regime)
    }

    async fn calculate_market_volatility(&self) -> Result<f64> {
        // Calculate average volatility across markets
        let result: Option<Decimal> = sqlx::query_scalar(
            r#"
            SELECT AVG(spread) as avg_spread
            FROM arb_opportunities
            WHERE timestamp > NOW() - INTERVAL '24 hours'
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(result.map(rust_decimal_to_f64).unwrap_or(0.15))
    }

    async fn calculate_market_trend(&self) -> Result<f64> {
        // Compare recent 6h arb opportunity volume vs prior 6h to detect trend.
        // Positive = more opportunities (spread widening / bear), negative = fewer (bull/calm).
        let result: Option<(Option<Decimal>, Option<Decimal>)> = sqlx::query_as(
            r#"
            SELECT
                (SELECT COUNT(*)::numeric FROM arb_opportunities
                 WHERE timestamp > NOW() - INTERVAL '6 hours') as recent_count,
                (SELECT COUNT(*)::numeric FROM arb_opportunities
                 WHERE timestamp BETWEEN NOW() - INTERVAL '12 hours'
                   AND NOW() - INTERVAL '6 hours') as prior_count
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((Some(recent), Some(prior))) => {
                let recent_f = rust_decimal_to_f64(recent);
                let prior_f = rust_decimal_to_f64(prior);
                if prior_f == 0.0 {
                    return Ok(0.0);
                }
                // Rate of change: positive = more arb opportunities = bearish market
                // Negative = fewer opportunities = bullish/tighter spreads
                let trend = (recent_f - prior_f) / prior_f;
                // Negate so positive = bullish (fewer arbs = tighter spreads = healthy market)
                Ok(-trend)
            }
            _ => Ok(0.0),
        }
    }

    /// Detect market regime using a blend of wallet performance aggregates
    /// and orderbook microstructure data.
    ///
    /// **Layer 1 — Wallet metrics** (from `wallet_success_metrics`):
    /// - Average win rate classifies bull (>0.58), bear (<0.48), or ranging
    /// - Standard deviation of ROI classifies volatility
    ///
    /// **Layer 2 — Orderbook microstructure** (from `orderbook_hourly`):
    /// - 24h price momentum (avg close change across active markets)
    /// - Spread volatility (stddev of hourly avg_spread)
    ///
    /// Used as a tiebreaker when wallet metrics alone fall into the ambiguous zone.
    pub async fn detect_regime_from_metrics(&self) -> Result<MarketRegime> {
        // ── Layer 1: Wallet performance aggregates ──
        // Only wallets with closed trades to judge: one that has only bought
        // has a stored win rate of 0, not a losing record. Both columns are
        // fractions (an ROI of 1.5 is +150%).
        let row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
            r#"
            SELECT
                AVG(win_rate_30d::FLOAT8) AS avg_win_rate,
                STDDEV_POP(roi_30d::FLOAT8) AS roi_stddev
            FROM wallet_success_metrics
            WHERE last_computed > NOW() - INTERVAL '7 days'
              AND winning_trades_30d + losing_trades_30d >= $1
            "#,
        )
        .bind(MIN_CLOSED_TRADES_FOR_REGIME)
        .fetch_optional(&self.pool)
        .await?;

        let (avg_win_rate, roi_stddev) = row.unwrap_or((None, None));
        let avg_win_rate = avg_win_rate.unwrap_or(0.50);
        let roi_stddev = roi_stddev.unwrap_or(0.15);

        // ── Layer 2: Orderbook microstructure (24h momentum + spread volatility) ──
        // Query the hourly continuous aggregate for the last 24h, per market:
        // momentum: average hourly (close - open) / open
        // spread_vol: how much the market's hourly spread moved over the day
        // (not how spreads differ between markets), averaged across markets
        let ob_row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
            r#"
            SELECT
                AVG(momentum) AS avg_momentum,
                AVG(spread_volatility) AS spread_volatility
            FROM (
                SELECT
                    AVG(
                        CASE WHEN open > 0 THEN (close - open)::FLOAT8 / open::FLOAT8 ELSE 0 END
                    ) AS momentum,
                    STDDEV_POP(avg_spread::FLOAT8) AS spread_volatility
                FROM orderbook_hourly
                WHERE bucket >= NOW() - INTERVAL '24 hours'
                  AND tick_count >= 2
                GROUP BY market_id
            ) per_market
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None);

        let (avg_momentum, spread_vol) = ob_row.unwrap_or((None, None));
        let avg_momentum = avg_momentum.unwrap_or(0.0);
        let spread_vol = spread_vol.unwrap_or(0.0);

        // ── Classification ──
        // Primary signal: wallet metrics
        let mut is_bullish = avg_win_rate > 0.58;
        let mut is_bearish = avg_win_rate < 0.48;
        let is_volatile = roi_stddev > self.volatility_threshold_high || spread_vol > 0.02;
        let is_calm = roi_stddev < self.volatility_threshold_low && spread_vol < 0.005;

        // Tiebreaker: when wallet metrics are in the ambiguous zone (0.48–0.58),
        // use orderbook momentum to nudge the classification.
        if !is_bullish && !is_bearish {
            if avg_momentum > 0.005 {
                is_bullish = true;
            } else if avg_momentum < -0.005 {
                is_bearish = true;
            }
        }

        let regime = if is_bullish && is_volatile {
            MarketRegime::BullVolatile
        } else if is_bullish && is_calm {
            MarketRegime::BullCalm
        } else if is_bearish && is_volatile {
            MarketRegime::BearVolatile
        } else if is_bearish && is_calm {
            MarketRegime::BearCalm
        } else if !is_bullish && !is_bearish {
            MarketRegime::Ranging
        } else {
            MarketRegime::Uncertain
        };

        debug!(
            avg_win_rate = avg_win_rate,
            roi_stddev = roi_stddev,
            avg_momentum = avg_momentum,
            spread_vol = spread_vol,
            regime = ?regime,
            "Detected market regime (wallet + orderbook)"
        );

        Ok(regime)
    }
}
