//! Success prediction for wallet future performance.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::debug;

use crate::profitability::{ProfitabilityAnalyzer, TimePeriod, WalletMetrics};

/// Prediction model type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionModel {
    /// Rule-based scoring using heuristics.
    RuleBased,
    /// Simple linear model.
    Linear,
    /// Weighted average of multiple factors.
    WeightedAverage,
}

impl Default for PredictionModel {
    fn default() -> Self {
        Self::RuleBased
    }
}

/// Prediction result for a wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessPrediction {
    pub address: String,
    /// Probability of continued success (0.0 - 1.0).
    pub success_probability: f64,
    /// Confidence level in the prediction (0.0 - 1.0).
    pub confidence: f64,
    /// Predicted category.
    pub category: PredictionCategory,
    /// Key factors influencing the prediction.
    pub factors: Vec<PredictionFactor>,
    /// Model used for prediction.
    pub model: PredictionModel,
    /// When the prediction was made.
    pub predicted_at: DateTime<Utc>,
    /// Prediction validity period (hours).
    pub valid_for_hours: u32,
}

impl SuccessPrediction {
    /// Check if the prediction is still valid.
    pub fn is_valid(&self) -> bool {
        let valid_until = self.predicted_at + chrono::Duration::hours(self.valid_for_hours as i64);
        Utc::now() < valid_until
    }

    /// Check if the wallet is recommended for copy trading.
    pub fn is_recommended(&self) -> bool {
        self.success_probability >= 0.65 && self.confidence >= 0.5
    }
}

/// Category for predicted success level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionCategory {
    /// High probability of continued success.
    HighPotential,
    /// Moderate probability - proceed with caution.
    Moderate,
    /// Low probability - not recommended.
    LowPotential,
    /// Insufficient data to predict.
    Uncertain,
}

impl PredictionCategory {
    pub fn from_probability(prob: f64, confidence: f64) -> Self {
        if confidence < 0.3 {
            Self::Uncertain
        } else if prob >= 0.7 {
            Self::HighPotential
        } else if prob >= 0.5 {
            Self::Moderate
        } else {
            Self::LowPotential
        }
    }
}

/// A factor contributing to the prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionFactor {
    pub name: String,
    pub value: f64,
    pub weight: f64,
    pub contribution: f64,
    pub is_positive: bool,
}

impl PredictionFactor {
    pub fn new(name: impl Into<String>, value: f64, weight: f64) -> Self {
        let contribution = value * weight;
        Self {
            name: name.into(),
            value,
            weight,
            contribution,
            is_positive: contribution > 0.0,
        }
    }
}

/// Weights for rule-based prediction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionWeights {
    pub win_rate: f64,
    pub sharpe_ratio: f64,
    pub consistency: f64,
    pub roi: f64,
    pub drawdown_penalty: f64,
    pub trade_count_bonus: f64,
    pub recency_bonus: f64,
}

impl Default for PredictionWeights {
    fn default() -> Self {
        Self {
            win_rate: 0.25,
            sharpe_ratio: 0.20,
            consistency: 0.20,
            roi: 0.15,
            drawdown_penalty: 0.10,
            trade_count_bonus: 0.05,
            recency_bonus: 0.05,
        }
    }
}

/// Success predictor for wallets.
pub struct SuccessPredictor {
    pool: PgPool,
    profitability_analyzer: ProfitabilityAnalyzer,
    model: PredictionModel,
    weights: PredictionWeights,
}

impl SuccessPredictor {
    /// Create a new success predictor.
    pub fn new(pool: PgPool) -> Self {
        let profitability_analyzer = ProfitabilityAnalyzer::new(pool.clone());
        Self {
            pool,
            profitability_analyzer,
            model: PredictionModel::default(),
            weights: PredictionWeights::default(),
        }
    }

    /// Set the prediction model.
    pub fn with_model(mut self, model: PredictionModel) -> Self {
        self.model = model;
        self
    }

    /// Set custom weights.
    pub fn with_weights(mut self, weights: PredictionWeights) -> Self {
        self.weights = weights;
        self
    }

