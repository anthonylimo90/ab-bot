//! Strategy types for trading strategy classification.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Detected trading strategy based on wallet behavior analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectedStrategy {
    /// Arbitrage: Exploits price discrepancies between yes/no outcomes.
    /// Signals: opposing positions, low latency, high win rate, same-block trades.
    Arbitrage,

    /// Momentum: Follows price trends, buys rising assets.
    /// Signals: trend-following entries, moderate hold times (1-24h), volume confirmation.
    Momentum,

    /// Mean Reversion: Bets on prices returning to average.
    /// Signals: counter-trend entries, quick reversals, z-score correlation.
    MeanReversion,

    /// Grid Trading: Places orders at regular price intervals.
    /// Signals: regular intervals, symmetric buy/sell, consistent position sizes.
    GridTrading,

    /// Market Making: Provides liquidity on both sides.
    /// Signals: high frequency (>100/day), both bid/ask activity, tight spreads.
    MarketMaking,

    /// Copy Trading: Follows successful traders with delay.
    /// Signals: execution delay (1-60s), correlation with leader wallets.
    CopyTrading,

    /// Unknown: No clear strategy pattern detected.
    Unknown,
}

impl DetectedStrategy {
    /// Get human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            DetectedStrategy::Arbitrage => "Arbitrage",
            DetectedStrategy::Momentum => "Momentum",
            DetectedStrategy::MeanReversion => "Mean Reversion",
            DetectedStrategy::GridTrading => "Grid Trading",
            DetectedStrategy::MarketMaking => "Market Making",
            DetectedStrategy::CopyTrading => "Copy Trading",
            DetectedStrategy::Unknown => "Unknown",
        }
    }

    /// Get strategy description.
    pub fn description(&self) -> &'static str {
        match self {
            DetectedStrategy::Arbitrage => "Exploits price discrepancies between yes/no outcomes",
            DetectedStrategy::Momentum => "Follows price trends, buying rising assets",
            DetectedStrategy::MeanReversion => "Bets on prices returning to historical average",
            DetectedStrategy::GridTrading => "Places orders at regular price intervals",
            DetectedStrategy::MarketMaking => "Provides liquidity on both buy and sell sides",
            DetectedStrategy::CopyTrading => "Mirrors trades from successful wallets",
            DetectedStrategy::Unknown => "No clear trading pattern identified",
        }
    }

    /// Get all known strategies (excluding Unknown).
    pub fn all_known() -> &'static [DetectedStrategy] {
        &[
            DetectedStrategy::Arbitrage,
            DetectedStrategy::Momentum,
            DetectedStrategy::MeanReversion,
            DetectedStrategy::GridTrading,
            DetectedStrategy::MarketMaking,
            DetectedStrategy::CopyTrading,
        ]
    }
}

impl std::fmt::Display for DetectedStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A strategy classification signal with confidence and evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignal {
    /// The detected strategy type.
    pub strategy: DetectedStrategy,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f64,
    /// Evidence supporting this classification.
    pub evidence: Vec<StrategyEvidence>,
    /// When this signal was computed.
    pub computed_at: DateTime<Utc>,
}

impl StrategySignal {
    /// Create a new strategy signal.
    pub fn new(strategy: DetectedStrategy, confidence: f64, evidence: Vec<StrategyEvidence>) -> Self {
        Self {
            strategy,
            confidence: confidence.clamp(0.0, 1.0),
            evidence,
            computed_at: Utc::now(),
        }
    }

    /// Check if this is a high-confidence classification.
    pub fn is_high_confidence(&self) -> bool {
        self.confidence >= 0.7
    }

    /// Check if this is a medium-confidence classification.
    pub fn is_medium_confidence(&self) -> bool {
        self.confidence >= 0.4 && self.confidence < 0.7
    }
}

/// Evidence supporting a strategy classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StrategyEvidence {
    /// Opposing positions detected (arbitrage indicator).
    OpposingPositions {
        count: u64,
        weight: f64,
    },

    /// Low latency trades (arbitrage/market-making indicator).
    LowLatency {
        avg_ms: f64,
        weight: f64,
    },

    /// High win rate (arbitrage indicator).
    HighWinRate {
        rate: f64,
        trade_count: u64,
        weight: f64,
    },

    /// Trend-following pattern (momentum indicator).
    TrendFollowing {
        correlation: f64,
        weight: f64,
    },

    /// Counter-trend pattern (mean reversion indicator).
    CounterTrend {
        reversion_rate: f64,
        weight: f64,
    },

    /// Regular trade intervals (grid trading indicator).
    RegularIntervals {
        interval_cv: f64,
        weight: f64,
    },

    /// Symmetric buy/sell activity (grid/market-making indicator).
    SymmetricActivity {
        buy_sell_ratio: f64,
        weight: f64,
    },

    /// High frequency trading (market-making indicator).
    HighFrequency {
        trades_per_day: f64,
        weight: f64,
    },

    /// Delayed execution (copy trading indicator).
    DelayedExecution {
        avg_delay_seconds: f64,
        weight: f64,
    },

    /// Correlation with leader wallets (copy trading indicator).
    LeaderCorrelation {
        correlation: f64,
        leader_address: String,
        weight: f64,
    },

    /// Hold time pattern.
    HoldTime {
        avg_hours: f64,
        weight: f64,
    },
}

