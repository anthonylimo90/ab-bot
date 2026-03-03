//! Smart money flow signal generator.
//!
//! Polls `market_flow_features` every 5 minutes looking for significant
//! order flow imbalance driven by smart money (non-bot wallets).
//!
//! Trigger conditions (all must hold):
//!   - |imbalance_ratio| >= 0.25
//!   - smart_money_flow >= $500
//!   - trade_count >= 5
//!
//! Direction: positive imbalance → BuyYes, negative → BuyNo
//! Confidence: weighted blend of imbalance magnitude and smart money flow
//! Expiry: 30 minutes from generation

use chrono::{Duration, Utc};
use polymarket_core::types::signal::{QuantSignal, QuantSignalKind, SignalDirection};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::time;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Configuration for the flow signal generator.
#[derive(Debug, Clone)]
pub struct FlowSignalConfig {
    /// Whether the generator is enabled.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    /// Minimum |imbalance_ratio| to trigger.
    pub min_imbalance: f64,
    /// Minimum smart money flow in USD.
    pub min_smart_money_flow: Decimal,
    /// Minimum trade count.
    pub min_trade_count: i32,
    /// Window size to scan (minutes).
    pub window_minutes: i32,
    /// Base position size for suggested_size_usd.
    pub base_position_size_usd: Decimal,
    /// Signal expiry in minutes.
    pub expiry_minutes: i64,
}

