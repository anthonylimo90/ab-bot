//! Advanced success prediction models.
//!
//! Enhanced prediction with ensemble methods, feature engineering,
//! market condition awareness, and time-series analysis.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::debug;

use crate::profitability::{ProfitabilityAnalyzer, TimePeriod, WalletMetrics};
use crate::success_predictor::{PredictionCategory, PredictionFactor, SuccessPrediction};

/// Ensemble prediction model combining multiple predictors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsemblePrediction {
    pub address: String,
    /// Final ensemble probability.
    pub probability: f64,
    /// Confidence from model agreement.
    pub confidence: f64,
    /// Category based on ensemble.
    pub category: PredictionCategory,
    /// Individual model predictions.
    pub model_predictions: Vec<ModelPrediction>,
    /// Ensemble weights used.
    pub weights: HashMap<String, f64>,
    /// When prediction was made.
    pub predicted_at: DateTime<Utc>,
}

/// Individual model prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrediction {
    pub model_name: String,
    pub probability: f64,
    pub weight: f64,
    pub features_used: Vec<String>,
}

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

/// Feature set for advanced prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionFeatures {
    // Core metrics
    pub win_rate: f64,
    pub sharpe_ratio: f64,
    pub sortino_ratio: f64,
    pub max_drawdown: f64,
    pub roi: f64,
    pub consistency: f64,

    // Trade characteristics
    pub total_trades: u64,
    pub avg_trade_size: f64,
    pub avg_holding_period: f64,
    pub trade_frequency: f64,

    // Risk metrics
    pub volatility: f64,
    pub var_95: f64,  // Value at Risk 95%
    pub calmar_ratio: f64,

    // Time-based features
    pub recent_performance_7d: f64,
    pub recent_performance_30d: f64,
    pub performance_trend: f64,  // Slope of performance over time

    // Market correlation
    pub correlation_to_market: f64,
    pub alpha: f64,  // Excess return vs market
    pub beta: f64,   // Market sensitivity

    // Behavioral features
    pub timing_score: f64,  // Entry/exit timing quality
    pub position_sizing_score: f64,
    pub diversification_score: f64,

    // Category performance
    pub category_specialization: Option<String>,
    pub category_win_rate: f64,
}

/// Advanced predictor with ensemble methods.
pub struct AdvancedPredictor {
    pool: PgPool,
    profitability_analyzer: ProfitabilityAnalyzer,
    model_weights: HashMap<String, f64>,
    market_regime: MarketRegime,
    regime_adjustments: HashMap<MarketRegime, f64>,
}

impl AdvancedPredictor {
    /// Create a new advanced predictor.
    pub fn new(pool: PgPool) -> Self {
        let profitability_analyzer = ProfitabilityAnalyzer::new(pool.clone());

        let mut model_weights = HashMap::new();
        model_weights.insert("statistical".to_string(), 0.30);
        model_weights.insert("momentum".to_string(), 0.25);
        model_weights.insert("risk_adjusted".to_string(), 0.25);
        model_weights.insert("behavioral".to_string(), 0.20);

        let mut regime_adjustments = HashMap::new();
        regime_adjustments.insert(MarketRegime::BullVolatile, 0.9);
        regime_adjustments.insert(MarketRegime::BullCalm, 1.1);
        regime_adjustments.insert(MarketRegime::BearVolatile, 0.7);
        regime_adjustments.insert(MarketRegime::BearCalm, 0.85);
        regime_adjustments.insert(MarketRegime::Ranging, 1.0);
        regime_adjustments.insert(MarketRegime::Uncertain, 0.8);

        Self {
            pool,
            profitability_analyzer,
            model_weights,
            market_regime: MarketRegime::Uncertain,
            regime_adjustments,
        }
    }

    /// Set current market regime.
    pub fn set_market_regime(&mut self, regime: MarketRegime) {
        self.market_regime = regime;
    }

    /// Update model weights.
    pub fn set_model_weights(&mut self, weights: HashMap<String, f64>) {
        self.model_weights = weights;
    }

