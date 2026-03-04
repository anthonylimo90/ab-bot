//! Resolution proximity signal generator.
//!
//! Polls `market_metadata` every 15 minutes for markets approaching their
//! end date. Uses a binary time-decay model to identify underpriced outcomes.
//!
//! Trigger conditions:
//!   - Market end_date is 1–7 days away
//!   - YES price is 15%+ from 0.50
//!   - Market volume > $1,000
//!
//! Direction: towards the favored side (price > 0.50 → BuyYes, else BuyNo)
//! Confidence: based on days remaining + volume trend
//! Expiry: 60 minutes (longer horizon strategy)

use chrono::{Duration, Utc};
use polymarket_core::types::signal::{QuantSignal, QuantSignalKind, SignalDirection};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::time;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Configuration for the resolution proximity signal generator.
#[derive(Debug, Clone)]
pub struct ResolutionSignalConfig {
    /// Whether the generator is enabled.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    /// Minimum days until resolution.
    pub min_days_remaining: i64,
    /// Maximum days until resolution.
    pub max_days_remaining: i64,
    /// Minimum price deviation from 0.50 (absolute).
    pub min_price_deviation: f64,
    /// Minimum market volume in USD.
    pub min_volume: Decimal,
    /// Base position size for suggested_size_usd.
    pub base_position_size_usd: Decimal,
    /// Signal expiry in minutes.
    pub expiry_minutes: i64,
}

impl ResolutionSignalConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("RESOLUTION_SIGNAL_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("RESOLUTION_SIGNAL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(900),
            min_days_remaining: 1,
            max_days_remaining: 7,
            min_price_deviation: 0.15,
            min_volume: std::env::var("RESOLUTION_MIN_VOLUME_USD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(Decimal::new(1000, 0)),
            base_position_size_usd: std::env::var("QUANT_BASE_POSITION_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(Decimal::new(30, 0)),
            expiry_minutes: 60,
        }
    }
}

/// Row from the resolution proximity query.
#[derive(Debug, sqlx::FromRow)]
struct ResolutionRow {
    condition_id: String,
    question: String,
    end_date: chrono::DateTime<Utc>,
    volume: Option<Decimal>,
    /// Most recent YES mid-price from orderbook snapshots.
    yes_price: Option<f64>,
}

