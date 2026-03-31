//! Position sizing utilities — Kelly Criterion and helpers.
//!
//! Provides mathematically-grounded position sizing for binary prediction
//! markets. The Kelly Criterion maximizes long-term growth rate by sizing
//! proportional to edge.

use rust_decimal::Decimal;

/// Configuration for Kelly-based position sizing.
#[derive(Debug, Clone)]
pub struct KellyConfig {
    /// Fractional Kelly multiplier (0.0–1.0). Lower values are more
    /// conservative. 0.25 = quarter-Kelly. Default: 0.25.
    pub fraction: f64,
    /// Total bankroll available for sizing. Positions are sized as a
    /// fraction of this amount.
    pub bankroll: Decimal,
    /// Hard upper cap on a single position regardless of Kelly output.
    pub max_position: Decimal,
    /// Minimum position size — skip if Kelly recommends less than this.
    pub min_position: Decimal,
}

impl Default for KellyConfig {
    fn default() -> Self {
        Self {
            fraction: 0.25,
            bankroll: Decimal::new(1000, 0),
            max_position: Decimal::new(200, 0),
            min_position: Decimal::new(5, 0),
        }
    }
}

/// Compute the raw Kelly fraction for a binary outcome bet.
///
/// # Formula
///
/// `f = (p * b - q) / b`
///
/// where:
/// - `p` = probability of winning
/// - `q` = 1 - p (probability of losing)
/// - `b` = net odds (payout per $1 risked = `(1 - price) / price`)
///
/// Returns the optimal fraction of bankroll to bet. Clamped to `[0.0, 1.0]`.
/// Returns 0.0 when there is no edge or inputs are degenerate.
pub fn kelly_fraction(p_win: f64, price: f64) -> f64 {
    if price <= 0.0 || price >= 1.0 || p_win <= 0.0 || p_win >= 1.0 {
        return 0.0;
    }
    let b = (1.0 - price) / price; // net odds
    let q = 1.0 - p_win;
    let f = (p_win * b - q) / b;
    f.clamp(0.0, 1.0)
}

/// Compute a Kelly-sized position in USD.
///
/// Returns `None` if the computed size is below `config.min_position` or
/// if there is no edge (Kelly fraction <= 0).
///
/// # Arguments
///
/// * `p_win` — Estimated probability of winning (0.0–1.0)
/// * `price` — The contract price being bought (0.0–1.0)
/// * `config` — Sizing parameters (bankroll, fraction, caps)
pub fn kelly_position_size(p_win: f64, price: f64, config: &KellyConfig) -> Option<Decimal> {
    let f = kelly_fraction(p_win, price);
    if f <= 0.0 {
        return None;
    }

    let bankroll_f64: f64 = config.bankroll.to_string().parse().unwrap_or(0.0);
    let raw_size = config.fraction * f * bankroll_f64;

    let size = Decimal::from_f64_retain(raw_size)?;
    let clamped = size.min(config.max_position);

    if clamped < config.min_position {
        None
    } else {
        Some(clamped)
    }
}

