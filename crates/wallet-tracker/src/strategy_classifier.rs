//! Rule-based strategy classification for wallets.
//!
//! Analyzes wallet trading behavior to identify which trading strategy
//! (or strategies) the wallet is likely using.

use polymarket_core::types::{
    DetectedStrategy, StrategyClassification, StrategyEvidence, StrategySignal, WalletFeatures,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Configuration for strategy classification thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierConfig {
    // Arbitrage thresholds
    pub arb_min_opposing_positions: u64,
    pub arb_max_latency_ms: f64,
    pub arb_min_win_rate: f64,

    // Momentum thresholds
    pub momentum_min_trend_correlation: f64,
    pub momentum_min_hold_hours: f64,
    pub momentum_max_hold_hours: f64,

    // Mean Reversion thresholds
    pub mean_reversion_min_reversion_rate: f64,
    pub mean_reversion_max_hold_hours: f64,

    // Grid Trading thresholds
    pub grid_max_interval_cv: f64,
    pub grid_min_buy_sell_ratio: f64,
    pub grid_max_buy_sell_ratio: f64,

    // Market Making thresholds
    pub mm_min_trades_per_day: f64,
    pub mm_min_buy_sell_ratio: f64,
    pub mm_max_buy_sell_ratio: f64,

    // Copy Trading thresholds
    pub copy_min_delay_seconds: f64,
    pub copy_max_delay_seconds: f64,
    pub copy_min_leader_correlation: f64,

    /// Minimum confidence to include a strategy in results.
    pub min_confidence_threshold: f64,
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self {
            // Arbitrage: opposing positions, low latency, high win rate
            arb_min_opposing_positions: 5,
            arb_max_latency_ms: 500.0,
            arb_min_win_rate: 0.85,

            // Momentum: trend following, moderate hold times
            momentum_min_trend_correlation: 0.6,
            momentum_min_hold_hours: 1.0,
            momentum_max_hold_hours: 24.0,

            // Mean Reversion: counter-trend, quick reversals
            mean_reversion_min_reversion_rate: 0.6,
            mean_reversion_max_hold_hours: 12.0,

            // Grid Trading: regular intervals, symmetric activity
            grid_max_interval_cv: 0.15,
            grid_min_buy_sell_ratio: 0.8,
            grid_max_buy_sell_ratio: 1.2,

            // Market Making: high frequency, both sides
            mm_min_trades_per_day: 100.0,
            mm_min_buy_sell_ratio: 0.7,
            mm_max_buy_sell_ratio: 1.3,

            // Copy Trading: delayed execution, follows leaders
            copy_min_delay_seconds: 1.0,
            copy_max_delay_seconds: 60.0,
            copy_min_leader_correlation: 0.8,

            min_confidence_threshold: 0.3,
        }
    }
}

/// Extended wallet features for strategy classification.
/// These supplement the base WalletFeatures with strategy-specific signals.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtendedFeatures {
    /// Base wallet features.
    pub base: WalletFeatures,

    /// Trend correlation (positive = momentum, negative = mean reversion).
    pub trend_correlation: Option<f64>,

    /// Rate of price reversion after trades.
    pub reversion_rate: Option<f64>,

    /// Average hold time in hours.
    pub avg_hold_hours: Option<f64>,

    /// Ratio of buy to sell trades.
    pub buy_sell_ratio: Option<f64>,

    /// Trades per day average.
    pub trades_per_day: Option<f64>,

    /// Average execution delay from market events (seconds).
    pub avg_execution_delay_seconds: Option<f64>,

    /// Correlation with known leader wallets.
    pub leader_correlations: Vec<(String, f64)>,

    /// Total buy trades.
    pub buy_count: u64,

    /// Total sell trades.
    pub sell_count: u64,
}

impl ExtendedFeatures {
    /// Create from base features with additional computed metrics.
    pub fn from_base(base: WalletFeatures) -> Self {
        let mut extended = Self {
            base: base.clone(),
            ..Default::default()
        };

        // Calculate trades per day if we have date range
        if let (Some(first), Some(last)) = (base.first_trade, base.last_trade) {
            let days = (last - first).num_days().max(1) as f64;
            extended.trades_per_day = Some(base.total_trades as f64 / days);
        }

        extended
    }

