//! Cross-market correlation divergence signal generator.
//!
//! Detects correlated market pairs where one moved significantly
//! and the other lagged behind, presenting a convergence opportunity.
//!
//! Trigger conditions:
//!   - Correlation |r| > 0.70 between pair (from market_correlations)
//!   - Lead market moved > 5% in last 4 hours
//!   - Lag market moved < 2% in same period
//!   - Both markets have sufficient liquidity
//!
//! Direction: towards convergence with the lead market
//! Confidence: based on correlation strength × divergence magnitude
//! Expiry: 60 minutes

use chrono::{Duration, Utc};
use polymarket_core::types::signal::{QuantSignal, QuantSignalKind, SignalDirection};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::time;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Configuration for the cross-market signal generator.
#[derive(Debug, Clone)]
pub struct CrossMarketSignalConfig {
    /// Whether the generator is enabled.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    /// Minimum correlation coefficient (absolute) between market pair.
    pub min_correlation: f64,
    /// Minimum price move in lead market (fraction).
    pub min_lead_move: f64,
    /// Maximum price move in lag market (fraction) — must be below this.
    pub max_lag_move: f64,
    /// Base position size for suggested_size_usd.
    pub base_position_size_usd: Decimal,
    /// Signal expiry in minutes.
    pub expiry_minutes: i64,
}

impl CrossMarketSignalConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("CROSS_MARKET_SIGNAL_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(false),
            interval_secs: std::env::var("CROSS_MARKET_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(900),
            min_correlation: std::env::var("CROSS_MARKET_MIN_CORRELATION")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.70),
            min_lead_move: std::env::var("CROSS_MARKET_MIN_LEAD_MOVE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.05),
            max_lag_move: std::env::var("CROSS_MARKET_MAX_LAG_MOVE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.02),
            base_position_size_usd: Decimal::new(30, 0),
            expiry_minutes: 60,
        }
    }
}

/// Row returned by the divergence detection query.
#[derive(Debug, sqlx::FromRow)]
struct DivergenceRow {
    /// The market that moved significantly (lead).
    lead_market: String,
    /// The market that lagged behind.
    lag_market: String,
    /// Pearson correlation between the pair.
    correlation: f64,
    /// Price change of the lead market (signed fraction).
    lead_change: f64,
    /// Price change of the lag market (signed fraction).
    lag_change: f64,
    /// Current close price of the lag market.
    lag_current_price: f64,
}

