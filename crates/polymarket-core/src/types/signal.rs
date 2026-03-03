//! Quantitative signal types for multi-strategy trading.
//!
//! These types define the interface between signal generators and the
//! quant signal executor. Each generator produces `QuantSignal` instances
//! that flow through a broadcast channel for evaluation and execution.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Kind of quantitative signal — maps 1:1 to a signal generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantSignalKind {
    /// Smart money order flow imbalance.
    Flow,
    /// Cross-market correlation divergence.
    CrossMarket,
    /// Short-term price mean reversion.
    MeanReversion,
    /// Resolution proximity time-decay signal.
    ResolutionProximity,
}

impl QuantSignalKind {
    /// Human-readable label for logging and DB storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::CrossMarket => "cross_market",
            Self::MeanReversion => "mean_reversion",
            Self::ResolutionProximity => "resolution_proximity",
        }
    }
}

impl std::fmt::Display for QuantSignalKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Direction of a quantitative signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalDirection {
    /// Buy the YES outcome token.
    BuyYes,
    /// Buy the NO outcome token.
    BuyNo,
}

impl SignalDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BuyYes => "buy_yes",
            Self::BuyNo => "buy_no",
        }
    }
}

impl std::fmt::Display for SignalDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A quantitative trading signal produced by a signal generator.
///
/// Signals flow through the broadcast channel from generators to the executor.
/// The executor evaluates each signal against risk limits, dedup, and confidence
/// thresholds before creating a position.
///
/// Reuses `PositionSource::Recommendation { signal_id }` for position attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantSignal {
    /// Unique signal identifier (stored in `quant_signals` table).
    pub id: Uuid,
    /// Which generator produced this signal.
    pub kind: QuantSignalKind,
    /// Polymarket condition ID for the target market.
    pub condition_id: String,
    /// Trade direction.
    pub direction: SignalDirection,
    /// Confidence score (0.0–1.0). Higher = more conviction.
    pub confidence: f64,
    /// Suggested position size in USD (before confidence weighting).
    pub suggested_size_usd: Decimal,
    /// Signal expiry — executor should discard if `now > expiry`.
    pub expiry: DateTime<Utc>,
    /// Generator-specific metadata (thresholds hit, feature values, etc.).
    pub metadata: serde_json::Value,
    /// When the signal was generated.
    pub generated_at: DateTime<Utc>,
}

impl QuantSignal {
    /// Create a new signal with a fresh UUID and current timestamp.
    pub fn new(
        kind: QuantSignalKind,
        condition_id: String,
        direction: SignalDirection,
        confidence: f64,
        suggested_size_usd: Decimal,
        expiry: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            condition_id,
            direction,
            confidence: confidence.clamp(0.0, 1.0),
            suggested_size_usd,
            expiry,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            generated_at: Utc::now(),
        }
    }

    /// Attach metadata to the signal.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Check if the signal has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expiry
    }

    /// Check if the signal meets a minimum confidence threshold.
    pub fn meets_confidence(&self, min_confidence: f64) -> bool {
        self.confidence >= min_confidence
    }
}

/// Execution status for a persisted quant signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalExecutionStatus {
    /// Awaiting executor evaluation.
    Pending,
    /// Executor decided to execute — position created.
    Executed,
    /// Executor evaluated but skipped (see skip_reason).
    Skipped,
    /// Execution attempted but failed.
    Failed,
}

impl SignalExecutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executed => "executed",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for SignalExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quant_signal_creation() {
        let signal = QuantSignal::new(
            QuantSignalKind::Flow,
            "0x1234".to_string(),
            SignalDirection::BuyYes,
            0.75,
            Decimal::new(30, 0),
            Utc::now() + chrono::Duration::minutes(30),
        );

        assert_eq!(signal.kind, QuantSignalKind::Flow);
        assert_eq!(signal.direction, SignalDirection::BuyYes);
        assert_eq!(signal.confidence, 0.75);
        assert!(!signal.is_expired());
        assert!(signal.meets_confidence(0.65));
        assert!(!signal.meets_confidence(0.80));
    }

    #[test]
    fn test_confidence_clamping() {
        let signal = QuantSignal::new(
            QuantSignalKind::MeanReversion,
            "0xabcd".to_string(),
            SignalDirection::BuyNo,
            1.5, // exceeds 1.0
            Decimal::new(50, 0),
            Utc::now() + chrono::Duration::minutes(10),
        );

        assert_eq!(signal.confidence, 1.0);
    }

    #[test]
    fn test_signal_expiry() {
        let signal = QuantSignal::new(
            QuantSignalKind::ResolutionProximity,
            "0xdead".to_string(),
            SignalDirection::BuyYes,
            0.65,
            Decimal::new(20, 0),
            Utc::now() - chrono::Duration::minutes(1), // already expired
        );

        assert!(signal.is_expired());
    }

    #[test]
    fn test_signal_serialization() {
        let signal = QuantSignal::new(
            QuantSignalKind::CrossMarket,
            "0xbeef".to_string(),
            SignalDirection::BuyNo,
            0.82,
            Decimal::new(40, 0),
            Utc::now() + chrono::Duration::hours(1),
        );

        let json = serde_json::to_string(&signal).unwrap();
        let deserialized: QuantSignal = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.kind, QuantSignalKind::CrossMarket);
        assert_eq!(deserialized.direction, SignalDirection::BuyNo);
        assert_eq!(deserialized.condition_id, "0xbeef");
    }

    #[test]
    fn test_signal_kind_display() {
        assert_eq!(QuantSignalKind::Flow.to_string(), "flow");
        assert_eq!(QuantSignalKind::CrossMarket.to_string(), "cross_market");
        assert_eq!(QuantSignalKind::MeanReversion.to_string(), "mean_reversion");
        assert_eq!(
            QuantSignalKind::ResolutionProximity.to_string(),
            "resolution_proximity"
        );
    }
}