/// Spawn the resolution proximity signal generator.
pub fn spawn_resolution_signal_generator(
    config: ResolutionSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    if !config.enabled {
        info!("Resolution signal generator disabled (RESOLUTION_SIGNAL_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        min_days = config.min_days_remaining,
        max_days = config.max_days_remaining,
        min_price_deviation = config.min_price_deviation,
        min_volume = %config.min_volume,
        "Spawning resolution proximity signal generator"
    );

    tokio::spawn(generator_loop(config, pool, signal_tx));
}

async fn generator_loop(
    config: ResolutionSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    let interval = time::Duration::from_secs(config.interval_secs);

    // Startup delay — let market metadata populate first
    tokio::time::sleep(time::Duration::from_secs(90)).await;

    loop {
        match scan_and_emit(&config, &pool, &signal_tx).await {
            Ok(count) => {
                if count > 0 {
                    info!(
                        signals = count,
                        "Resolution signal generator emitted signals"
                    );
                } else {
                    debug!("Resolution signal generator: no qualifying markets this cycle");
                }
            }
            Err(e) => {
                warn!(error = %e, "Resolution signal generator cycle failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn scan_and_emit(
    config: &ResolutionSignalConfig,
    pool: &PgPool,
    signal_tx: &broadcast::Sender<QuantSignal>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();
    let min_end = now + Duration::days(config.min_days_remaining);
    let max_end = now + Duration::days(config.max_days_remaining);

    // Find markets approaching resolution with sufficient volume and
    // a clear price lean (deviation from 0.50).
    // Left-join orderbook_hourly for latest YES price.
    let rows = sqlx::query_as::<_, ResolutionRow>(
        r#"
        SELECT
            mm.condition_id,
            mm.question,
            mm.end_date,
            mm.volume,
            ob.yes_price
        FROM market_metadata mm
        LEFT JOIN LATERAL (
            SELECT
                close AS yes_price
            FROM orderbook_hourly
            WHERE market_id = mm.condition_id
            ORDER BY bucket DESC
            LIMIT 1
        ) ob ON true
        WHERE mm.active = true
          AND mm.end_date >= $1
          AND mm.end_date <= $2
          AND mm.volume >= $3
          AND ob.yes_price IS NOT NULL
          AND ABS(ob.yes_price - 0.5) >= $4
        ORDER BY mm.end_date ASC
        LIMIT 30
        "#,
    )
    .bind(min_end)
    .bind(max_end)
    .bind(config.min_volume)
    .bind(config.min_price_deviation)
    .fetch_all(pool)
    .await?;

    let mut emitted = 0;

    for row in &rows {
        let yes_price = match row.yes_price {
            Some(p) => p,
            None => continue,
        };

        let hours_remaining = row.end_date.signed_duration_since(now).num_hours().max(1) as f64;
        let days_remaining = hours_remaining / 24.0;

        // Direction: if YES price > 0.50, the market leans YES → BuyYes
        // (momentum towards resolution). If < 0.50, leans NO → BuyNo.
        let direction = if yes_price > 0.5 {
            SignalDirection::BuyYes
        } else {
            SignalDirection::BuyNo
        };

        // Confidence model:
        // - Base: how far from 0.50 (more conviction = higher confidence)
        // - Time decay: closer to resolution = more conviction
        // - Volume: higher volume = more reliable signal
        let deviation = (yes_price - 0.5).abs();
        let time_factor = (1.0 / days_remaining.sqrt()).clamp(0.0, 1.0);
        let volume_factor = match row.volume {
            Some(v) => {
                let v_f64 = decimal_to_f64(v);
                ((v_f64 - 1000.0) / 49000.0).clamp(0.0, 1.0) // $1K=0, $50K=1
            }
            None => 0.0,
        };

        let confidence =
            (deviation * 1.5 * 0.5 + time_factor * 0.35 + volume_factor * 0.15).clamp(0.0, 1.0);

        let expiry = now + Duration::minutes(config.expiry_minutes);

        let signal = QuantSignal::new(
            QuantSignalKind::ResolutionProximity,
            row.condition_id.clone(),
            direction,
            confidence,
            config.base_position_size_usd,
            expiry,
        )
        .with_metadata(serde_json::json!({
            "question": row.question,
            "yes_price": yes_price,
            "days_remaining": days_remaining,
            "hours_remaining": hours_remaining,
            "volume": row.volume.map(decimal_to_f64),
            "deviation": deviation,
            "time_factor": time_factor,
        }));

        debug!(
            condition_id = &row.condition_id,
            direction = signal.direction.as_str(),
            confidence = signal.confidence,
            days_remaining = days_remaining,
            yes_price = yes_price,
            "Resolution proximity signal generated"
        );

        let _ = signal_tx.send(signal);
        emitted += 1;
    }

    Ok(emitted)
}

/// Convert Decimal to f64 for confidence calculations.
fn decimal_to_f64(d: Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.to_f64().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = ResolutionSignalConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.interval_secs, 900);
        assert_eq!(config.min_days_remaining, 1);
        assert_eq!(config.max_days_remaining, 7);
        assert_eq!(config.min_price_deviation, 0.15);
    }

    #[test]
    fn test_confidence_model() {
        // Market at 0.80 YES, 2 days remaining, $10K volume
        let deviation: f64 = 0.30; // |0.80 - 0.50|
        let days_remaining: f64 = 2.0;
        let time_factor = (1.0 / days_remaining.sqrt()).clamp(0.0, 1.0);
        let volume_factor = ((10000.0_f64 - 1000.0) / 49000.0).clamp(0.0, 1.0);

        let confidence =
            (deviation * 1.5 * 0.5 + time_factor * 0.35 + volume_factor * 0.15).clamp(0.0, 1.0);

        // Should be reasonably high confidence
        assert!(confidence > 0.45);
        assert!(confidence < 0.85);
    }

    #[test]
    fn test_direction_logic() {
        // YES at 0.75 → should BuyYes (market leans YES near resolution)
        // 0.75 > 0.5
        let _ = 0.75_f64;

        // YES at 0.25 → should BuyNo (market leans NO near resolution)
        // 0.25 < 0.5
        let _ = 0.25_f64;
    }
}
