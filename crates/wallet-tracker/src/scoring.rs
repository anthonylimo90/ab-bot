//! Unified scoring interface for wallet evaluation.
//!
//! This module provides a single normalized scoring system used across
//! discovery, allocation, and exploration contexts. It replaces the
//! previous parallel scoring systems with a composable, weight-configurable
//! approach.

use serde::{Deserialize, Serialize};

use crate::advanced_predictor::MarketRegime;

/// Normalized wallet score with all components in the 0-1 range.
///
/// All components are clamped to [0.0, 1.0] so that weighted composites
/// are always in the same range regardless of which context uses them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletScore {
    pub address: String,
    /// ROI score normalized to 0-1 (20% monthly ROI = 1.0).
    pub roi_score: f64,
    /// Sharpe ratio normalized to 0-1 (Sharpe 3.0 = 1.0).
    pub sharpe_score: f64,
    /// Sortino ratio normalized to 0-1 (Sortino 3.0 = 1.0).
    pub sortino_score: f64,
    /// Win rate as-is (already 0-1).
    pub win_rate_score: f64,
    /// Consistency score (0-1).
    pub consistency_score: f64,
    /// Drawdown score: lower drawdown = higher score (0-1).
    pub drawdown_score: f64,
    /// Time-decay factor (0-1), where 1.0 = freshly computed.
    pub recency_weight: f64,
}

impl WalletScore {
    /// Create a new wallet score from raw metrics.
    ///
    /// All inputs are normalized to [0.0, 1.0]:
    /// - `roi`: raw ROI as a ratio (e.g., 0.15 for 15% ROI)
    /// - `sharpe`: raw Sharpe ratio
    /// - `sortino`: raw Sortino ratio
    /// - `win_rate`: raw win rate (0-1)
    /// - `consistency`: raw consistency score (0-1)
    /// - `max_drawdown`: raw max drawdown as a positive ratio (e.g., 0.20 for 20%)
    /// - `staleness_days`: days since last data update (0 = fresh)
    #[allow(clippy::too_many_arguments)]
    pub fn from_raw(
        address: String,
        roi: f64,
        sharpe: f64,
        sortino: f64,
        win_rate: f64,
        consistency: f64,
        max_drawdown: f64,
        staleness_days: f64,
    ) -> Self {
        Self {
            address,
            roi_score: (roi / 0.20).clamp(0.0, 1.0),
            sharpe_score: (sharpe / 3.0).clamp(0.0, 1.0),
            sortino_score: (sortino / 3.0).clamp(0.0, 1.0),
            win_rate_score: win_rate.clamp(0.0, 1.0),
            consistency_score: consistency.clamp(0.0, 1.0),
            drawdown_score: (1.0 - max_drawdown / 0.30).clamp(0.0, 1.0),
            recency_weight: (1.0 - staleness_days / 60.0).clamp(0.5, 1.0),
        }
    }

    /// Compute a weighted composite score using the given weights.
    ///
    /// The result is in [0.0, 1.0] and is multiplied by the recency weight.
    pub fn composite(&self, weights: &ScoringWeights) -> f64 {
        let raw = self.roi_score * weights.roi
            + self.sharpe_score * weights.sharpe
            + self.sortino_score * weights.sortino
            + self.win_rate_score * weights.win_rate
            + self.consistency_score * weights.consistency
            + self.drawdown_score * weights.drawdown;
        raw * self.recency_weight
    }

    /// Score optimized for discovery (finding new high-quality wallets).
    pub fn for_discovery(&self) -> f64 {
        self.composite(&ScoringWeights::DISCOVERY)
    }

    /// Score optimized for allocation sizing (balancing risk/reward).
    pub fn for_allocation(&self) -> f64 {
        self.composite(&ScoringWeights::ALLOCATION)
    }

    /// Score optimized for exploration (high-upside, lower confidence picks).
    pub fn for_exploration(&self) -> f64 {
        self.composite(&ScoringWeights::EXPLORATION)
    }
}

