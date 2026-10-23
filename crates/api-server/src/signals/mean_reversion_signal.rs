//! Mean reversion signal generator.
//!
//! Polls `orderbook_hourly` every 10 minutes for markets with abnormally
//! large price moves on below-median volume — classic mean reversion setups.
//!
//! Trigger conditions:
//!   - Price moved > 10% in the last hour
//!   - Volume is below the market's own 24h median
//!
//! Direction: opposite of the move (bet on reversion)
//! Confidence: 0.55 + |price_change| * 1.5, capped at 0.80
//! Expiry: 20 minutes (short-lived reversion window)

use chrono::{Duration, Utc};
use polymarket_core::types::signal::{
    PriceReference, QuantSignal, QuantSignalKind, SignalDirection,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::time;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Configuration for the mean reversion signal generator.
#[derive(Debug, Clone)]
pub struct MeanReversionSignalConfig {
    /// Whether the generator is enabled.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    /// Minimum absolute price change (fraction) to trigger.
    pub min_move_pct: f64,
    /// Base position size for suggested_size_usd.
    pub base_position_size_usd: Decimal,
    /// Signal expiry in minutes.
    pub expiry_minutes: i64,
}

impl MeanReversionSignalConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_flag("MEAN_REVERSION_SIGNAL_ENABLED", true),
            interval_secs: polymarket_core::config::env_count("MEAN_REVERSION_INTERVAL_SECS", 600),
            min_move_pct: polymarket_core::config::env_number("MEAN_REV_MIN_MOVE_PCT", 0.10),
            base_position_size_usd: polymarket_core::config::env_positive_decimal(
                "QUANT_BASE_POSITION_SIZE",
                Decimal::new(30, 0),
            ),
            expiry_minutes: 20,
        }
    }
}

/// Row from the mean reversion scan query.
#[derive(Debug, sqlx::FromRow)]
struct MeanReversionRow {
    condition_id: String,
    /// Current (most recent) hourly mid-price for YES.
    current_price: f64,
    /// Start of the hourly bar `current_price` closed.
    current_bucket: chrono::DateTime<Utc>,
    /// Previous hourly mid-price for YES.
    previous_price: f64,
    /// Price change as a fraction.
    price_change: f64,
    /// Current hour's volume.
    current_volume: f64,
    /// Median hourly volume over last 24h.
    median_volume: f64,
}

