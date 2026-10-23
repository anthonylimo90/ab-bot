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
//! Direction: positive imbalance → BuyYes, negative → BuyNo. The imbalance is
//! signed towards YES by the flow feature calculator (BUY YES / SELL NO are
//! bullish, SELL YES / BUY NO bearish), so buying NO never counts as YES demand.
//! Smart money means wallets with a bot score below the threshold; wallets
//! without a score are unknown and do not count.
//! Confidence: heuristic score using imbalance, smart-money share, participant
//! breadth, liquidity, and price regime
//! Expiry: 30 minutes from generation

use chrono::{Duration, Utc};
use polymarket_core::types::signal::{
    PriceReference, QuantSignal, QuantSignalKind, SignalDirection,
};
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
    /// Minimum share of recent trades that have a known bot score.
    pub min_bot_score_coverage: f64,
    /// Require a recent yes-price mark before scoring the market.
    pub require_yes_price: bool,
    /// Maximum signals to emit per scan.
    pub max_signals_per_cycle: i64,
    /// Window size to scan (minutes).
    pub window_minutes: i32,
    /// Base position size for suggested_size_usd.
    pub base_position_size_usd: Decimal,
    /// Signal expiry in minutes.
    pub expiry_minutes: i64,
    /// Lookback window for realized calibration.
    pub calibration_lookback_days: i64,
    /// Minimum recent closed trades required to use calibrated edges.
    pub calibration_min_closed_trades: i64,
    /// Whether signals need calibration from closed flow trades before they
    /// may trade live. Uncalibrated signals are still emitted, marked
    /// `uncalibrated`, so paper trading can build the calibration.
    pub require_calibration: bool,
}