/// Weight configuration for composite scoring.
///
/// All weights should sum to ~1.0 for normalized output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringWeights {
    pub roi: f64,
    pub sharpe: f64,
    pub sortino: f64,
    pub win_rate: f64,
    pub consistency: f64,
    pub drawdown: f64,
}

impl ScoringWeights {
    /// Discovery: balanced across performance signals, no drawdown penalty.
    pub const DISCOVERY: Self = Self {
        roi: 0.25,
        sharpe: 0.25,
        sortino: 0.15,
        win_rate: 0.20,
        consistency: 0.15,
        drawdown: 0.0,
    };

    /// Allocation: emphasizes downside protection and consistency.
    pub const ALLOCATION: Self = Self {
        roi: 0.15,
        sharpe: 0.15,
        sortino: 0.30,
        win_rate: 0.10,
        consistency: 0.20,
        drawdown: 0.10,
    };

    /// Exploration: high weight on ROI upside, accepts higher risk.
    pub const EXPLORATION: Self = Self {
        roi: 0.40,
        sharpe: 0.15,
        sortino: 0.10,
        win_rate: 0.10,
        consistency: 0.10,
        drawdown: 0.15,
    };

    /// Return regime-adjusted weights for discovery.
    pub fn discovery_for_regime(regime: MarketRegime) -> Self {
        match regime {
            MarketRegime::BearVolatile => Self {
                roi: 0.15,
                sharpe: 0.15,
                sortino: 0.30, // Downside protection paramount
                win_rate: 0.15,
                consistency: 0.25,
                drawdown: 0.0,
            },
            MarketRegime::BearCalm => Self {
                roi: 0.20,
                sharpe: 0.20,
                sortino: 0.25,
                win_rate: 0.15,
                consistency: 0.20,
                drawdown: 0.0,
            },
            MarketRegime::BullCalm => Self {
                roi: 0.30,
                sharpe: 0.20,
                sortino: 0.10,
                win_rate: 0.20,
                consistency: 0.20,
                drawdown: 0.0,
            },
            _ => Self::DISCOVERY, // BullVolatile, Ranging, Uncertain use defaults
        }
    }