    /// Add buy/sell counts.
    pub fn with_trade_counts(mut self, buy_count: u64, sell_count: u64) -> Self {
        self.buy_count = buy_count;
        self.sell_count = sell_count;

        if sell_count > 0 {
            self.buy_sell_ratio = Some(buy_count as f64 / sell_count as f64);
        }

        self
    }

    /// Add hold time analysis.
    pub fn with_hold_time(mut self, avg_hold_hours: f64) -> Self {
        self.avg_hold_hours = Some(avg_hold_hours);
        self
    }

    /// Add trend correlation.
    pub fn with_trend_correlation(mut self, correlation: f64) -> Self {
        self.trend_correlation = Some(correlation);
        self
    }

    /// Add reversion rate.
    pub fn with_reversion_rate(mut self, rate: f64) -> Self {
        self.reversion_rate = Some(rate);
        self
    }

    /// Add execution delay.
    pub fn with_execution_delay(mut self, avg_seconds: f64) -> Self {
        self.avg_execution_delay_seconds = Some(avg_seconds);
        self
    }

    /// Add leader correlations.
    pub fn with_leader_correlations(mut self, correlations: Vec<(String, f64)>) -> Self {
        self.leader_correlations = correlations;
        self
    }
}

/// Strategy classifier using rule-based scoring.
pub struct StrategyClassifier {
    config: ClassifierConfig,
}

impl StrategyClassifier {
    /// Create a new classifier with default config.
    pub fn new() -> Self {
        Self {
            config: ClassifierConfig::default(),
        }
    }

    /// Create with custom config.
    pub fn with_config(config: ClassifierConfig) -> Self {
        Self { config }
    }

    /// Classify a wallet's trading strategy based on extended features.
    pub fn classify(&self, features: &ExtendedFeatures) -> StrategyClassification {
        let mut signals = Vec::new();

        // Score each strategy
        if let Some(signal) = self.score_arbitrage(features) {
            signals.push(signal);
        }
        if let Some(signal) = self.score_momentum(features) {
            signals.push(signal);
        }
        if let Some(signal) = self.score_mean_reversion(features) {
            signals.push(signal);
        }
        if let Some(signal) = self.score_grid_trading(features) {
            signals.push(signal);
        }
        if let Some(signal) = self.score_market_making(features) {
            signals.push(signal);
        }
        if let Some(signal) = self.score_copy_trading(features) {
            signals.push(signal);
        }

        // Filter by minimum confidence
        signals.retain(|s| s.confidence >= self.config.min_confidence_threshold);

        // If no strategies detected, add Unknown
        if signals.is_empty() {
            signals.push(StrategySignal::new(DetectedStrategy::Unknown, 1.0, vec![]));
        }

        debug!(
            address = %features.base.address,
            strategies = signals.len(),
            primary = ?signals.first().map(|s| s.strategy),
            "Classified wallet strategy"
        );

        StrategyClassification::new(features.base.address.clone(), signals)
    }

    /// Classify from base features (with limited analysis).
    pub fn classify_basic(&self, features: &WalletFeatures) -> StrategyClassification {
        let extended = ExtendedFeatures::from_base(features.clone());
        self.classify(&extended)
    }