impl FlowSignalConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_flag("FLOW_SIGNAL_ENABLED", true),
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
            min_bot_score_coverage: std::env::var("FLOW_MIN_BOT_SCORE_COVERAGE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.50),
            require_yes_price: polymarket_core::config::env_flag("FLOW_REQUIRE_YES_PRICE", true),
            max_signals_per_cycle: std::env::var("FLOW_MAX_SIGNALS_PER_CYCLE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15),
            window_minutes: std::env::var("FLOW_SIGNAL_WINDOW_MINUTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            base_position_size_usd: polymarket_core::config::env_decimal(
                "QUANT_BASE_POSITION_SIZE",
                Decimal::new(30, 0),
            ),
            expiry_minutes: 30,
            calibration_lookback_days: std::env::var("FLOW_CALIBRATION_LOOKBACK_DAYS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(14),
            calibration_min_closed_trades: std::env::var("FLOW_CALIBRATION_MIN_CLOSED_TRADES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
            require_calibration: polymarket_core::config::env_flag(
                "FLOW_REQUIRE_CALIBRATION",
                true,
            ),
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
    /// Start of the hourly bar `yes_price` closed.
    yes_price_bucket: Option<chrono::DateTime<Utc>>,
    bot_score_coverage: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct FlowCalibrationStats {
    closed_trades: i64,
    win_rate: f64,
    avg_realized_return_bps: f64,
    edge_capture_ratio: f64,
}

impl FlowCalibrationStats {
    fn usable(self, min_closed_trades: i64) -> bool {
        self.closed_trades >= min_closed_trades && self.edge_capture_ratio.is_finite()
    }
}

#[derive(Debug, Clone, Default)]
struct FlowCalibrationSnapshot {
    overall: Option<FlowCalibrationStats>,
    buy_yes: Option<FlowCalibrationStats>,
    buy_no: Option<FlowCalibrationStats>,
}

impl FlowCalibrationSnapshot {
    fn usable_for(
        &self,
        direction: SignalDirection,
        min_closed_trades: i64,
    ) -> Option<FlowCalibrationStats> {
        let directional = match direction {
            SignalDirection::BuyYes => self.buy_yes,
            SignalDirection::BuyNo => self.buy_no,
        };

        directional
            .filter(|stats| stats.usable(min_closed_trades))
            .or_else(|| self.overall.filter(|stats| stats.usable(min_closed_trades)))
    }

    fn has_usable_data(&self, min_closed_trades: i64) -> bool {
        self.overall
            .map(|stats| stats.usable(min_closed_trades))
            .unwrap_or(false)
    }
}

#[derive(Debug, sqlx::FromRow)]
struct FlowCalibrationRow {
    direction: Option<String>,
    closed_trades: i64,
    win_rate: Option<f64>,
    avg_realized_return_bps: Option<f64>,
    edge_capture_ratio: Option<f64>,
}

/// Spawn the flow signal generator background task.
pub fn spawn_flow_signal_generator(
    config: FlowSignalConfig,
    pool: PgPool,
    signal_tx: broadcast::Sender<QuantSignal>,
) {
    if !config.enabled {
        info!("Flow signal generator disabled (FLOW_SIGNAL_ENABLED is off)");
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
        min_bot_score_coverage = config.min_bot_score_coverage,
        require_yes_price = config.require_yes_price,
        max_signals_per_cycle = config.max_signals_per_cycle,
        window_minutes = config.window_minutes,
        calibration_lookback_days = config.calibration_lookback_days,
        calibration_min_closed_trades = config.calibration_min_closed_trades,
        require_calibration = config.require_calibration,
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

/// Direction of the signal from the YES-signed imbalance: positive flow favours
/// YES → BuyYes, negative → BuyNo.
///
/// The imbalance stored in `market_flow_features` already accounts for which
/// outcome token each trade bought or sold, so the sign can be used directly.
pub(crate) fn direction_from_imbalance(imbalance_ratio: Decimal) -> SignalDirection {
    if imbalance_ratio > Decimal::ZERO {
        SignalDirection::BuyYes
    } else {
        SignalDirection::BuyNo
    }
}

/// The direction to follow, only when the overall imbalance, the net flow and
/// the smart-money flow all point the same way. Smart money is a subset of
/// the flow and can oppose it; following the crowd against it is not a flow
/// signal.
fn agreed_flow_direction(
    imbalance_ratio: Decimal,
    net_flow: Decimal,
    smart_money_flow: Decimal,
) -> Option<SignalDirection> {
    let sign = |value: Decimal| {
        if value > Decimal::ZERO {
            1
        } else if value < Decimal::ZERO {
            -1
        } else {
            0
        }
    };
    let direction = sign(imbalance_ratio);
    (direction != 0 && sign(net_flow) == direction && sign(smart_money_flow) == direction)
        .then(|| direction_from_imbalance(imbalance_ratio))
}

/// Metadata flag of a flow signal emitted without the calibration its
/// configuration requires: it may be paper traded, never traded live.
pub(crate) const UNCALIBRATED_METADATA_KEY: &str = "uncalibrated";

async fn scan_and_emit(
    config: &FlowSignalConfig,
    pool: &PgPool,
    signal_tx: &broadcast::Sender<QuantSignal>,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();
    let window_start = now - Duration::minutes(config.window_minutes as i64);
    let calibration =
        load_flow_calibration(pool, now - Duration::days(config.calibration_lookback_days)).await?;

    if config.require_calibration
        && !calibration.has_usable_data(config.calibration_min_closed_trades)
    {
        // Calibration comes from closed flow trades, which only signals can
        // open: emit them marked uncalibrated, for paper trading only.
        info!(
            lookback_days = config.calibration_lookback_days,
            min_closed_trades = config.calibration_min_closed_trades,
            "Flow calibration unavailable; signals are emitted for paper trading only"
        );
    }

    // Query the most recent flow features for the configured window size
    // that meet our minimum thresholds.
    let rows = sqlx::query_as::<_, FlowFeatureRow>(
        r#"
        WITH newest_features AS (
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
            ORDER BY mff.condition_id, mff.window_end DESC
        ),
        -- Thresholds apply to each market's newest window only: an older
        -- window must never stand in for a newer one that no longer qualifies.
        latest_features AS (
            SELECT nf.*
            FROM newest_features nf
            WHERE ABS(nf.imbalance_ratio) >= $3
              AND ABS(nf.smart_money_flow) >= $4
              AND nf.trade_count >= $5
        ),
        recent_window_trades AS (
            SELECT
                wt.condition_id,
                wt.wallet_address
            FROM wallet_trades wt
            INNER JOIN latest_features lf
              ON lf.condition_id = wt.condition_id
            WHERE wt.condition_id IS NOT NULL
              AND LOWER(wt.outcome) IN ('yes', 'no')
              AND wt.timestamp >= $6
              AND wt.timestamp <= $2
        ),
        latest_bot_scores AS (
            SELECT DISTINCT ON (bs.address)
                bs.address,
                bs.total_score
            FROM bot_scores bs
            INNER JOIN (
                SELECT DISTINCT wallet_address
                FROM recent_window_trades
            ) rw
              ON rw.wallet_address = bs.address
            ORDER BY bs.address, bs.computed_at DESC
        ),
        score_coverage AS (
            SELECT
                rwt.condition_id,
                CASE
                    WHEN COUNT(*) = 0 THEN 0::double precision
                    ELSE COUNT(*) FILTER (WHERE lbs.total_score IS NOT NULL)::double precision
                        / COUNT(*)::double precision
                END AS bot_score_coverage
            FROM recent_window_trades rwt
            LEFT JOIN latest_bot_scores lbs
              ON lbs.address = rwt.wallet_address
            GROUP BY rwt.condition_id
        ),
        latest_orderbook_prices AS (
            SELECT DISTINCT ON (oh.market_id)
                oh.market_id,
                oh.close::float8 AS yes_price,
                oh.bucket AS yes_price_bucket
            FROM orderbook_hourly oh
            INNER JOIN latest_features lf
              ON lf.condition_id = oh.market_id
            ORDER BY oh.market_id, oh.bucket DESC
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
            ob.yes_price,
            ob.yes_price_bucket,
            COALESCE(sc.bot_score_coverage, 0) AS bot_score_coverage
        FROM latest_features lf
        LEFT JOIN market_metadata mm
          ON mm.condition_id = lf.condition_id
        LEFT JOIN latest_orderbook_prices ob
          ON ob.market_id = lf.condition_id
        LEFT JOIN score_coverage sc
          ON sc.condition_id = lf.condition_id
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
    .bind(window_start)
    .fetch_all(pool)
    .await?;

    let mut emitted = 0;
    type ScoredFlowRow<'a> = (f64, f64, &'a FlowFeatureRow, f64, f64, f64, f64, f64, f64);
    let mut scored_rows: Vec<ScoredFlowRow<'_>> = Vec::new();

    for row in &rows {
        if config.require_yes_price && row.yes_price.is_none() {
            continue;
        }

        if row.bot_score_coverage < config.min_bot_score_coverage {
            continue;
        }

        if agreed_flow_direction(row.imbalance_ratio, row.net_flow, row.smart_money_flow).is_none()
        {
            continue;
        }

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

        let Some(direction) =
            agreed_flow_direction(row.imbalance_ratio, row.net_flow, row.smart_money_flow)
        else {
            continue;
        };

        let usable = calibration.usable_for(direction, config.calibration_min_closed_trades);
        // Without usable calibration a required one is missing: the raw edge
        // stands, and the signal may only be paper traded.
        let uncalibrated = config.require_calibration && usable.is_none();
        let calibration_stats = usable.unwrap_or_default();
        let calibrated_expected_edge_bps = if config.require_calibration && !uncalibrated {
            calibrate_expected_edge_bps(expected_edge_bps, calibration_stats)
        } else {
            expected_edge_bps
        };

        if calibrated_expected_edge_bps < config.min_expected_edge_bps {
            continue;
        }

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
        .with_reference(
            row.yes_price
                .zip(row.yes_price_bucket)
                .and_then(|(price, at)| PriceReference::from_yes_price(price, at)),
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
            "raw_expected_edge_bps": expected_edge_bps,
            "expected_edge_bps": calibrated_expected_edge_bps,
            "smart_money_share": smart_money_share,
            "smart_money_intensity": smart_money_intensity,
            "breadth_score": breadth_score,
            "liquidity_score": liquidity_score,
            "volume_score": volume_score,
            "price_regime_score": price_score,
            "bot_score_coverage": row.bot_score_coverage,
            "calibration_closed_trades": calibration_stats.closed_trades,
            "calibration_win_rate": calibration_stats.win_rate,
            "calibration_avg_realized_return_bps": calibration_stats.avg_realized_return_bps,
            "calibration_edge_capture_ratio": calibration_stats.edge_capture_ratio,
            UNCALIBRATED_METADATA_KEY: uncalibrated,
            "window_minutes": config.window_minutes,
        }));

        debug!(
            condition_id = &row.condition_id,
            direction = signal.direction.as_str(),
            confidence = signal.confidence,
            imbalance = imbalance_abs,
            smart_money = smart_money_abs,
            raw_expected_edge_bps = expected_edge_bps,
            calibrated_expected_edge_bps = calibrated_expected_edge_bps,
            bot_score_coverage = row.bot_score_coverage,
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

fn calibrate_expected_edge_bps(raw_expected_edge_bps: f64, stats: FlowCalibrationStats) -> f64 {
    let win_rate_centered = ((stats.win_rate - 0.5) * 2.0).clamp(-1.0, 1.0);
    let realized_return_scale = (stats.avg_realized_return_bps / 100.0).clamp(-1.0, 1.0);
    let capture_scale = stats.edge_capture_ratio.clamp(-1.5, 1.5);
    let scale =
        (capture_scale * 0.60) + (win_rate_centered * 0.30) + (realized_return_scale * 0.10);

    raw_expected_edge_bps * scale.clamp(-1.5, 1.5)
}

#[allow(clippy::too_many_arguments)]
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

async fn load_flow_calibration(
    pool: &PgPool,
    lookback_start: chrono::DateTime<Utc>,
) -> Result<FlowCalibrationSnapshot, sqlx::Error> {
    let rows: Vec<FlowCalibrationRow> = sqlx::query_as(
        r#"
        -- Returns are measured on what the position cost, not the signal's
        -- suggested size (the executor scales that down), and against the
        -- raw edge the calibration scales: the emitted edge is already
        -- calibrated, and measuring against it would feed calibration back
        -- into itself. A win a resolution close could not confirm in the
        -- wallet is left out, as the executor's strategy tally leaves it out.
        WITH closed_flow AS (
            SELECT
                qs.direction,
                p.quantity * (p.yes_entry_price + p.no_entry_price) AS size_usd,
                p.realized_pnl,
                COALESCE(
                    NULLIF(qs.metadata ->> 'raw_expected_edge_bps', ''),
                    NULLIF(qs.metadata ->> 'expected_edge_bps', '')
                )::double precision AS predicted_edge_bps
            FROM quant_signals qs
            JOIN positions p
              ON p.source_signal_id = qs.id
            WHERE qs.kind = 'flow'
              AND qs.execution_status = 'executed'
              AND p.state = 4
              AND p.realized_pnl IS NOT NULL
              AND qs.generated_at >= $1
              AND NOT (p.realized_pnl > 0 AND EXISTS (
                  SELECT 1 FROM trade_events te
                  WHERE te.position_id = p.id
                    AND te.event_type = 'closed_via_resolution'
                    AND te.metadata ? 'unmeasured'))
        ),
        grouped AS (
            SELECT direction, size_usd, realized_pnl, predicted_edge_bps
            FROM closed_flow
            UNION ALL
            SELECT NULL::text AS direction, size_usd, realized_pnl, predicted_edge_bps
            FROM closed_flow
        )
        SELECT
            direction,
            COUNT(*)::bigint AS closed_trades,
            AVG(CASE WHEN realized_pnl > 0 THEN 1.0 ELSE 0.0 END)::double precision AS win_rate,
            AVG(
                CASE
                    WHEN size_usd IS NOT NULL AND size_usd > 0
                    THEN (realized_pnl::double precision / size_usd::double precision) * 10000.0
                    ELSE NULL
                END
            ) AS avg_realized_return_bps,
            CASE
                WHEN COALESCE(SUM(ABS(predicted_edge_bps)), 0) = 0 THEN NULL
                ELSE SUM(
                    CASE
                        WHEN size_usd IS NOT NULL AND size_usd > 0
                        THEN (realized_pnl::double precision / size_usd::double precision) * 10000.0
                        ELSE 0
                    END
                ) / SUM(ABS(predicted_edge_bps))
            END AS edge_capture_ratio
        FROM grouped
        GROUP BY direction
        "#,
    )
    .bind(lookback_start)
    .fetch_all(pool)
    .await?;

    let mut snapshot = FlowCalibrationSnapshot::default();
    for row in rows {
        let stats = FlowCalibrationStats {
            closed_trades: row.closed_trades,
            win_rate: row.win_rate.unwrap_or(0.0),
            avg_realized_return_bps: row.avg_realized_return_bps.unwrap_or(0.0),
            edge_capture_ratio: row.edge_capture_ratio.unwrap_or(0.0),
        };

        match row.direction.as_deref() {
            Some("buy_yes") => snapshot.buy_yes = Some(stats),
            Some("buy_no") => snapshot.buy_no = Some(stats),
            _ => snapshot.overall = Some(stats),
        }
    }

    Ok(snapshot)
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
        assert!(config.require_yes_price);
        assert!(config.require_calibration);
        assert!(config.min_bot_score_coverage > 0.0);
        assert_eq!(config.window_minutes, 60);
    }

    #[test]
    fn direction_follows_the_sign_of_the_yes_signed_imbalance() {
        assert_eq!(
            direction_from_imbalance(Decimal::new(25, 2)),
            SignalDirection::BuyYes
        );
        assert_eq!(
            direction_from_imbalance(Decimal::new(-25, 2)),
            SignalDirection::BuyNo
        );
        // Flat flow never qualifies for a signal (|imbalance| is gated), but the
        // mapping itself must be total.
        assert_eq!(
            direction_from_imbalance(Decimal::ZERO),
            SignalDirection::BuyNo
        );
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

    #[test]
    fn test_calibrated_edge_turns_negative_after_bad_realized_outcomes() {
        let stats = FlowCalibrationStats {
            closed_trades: 18,
            win_rate: 0.0,
            avg_realized_return_bps: -35.0,
            edge_capture_ratio: -0.30,
        };

        let calibrated = calibrate_expected_edge_bps(120.0, stats);
        assert!(calibrated < 0.0);
    }

    #[test]
    fn test_directional_calibration_falls_back_to_overall() {
        let snapshot = FlowCalibrationSnapshot {
            overall: Some(FlowCalibrationStats {
                closed_trades: 10,
                win_rate: 0.55,
                avg_realized_return_bps: 12.0,
                edge_capture_ratio: 0.45,
            }),
            buy_yes: None,
            buy_no: None,
        };

        assert!(snapshot.usable_for(SignalDirection::BuyYes, 8).is_some());
        assert!(snapshot.usable_for(SignalDirection::BuyNo, 8).is_some());
    }

    #[test]
    fn flow_is_followed_only_when_smart_money_agrees() {
        let d = |v: i64| Decimal::from(v);
        // Unscored wallets buy YES while smart money sells it: the crowd's
        // +0.389 imbalance must not become a BuyYes "smart money" signal.
        assert_eq!(
            agreed_flow_direction(Decimal::new(389, 3), d(2800), d(-1200)),
            None
        );
        assert_eq!(
            agreed_flow_direction(Decimal::new(40, 2), d(2000), d(1500)),
            Some(SignalDirection::BuyYes)
        );
        assert_eq!(
            agreed_flow_direction(Decimal::new(-40, 2), d(-2000), d(-1500)),
            Some(SignalDirection::BuyNo)
        );
        assert_eq!(agreed_flow_direction(Decimal::ZERO, d(0), d(900)), None);
        assert_eq!(
            agreed_flow_direction(Decimal::new(40, 2), d(-5), d(900)),
            None
        );
    }

    async fn create_calibration_tables(pool: &PgPool) {
        sqlx::Executor::execute(
            pool,
            r#"
            CREATE TABLE positions (
                id UUID, state SMALLINT, realized_pnl NUMERIC, source_signal_id UUID,
                quantity NUMERIC, yes_entry_price NUMERIC, no_entry_price NUMERIC
            );
            CREATE TABLE quant_signals (
                id UUID, kind TEXT, direction TEXT, size_usd NUMERIC, metadata JSONB,
                execution_status TEXT, generated_at TIMESTAMPTZ
            );
            CREATE TABLE trade_events (position_id UUID, event_type TEXT, metadata JSONB);
            "#,
        )
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = false)]
    async fn calibration_measures_returns_on_cost_against_the_raw_edge(pool: PgPool) {
        create_calibration_tables(&pool).await;
        let closed = |pnl: Decimal, raw_edge: f64, calibrated_edge: f64| {
            let pool = pool.clone();
            async move {
                let signal_id = uuid::Uuid::new_v4();
                let position_id = uuid::Uuid::new_v4();
                // The signal suggested $30; the position cost 10 x 0.90 = $9.
                sqlx::query(
                    "INSERT INTO quant_signals VALUES
                     ($1, 'flow', 'buy_yes', 30, $2, 'executed', NOW())",
                )
                .bind(signal_id)
                .bind(serde_json::json!({
                    "raw_expected_edge_bps": raw_edge,
                    "expected_edge_bps": calibrated_edge,
                }))
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO positions VALUES ($1, 4, $2, $3, 10, 0.90, 0)")
                    .bind(position_id)
                    .bind(pnl)
                    .bind(signal_id)
                    .execute(&pool)
                    .await
                    .unwrap();
                position_id
            }
        };
        // +$0.36 on $9 is +400 bps, exactly the raw edge predicted.
        closed(Decimal::new(36, 2), 400.0, 120.0).await;
        // A resolution win the wallet could not confirm does not count.
        let unmeasured = closed(Decimal::new(900, 2), 400.0, 120.0).await;
        sqlx::query(
            "INSERT INTO trade_events VALUES
             ($1, 'closed_via_resolution', '{\"unmeasured\": true}')",
        )
        .bind(unmeasured)
        .execute(&pool)
        .await
        .unwrap();

        let snapshot = load_flow_calibration(&pool, Utc::now() - Duration::days(1))
            .await
            .unwrap();
        let stats = snapshot.buy_yes.unwrap();
        assert_eq!(stats.closed_trades, 1);
        assert!((stats.avg_realized_return_bps - 400.0).abs() < 1e-6);
        assert!((stats.edge_capture_ratio - 1.0).abs() < 1e-9);
    }

    #[sqlx::test(migrations = false)]
    async fn scan_follows_only_each_market_newest_agreeing_window(pool: PgPool) {
        use crate::signals::hourly_bars;
        sqlx::Executor::execute(
            &pool,
            r#"
            CREATE TABLE market_flow_features (
                condition_id TEXT, window_end TIMESTAMPTZ, window_minutes INT,
                buy_volume NUMERIC, sell_volume NUMERIC, net_flow NUMERIC,
                imbalance_ratio NUMERIC, unique_buyers INT, unique_sellers INT,
                smart_money_flow NUMERIC, trade_count INT
            );
            CREATE TABLE wallet_trades (
                condition_id TEXT, wallet_address TEXT, outcome TEXT, timestamp TIMESTAMPTZ
            );
            CREATE TABLE bot_scores (address TEXT, total_score INT, computed_at TIMESTAMPTZ);
            "#,
        )
        .await
        .unwrap();
        create_calibration_tables(&pool).await;
        sqlx::Executor::execute(
            &pool,
            include_str!("../../../../migrations/038_market_metadata.sql"),
        )
        .await
        .unwrap();
        hourly_bars::create(&pool).await;

        let now = Utc::now();
        let window = |market: &str, minutes_ago: i64, imbalance: i64, net: i64, smart: i64| {
            let pool = pool.clone();
            let market = market.to_string();
            async move {
                sqlx::query(
                    "INSERT INTO market_flow_features VALUES
                     ($1, $2, 60, 3500, 1500, $4, $3, 6, 2, $5, 10)",
                )
                .bind(&market)
                .bind(now - Duration::minutes(minutes_ago))
                .bind(Decimal::new(imbalance, 3))
                .bind(Decimal::from(net))
                .bind(Decimal::from(smart))
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO wallet_trades VALUES ($1, '0xscored', 'Yes', $2)")
                    .bind(&market)
                    .bind(now - Duration::minutes(5))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        // Everyone agrees: follow it.
        window("0xgood", 2, 400, 2000, 1500).await;
        // Smart money sells into an unscored crowd's buying.
        window("0xsplit", 2, 389, 2800, -1200).await;
        // Qualified eight minutes ago; the newest window no longer does.
        window("0xfaded", 8, 400, 2000, 1500).await;
        window("0xfaded", 3, 50, 250, 600).await;
        sqlx::query("INSERT INTO bot_scores VALUES ('0xscored', 10, NOW())")
            .execute(&pool)
            .await
            .unwrap();
        let bar = now - Duration::minutes(30);
        for market in ["0xgood", "0xsplit", "0xfaded"] {
            hourly_bars::insert(&pool, market, bar, Decimal::new(55, 2), Decimal::ONE).await;
        }

        let mut config = FlowSignalConfig::from_env();
        config.require_calibration = false;
        config.min_score = 0.0;
        config.min_expected_edge_bps = 0.0;
        let (tx, mut rx) = broadcast::channel(8);
        assert_eq!(scan_and_emit(&config, &pool, &tx).await.unwrap(), 1);

        let signal = rx.try_recv().unwrap();
        assert_eq!(signal.condition_id, "0xgood");
        assert_eq!(signal.direction, SignalDirection::BuyYes);
        assert_eq!(signal.metadata["uncalibrated"], serde_json::json!(false));
        let reference = signal.reference.unwrap();
        assert_eq!(reference.yes_price, Decimal::new(55, 2));
        assert_eq!(
            reference.observed_at.timestamp_micros(),
            bar.timestamp_micros()
        );

        // Calibration required but no closed flow trades yet: the signal is
        // still emitted, at its raw edge, marked for paper trading only, so
        // the calibration can ever exist.
        config.require_calibration = true;
        assert_eq!(scan_and_emit(&config, &pool, &tx).await.unwrap(), 1);
        let signal = rx.try_recv().unwrap();
        assert_eq!(signal.condition_id, "0xgood");
        assert_eq!(signal.metadata["uncalibrated"], serde_json::json!(true));
        assert_eq!(
            signal.metadata["expected_edge_bps"],
            signal.metadata["raw_expected_edge_bps"]
        );
    }
}
