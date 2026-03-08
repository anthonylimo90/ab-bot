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
//! Confidence: EV-style score using imbalance, smart-money share, participant
//! breadth, liquidity, and price regime
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
    /// Minimum EV-style score (0.0–1.0) required to emit a signal.
    pub min_score: f64,
    /// Minimum expected edge in basis points required to emit a signal.
    pub min_expected_edge_bps: f64,
    /// Minimum fraction of net flow attributable to smart money.
    pub min_smart_money_share: f64,
    /// Maximum signals to emit per scan.
    pub max_signals_per_cycle: i64,
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
            min_score: std::env::var("FLOW_MIN_SCORE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.58),
            min_expected_edge_bps: std::env::var("FLOW_MIN_EXPECTED_EDGE_BPS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(35.0),
            min_smart_money_share: std::env::var("FLOW_MIN_SMART_MONEY_SHARE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.35),
            max_signals_per_cycle: std::env::var("FLOW_MAX_SIGNALS_PER_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15),
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
    unique_buyers: i32,
    unique_sellers: i32,
    buy_volume: Decimal,
    sell_volume: Decimal,
    net_flow: Decimal,
    liquidity: Decimal,
    market_volume: Decimal,
    yes_price: Option<f64>,
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
        min_score = config.min_score,
        min_expected_edge_bps = config.min_expected_edge_bps,
        min_smart_money_share = config.min_smart_money_share,
        max_signals_per_cycle = config.max_signals_per_cycle,
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
        WITH latest_features AS (
            SELECT DISTINCT ON (mff.condition_id)
                mff.condition_id,
                mff.imbalance_ratio,
                mff.smart_money_flow,
                mff.trade_count,
                mff.unique_buyers,
                mff.unique_sellers,
                mff.buy_volume,
                mff.sell_volume,
                mff.net_flow,
                mff.window_end
            FROM market_flow_features mff
            WHERE mff.window_minutes = $1
              AND mff.window_end >= $2 - INTERVAL '10 minutes'
              AND ABS(mff.imbalance_ratio) >= $3
              AND ABS(mff.smart_money_flow) >= $4
              AND mff.trade_count >= $5
            ORDER BY mff.condition_id, mff.window_end DESC
        )
        SELECT
            lf.condition_id,
            lf.imbalance_ratio,
            lf.smart_money_flow,
            lf.trade_count,
            lf.unique_buyers,
            lf.unique_sellers,
            lf.buy_volume,
            lf.sell_volume,
            lf.net_flow,
            COALESCE(mm.liquidity, 0) AS liquidity,
            COALESCE(mm.volume, 0) AS market_volume,
            ob.yes_price
        FROM latest_features lf
        LEFT JOIN market_metadata mm
          ON mm.condition_id = lf.condition_id
        LEFT JOIN LATERAL (
            SELECT close AS yes_price
            FROM orderbook_hourly
            WHERE market_id = lf.condition_id
            ORDER BY bucket DESC
            LIMIT 1
        ) ob ON true
        WHERE COALESCE(mm.active, true) = true
        ORDER BY ABS(lf.smart_money_flow) DESC, ABS(lf.imbalance_ratio) DESC
        LIMIT 100
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
    let mut scored_rows: Vec<(f64, f64, &FlowFeatureRow, f64, f64, f64, f64, f64, f64)> =
        Vec::new();

    for row in &rows {
        let imbalance_abs = decimal_to_f64(row.imbalance_ratio.abs());
        let smart_money_abs = decimal_to_f64(row.smart_money_flow.abs());
        let total_flow_abs = decimal_to_f64(row.buy_volume + row.sell_volume).max(1.0);
        let net_flow_abs = decimal_to_f64(row.net_flow.abs()).max(1.0);
        let smart_money_share = (smart_money_abs / net_flow_abs).clamp(0.0, 1.5);
        let smart_money_intensity = (smart_money_abs / total_flow_abs).clamp(0.0, 1.0);
        let participant_count = (row.unique_buyers + row.unique_sellers).max(0) as f64;
        let breadth_score = (participant_count / 14.0).clamp(0.0, 1.0);
        let trade_count_score = (row.trade_count as f64 / 14.0).clamp(0.0, 1.0);
        let liquidity_score = (decimal_to_f64(row.liquidity) / 25_000.0).clamp(0.0, 1.0);
        let volume_score = (decimal_to_f64(row.market_volume) / 50_000.0).clamp(0.0, 1.0);
        let price_score = price_regime_score(row.yes_price);

        if smart_money_share < config.min_smart_money_share {
            continue;
        }

        let (score, expected_edge_bps) = flow_score_components(
            imbalance_abs,
            smart_money_share,
            smart_money_intensity,
            breadth_score,
            trade_count_score,
            liquidity_score,
            volume_score,
            price_score,
        );

        if score < config.min_score || expected_edge_bps < config.min_expected_edge_bps {
            continue;
        }

        scored_rows.push((
            expected_edge_bps,
            score,
            row,
            smart_money_share,
            smart_money_intensity,
            breadth_score,
            liquidity_score,
            volume_score,
            price_score,
        ));
    }

    scored_rows.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal))
    });

    for (
        expected_edge_bps,
        score,
        row,
        smart_money_share,
        smart_money_intensity,
        breadth_score,
        liquidity_score,
        volume_score,
        price_score,
    ) in scored_rows
        .into_iter()
        .take(config.max_signals_per_cycle as usize)
    {
        let imbalance_abs = decimal_to_f64(row.imbalance_ratio.abs());
        let smart_money_abs = decimal_to_f64(row.smart_money_flow.abs());

        // Direction: positive imbalance (more buys) → BuyYes, negative → BuyNo
        let direction = if row.imbalance_ratio > Decimal::ZERO {
            SignalDirection::BuyYes
        } else {
            SignalDirection::BuyNo
        };

        let confidence = score.clamp(0.0, 0.95);

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
            "unique_buyers": row.unique_buyers,
            "unique_sellers": row.unique_sellers,
            "buy_volume": decimal_to_f64(row.buy_volume),
            "sell_volume": decimal_to_f64(row.sell_volume),
            "net_flow": decimal_to_f64(row.net_flow),
            "liquidity": decimal_to_f64(row.liquidity),
            "market_volume": decimal_to_f64(row.market_volume),
            "yes_price": row.yes_price,
            "score": score,
            "expected_edge_bps": expected_edge_bps,
            "smart_money_share": smart_money_share,
            "smart_money_intensity": smart_money_intensity,
            "breadth_score": breadth_score,
            "liquidity_score": liquidity_score,
            "volume_score": volume_score,
            "price_regime_score": price_score,
            "window_minutes": config.window_minutes,
        }));

        debug!(
            condition_id = &row.condition_id,
            direction = signal.direction.as_str(),
            confidence = signal.confidence,
            imbalance = imbalance_abs,
            smart_money = smart_money_abs,
            expected_edge_bps = expected_edge_bps,
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

fn price_regime_score(yes_price: Option<f64>) -> f64 {
    yes_price
        .map(|price| 1.0 - ((price - 0.5).abs() / 0.45).clamp(0.0, 1.0))
        .unwrap_or(0.5)
}

fn flow_score_components(
    imbalance_abs: f64,
    smart_money_share: f64,
    smart_money_intensity: f64,
    breadth_score: f64,
    trade_count_score: f64,
    liquidity_score: f64,
    volume_score: f64,
    price_score: f64,
) -> (f64, f64) {
    let score = (imbalance_abs * 0.32
        + smart_money_share.min(1.0) * 0.26
        + smart_money_intensity * 0.18
        + breadth_score * 0.10
        + trade_count_score * 0.07
        + liquidity_score * 0.04
        + volume_score * 0.03)
        * (0.75 + 0.25 * price_score);

    let expected_edge_bps = (imbalance_abs * 55.0)
        + (smart_money_share.min(1.0) * 35.0)
        + (smart_money_intensity * 20.0)
        + (breadth_score * 12.0)
        + (liquidity_score * 8.0)
        - ((1.0 - price_score) * 18.0);

    (score, expected_edge_bps)
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
        assert!(config.min_score > 0.0);
        assert!(config.min_expected_edge_bps > 0.0);
        assert_eq!(config.window_minutes, 60);
    }

    #[test]
    fn test_confidence_calculation() {
        let (score, expected_edge_bps) =
            flow_score_components(0.50, 0.80, 0.60, 0.70, 0.60, 0.50, 0.40, 0.90);

        assert!(score > 0.58);
        assert!(score < 0.95);
        assert!(expected_edge_bps > 35.0);
    }

    #[test]
    fn test_confidence_max() {
        let (score, expected_edge_bps) =
            flow_score_components(1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0);

        assert!(score > 0.90);
        assert!(expected_edge_bps > 100.0);
    }

    #[test]
    fn test_price_regime_penalizes_extremes() {
        assert!(price_regime_score(Some(0.50)) > price_regime_score(Some(0.92)));
        assert!(price_regime_score(Some(0.50)) > price_regime_score(Some(0.08)));
    }
}