/// Simplified sizing for spread arbitrage positions where the edge is
/// the net profit margin (1.0 - yes_ask - no_ask - fees).
///
/// Uses a linear model: size scales from `min_position` to `max_position`
/// proportional to where `edge` falls in `[min_edge, max_edge]`.
///
/// This is the existing arb executor approach, preserved as a fallback
/// for operators who prefer deterministic sizing over Kelly.
pub fn linear_position_size(
    edge: Decimal,
    min_edge: Decimal,
    max_edge: Decimal,
    min_position: Decimal,
    max_position: Decimal,
) -> Decimal {
    let range = max_edge - min_edge;
    if range.is_zero() {
        return min_position;
    }
    let t = ((edge - min_edge) / range)
        .max(Decimal::ZERO)
        .min(Decimal::ONE);
    let size_range = max_position - min_position;
    min_position + size_range * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kelly_fraction_with_edge() {
        // p=0.85, price=0.55 → b = 0.45/0.55 ≈ 0.818
        // f = (0.85*0.818 - 0.15) / 0.818 ≈ 0.667
        let f = kelly_fraction(0.85, 0.55);
        assert!((f - 0.667).abs() < 0.01, "Expected ~0.667, got {f}");
    }

    #[test]
    fn test_kelly_fraction_no_edge() {
        // Fair coin, fair price → zero edge
        let f = kelly_fraction(0.50, 0.50);
        assert!(f.abs() < 0.001);
    }

    #[test]
    fn test_kelly_fraction_negative_edge() {
        // Negative edge should return 0.0, not negative
        let f = kelly_fraction(0.40, 0.60);
        assert_eq!(f, 0.0);
    }

    #[test]
    fn test_kelly_fraction_edge_cases() {
        assert_eq!(kelly_fraction(0.0, 0.50), 0.0);
        assert_eq!(kelly_fraction(1.0, 0.50), 0.0);
        assert_eq!(kelly_fraction(0.50, 0.0), 0.0);
        assert_eq!(kelly_fraction(0.50, 1.0), 0.0);
    }

    #[test]
    fn test_kelly_fraction_clamped_to_one() {
        // Very high edge should not exceed 1.0
        let f = kelly_fraction(0.99, 0.10);
        assert!(f <= 1.0, "Kelly fraction should be clamped to 1.0, got {f}");
    }

    #[test]
    fn test_kelly_position_size_basic() {
        let config = KellyConfig {
            fraction: 0.25,
            bankroll: Decimal::new(1000, 0),
            max_position: Decimal::new(200, 0),
            min_position: Decimal::new(5, 0),
        };

        // p=0.85, price=0.55 → f≈0.667 → size = 0.25 * 0.667 * 1000 ≈ $167
        let size = kelly_position_size(0.85, 0.55, &config);
        assert!(size.is_some());
        let s = size.unwrap();
        assert!(
            s > Decimal::new(100, 0) && s < Decimal::new(200, 0),
            "Expected ~$167, got {s}"
        );
    }

    #[test]
    fn test_kelly_position_size_capped() {
        let config = KellyConfig {
            fraction: 1.0, // Full Kelly
            bankroll: Decimal::new(10000, 0),
            max_position: Decimal::new(50, 0), // Low cap
            min_position: Decimal::new(5, 0),
        };

        let size = kelly_position_size(0.85, 0.55, &config);
        assert_eq!(size, Some(Decimal::new(50, 0))); // Capped at max
    }

    #[test]
    fn test_kelly_position_size_too_small() {
        let config = KellyConfig {
            fraction: 0.01,
            bankroll: Decimal::new(100, 0),
            max_position: Decimal::new(200, 0),
            min_position: Decimal::new(5, 0),
        };

        // Very small Kelly → below min
        let size = kelly_position_size(0.55, 0.50, &config);
        assert!(size.is_none());
    }

    #[test]
    fn test_kelly_position_size_no_edge() {
        let config = KellyConfig::default();
        let size = kelly_position_size(0.40, 0.60, &config);
        assert!(size.is_none());
    }

    #[test]
    fn test_linear_position_size() {
        let min_edge = Decimal::new(1, 3); // 0.001
        let max_edge = Decimal::new(5, 2); // 0.05
        let min_pos = Decimal::new(25, 0);
        let max_pos = Decimal::new(200, 0);

        // Edge = 0.025 → midpoint → ~$112.50
        let size = linear_position_size(Decimal::new(25, 3), min_edge, max_edge, min_pos, max_pos);
        assert!(
            size > Decimal::new(100, 0) && size < Decimal::new(130, 0),
            "Expected ~$112, got {size}"
        );
    }

    #[test]
    fn test_linear_position_size_clamped() {
        let min_edge = Decimal::new(1, 3);
        let max_edge = Decimal::new(5, 2);
        let min_pos = Decimal::new(25, 0);
        let max_pos = Decimal::new(200, 0);

        // Edge above max → clamped to max position
        let size = linear_position_size(Decimal::new(1, 1), min_edge, max_edge, min_pos, max_pos);
        assert_eq!(size, max_pos);

        // Edge below min → clamped to min position
        let size = linear_position_size(Decimal::ZERO, min_edge, max_edge, min_pos, max_pos);
        assert_eq!(size, min_pos);
    }
}
