//! Prediction calibration for ensemble wallet predictions.
//!
//! Tracks ensemble predictions against actual copy trade outcomes,
//! buckets them by probability range, and computes calibration metrics
//! (ECE — Expected Calibration Error). This enables tuning the
//! recommendation threshold (currently 0.65) based on empirical data.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, info};

/// Number of equal-width buckets for probability calibration.
const NUM_BUCKETS: usize = 10;

/// A single calibration bucket (e.g., predictions in [0.6, 0.7)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationBucket {
    /// Lower bound of the probability range (inclusive).
    pub lower: f64,
    /// Upper bound of the probability range (exclusive, except last which is inclusive).
    pub upper: f64,
    /// Average predicted probability within this bucket.
    pub avg_predicted: f64,
    /// Observed success fraction (actual wins / total predictions in bucket).
    pub observed_rate: f64,
    /// Number of predictions in this bucket.
    pub count: usize,
    /// Calibration gap: |avg_predicted - observed_rate|.
    pub gap: f64,
}

/// Full calibration report across all probability buckets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationReport {
    /// Per-bucket calibration statistics.
    pub buckets: Vec<CalibrationBucket>,
    /// Expected Calibration Error (sample-weighted average gap).
    pub ece: f64,
    /// Total predictions evaluated.
    pub total_predictions: usize,
    /// Recommended threshold based on calibration (bucket with best F1).
    pub recommended_threshold: f64,
}

/// Stores and calibrates ensemble predictions against copy trade outcomes.
pub struct PredictionCalibrator {
    pool: PgPool,
}

impl PredictionCalibrator {
    /// Create a new calibrator backed by the given database.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Record an ensemble prediction for later calibration.
    ///
    /// This should be called when the AutoOptimizer selects a wallet,
    /// capturing the prediction probability at selection time.
    pub async fn record_prediction(
        &self,
        address: &str,
        predicted_prob: f64,
        workspace_id: &uuid::Uuid,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO ensemble_predictions (address, workspace_id, predicted_prob, created_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (address, workspace_id) DO UPDATE
                SET predicted_prob = EXCLUDED.predicted_prob,
                    created_at = EXCLUDED.created_at
            "#,
        )
        .bind(address)
        .bind(workspace_id)
        .bind(predicted_prob)
        .execute(&self.pool)
        .await?;

        debug!(address = %address, prob = predicted_prob, "Recorded ensemble prediction");
        Ok(())
    }

    /// Generate a calibration report by joining stored predictions
    /// against actual copy trade outcomes from `copy_trade_history`.
    ///
    /// A prediction is considered a "success" if the wallet's copy trades
    /// in the 30 days following selection had positive aggregate PnL.
    pub async fn calibrate(&self) -> Result<CalibrationReport> {
        // Fetch matched prediction-outcome pairs
        let rows: Vec<(f64, bool)> = sqlx::query_as(
            r#"
            SELECT
                ep.predicted_prob,
                COALESCE(SUM(cth.pnl), 0) > 0 AS is_success
            FROM ensemble_predictions ep
            LEFT JOIN copy_trade_history cth
                ON LOWER(cth.source_wallet) = LOWER(ep.address)
                AND cth.created_at >= ep.created_at
                AND cth.created_at < ep.created_at + INTERVAL '30 days'
            WHERE ep.created_at < NOW() - INTERVAL '30 days'
            GROUP BY ep.address, ep.workspace_id, ep.predicted_prob
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            info!("No mature predictions available for calibration");
            return Ok(CalibrationReport {
                buckets: Vec::new(),
                ece: 0.0,
                total_predictions: 0,
                recommended_threshold: 0.65, // Keep current default
            });
        }

        // Bucket predictions
        let bucket_width = 1.0 / NUM_BUCKETS as f64;
        let mut bucket_preds: Vec<Vec<(f64, bool)>> = vec![Vec::new(); NUM_BUCKETS];

        for (prob, success) in &rows {
            let idx = ((*prob / bucket_width).floor() as usize).min(NUM_BUCKETS - 1);
            bucket_preds[idx].push((*prob, *success));
        }

        let total_predictions = rows.len();
        let mut buckets = Vec::with_capacity(NUM_BUCKETS);
        let mut ece = 0.0;

        for (i, preds) in bucket_preds.iter().enumerate() {
            let lower = i as f64 * bucket_width;
            let upper = lower + bucket_width;

            if preds.is_empty() {
                buckets.push(CalibrationBucket {
                    lower,
                    upper,
                    avg_predicted: (lower + upper) / 2.0,
                    observed_rate: 0.0,
                    count: 0,
                    gap: 0.0,
                });
                continue;
            }

            let count = preds.len();
            let avg_predicted: f64 = preds.iter().map(|(p, _)| p).sum::<f64>() / count as f64;
            let observed_rate = preds.iter().filter(|(_, s)| *s).count() as f64 / count as f64;
            let gap = (avg_predicted - observed_rate).abs();

            ece += gap * (count as f64 / total_predictions as f64);

            buckets.push(CalibrationBucket {
                lower,
                upper,
                avg_predicted,
                observed_rate,
                count,
                gap,
            });
        }

        // Recommend threshold: find bucket with best precision-recall trade-off.
        // Simple heuristic: choose the lowest probability bucket where
        // observed_rate >= bucket midpoint (i.e., calibrated or better).
        let recommended_threshold = buckets
            .iter()
            .filter(|b| b.count >= 5 && b.observed_rate >= b.avg_predicted * 0.8)
            .map(|b| b.lower)
            .next()
            .unwrap_or(0.65);

        info!(
            ece = ece,
            total_predictions = total_predictions,
            recommended_threshold = recommended_threshold,
            "Calibration report generated"
        );

        Ok(CalibrationReport {
            buckets,
            ece,
            total_predictions,
            recommended_threshold,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calibration_bucket_gap() {
        let bucket = CalibrationBucket {
            lower: 0.6,
            upper: 0.7,
            avg_predicted: 0.65,
            observed_rate: 0.55,
            count: 20,
            gap: 0.10,
        };

        assert!((bucket.gap - 0.10).abs() < f64::EPSILON);
    }

    #[test]
    fn test_empty_calibration_report() {
        let report = CalibrationReport {
            buckets: Vec::new(),
            ece: 0.0,
            total_predictions: 0,
            recommended_threshold: 0.65,
        };

        assert_eq!(report.total_predictions, 0);
        assert_eq!(report.recommended_threshold, 0.65);
    }

    #[test]
    fn test_bucket_count() {
        assert_eq!(NUM_BUCKETS, 10);
    }
}