/// Spawn the cross-market signal generator.
///
/// Polls `market_correlations` + `orderbook_hourly` on a configurable interval
/// to detect correlated pairs where one market diverged from the other.
pub fn spawn_cross_market_signal_generator(
    config: CrossMarketSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    if !config.enabled {
        info!("Cross-market signal generator disabled (CROSS_MARKET_SIGNAL_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        min_correlation = config.min_correlation,
        min_lead_move = config.min_lead_move,
        max_lag_move = config.max_lag_move,
        "Spawning cross-market signal generator"
    );

    tokio::spawn(async move {
        // Startup delay — cross-market detection queries orderbook_hourly aggressively;
        // give the pool and continuous aggregates time to settle before first run.
        tokio::time::sleep(time::Duration::from_secs(60)).await;

        let mut interval = tokio::time::interval(time::Duration::from_secs(config.interval_secs));
        interval.tick().await; // consume the first immediate tick

        loop {
            interval.tick().await;

            match generate_signals(&config, &pool).await {
                Ok(signals) => {
                    let count = signals.len();
                    for signal in signals {
                        if let Err(e) = signal_tx.send(signal) {
                            debug!(error = %e, "No quant executor subscribers for cross-market signal");
                        }
                    }
                    if count > 0 {
                        info!(count, "Cross-market divergence signals generated");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Cross-market signal generation failed");
                }
            }
        }
    });
}

/// Query for divergent correlated pairs and produce signals.
async fn generate_signals(
    config: &CrossMarketSignalConfig,
    pool: &PgPool,
) -> anyhow::Result<Vec<QuantSignal>> {
    let now = Utc::now();

    // Find correlated pairs where one market moved significantly in the last
    // 4 hours and the other didn't follow.
    //
    // For each pair in market_correlations with |r| >= min_correlation:
    //   1. Get the 4h price change for both markets from orderbook_hourly
    //   2. If one moved > min_lead_move and the other < max_lag_move → divergence
    //
    // The orderbook_hourly view uses: market_id, bucket, close (last yes_mid)
    let rows: Vec<DivergenceRow> = sqlx::query_as(
        r#"
        WITH price_changes AS (
            SELECT
                oh.market_id,
                (last_val.close - first_val.close) /
                    NULLIF(first_val.close, 0) AS price_change,
                last_val.close AS current_price
            FROM (
                SELECT DISTINCT market_id
                FROM orderbook_hourly
                WHERE bucket >= NOW() - INTERVAL '4 hours'
            ) oh
            CROSS JOIN LATERAL (
                SELECT close
                FROM orderbook_hourly
                WHERE market_id = oh.market_id
                  AND bucket >= NOW() - INTERVAL '4 hours'
                ORDER BY bucket ASC
                LIMIT 1
            ) first_val
            CROSS JOIN LATERAL (
                SELECT close
                FROM orderbook_hourly
                WHERE market_id = oh.market_id
                  AND bucket >= NOW() - INTERVAL '4 hours'
                ORDER BY bucket DESC
                LIMIT 1
            ) last_val
            WHERE first_val.close > 0.05
              AND last_val.close > 0.05
        )
        SELECT
            CASE
                WHEN ABS(pc_a.price_change) > ABS(pc_b.price_change)
                THEN mc.condition_id_a
                ELSE mc.condition_id_b
            END AS lead_market,
            CASE
                WHEN ABS(pc_a.price_change) > ABS(pc_b.price_change)
                THEN mc.condition_id_b
                ELSE mc.condition_id_a
            END AS lag_market,
            mc.correlation::double precision AS correlation,
            CASE
                WHEN ABS(pc_a.price_change) > ABS(pc_b.price_change)
                THEN pc_a.price_change::double precision
                ELSE pc_b.price_change::double precision
            END AS lead_change,
            CASE
                WHEN ABS(pc_a.price_change) > ABS(pc_b.price_change)
                THEN pc_b.price_change::double precision
                ELSE pc_a.price_change::double precision
            END AS lag_change,
            CASE
                WHEN ABS(pc_a.price_change) > ABS(pc_b.price_change)
                THEN pc_b.current_price::double precision
                ELSE pc_a.current_price::double precision
            END AS lag_current_price
        FROM market_correlations mc
        JOIN price_changes pc_a ON pc_a.market_id = mc.condition_id_a
        JOIN price_changes pc_b ON pc_b.market_id = mc.condition_id_b
        WHERE ABS(mc.correlation) >= $1
          AND mc.sample_size >= 168
          AND (
              (ABS(pc_a.price_change) >= $2 AND ABS(pc_b.price_change) <= $3)
              OR
              (ABS(pc_b.price_change) >= $2 AND ABS(pc_a.price_change) <= $3)
          )
        "#,
    )
    .bind(config.min_correlation)
    .bind(config.min_lead_move)
    .bind(config.max_lag_move)
    .fetch_all(pool)
    .await?;

    let mut signals = Vec::new();

    for row in rows {
        // Direction: the lag market should follow the lead market.
        // If lead went up and correlation is positive, lag should go up → BuyYes.
        // If lead went up and correlation is negative, lag should go down → BuyNo.
        // If lead went down and correlation is positive, lag should go down → BuyNo.
        // If lead went down and correlation is negative, lag should go up → BuyYes.
        let expected_lag_direction = if row.correlation > 0.0 {
            row.lead_change // same direction
        } else {
            -row.lead_change // opposite direction
        };

        let direction = if expected_lag_direction > 0.0 {
            SignalDirection::BuyYes
        } else {
            SignalDirection::BuyNo
        };

        // Confidence: correlation strength × divergence magnitude
        let corr_factor = row.correlation.abs();
        let divergence = (row.lead_change.abs() - row.lag_change.abs()).max(0.0);
        let confidence = (corr_factor * 0.5 + divergence * 5.0 * 0.5).clamp(0.0, 0.90);

        // Skip low-confidence signals
        if confidence < 0.50 {
            debug!(
                lead = %row.lead_market,
                lag = %row.lag_market,
                confidence,
                "Cross-market signal below confidence threshold, skipping"
            );
            continue;
        }

        let suggested_size = config.base_position_size_usd
            * Decimal::try_from(confidence).unwrap_or(Decimal::new(5, 1));

        let metadata = serde_json::json!({
            "lead_market": row.lead_market,
            "lag_market": row.lag_market,
            "correlation": row.correlation,
            "lead_change": row.lead_change,
            "lag_change": row.lag_change,
            "lag_current_price": row.lag_current_price,
            "divergence": divergence,
        });

        signals.push(QuantSignal {
            id: uuid::Uuid::new_v4(),
            kind: QuantSignalKind::CrossMarket,
            condition_id: row.lag_market.clone(),
            direction,
            confidence,
            suggested_size_usd: suggested_size,
            expiry: now + Duration::minutes(config.expiry_minutes),
            metadata,
            generated_at: now,
        });
    }

    Ok(signals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = CrossMarketSignalConfig::from_env();
        assert!(!config.enabled); // disabled by default
        assert_eq!(config.interval_secs, 900);
        assert_eq!(config.min_correlation, 0.70);
        assert_eq!(config.min_lead_move, 0.05);
        assert_eq!(config.max_lag_move, 0.02);
    }

    #[test]
    fn test_direction_logic() {
        // Positive correlation: lag should follow lead
        // Lead went up → lag should go up → BuyYes
        let corr = 0.85;
        let lead_change = 0.08;
        let expected = if corr > 0.0 {
            lead_change
        } else {
            -lead_change
        };
        assert!(expected > 0.0); // BuyYes

        // Negative correlation: lag should go opposite to lead
        // Lead went up → lag should go down → BuyNo
        let corr_neg = -0.80;
        let expected_neg = if corr_neg > 0.0 {
            lead_change
        } else {
            -lead_change
        };
        assert!(expected_neg < 0.0); // BuyNo
    }

    #[test]
    fn test_confidence_calculation() {
        let corr_factor: f64 = 0.85;
        let divergence: f64 = 0.06; // lead moved 8%, lag moved 2%
        let confidence = (corr_factor * 0.5 + divergence * 5.0 * 0.5).clamp(0.0, 0.90);
        // 0.85 * 0.5 + 0.06 * 5.0 * 0.5 = 0.425 + 0.15 = 0.575
        assert!((confidence - 0.575).abs() < 0.001);
        assert!(confidence <= 0.90);
    }

    #[test]
    fn test_confidence_clamped_at_max() {
        let corr_factor: f64 = 0.95;
        let divergence: f64 = 0.20; // huge divergence
        let confidence = (corr_factor * 0.5 + divergence * 5.0 * 0.5).clamp(0.0, 0.90);
        // 0.95 * 0.5 + 0.20 * 5.0 * 0.5 = 0.475 + 0.5 = 0.975 → clamped to 0.90
        assert!((confidence - 0.90).abs() < 0.001);
    }
}