impl StrategyEvidence {
    /// Get the weight of this evidence.
    pub fn weight(&self) -> f64 {
        match self {
            StrategyEvidence::OpposingPositions { weight, .. } => *weight,
            StrategyEvidence::LowLatency { weight, .. } => *weight,
            StrategyEvidence::HighWinRate { weight, .. } => *weight,
            StrategyEvidence::TrendFollowing { weight, .. } => *weight,
            StrategyEvidence::CounterTrend { weight, .. } => *weight,
            StrategyEvidence::RegularIntervals { weight, .. } => *weight,
            StrategyEvidence::SymmetricActivity { weight, .. } => *weight,
            StrategyEvidence::HighFrequency { weight, .. } => *weight,
            StrategyEvidence::DelayedExecution { weight, .. } => *weight,
            StrategyEvidence::LeaderCorrelation { weight, .. } => *weight,
            StrategyEvidence::HoldTime { weight, .. } => *weight,
        }
    }

    /// Get a human-readable description of this evidence.
    pub fn description(&self) -> String {
        match self {
            StrategyEvidence::OpposingPositions { count, .. } => {
                format!("{} opposing position instances", count)
            }
            StrategyEvidence::LowLatency { avg_ms, .. } => {
                format!("Average latency: {:.0}ms", avg_ms)
            }
            StrategyEvidence::HighWinRate { rate, trade_count, .. } => {
                format!("{:.1}% win rate over {} trades", rate * 100.0, trade_count)
            }
            StrategyEvidence::TrendFollowing { correlation, .. } => {
                format!("Trend correlation: {:.2}", correlation)
            }
            StrategyEvidence::CounterTrend { reversion_rate, .. } => {
                format!("Reversion rate: {:.1}%", reversion_rate * 100.0)
            }
            StrategyEvidence::RegularIntervals { interval_cv, .. } => {
                format!("Interval CV: {:.3}", interval_cv)
            }
            StrategyEvidence::SymmetricActivity { buy_sell_ratio, .. } => {
                format!("Buy/sell ratio: {:.2}", buy_sell_ratio)
            }
            StrategyEvidence::HighFrequency { trades_per_day, .. } => {
                format!("{:.0} trades/day", trades_per_day)
            }
            StrategyEvidence::DelayedExecution { avg_delay_seconds, .. } => {
                format!("Avg delay: {:.1}s", avg_delay_seconds)
            }
            StrategyEvidence::LeaderCorrelation { correlation, leader_address, .. } => {
                format!("Correlation {:.2} with {}", correlation, &leader_address[..8])
            }
            StrategyEvidence::HoldTime { avg_hours, .. } => {
                format!("Avg hold time: {:.1}h", avg_hours)
            }
        }
    }
}

/// Result of strategy classification for a wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyClassification {
    /// Wallet address.
    pub address: String,
    /// Primary detected strategy (highest confidence).
    pub primary_strategy: DetectedStrategy,
    /// All detected strategy signals, sorted by confidence.
    pub signals: Vec<StrategySignal>,
    /// When classification was computed.
    pub computed_at: DateTime<Utc>,
}

impl StrategyClassification {
    /// Create a new classification result.
    pub fn new(address: String, mut signals: Vec<StrategySignal>) -> Self {
        // Sort by confidence descending
        signals.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

        let primary_strategy = signals
            .first()
            .map(|s| s.strategy)
            .unwrap_or(DetectedStrategy::Unknown);

        Self {
            address,
            primary_strategy,
            signals,
            computed_at: Utc::now(),
        }
    }

    /// Get strategies with confidence above threshold.
    pub fn strategies_above_threshold(&self, threshold: f64) -> Vec<&StrategySignal> {
        self.signals
            .iter()
            .filter(|s| s.confidence >= threshold)
            .collect()
    }

    /// Check if wallet uses multiple strategies.
    pub fn is_multi_strategy(&self, threshold: f64) -> bool {
        self.strategies_above_threshold(threshold).len() > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detected_strategy_names() {
        assert_eq!(DetectedStrategy::Arbitrage.name(), "Arbitrage");
        assert_eq!(DetectedStrategy::Momentum.name(), "Momentum");
        assert_eq!(DetectedStrategy::Unknown.name(), "Unknown");
    }

    #[test]
    fn test_strategy_signal_creation() {
        let evidence = vec![
            StrategyEvidence::OpposingPositions { count: 10, weight: 0.3 },
            StrategyEvidence::HighWinRate { rate: 0.92, trade_count: 150, weight: 0.4 },
        ];

        let signal = StrategySignal::new(DetectedStrategy::Arbitrage, 0.85, evidence);

        assert_eq!(signal.strategy, DetectedStrategy::Arbitrage);
        assert!(signal.is_high_confidence());
        assert_eq!(signal.evidence.len(), 2);
    }

    #[test]
    fn test_strategy_classification() {
        let signals = vec![
            StrategySignal::new(DetectedStrategy::Arbitrage, 0.85, vec![]),
            StrategySignal::new(DetectedStrategy::Momentum, 0.45, vec![]),
            StrategySignal::new(DetectedStrategy::GridTrading, 0.30, vec![]),
        ];

        let classification = StrategyClassification::new("0x123".to_string(), signals);

        assert_eq!(classification.primary_strategy, DetectedStrategy::Arbitrage);
        assert_eq!(classification.strategies_above_threshold(0.4).len(), 2);
        assert!(classification.is_multi_strategy(0.4));
    }

    #[test]
    fn test_evidence_weight() {
        let evidence = StrategyEvidence::HighWinRate {
            rate: 0.9,
            trade_count: 100,
            weight: 0.5,
        };
        assert_eq!(evidence.weight(), 0.5);
    }
}