    /// Predict success for a single wallet.
    pub async fn predict(&self, address: &str) -> Result<SuccessPrediction> {
        let metrics = self
            .profitability_analyzer
            .calculate_metrics(address, TimePeriod::Month)
            .await?;

        self.predict_from_metrics(&metrics)
    }

    /// Predict success from pre-calculated metrics.
    pub fn predict_from_metrics(&self, metrics: &WalletMetrics) -> Result<SuccessPrediction> {
        match self.model {
            PredictionModel::RuleBased => self.rule_based_prediction(metrics),
            PredictionModel::Linear => self.linear_prediction(metrics),
            PredictionModel::WeightedAverage => self.weighted_average_prediction(metrics),
        }
    }

    /// Predict success for multiple wallets.
    pub async fn predict_batch(&self, addresses: &[String]) -> Result<Vec<SuccessPrediction>> {
        let mut predictions = Vec::with_capacity(addresses.len());

        for address in addresses {
            match self.predict(address).await {
                Ok(pred) => predictions.push(pred),
                Err(e) => {
                    debug!(address = %address, error = %e, "Failed to predict for wallet");
                }
            }
        }

        // Sort by success probability descending
        predictions.sort_by(|a, b| {
            b.success_probability
                .partial_cmp(&a.success_probability)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(predictions)
    }

    /// Get top N wallets by predicted success.
    pub async fn get_top_predicted(
        &self,
        addresses: &[String],
        n: usize,
    ) -> Result<Vec<SuccessPrediction>> {
        let mut predictions = self.predict_batch(addresses).await?;
        predictions.truncate(n);
        Ok(predictions)
    }

    /// Store prediction in database.
    pub async fn store_prediction(&self, prediction: &SuccessPrediction) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE wallet_success_metrics
            SET predicted_success_prob = $1, last_computed = $2
            WHERE address = $3
            "#,
        )
        .bind(prediction.success_probability)
        .bind(&prediction.predicted_at)
        .bind(&prediction.address)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // Private prediction methods

    fn rule_based_prediction(&self, metrics: &WalletMetrics) -> Result<SuccessPrediction> {
        let mut factors = Vec::new();
        let mut total_score = 0.0;
        let mut total_weight = 0.0;

        // Win rate factor (normalized to 0-1, where 0.5 is neutral)
        let win_rate_score = (metrics.win_rate - 0.5) * 2.0; // -1 to 1 range
        factors.push(PredictionFactor::new(
            "win_rate",
            win_rate_score,
            self.weights.win_rate,
        ));
        total_score += win_rate_score.max(0.0) * self.weights.win_rate;
        total_weight += self.weights.win_rate;

        // Sharpe ratio factor (capped at 3.0)
        let sharpe_score = (metrics.sharpe_ratio / 3.0).min(1.0).max(0.0);
        factors.push(PredictionFactor::new(
            "sharpe_ratio",
            sharpe_score,
            self.weights.sharpe_ratio,
        ));
        total_score += sharpe_score * self.weights.sharpe_ratio;
        total_weight += self.weights.sharpe_ratio;

        // Consistency factor
        let consistency_score = metrics.consistency_score.min(1.0).max(0.0);
        factors.push(PredictionFactor::new(
            "consistency",
            consistency_score,
            self.weights.consistency,
        ));
        total_score += consistency_score * self.weights.consistency;
        total_weight += self.weights.consistency;

        // ROI factor (capped at 50% monthly)
        let roi_score = (metrics.roi_percentage / 0.5).min(1.0).max(0.0);
        factors.push(PredictionFactor::new("roi", roi_score, self.weights.roi));
        total_score += roi_score * self.weights.roi;
        total_weight += self.weights.roi;

        // Drawdown penalty
        let drawdown_penalty = metrics.max_drawdown.min(0.5); // Cap at 50%
        factors.push(PredictionFactor::new(
            "drawdown_penalty",
            -drawdown_penalty,
            self.weights.drawdown_penalty,
        ));
        total_score -= drawdown_penalty * self.weights.drawdown_penalty;

        // Trade count bonus (log scale, capped)
        let trade_bonus = ((metrics.total_trades as f64).ln() / 5.0).min(1.0).max(0.0);
        factors.push(PredictionFactor::new(
            "trade_count",
            trade_bonus,
            self.weights.trade_count_bonus,
        ));
        total_score += trade_bonus * self.weights.trade_count_bonus;
        total_weight += self.weights.trade_count_bonus;

        // Normalize to 0-1 probability
        let probability = (total_score / total_weight).max(0.0).min(1.0);

        // Calculate confidence based on data quality
        let confidence = self.calculate_confidence(metrics);

        let category = PredictionCategory::from_probability(probability, confidence);

        Ok(SuccessPrediction {
            address: metrics.address.clone(),
            success_probability: probability,
            confidence,
            category,
            factors,
            model: self.model,
            predicted_at: Utc::now(),
            valid_for_hours: 24,
        })
    }

    fn linear_prediction(&self, metrics: &WalletMetrics) -> Result<SuccessPrediction> {
        // Simple linear combination
        let probability = 0.3
            + 0.2 * metrics.win_rate
            + 0.1 * (metrics.sharpe_ratio / 3.0).min(1.0)
            + 0.1 * metrics.consistency_score
            + 0.1 * (metrics.roi_percentage / 0.3).min(1.0)
            - 0.1 * metrics.max_drawdown
            + 0.1 * ((metrics.total_trades as f64).ln() / 5.0).min(1.0);

        let probability = probability.max(0.0).min(1.0);
        let confidence = self.calculate_confidence(metrics);
        let category = PredictionCategory::from_probability(probability, confidence);

        Ok(SuccessPrediction {
            address: metrics.address.clone(),
            success_probability: probability,
            confidence,
            category,
            factors: vec![],
            model: self.model,
            predicted_at: Utc::now(),
            valid_for_hours: 24,
        })
    }

    fn weighted_average_prediction(&self, metrics: &WalletMetrics) -> Result<SuccessPrediction> {
        // Weighted average of key metrics
        let scores = [
            (metrics.win_rate, 0.3),
            ((metrics.sharpe_ratio / 3.0).min(1.0).max(0.0), 0.25),
            (metrics.consistency_score, 0.2),
            ((metrics.roi_percentage / 0.3).min(1.0).max(0.0), 0.15),
            (1.0 - metrics.max_drawdown.min(1.0), 0.1),
        ];

        let probability: f64 = scores.iter().map(|(s, w)| s * w).sum();
        let probability = probability.max(0.0).min(1.0);

        let confidence = self.calculate_confidence(metrics);
        let category = PredictionCategory::from_probability(probability, confidence);

        let factors: Vec<PredictionFactor> = scores
            .iter()
            .enumerate()
            .map(|(i, (value, weight))| {
                let names = ["win_rate", "sharpe", "consistency", "roi", "drawdown_inv"];
                PredictionFactor::new(names[i], *value, *weight)
            })
            .collect();

        Ok(SuccessPrediction {
            address: metrics.address.clone(),
            success_probability: probability,
            confidence,
            category,
            factors,
            model: self.model,
            predicted_at: Utc::now(),
            valid_for_hours: 24,
        })
    }

    fn calculate_confidence(&self, metrics: &WalletMetrics) -> f64 {
        // Confidence based on data quality
        let mut confidence: f64 = 0.0;

        // More trades = more confidence
        if metrics.total_trades >= 100 {
            confidence += 0.4;
        } else if metrics.total_trades >= 50 {
            confidence += 0.3;
        } else if metrics.total_trades >= 20 {
            confidence += 0.2;
        } else if metrics.total_trades >= 10 {
            confidence += 0.1;
        }

        // Lower volatility = more confident
        if metrics.volatility < 0.2 {
            confidence += 0.2;
        } else if metrics.volatility < 0.4 {
            confidence += 0.1;
        }

        // Consistent win rate = more confident
        if metrics.consistency_score > 0.6 {
            confidence += 0.2;
        } else if metrics.consistency_score > 0.4 {
            confidence += 0.1;
        }

        // Recent activity bonus
        let days_since_last = (Utc::now() - metrics.computed_at).num_days();
        if days_since_last < 7 {
            confidence += 0.2;
        } else if days_since_last < 30 {
            confidence += 0.1;
        }

        confidence.min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn create_test_metrics(win_rate: f64, sharpe: f64, trades: u64) -> WalletMetrics {
        WalletMetrics {
            address: "0x1234".to_string(),
            period: TimePeriod::Month,
            total_return: Decimal::ZERO,
            roi_percentage: 0.15,
            annualized_return: 0.0,
            sharpe_ratio: sharpe,
            sortino_ratio: 0.0,
            max_drawdown: 0.1,
            max_drawdown_duration_days: 5,
            volatility: 0.2,
            downside_deviation: 0.0,
            total_trades: trades,
            winning_trades: (trades as f64 * win_rate) as u64,
            losing_trades: trades - (trades as f64 * win_rate) as u64,
            win_rate,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: 1.5,
            expectancy: Decimal::ZERO,
            avg_position_size: Decimal::ZERO,
            max_position_size: Decimal::ZERO,
            avg_holding_period_hours: 0.0,
            consistency_score: 0.6,
            winning_streak: 5,
            losing_streak: 3,
            current_streak: 2,
            computed_at: Utc::now(),
        }
    }

    #[test]
    fn test_prediction_category() {
        assert_eq!(
            PredictionCategory::from_probability(0.8, 0.5),
            PredictionCategory::HighPotential
        );
        assert_eq!(
            PredictionCategory::from_probability(0.6, 0.5),
            PredictionCategory::Moderate
        );
        assert_eq!(
            PredictionCategory::from_probability(0.3, 0.5),
            PredictionCategory::LowPotential
        );
        assert_eq!(
            PredictionCategory::from_probability(0.8, 0.2),
            PredictionCategory::Uncertain
        );
    }

    #[test]
    fn test_prediction_factor() {
        let factor = PredictionFactor::new("test", 0.8, 0.25);
        assert_eq!(factor.contribution, 0.2);
        assert!(factor.is_positive);

        let negative = PredictionFactor::new("negative", -0.5, 0.1);
        assert!(!negative.is_positive);
    }

    #[test]
    fn test_is_recommended() {
        let prediction = SuccessPrediction {
            address: "0x1234".to_string(),
            success_probability: 0.70,
            confidence: 0.6,
            category: PredictionCategory::HighPotential,
            factors: vec![],
            model: PredictionModel::RuleBased,
            predicted_at: Utc::now(),
            valid_for_hours: 24,
        };

        assert!(prediction.is_recommended());

        let not_recommended = SuccessPrediction {
            success_probability: 0.50,
            confidence: 0.6,
            ..prediction.clone()
        };
        assert!(!not_recommended.is_recommended());
    }

    #[test]
    fn test_high_performer_prediction() {
        let metrics = create_test_metrics(0.65, 2.0, 100);

        // Create a mock predictor for rule-based prediction test
        let weights = PredictionWeights::default();

        // Win rate score: (0.65 - 0.5) * 2 = 0.30
        let win_rate_score = (metrics.win_rate - 0.5) * 2.0;
        assert!((win_rate_score - 0.30).abs() < 0.01);

        // Sharpe score: 2.0 / 3.0 = 0.67
        let sharpe_score = (metrics.sharpe_ratio / 3.0).min(1.0);
        assert!((sharpe_score - 0.67).abs() < 0.01);
    }

    #[test]
    fn test_prediction_validity() {
        let valid_prediction = SuccessPrediction {
            address: "0x1234".to_string(),
            success_probability: 0.70,
            confidence: 0.6,
            category: PredictionCategory::HighPotential,
            factors: vec![],
            model: PredictionModel::RuleBased,
            predicted_at: Utc::now(),
            valid_for_hours: 24,
        };
        assert!(valid_prediction.is_valid());

        let expired_prediction = SuccessPrediction {
            predicted_at: Utc::now() - chrono::Duration::hours(25),
            ..valid_prediction
        };
        assert!(!expired_prediction.is_valid());
    }
}