    /// Score arbitrage strategy indicators.
    fn score_arbitrage(&self, features: &ExtendedFeatures) -> Option<StrategySignal> {
        let mut score = 0.0;
        let mut evidence = Vec::new();

        // Check opposing positions (strong indicator)
        if features.base.has_opposing_positions
            && features.base.opposing_position_count >= self.config.arb_min_opposing_positions
        {
            let weight = 0.4;
            score += weight;
            evidence.push(StrategyEvidence::OpposingPositions {
                count: features.base.opposing_position_count,
                weight,
            });
        }

        // Check low latency
        if let Some(latency) = features.base.avg_latency_ms {
            if latency <= self.config.arb_max_latency_ms {
                let weight = 0.3;
                score += weight;
                evidence.push(StrategyEvidence::LowLatency {
                    avg_ms: latency,
                    weight,
                });
            }
        }

        // Check high win rate
        if let Some(win_rate) = features.base.win_rate {
            if win_rate >= self.config.arb_min_win_rate && features.base.total_trades >= 50 {
                let weight = 0.3;
                score += weight;
                evidence.push(StrategyEvidence::HighWinRate {
                    rate: win_rate,
                    trade_count: features.base.total_trades,
                    weight,
                });
            }
        }

        if score > 0.0 {
            Some(StrategySignal::new(
                DetectedStrategy::Arbitrage,
                score,
                evidence,
            ))
        } else {
            None
        }
    }

    /// Score momentum strategy indicators.
    fn score_momentum(&self, features: &ExtendedFeatures) -> Option<StrategySignal> {
        let mut score = 0.0;
        let mut evidence = Vec::new();

        // Check trend correlation
        if let Some(corr) = features.trend_correlation {
            if corr >= self.config.momentum_min_trend_correlation {
                let weight = 0.5;
                score += weight;
                evidence.push(StrategyEvidence::TrendFollowing {
                    correlation: corr,
                    weight,
                });
            }
        }

        // Check hold time
        if let Some(hold_hours) = features.avg_hold_hours {
            if hold_hours >= self.config.momentum_min_hold_hours
                && hold_hours <= self.config.momentum_max_hold_hours
            {
                let weight = 0.3;
                score += weight;
                evidence.push(StrategyEvidence::HoldTime {
                    avg_hours: hold_hours,
                    weight,
                });
            }
        }

        // Volume/activity spread suggests active trading
        if features.base.activity_spread >= 0.5 {
            let weight = 0.2;
            score += weight;
        }

        if score > 0.0 {
            Some(StrategySignal::new(
                DetectedStrategy::Momentum,
                score,
                evidence,
            ))
        } else {
            None
        }
    }

    /// Score mean reversion strategy indicators.
    fn score_mean_reversion(&self, features: &ExtendedFeatures) -> Option<StrategySignal> {
        let mut score = 0.0;
        let mut evidence = Vec::new();

        // Check counter-trend correlation
        if let Some(corr) = features.trend_correlation {
            if corr <= -self.config.momentum_min_trend_correlation {
                let weight = 0.4;
                score += weight;
                evidence.push(StrategyEvidence::CounterTrend {
                    reversion_rate: corr.abs(),
                    weight,
                });
            }
        }

        // Check reversion rate
        if let Some(rate) = features.reversion_rate {
            if rate >= self.config.mean_reversion_min_reversion_rate {
                let weight = 0.3;
                score += weight;
                evidence.push(StrategyEvidence::CounterTrend {
                    reversion_rate: rate,
                    weight,
                });
            }
        }

        // Quick hold times
        if let Some(hold_hours) = features.avg_hold_hours {
            if hold_hours <= self.config.mean_reversion_max_hold_hours && hold_hours > 0.0 {
                let weight = 0.3;
                score += weight;
                evidence.push(StrategyEvidence::HoldTime {
                    avg_hours: hold_hours,
                    weight,
                });
            }
        }

        if score > 0.0 {
            Some(StrategySignal::new(
                DetectedStrategy::MeanReversion,
                score,
                evidence,
            ))
        } else {
            None
        }
    }