    /// Predict using ensemble of models.
    pub async fn predict_ensemble(&self, address: &str) -> Result<EnsemblePrediction> {
        let metrics = self.profitability_analyzer
            .calculate_metrics(address, TimePeriod::Month)
            .await?;

        let features = self.extract_features(&metrics).await?;
        self.ensemble_predict(address, &features)
    }

    /// Extract prediction features from metrics.
    async fn extract_features(&self, metrics: &WalletMetrics) -> Result<PredictionFeatures> {
        // Calculate additional features
        let calmar = if metrics.max_drawdown > 0.0 {
            metrics.annualized_return / metrics.max_drawdown
        } else {
            0.0
        };

        // Calculate VaR (simplified - assumes normal distribution)
        let var_95 = metrics.volatility * 1.645; // 95% confidence

        // Get recent performance
        let recent_7d = self.get_recent_performance(
            &metrics.address,
            Duration::days(7),
        ).await.unwrap_or(0.0);

        let recent_30d = self.get_recent_performance(
            &metrics.address,
            Duration::days(30),
        ).await.unwrap_or(0.0);

        // Performance trend (simplified slope)
        let trend = if recent_30d != 0.0 {
            (recent_7d - recent_30d / 4.0) / (recent_30d / 4.0).abs().max(0.01)
        } else {
            0.0
        };

        Ok(PredictionFeatures {
            win_rate: metrics.win_rate,
            sharpe_ratio: metrics.sharpe_ratio,
            sortino_ratio: metrics.sortino_ratio,
            max_drawdown: metrics.max_drawdown,
            roi: metrics.roi_percentage,
            consistency: metrics.consistency_score,
            total_trades: metrics.total_trades,
            avg_trade_size: rust_decimal_to_f64(metrics.avg_position_size),
            avg_holding_period: metrics.avg_holding_period_hours,
            trade_frequency: metrics.total_trades as f64 / 30.0, // trades per day
            volatility: metrics.volatility,
            var_95,
            calmar_ratio: calmar,
            recent_performance_7d: recent_7d,
            recent_performance_30d: recent_30d,
            performance_trend: trend,
            correlation_to_market: 0.0, // Would need market data
            alpha: 0.0,
            beta: 1.0,
            timing_score: self.calculate_timing_score(metrics),
            position_sizing_score: self.calculate_position_sizing_score(metrics),
            diversification_score: 0.5, // Would need position data
            category_specialization: None,
            category_win_rate: metrics.win_rate,
        })
    }

    /// Get recent performance for a wallet.
    async fn get_recent_performance(&self, address: &str, period: Duration) -> Result<f64> {
        let since = Utc::now() - period;

        let result: Option<Decimal> = sqlx::query_scalar(
            r#"
            SELECT COALESCE(SUM(pnl), 0) as total_pnl
            FROM copy_trade_history
            WHERE source_wallet = $1 AND created_at >= $2
            "#,
        )
        .bind(address)
        .bind(since)
        .fetch_optional(&self.pool)
        .await?;

        Ok(result.map(rust_decimal_to_f64).unwrap_or(0.0))
    }

    fn calculate_timing_score(&self, metrics: &WalletMetrics) -> f64 {
        // Timing score based on win rate and profit factor
        let base = metrics.win_rate * 0.5;
        let profit_bonus = (metrics.profit_factor - 1.0).max(0.0).min(2.0) * 0.25;
        (base + profit_bonus).min(1.0)
    }

    fn calculate_position_sizing_score(&self, metrics: &WalletMetrics) -> f64 {
        // Position sizing score based on drawdown and consistency
        let drawdown_penalty = metrics.max_drawdown.min(0.5);
        let consistency_bonus = metrics.consistency_score * 0.5;
        (0.5 + consistency_bonus - drawdown_penalty).max(0.0).min(1.0)
    }