    /// Return regime-adjusted weights for allocation.
    pub fn allocation_for_regime(regime: MarketRegime) -> Self {
        match regime {
            MarketRegime::BearVolatile => Self {
                roi: 0.10,
                sharpe: 0.10,
                sortino: 0.40, // Maximum downside protection
                win_rate: 0.10,
                consistency: 0.20,
                drawdown: 0.10,
            },
            MarketRegime::BullCalm => Self {
                roi: 0.25,
                sharpe: 0.15,
                sortino: 0.15,
                win_rate: 0.15,
                consistency: 0.20,
                drawdown: 0.10,
            },
            _ => Self::ALLOCATION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_score_from_raw() {
        let score = WalletScore::from_raw(
            "0x1234".to_string(),
            0.10, // 10% ROI -> 0.5 normalized
            1.5,  // Sharpe 1.5 -> 0.5 normalized
            2.0,  // Sortino 2.0 -> 0.667 normalized
            0.65, // 65% win rate
            0.70, // 70% consistency
            0.15, // 15% max drawdown -> 0.5 drawdown_score
            0.0,  // Fresh data
        );

        assert!((score.roi_score - 0.5).abs() < 0.01);
        assert!((score.sharpe_score - 0.5).abs() < 0.01);
        assert!((score.sortino_score - 0.667).abs() < 0.01);
        assert!((score.win_rate_score - 0.65).abs() < 0.01);
        assert!((score.recency_weight - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_composite_weights_sum_to_one() {
        let check = |w: &ScoringWeights, name: &str| {
            let sum = w.roi + w.sharpe + w.sortino + w.win_rate + w.consistency + w.drawdown;
            assert!(
                (sum - 1.0).abs() < 0.01,
                "{} weights sum to {} instead of 1.0",
                name,
                sum
            );
        };

        check(&ScoringWeights::DISCOVERY, "DISCOVERY");
        check(&ScoringWeights::ALLOCATION, "ALLOCATION");
        check(&ScoringWeights::EXPLORATION, "EXPLORATION");
    }

    #[test]
    fn test_recency_decay() {
        let fresh = WalletScore::from_raw("0xa".into(), 0.1, 1.5, 2.0, 0.65, 0.7, 0.15, 0.0);
        // 15 days: recency_weight = 1.0 - 15/60 = 0.75
        let stale = WalletScore::from_raw("0xb".into(), 0.1, 1.5, 2.0, 0.65, 0.7, 0.15, 15.0);
        // 45 days: recency_weight = 1.0 - 45/60 = 0.25 -> clamped to 0.5
        let very_stale = WalletScore::from_raw("0xc".into(), 0.1, 1.5, 2.0, 0.65, 0.7, 0.15, 45.0);
        // 90 days: recency_weight floors at 0.5
        let floored = WalletScore::from_raw("0xd".into(), 0.1, 1.5, 2.0, 0.65, 0.7, 0.15, 90.0);

        let fresh_score = fresh.for_discovery();
        let stale_score = stale.for_discovery();
        let very_stale_score = very_stale.for_discovery();

        assert!(
            fresh_score > stale_score,
            "Fresh should score higher than stale"
        );
        assert!(
            stale_score > very_stale_score,
            "Stale should score higher than very stale"
        );
        // Beyond 60 days the recency weight floors at 0.5
        assert!(
            (floored.recency_weight - 0.5).abs() < 0.01,
            "Recency weight should floor at 0.5 for 90-day staleness"
        );
    }

    #[test]
    fn test_regime_weights_sum_to_one() {
        let regimes = [
            MarketRegime::BullVolatile,
            MarketRegime::BullCalm,
            MarketRegime::BearVolatile,
            MarketRegime::BearCalm,
            MarketRegime::Ranging,
            MarketRegime::Uncertain,
        ];

        for regime in regimes {
            let dw = ScoringWeights::discovery_for_regime(regime);
            let sum = dw.roi + dw.sharpe + dw.sortino + dw.win_rate + dw.consistency + dw.drawdown;
            assert!(
                (sum - 1.0).abs() < 0.01,
                "discovery_for_regime({:?}) weights sum to {}",
                regime,
                sum
            );

            let aw = ScoringWeights::allocation_for_regime(regime);
            let sum = aw.roi + aw.sharpe + aw.sortino + aw.win_rate + aw.consistency + aw.drawdown;
            assert!(
                (sum - 1.0).abs() < 0.01,
                "allocation_for_regime({:?}) weights sum to {}",
                regime,
                sum
            );
        }
    }

    #[test]
    fn test_discovery_vs_exploration_ordering() {
        // Consistent performer: low ROI, moderate other metrics
        // Discovery likes the balanced profile; exploration sees limited upside.
        let consistent =
            WalletScore::from_raw("0xcon".into(), 0.03, 1.8, 2.0, 0.65, 0.70, 0.08, 0.0);
        // Risky performer: maximal ROI, weak everywhere else
        let risky = WalletScore::from_raw("0xrisk".into(), 0.20, 0.3, 0.3, 0.40, 0.15, 0.29, 0.0);

        // Discovery weights are balanced — consistent's breadth of quality should win
        assert!(
            consistent.for_discovery() > risky.for_discovery(),
            "Discovery should prefer consistent wallet (got consistent={}, risky={})",
            consistent.for_discovery(),
            risky.for_discovery(),
        );

        // Exploration puts 40% weight on ROI — risky's max ROI score should dominate
        assert!(
            risky.for_exploration() > consistent.for_exploration(),
            "Exploration should prefer high-ROI risky wallet (got risky={}, consistent={})",
            risky.for_exploration(),
            consistent.for_exploration(),
        );
    }
}