impl FlowSignalConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("FLOW_SIGNAL_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("FLOW_SIGNAL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            min_imbalance: std::env::var("FLOW_MIN_IMBALANCE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.25),
            min_smart_money_flow: std::env::var("FLOW_MIN_SMART_MONEY_USD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(Decimal::new(500, 0)),
            min_trade_count: std::env::var("FLOW_MIN_TRADE_COUNT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            window_minutes: std::env::var("FLOW_SIGNAL_WINDOW_MINUTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            base_position_size_usd: std::env::var("QUANT_BASE_POSITION_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(Decimal::new(30, 0)),
            expiry_minutes: 30,
        }
    }
}

/// Row from the flow features query.
#[derive(Debug, sqlx::FromRow)]
struct FlowFeatureRow {
    condition_id: String,
    imbalance_ratio: Decimal,
    smart_money_flow: Decimal,
    trade_count: i32,
    buy_volume: Decimal,
    sell_volume: Decimal,
    net_flow: Decimal,
}

/// Spawn the flow signal generator background task.
pub fn spawn_flow_signal_generator(
    config: FlowSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    if !config.enabled {
        info!("Flow signal generator disabled (FLOW_SIGNAL_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        min_imbalance = config.min_imbalance,
        min_smart_money_flow = %config.min_smart_money_flow,
        min_trade_count = config.min_trade_count,
        window_minutes = config.window_minutes,
        "Spawning flow signal generator"
    );

    tokio::spawn(generator_loop(config, pool, signal_tx));
}

async fn generator_loop(
    config: FlowSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    let interval = time::Duration::from_secs(config.interval_secs);

    // Startup delay — let flow features populate first
    tokio::time::sleep(time::Duration::from_secs(60)).await;

    loop {
        match scan_and_emit(&config, &pool, &signal_tx).await {
            Ok(count) => {
                if count > 0 {
                    info!(signals = count, "Flow signal generator emitted signals");
                } else {
                    debug!("Flow signal generator: no qualifying markets this cycle");
                }
            }
            Err(e) => {
                warn!(error = %e, "Flow signal generator cycle failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

async fn scan_and_emit(
    config: &FlowSignalConfig,
    pool: &PgPool,
    signal_tx: &broadcast::Sender<QuantSignal>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();

    // Query the most recent flow features for the configured window size
    // that meet our minimum thresholds.
    let rows = sqlx::query_as::<_, FlowFeatureRow>(
        r#"
        SELECT
            condition_id,
            imbalance_ratio,
            smart_money_flow,
            trade_count,
            buy_volume,
            sell_volume,
            net_flow
        FROM market_flow_features
        WHERE window_minutes = $1
          AND window_end >= $2 - INTERVAL '10 minutes'
          AND ABS(imbalance_ratio) >= $3
          AND ABS(smart_money_flow) >= $4
          AND trade_count >= $5
        ORDER BY ABS(imbalance_ratio) DESC
        LIMIT 50
        "#,
    )
    .bind(config.window_minutes)
    .bind(now)
    .bind(Decimal::try_from(config.min_imbalance).unwrap_or(Decimal::new(25, 2)))
    .bind(config.min_smart_money_flow)
    .bind(config.min_trade_count)
    .fetch_all(pool)
    .await?;

    let mut emitted = 0;

    for row in &rows {
        let imbalance_abs = decimal_to_f64(row.imbalance_ratio.abs());
        let smart_money_abs = decimal_to_f64(row.smart_money_flow.abs());

        // Direction: positive imbalance (more buys) → BuyYes, negative → BuyNo
        let direction = if row.imbalance_ratio > Decimal::ZERO {
            SignalDirection::BuyYes
        } else {
            SignalDirection::BuyNo
        };

        // Confidence: weighted blend of imbalance magnitude + smart money flow
        // imbalance_ratio ranges [-1, 1], so abs gives [0, 1]
        // Normalize smart money flow: $500 = 0.0, $5000 = 1.0
        let smart_money_normalized = ((smart_money_abs - 500.0) / 4500.0).clamp(0.0, 1.0);
        let confidence = (imbalance_abs * 0.6 + smart_money_normalized * 0.4).clamp(0.0, 1.0);

        let expiry = now + Duration::minutes(config.expiry_minutes);

        let signal = QuantSignal::new(
            QuantSignalKind::Flow,
            row.condition_id.clone(),
            direction,
            confidence,
            config.base_position_size_usd,
            expiry,
        )
        .with_metadata(serde_json::json!({
            "imbalance_ratio": decimal_to_f64(row.imbalance_ratio),
            "smart_money_flow": decimal_to_f64(row.smart_money_flow),
            "trade_count": row.trade_count,
            "buy_volume": decimal_to_f64(row.buy_volume),
            "sell_volume": decimal_to_f64(row.sell_volume),
            "net_flow": decimal_to_f64(row.net_flow),
            "window_minutes": config.window_minutes,
        }));

        debug!(
            condition_id = &row.condition_id,
            direction = signal.direction.as_str(),
            confidence = signal.confidence,
            imbalance = imbalance_abs,
            smart_money = smart_money_abs,
            "Flow signal generated"
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
        let config = FlowSignalConfig::from_env();
        assert!(config.enabled);
        assert_eq!(config.interval_secs, 300);
        assert_eq!(config.min_imbalance, 0.25);
        assert_eq!(config.min_smart_money_flow, Decimal::new(500, 0));
        assert_eq!(config.min_trade_count, 5);
        assert_eq!(config.window_minutes, 60);
    }

    #[test]
    fn test_confidence_calculation() {
        // imbalance = 0.5, smart_money = $2750 (midpoint)
        let imbalance_abs: f64 = 0.5;
        let smart_money_abs: f64 = 2750.0;
        let smart_money_normalized = ((smart_money_abs - 500.0) / 4500.0).clamp(0.0, 1.0);
        let confidence = (imbalance_abs * 0.6 + smart_money_normalized * 0.4).clamp(0.0, 1.0);

        assert!((confidence - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_confidence_max() {
        // Max imbalance = 1.0, max smart money = $5000+
        let imbalance_abs: f64 = 1.0;
        let smart_money_abs: f64 = 10000.0;
        let smart_money_normalized = ((smart_money_abs - 500.0) / 4500.0).clamp(0.0, 1.0);
        let confidence = (imbalance_abs * 0.6 + smart_money_normalized * 0.4).clamp(0.0, 1.0);

        assert_eq!(confidence, 1.0);
    }
}