    /// Score grid trading strategy indicators.
    fn score_grid_trading(&self, features: &ExtendedFeatures) -> Option<StrategySignal> {
        let mut score = 0.0;
        let mut evidence = Vec::new();

        // Check regular intervals (low CV)
        if let Some(cv) = features.base.interval_cv {
            if cv <= self.config.grid_max_interval_cv {
                let weight = 0.4;
                score += weight;
                evidence.push(StrategyEvidence::RegularIntervals {
                    interval_cv: cv,
                    weight,
                });
            }
        }

        // Check symmetric buy/sell
        if let Some(ratio) = features.buy_sell_ratio {
            if ratio >= self.config.grid_min_buy_sell_ratio
                && ratio <= self.config.grid_max_buy_sell_ratio
            {
                let weight = 0.4;
                score += weight;
                evidence.push(StrategyEvidence::SymmetricActivity {
                    buy_sell_ratio: ratio,
                    weight,
                });
            }
        }

        // Needs decent trade volume
        if features.base.total_trades >= 50 {
            score += 0.2;
        }

        if score > 0.0 {
            Some(StrategySignal::new(
                DetectedStrategy::GridTrading,
                score,
                evidence,
            ))
        } else {
            None
        }
    }

    /// Score market making strategy indicators.
    fn score_market_making(&self, features: &ExtendedFeatures) -> Option<StrategySignal> {
        let mut score = 0.0;
        let mut evidence = Vec::new();

        // Check high frequency
        if let Some(tpd) = features.trades_per_day {
            if tpd >= self.config.mm_min_trades_per_day {
                let weight = 0.4;
                score += weight;
                evidence.push(StrategyEvidence::HighFrequency {
                    trades_per_day: tpd,
                    weight,
                });
            }
        }

        // Check both-sided activity
        if let Some(ratio) = features.buy_sell_ratio {
            if ratio >= self.config.mm_min_buy_sell_ratio
                && ratio <= self.config.mm_max_buy_sell_ratio
            {
                let weight = 0.3;
                score += weight;
                evidence.push(StrategyEvidence::SymmetricActivity {
                    buy_sell_ratio: ratio,
                    weight,
                });
            }
        }

        // Check 24/7 activity
        if features.base.is_24_7_active() {
            score += 0.2;
        }

        // Low latency common for MM
        if let Some(latency) = features.base.avg_latency_ms {
            if latency <= 1000.0 {
                let weight = 0.1;
                score += weight;
                evidence.push(StrategyEvidence::LowLatency {
                    avg_ms: latency,
                    weight,
                });
            }
        }

        if score > 0.0 {
            Some(StrategySignal::new(
                DetectedStrategy::MarketMaking,
                score,
                evidence,
            ))
        } else {
            None
        }
    }

    /// Score copy trading strategy indicators.
    fn score_copy_trading(&self, features: &ExtendedFeatures) -> Option<StrategySignal> {
        let mut score = 0.0;
        let mut evidence = Vec::new();

        // Check execution delay
        if let Some(delay) = features.avg_execution_delay_seconds {
            if delay >= self.config.copy_min_delay_seconds
                && delay <= self.config.copy_max_delay_seconds
            {
                let weight = 0.4;
                score += weight;
                evidence.push(StrategyEvidence::DelayedExecution {
                    avg_delay_seconds: delay,
                    weight,
                });
            }
        }

        // Check leader correlations
        for (leader, corr) in &features.leader_correlations {
            if *corr >= self.config.copy_min_leader_correlation {
                let weight = 0.5;
                score += weight;
                evidence.push(StrategyEvidence::LeaderCorrelation {
                    correlation: *corr,
                    leader_address: leader.clone(),
                    weight,
                });
                break; // Only count one leader
            }
        }

        if score > 0.0 {
            Some(StrategySignal::new(
                DetectedStrategy::CopyTrading,
                score,
                evidence,
            ))
        } else {
            None
        }
    }
}