/// Spawn the mean reversion signal generator.
pub fn spawn_mean_reversion_signal_generator(
    config: MeanReversionSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    if !config.enabled {
        info!("Mean reversion signal generator disabled (MEAN_REVERSION_SIGNAL_ENABLED is off)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        min_move_pct = config.min_move_pct,
        "Spawning mean reversion signal generator"
    );

    tokio::spawn(generator_loop(config, pool, signal_tx));
}

async fn generator_loop(
    config: MeanReversionSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    let interval = time::Duration::from_secs(config.interval_secs);

    // Startup delay — let orderbook data accumulate
    tokio::time::sleep(time::Duration::from_secs(45)).await;

    loop {
        match scan_and_emit(&config, &pool, &signal_tx).await {
            Ok(count) => {
                if count > 0 {
                    info!(
                        signals = count,
                        "Mean reversion signal generator emitted signals"
                    );
                } else {
                    debug!("Mean reversion signal generator: no qualifying markets this cycle");
                }
            }
            Err(e) => {
                warn!(error = %e, "Mean reversion signal generator cycle failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn scan_and_emit(
    config: &MeanReversionSignalConfig,
    pool: &PgPool,
    signal_tx: &broadcast::Sender<QuantSignal>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();

    // Find markets where the most recent hourly price moved > min_move_pct
    // compared to the previous hour, and the market's rolling 24h volume is
    // not above its median over the day (the move did not come with a surge
    // of trading).
    //
    // Only active markets that are not about to end, priced inside
    // 0.08-0.92 on both hours: a market jumping towards 0 or 1 is resolving,
    // not overshooting, and buying against it buys a near-worthless side.
    //
    // Uses `orderbook_hourly` continuous aggregate:
    //   - market_id: condition ID
    //   - bucket: hourly timestamp
    //   - close: last yes_mid price
    //   - avg_volume: average rolling 24h volume
    let rows = sqlx::query_as::<_, MeanReversionRow>(
        r#"
        WITH recent AS (
            SELECT
                market_id AS condition_id,
                bucket,
                close AS mid_price,
                avg_volume,
                ROW_NUMBER() OVER (PARTITION BY market_id ORDER BY bucket DESC) AS rn
            FROM orderbook_hourly
            WHERE bucket >= $1 - INTERVAL '2 hours'
        ),
        current_prev AS (
            SELECT
                c.condition_id,
                c.mid_price::float8 AS current_price,
                c.bucket AS current_bucket,
                p.mid_price::float8 AS previous_price,
                (CASE
                    WHEN p.mid_price > 0 THEN (c.mid_price - p.mid_price) / p.mid_price
                    ELSE 0
                END)::float8 AS price_change,
                c.avg_volume::float8 AS current_volume
            FROM recent c
            JOIN recent p ON c.condition_id = p.condition_id AND p.rn = 2
            JOIN market_metadata mm ON mm.condition_id = c.condition_id
            WHERE c.rn = 1
              AND p.mid_price BETWEEN 0.08 AND 0.92
              AND c.mid_price BETWEEN 0.08 AND 0.92
              AND mm.active
              AND (mm.end_date IS NULL OR mm.end_date > $1 + INTERVAL '24 hours')
        ),
        vol_median AS (
            SELECT
                market_id AS condition_id,
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY avg_volume) AS median_volume
            FROM orderbook_hourly
            WHERE bucket >= $1 - INTERVAL '24 hours'
            GROUP BY market_id
        )
        SELECT
            cp.condition_id,
            cp.current_price,
            cp.current_bucket,
            cp.previous_price,
            cp.price_change,
            cp.current_volume,
            COALESCE(vm.median_volume, cp.current_volume)::float8 AS median_volume
        FROM current_prev cp
        LEFT JOIN vol_median vm ON cp.condition_id = vm.condition_id
        WHERE ABS(cp.price_change) >= $2
          AND cp.current_volume <= COALESCE(vm.median_volume, cp.current_volume + 1)
        ORDER BY ABS(cp.price_change) DESC
        LIMIT 20
        "#,
    )
    .bind(now)
    .bind(config.min_move_pct)
    .fetch_all(pool)
    .await?;

    let mut emitted = 0;

    for row in &rows {
        let abs_change = row.price_change.abs();

        // Direction: opposite of the move (mean reversion)
        // If price went UP sharply → BuyNo (bet it reverts down)
        // If price went DOWN sharply → BuyYes (bet it reverts up)
        let direction = if row.price_change > 0.0 {
            SignalDirection::BuyNo
        } else {
            SignalDirection::BuyYes
        };

        // Confidence: 0.55 base + magnitude bonus, capped at 0.80
        let confidence = (0.55 + abs_change * 1.5).min(0.80);

        let expiry = now + Duration::minutes(config.expiry_minutes);

        let signal = QuantSignal::new(
            QuantSignalKind::MeanReversion,
            row.condition_id.clone(),
            direction,
            confidence,
            config.base_position_size_usd,
            expiry,
        )
        .with_reference(PriceReference::from_yes_price(
            row.current_price,
            row.current_bucket,
        ))
        .with_metadata(serde_json::json!({
            "current_price": row.current_price,
            "previous_price": row.previous_price,
            "price_change": row.price_change,
            "price_change_pct": abs_change * 100.0,
            "current_volume": row.current_volume,
            "median_volume": row.median_volume,
            "volume_ratio": if row.median_volume > 0.0 {
                row.current_volume / row.median_volume
            } else {
                1.0
            },
        }));

        debug!(
            condition_id = &row.condition_id,
            direction = signal.direction.as_str(),
            confidence = signal.confidence,
            price_change_pct = abs_change * 100.0,
            "Mean reversion signal generated"
        );

        let _ = signal_tx.send(signal);
        emitted += 1;
    }

    Ok(emitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = MeanReversionSignalConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.interval_secs, 600);
        assert_eq!(config.min_move_pct, 0.10);
        assert_eq!(config.expiry_minutes, 20);
    }

    #[test]
    fn test_confidence_formula() {
        // 10% move → 0.55 + 0.10 * 1.5 = 0.70
        assert!((0.55_f64 + 0.10 * 1.5 - 0.70).abs() < 0.001);

        // 20% move → 0.55 + 0.20 * 1.5 = 0.85 → capped at 0.80
        assert_eq!((0.55_f64 + 0.20 * 1.5).min(0.80), 0.80);

        // 5% move → 0.55 + 0.05 * 1.5 = 0.625
        assert!((0.55_f64 + 0.05 * 1.5 - 0.625).abs() < 0.001);
    }

    #[test]
    fn test_direction_logic() {
        // Price went up (positive change) → BuyNo (reversion down)
        let price_change = 0.15;
        assert!(price_change > 0.0); // BuyNo

        // Price went down (negative change) → BuyYes (reversion up)
        let price_change = -0.12;
        assert!(price_change < 0.0); // BuyYes
    }

    #[sqlx::test(migrations = false)]
    async fn scan_decodes_hourly_bars_and_references_the_latest_close(pool: PgPool) {
        use crate::signals::hourly_bars;
        hourly_bars::create(&pool).await;
        sqlx::Executor::execute(
            &pool,
            include_str!("../../../../migrations/038_market_metadata.sql"),
        )
        .await
        .unwrap();
        let now = Utc::now();
        let current_bucket = now - Duration::minutes(40);
        // A spike, and moves that are resolution, an ended market, and an
        // unknown market.
        for (market, previous, current, active, ends_in_hours) in [
            ("0xspike", 50, 60, true, Some(240)),
            ("0xresolving", 90, 99, true, Some(240)),
            ("0xclosed", 50, 60, false, Some(240)),
            ("0xending", 50, 60, true, Some(2)),
            ("0xunknown", 50, 60, true, None),
        ] {
            for (minutes_ago, price) in [(100, previous), (40, current)] {
                hourly_bars::insert(
                    &pool,
                    market,
                    now - Duration::minutes(minutes_ago),
                    Decimal::new(price, 2),
                    Decimal::from(1000),
                )
                .await;
            }
            if market != "0xunknown" {
                sqlx::query(
                    "INSERT INTO market_metadata (condition_id, question, end_date, active)
                     VALUES ($1, 'q', $2, $3)",
                )
                .bind(market)
                .bind(ends_in_hours.map(|hours| now + Duration::hours(hours)))
                .bind(active)
                .execute(&pool)
                .await
                .unwrap();
            }
        }

        let config = MeanReversionSignalConfig::from_env();
        let (tx, mut rx) = broadcast::channel(8);
        assert_eq!(scan_and_emit(&config, &pool, &tx).await.unwrap(), 1);

        let signal = rx.try_recv().unwrap();
        assert_eq!(signal.condition_id, "0xspike");
        // +20% → bet on reversion by buying NO.
        assert_eq!(signal.direction, SignalDirection::BuyNo);
        let reference = signal.reference.unwrap();
        assert_eq!(reference.yes_price, Decimal::new(60, 2));
        assert_eq!(
            reference.observed_at.timestamp_micros(),
            current_bucket.timestamp_micros()
        );
    }
}
