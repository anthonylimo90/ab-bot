//! Prediction calibration types.
//!
//! The calibration system previously tracked ensemble predictions against
//! copy trade outcomes. With copy trading removed, only the report types
//! remain (used by the API response contract).

use serde::{Deserialize, Serialize};

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