    fn ensemble_predict(&self, address: &str, features: &PredictionFeatures) -> Result<EnsemblePrediction> {
        let mut predictions = Vec::new();
        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;

        // Statistical model
        let stat_pred = self.statistical_model(features);
        let stat_weight = *self.model_weights.get("statistical").unwrap_or(&0.25);
        predictions.push(ModelPrediction {
            model_name: "statistical".to_string(),
            probability: stat_pred,
            weight: stat_weight,
            features_used: vec!["sharpe_ratio", "sortino_ratio", "win_rate", "consistency"]
                .into_iter().map(String::from).collect(),
        });
        weighted_sum += stat_pred * stat_weight;
        total_weight += stat_weight;

        // Momentum model
        let mom_pred = self.momentum_model(features);
        let mom_weight = *self.model_weights.get("momentum").unwrap_or(&0.25);
        predictions.push(ModelPrediction {
            model_name: "momentum".to_string(),
            probability: mom_pred,
            weight: mom_weight,
            features_used: vec!["recent_performance_7d", "recent_performance_30d", "performance_trend"]
                .into_iter().map(String::from).collect(),
        });
        weighted_sum += mom_pred * mom_weight;
        total_weight += mom_weight;

        // Risk-adjusted model
        let risk_pred = self.risk_adjusted_model(features);
        let risk_weight = *self.model_weights.get("risk_adjusted").unwrap_or(&0.25);
        predictions.push(ModelPrediction {
            model_name: "risk_adjusted".to_string(),
            probability: risk_pred,
            weight: risk_weight,
            features_used: vec!["calmar_ratio", "var_95", "max_drawdown", "volatility"]
                .into_iter().map(String::from).collect(),
        });
        weighted_sum += risk_pred * risk_weight;
        total_weight += risk_weight;

        // Behavioral model
        let behav_pred = self.behavioral_model(features);
        let behav_weight = *self.model_weights.get("behavioral").unwrap_or(&0.20);
        predictions.push(ModelPrediction {
            model_name: "behavioral".to_string(),
            probability: behav_pred,
            weight: behav_weight,
            features_used: vec!["timing_score", "position_sizing_score", "trade_frequency"]
                .into_iter().map(String::from).collect(),
        });
        weighted_sum += behav_pred * behav_weight;
        total_weight += behav_weight;

        // Calculate ensemble probability
        let mut probability = weighted_sum / total_weight;

        // Apply market regime adjustment
        let regime_adj = *self.regime_adjustments.get(&self.market_regime).unwrap_or(&1.0);
        probability = (probability * regime_adj).max(0.0).min(1.0);

        // Calculate confidence from model agreement
        let probs: Vec<f64> = predictions.iter().map(|p| p.probability).collect();
        let mean = probs.iter().sum::<f64>() / probs.len() as f64;
        let variance = probs.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / probs.len() as f64;
        let std_dev = variance.sqrt();

        // Lower std_dev = higher confidence (models agree)
        let confidence = (1.0 - std_dev * 2.0).max(0.0).min(1.0);

        // Data quality adjustment
        let data_confidence = self.data_quality_confidence(features);
        let final_confidence = confidence * 0.6 + data_confidence * 0.4;

        let category = PredictionCategory::from_probability(probability, final_confidence);

        Ok(EnsemblePrediction {
            address: address.to_string(),
            probability,
            confidence: final_confidence,
            category,
            model_predictions: predictions,
            weights: self.model_weights.clone(),
            predicted_at: Utc::now(),
        })
    }

    fn statistical_model(&self, features: &PredictionFeatures) -> f64 {
        // Weighted combination of statistical measures
        let sharpe_score = (features.sharpe_ratio / 3.0).max(0.0).min(1.0);
        let sortino_score = (features.sortino_ratio / 4.0).max(0.0).min(1.0);
        let win_rate_score = features.win_rate;
        let consistency_score = features.consistency;

        0.3 * sharpe_score + 0.25 * sortino_score + 0.25 * win_rate_score + 0.2 * consistency_score
    }