impl Default for StrategyClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn mock_arb_features() -> ExtendedFeatures {
        ExtendedFeatures {
            base: WalletFeatures {
                address: "0xarb".to_string(),
                total_trades: 150,
                interval_cv: Some(0.08),
                win_rate: Some(0.92),
                avg_latency_ms: Some(100.0),
                has_opposing_positions: true,
                opposing_position_count: 25,
                hourly_distribution: [1; 24],
                activity_spread: 1.0,
                total_volume: Decimal::new(100000, 2),
                ..Default::default()
            },
            trades_per_day: Some(50.0),
            buy_sell_ratio: Some(1.0),
            ..Default::default()
        }
    }

    fn mock_momentum_features() -> ExtendedFeatures {
        ExtendedFeatures {
            base: WalletFeatures {
                address: "0xmom".to_string(),
                total_trades: 80,
                interval_cv: Some(0.5),
                win_rate: Some(0.65),
                avg_latency_ms: Some(3000.0),
                has_opposing_positions: false,
                opposing_position_count: 0,
                activity_spread: 0.6,
                total_volume: Decimal::new(50000, 2),
                ..Default::default()
            },
            trend_correlation: Some(0.75),
            avg_hold_hours: Some(8.0),
            trades_per_day: Some(10.0),
            buy_sell_ratio: Some(1.5),
            ..Default::default()
        }
    }

    fn mock_grid_features() -> ExtendedFeatures {
        ExtendedFeatures {
            base: WalletFeatures {
                address: "0xgrid".to_string(),
                total_trades: 200,
                interval_cv: Some(0.08),
                win_rate: Some(0.55),
                avg_latency_ms: Some(2000.0),
                has_opposing_positions: false,
                opposing_position_count: 0,
                activity_spread: 0.8,
                total_volume: Decimal::new(75000, 2),
                ..Default::default()
            },
            trades_per_day: Some(25.0),
            buy_sell_ratio: Some(1.02),
            ..Default::default()
        }
    }

    #[test]
    fn test_classify_arbitrage() {
        let classifier = StrategyClassifier::new();
        let features = mock_arb_features();
        let result = classifier.classify(&features);

        assert_eq!(result.primary_strategy, DetectedStrategy::Arbitrage);
        assert!(result.signals[0].confidence >= 0.7);
    }

    #[test]
    fn test_classify_momentum() {
        let classifier = StrategyClassifier::new();
        let features = mock_momentum_features();
        let result = classifier.classify(&features);

        assert_eq!(result.primary_strategy, DetectedStrategy::Momentum);
    }

    #[test]
    fn test_classify_grid_trading() {
        let classifier = StrategyClassifier::new();
        let features = mock_grid_features();
        let result = classifier.classify(&features);

        assert_eq!(result.primary_strategy, DetectedStrategy::GridTrading);
    }

    #[test]
    fn test_classify_unknown() {
        let classifier = StrategyClassifier::new();
        let features = ExtendedFeatures {
            base: WalletFeatures {
                address: "0xunknown".to_string(),
                total_trades: 10,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = classifier.classify(&features);

        assert_eq!(result.primary_strategy, DetectedStrategy::Unknown);
    }

    #[test]
    fn test_multi_strategy_detection() {
        let classifier = StrategyClassifier::new();

        // Features that show both arbitrage and grid trading patterns
        let features = ExtendedFeatures {
            base: WalletFeatures {
                address: "0xmulti".to_string(),
                total_trades: 200,
                interval_cv: Some(0.08),
                win_rate: Some(0.88),
                avg_latency_ms: Some(200.0),
                has_opposing_positions: true,
                opposing_position_count: 15,
                activity_spread: 0.9,
                total_volume: Decimal::new(100000, 2),
                ..Default::default()
            },
            buy_sell_ratio: Some(1.0),
            trades_per_day: Some(30.0),
            ..Default::default()
        };

        let result = classifier.classify(&features);

        // Should detect multiple strategies
        assert!(result.signals.len() >= 2);
        assert!(result.is_multi_strategy(0.3));
    }

    #[test]
    fn test_config_customization() {
        let config = ClassifierConfig {
            arb_min_opposing_positions: 10, // Stricter threshold
            ..Default::default()
        };

        let classifier = StrategyClassifier::with_config(config);

        // With only 5 opposing positions, shouldn't classify as arbitrage
        let features = ExtendedFeatures {
            base: WalletFeatures {
                address: "0xtest".to_string(),
                has_opposing_positions: true,
                opposing_position_count: 5,
                ..Default::default()
            },
            ..Default::default()
        };

        let result = classifier.classify(&features);

        // Primary should not be arbitrage with stricter threshold
        assert_ne!(result.primary_strategy, DetectedStrategy::Arbitrage);
    }
}