    fn momentum_model(&self, features: &PredictionFeatures) -> f64 {
        // Recent performance and trend
        let recent_score = if features.recent_performance_30d > 0.0 {
            (features.recent_performance_30d / 0.2).min(1.0) // Cap at 20% monthly return
        } else {
            (features.recent_performance_30d / 0.1).max(-1.0) * 0.5 + 0.5 // Penalize losses
        };

        let trend_score = (features.performance_trend * 0.5 + 0.5).max(0.0).min(1.0);
        let acceleration = if features.recent_performance_7d > features.recent_performance_30d / 4.0 {
            0.6 // Accelerating
        } else {
            0.4 // Decelerating
        };

        0.4 * recent_score + 0.3 * trend_score + 0.3 * acceleration
    }

    fn risk_adjusted_model(&self, features: &PredictionFeatures) -> f64 {
        // Focus on risk-adjusted returns
        let calmar_score = (features.calmar_ratio / 5.0).max(0.0).min(1.0);
        let drawdown_score = (1.0 - features.max_drawdown * 2.0).max(0.0);
        let var_score = (1.0 - features.var_95 / 0.3).max(0.0).min(1.0);
        let vol_score = (1.0 - features.volatility / 0.5).max(0.0).min(1.0);

        0.3 * calmar_score + 0.3 * drawdown_score + 0.2 * var_score + 0.2 * vol_score
    }

    fn behavioral_model(&self, features: &PredictionFeatures) -> f64 {
        // Trading behavior quality
        let timing = features.timing_score;
        let sizing = features.position_sizing_score;

        // Trade frequency score (not too much, not too little)
        let frequency_score = if features.trade_frequency < 0.5 {
            features.trade_frequency * 2.0 // Too few trades
        } else if features.trade_frequency > 10.0 {
            (20.0 - features.trade_frequency) / 10.0 // Too many trades
        } else {
            1.0 // Good frequency
        };
        let frequency_score = frequency_score.max(0.0).min(1.0);

        // Experience score
        let experience_score = ((features.total_trades as f64).ln() / 5.0).min(1.0).max(0.0);

        0.3 * timing + 0.3 * sizing + 0.2 * frequency_score + 0.2 * experience_score
    }

    fn data_quality_confidence(&self, features: &PredictionFeatures) -> f64 {
        let mut confidence: f64 = 0.0;

        // More trades = more confident
        if features.total_trades >= 100 {
            confidence += 0.4;
        } else if features.total_trades >= 50 {
            confidence += 0.3;
        } else if features.total_trades >= 20 {
            confidence += 0.2;
        } else if features.total_trades >= 10 {
            confidence += 0.1;
        }

        // Recent activity
        if features.recent_performance_7d != 0.0 {
            confidence += 0.2;
        }

        // Lower volatility = more stable metrics
        if features.volatility < 0.2 {
            confidence += 0.2;
        } else if features.volatility < 0.4 {
            confidence += 0.1;
        }

        // Consistent performance
        if features.consistency > 0.6 {
            confidence += 0.2;
        }

        confidence.min(1.0)
    }

    /// Get top wallets by ensemble prediction.
    pub async fn get_top_wallets(&self, addresses: &[String], n: usize) -> Result<Vec<EnsemblePrediction>> {
        let mut predictions = Vec::new();

        for address in addresses {
            match self.predict_ensemble(address).await {
                Ok(pred) => predictions.push(pred),
                Err(e) => debug!(address = %address, error = %e, "Failed to predict"),
            }
        }

        // Sort by probability descending
        predictions.sort_by(|a, b| {
            b.probability.partial_cmp(&a.probability).unwrap_or(std::cmp::Ordering::Equal)
        });

        predictions.truncate(n);
        Ok(predictions)
    }

    /// Explain prediction factors.
    pub fn explain_prediction(&self, prediction: &EnsemblePrediction) -> Vec<PredictionFactor> {
        let mut factors = Vec::new();

        for model in &prediction.model_predictions {
            let impact = model.probability * model.weight;
            factors.push(PredictionFactor::new(
                &model.model_name,
                model.probability,
                model.weight,
            ));
        }

        // Add regime factor
        let regime_adj = *self.regime_adjustments.get(&self.market_regime).unwrap_or(&1.0);
        factors.push(PredictionFactor::new(
            "market_regime",
            regime_adj,
            0.1, // Display weight
        ));

        factors
    }
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
        // Simplified trend calculation
        // Would typically use price data
        Ok(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_features() -> PredictionFeatures {
        PredictionFeatures {
            win_rate: 0.65,
            sharpe_ratio: 1.8,
            sortino_ratio: 2.2,
            max_drawdown: 0.15,
            roi: 0.12,
            consistency: 0.7,
            total_trades: 80,
            avg_trade_size: 500.0,
            avg_holding_period: 48.0,
            trade_frequency: 2.5,
            volatility: 0.2,
            var_95: 0.033,
            calmar_ratio: 0.8,
            recent_performance_7d: 0.02,
            recent_performance_30d: 0.08,
            performance_trend: 0.1,
            correlation_to_market: 0.3,
            alpha: 0.05,
            beta: 0.8,
            timing_score: 0.7,
            position_sizing_score: 0.75,
            diversification_score: 0.6,
            category_specialization: Some("crypto".to_string()),
            category_win_rate: 0.68,
        }
    }

    #[test]
    #[ignore = "Requires database connection - use integration tests"]
    fn test_statistical_model() {
        let pool = create_mock_pool();
        let predictor = AdvancedPredictor::new(pool);
        let features = create_test_features();

        let prob = predictor.statistical_model(&features);
        assert!(prob > 0.0 && prob <= 1.0);
        assert!(prob > 0.5); // Good metrics should produce good score
    }

    #[test]
    #[ignore = "Requires database connection - use integration tests"]
    fn test_momentum_model() {
        let pool = create_mock_pool();
        let predictor = AdvancedPredictor::new(pool);
        let features = create_test_features();

        let prob = predictor.momentum_model(&features);
        assert!(prob > 0.0 && prob <= 1.0);
    }

    #[test]
    #[ignore = "Requires database connection - use integration tests"]
    fn test_risk_adjusted_model() {
        let pool = create_mock_pool();
        let predictor = AdvancedPredictor::new(pool);
        let features = create_test_features();

        let prob = predictor.risk_adjusted_model(&features);
        assert!(prob > 0.0 && prob <= 1.0);
    }

    #[test]
    #[ignore = "Requires database connection - use integration tests"]
    fn test_behavioral_model() {
        let pool = create_mock_pool();
        let predictor = AdvancedPredictor::new(pool);
        let features = create_test_features();

        let prob = predictor.behavioral_model(&features);
        assert!(prob > 0.0 && prob <= 1.0);
    }

    #[test]
    #[ignore = "Requires database connection - use integration tests"]
    fn test_data_quality_confidence() {
        let pool = create_mock_pool();
        let predictor = AdvancedPredictor::new(pool);

        let good_features = create_test_features();
        let good_conf = predictor.data_quality_confidence(&good_features);
        assert!(good_conf > 0.5);

        let poor_features = PredictionFeatures {
            total_trades: 5,
            volatility: 0.5,
            consistency: 0.3,
            recent_performance_7d: 0.0,
            ..good_features
        };
        let poor_conf = predictor.data_quality_confidence(&poor_features);
        assert!(poor_conf < good_conf);
    }

    #[test]
    fn test_market_regime() {
        assert_eq!(MarketRegime::BullVolatile, MarketRegime::BullVolatile);
        assert_ne!(MarketRegime::BullVolatile, MarketRegime::BearVolatile);
    }

    // Mock pool for testing
    fn create_mock_pool() -> PgPool {
        // In real tests, use testcontainers or mock
        panic!("Use integration tests for database-dependent features")
    }
}
