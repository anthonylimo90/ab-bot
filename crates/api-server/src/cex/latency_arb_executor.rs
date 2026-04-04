//! Dedicated hot-path executor for CEX latency arbitrage signals.
//!
//! Consumes `PriceMovement` from the Binance WebSocket feed, maps them
//! to Polymarket contracts, computes Kelly-sized positions, and executes
//! FOK orders via the existing `OrderExecutor`.

use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};
use uuid::Uuid;

use risk_manager::circuit_breaker::CircuitBreaker;
use trading_engine::OrderExecutor;

use super::market_mapper::{MappedMarket, MarketMapper};
use super::price_tracker::{
    CexPriceTick, CexSymbol, PriceDirection, PriceMovement, PriceTracker, PriceTrackerConfig,
};

/// Configuration for the latency arb executor.
#[derive(Debug, Clone)]
pub struct LatencyArbExecutorConfig {
    pub enabled: bool,
    /// Minimum edge on Polymarket side (e.g. 0.10 = 10 cents).
    pub min_edge: f64,
    /// Maximum signal age before discarding.
    pub max_signal_age_ms: u64,
    /// Fractional Kelly multiplier (e.g. 0.10 = 10% of full Kelly).
    pub kelly_fraction: f64,
    /// Bankroll for Kelly sizing.
    pub kelly_bankroll: Decimal,
    /// Hard cap on position size.
    pub max_position_size: Decimal,
    /// Minimum position size (skip if Kelly recommends less).
    pub min_position_size: Decimal,
    /// Minimum YES price to consider (avoid dust).
    pub min_yes_price: f64,
    /// Maximum YES price to consider (avoid near-resolved).
    pub max_yes_price: f64,
    /// Per-market cooldown in milliseconds.
    pub cooldown_ms: u64,
}

impl LatencyArbExecutorConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("LATENCY_ARB_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(false),
            min_edge: std::env::var("LATENCY_ARB_MIN_EDGE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.10),
            max_signal_age_ms: std::env::var("LATENCY_ARB_MAX_SIGNAL_AGE_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1500),
            kelly_fraction: std::env::var("LATENCY_ARB_KELLY_FRACTION")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.10),
            kelly_bankroll: std::env::var("LATENCY_ARB_KELLY_BANKROLL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(1000, 0)),
            max_position_size: std::env::var("LATENCY_ARB_MAX_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(100, 0)),
            min_position_size: std::env::var("LATENCY_ARB_MIN_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(5, 0)),
            min_yes_price: std::env::var("LATENCY_ARB_MIN_YES_PRICE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.05),
            max_yes_price: std::env::var("LATENCY_ARB_MAX_YES_PRICE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.95),
            cooldown_ms: std::env::var("LATENCY_ARB_COOLDOWN_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30000),
        }
    }
}

/// Compute Kelly-sized position using the shared sizing module.
fn compute_kelly_size(
    p_win: f64,
    price: f64,
    config: &LatencyArbExecutorConfig,
) -> Option<Decimal> {
    let kelly_config = polymarket_core::sizing::KellyConfig {
        fraction: config.kelly_fraction,
        bankroll: config.kelly_bankroll,
        max_position: config.max_position_size,
        min_position: config.min_position_size,
    };
    polymarket_core::sizing::kelly_position_size(p_win, price, &kelly_config)
}

/// Spawn the latency arb executor loop.
pub fn spawn_latency_arb_executor(
    config: LatencyArbExecutorConfig,
    tracker_config: PriceTrackerConfig,
    mut price_rx: mpsc::Receiver<CexPriceTick>,
    market_mapper: Arc<MarketMapper>,
    circuit_breaker: Arc<CircuitBreaker>,
    pool: PgPool,
) -> JoinHandle<()> {
    info!(
        enabled = config.enabled,
        min_edge = config.min_edge,
        max_signal_age_ms = config.max_signal_age_ms,
        kelly_fraction = config.kelly_fraction,
        "Spawning latency arb executor"
    );

    tokio::spawn(async move {
        let mut tracker = PriceTracker::new(tracker_config);
        let mut cooldowns: HashMap<String, time::Instant> = HashMap::new();
        let cooldown_duration = time::Duration::from_millis(config.cooldown_ms);
        let mut signals_evaluated: u64 = 0;
        let mut signals_executed: u64 = 0;

        while let Some(tick) = price_rx.recv().await {
            // Run EMA update and check for significant movement
            let Some(movement) = tracker.on_tick(&tick) else {
                continue;
            };

            // Check signal age
            let age = movement.detected_at.elapsed();
            let age_ms = age.as_millis() as u64;
            if age_ms > config.max_signal_age_ms {
                debug!(
                    symbol = movement.symbol.as_str(),
                    age_ms, "Latency arb signal too old, discarding"
                );
                continue;
            }

            // Circuit breaker check
            if !circuit_breaker.can_trade().await {
                debug!("Latency arb: circuit breaker tripped, skipping");
                continue;
            }

            // Look up matched Polymarket markets
            let matched = market_mapper.get_markets_for_symbol(movement.symbol).await;

            if matched.is_empty() {
                continue;
            }

            for market in &matched {
                signals_evaluated += 1;

                // Cooldown check
                if let Some(last_trade) = cooldowns.get(&market.condition_id) {
                    if last_trade.elapsed() < cooldown_duration {
                        continue;
                    }
                }

                // Price bounds check
                if market.yes_price < config.min_yes_price
                    || market.yes_price > config.max_yes_price
                {
                    continue;
                }

                // Determine direction and edge
                let (should_buy_yes, p_win, edge) = compute_edge(&movement, market);

                if edge < config.min_edge {
                    continue;
                }

                let price = if should_buy_yes {
                    market.yes_price
                } else {
                    1.0 - market.yes_price // NO price
                };

                let Some(position_size) = compute_kelly_size(p_win, price, &config) else {
                    debug!(
                        condition_id = %market.condition_id,
                        p_win,
                        price,
                        "Kelly size too small or zero, skipping"
                    );
                    continue;
                };

                // Record signal to database (paper or live)
                let side = if should_buy_yes { "yes" } else { "no" };
                let direction = match movement.direction {
                    PriceDirection::Up => "up",
                    PriceDirection::Down => "down",
                };

                if let Err(e) = record_latency_arb_signal(
                    &pool,
                    movement.symbol.as_str(),
                    direction,
                    movement.magnitude_pct,
                    &market.condition_id,
                    side,
                    market.yes_price,
                    position_size,
                    age_ms as i32,
                )
                .await
                {
                    warn!(error = %e, "Failed to record latency arb signal");
                }

                info!(
                    symbol = movement.symbol.as_str(),
                    direction,
                    magnitude_pct = format!("{:.4}", movement.magnitude_pct),
                    condition_id = %market.condition_id,
                    side,
                    yes_price = format!("{:.4}", market.yes_price),
                    edge = format!("{:.4}", edge),
                    position_size = %position_size,
                    age_ms,
                    "Latency arb signal detected"
                );

                // Update cooldown
                cooldowns.insert(market.condition_id.clone(), time::Instant::now());
                signals_executed += 1;

                // TODO: Execute FOK order via OrderExecutor when live trading is enabled.
                // For now, signals are logged to the database for paper-mode validation.
                // The execution path will be wired once paper-mode results confirm
                // positive expected value over 50+ trades.
            }

            // Evict expired cooldowns every 100 signals to bound memory
            if signals_evaluated > 0 && signals_evaluated % 100 == 0 {
                let before = cooldowns.len();
                cooldowns.retain(|_, ts| ts.elapsed() < cooldown_duration);
                let evicted = before - cooldowns.len();
                info!(
                    evaluated = signals_evaluated,
                    executed = signals_executed,
                    cooldowns_active = cooldowns.len(),
                    cooldowns_evicted = evicted,
                    "Latency arb signal stats"
                );
            }
        }

        warn!("Latency arb executor: price channel closed, shutting down");
    })
}

/// Compute the edge for a given movement + market combination.
///
/// Returns (should_buy_yes, p_win, edge).
fn compute_edge(movement: &PriceMovement, market: &MappedMarket) -> (bool, f64, f64) {
    // If CEX shows BTC going UP and the market is "BTC above $X":
    //   - If BTC is now above threshold → YES should be ~1.0 → buy YES if underpriced
    //   - If BTC is below threshold but moving toward it → YES should increase
    let cex_above_threshold = movement.price_now > market.threshold_price;

    let (should_buy_yes, implied_prob) = if market.is_above {
        // "BTC above $X" market
        match movement.direction {
            PriceDirection::Up => {
                if cex_above_threshold {
                    // CEX confirms above threshold, YES should be high
                    (true, 0.85_f64.min(0.50 + movement.magnitude_pct * 50.0))
                } else {
                    // Moving up but not yet above — moderate confidence
                    (true, 0.60_f64.min(0.50 + movement.magnitude_pct * 20.0))
                }
            }
            PriceDirection::Down => {
                if !cex_above_threshold {
                    // CEX confirms below threshold, NO should be high
                    (false, 0.85_f64.min(0.50 + movement.magnitude_pct * 50.0))
                } else {
                    // Moving down but still above — moderate confidence for NO
                    (false, 0.60_f64.min(0.50 + movement.magnitude_pct * 20.0))
                }
            }
        }
    } else {
        // "BTC below $X" market — logic is inverted
        match movement.direction {
            PriceDirection::Down => {
                if !cex_above_threshold {
                    (true, 0.85_f64.min(0.50 + movement.magnitude_pct * 50.0))
                } else {
                    (true, 0.60_f64.min(0.50 + movement.magnitude_pct * 20.0))
                }
            }
            PriceDirection::Up => {
                if cex_above_threshold {
                    (false, 0.85_f64.min(0.50 + movement.magnitude_pct * 50.0))
                } else {
                    (false, 0.60_f64.min(0.50 + movement.magnitude_pct * 20.0))
                }
            }
        }
    };

    // Edge = how much Polymarket misprices relative to our implied probability
    let poly_price = if should_buy_yes {
        market.yes_price
    } else {
        1.0 - market.yes_price
    };

    let edge = implied_prob - poly_price;

    (should_buy_yes, implied_prob, edge)
}

/// Record a latency arb signal to the database for tracking and paper-mode analysis.
async fn record_latency_arb_signal(
    pool: &PgPool,
    cex_symbol: &str,
    direction: &str,
    magnitude_pct: f64,
    condition_id: &str,
    polymarket_side: &str,
    yes_price_at_signal: f64,
    kelly_size_usd: Decimal,
    signal_age_ms: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO latency_arb_signals (
            id, cex_symbol, direction, magnitude_pct, condition_id,
            polymarket_side, yes_price_at_signal, kelly_size_usd,
            executed, signal_age_ms, generated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, $9, NOW())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(cex_symbol)
    .bind(direction)
    .bind(magnitude_pct)
    .bind(condition_id)
    .bind(polymarket_side)
    .bind(Decimal::from_f64_retain(yes_price_at_signal))
    .bind(kelly_size_usd)
    .bind(signal_age_ms)
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_kelly_size() {
        let config = LatencyArbExecutorConfig {
            enabled: true,
            min_edge: 0.10,
            max_signal_age_ms: 1500,
            kelly_fraction: 0.25,
            kelly_bankroll: Decimal::new(1000, 0),
            max_position_size: Decimal::new(200, 0),
            min_position_size: Decimal::new(5, 0),
            min_yes_price: 0.05,
            max_yes_price: 0.95,
            cooldown_ms: 30000,
        };

        // p=0.85, price=0.55 → f=0.667 → size = 0.25 * 0.667 * 1000 = $167
        let size = compute_kelly_size(0.85, 0.55, &config);
        assert!(size.is_some());
        let s = size.unwrap();
        assert!(s > Decimal::new(100, 0) && s < Decimal::new(200, 0));
    }

    #[test]
    fn test_compute_kelly_size_too_small() {
        let config = LatencyArbExecutorConfig {
            enabled: true,
            min_edge: 0.10,
            max_signal_age_ms: 1500,
            kelly_fraction: 0.01,                 // Very conservative
            kelly_bankroll: Decimal::new(100, 0), // Small bankroll
            max_position_size: Decimal::new(200, 0),
            min_position_size: Decimal::new(5, 0),
            min_yes_price: 0.05,
            max_yes_price: 0.95,
            cooldown_ms: 30000,
        };

        // Very small Kelly → below min_position_size → None
        let size = compute_kelly_size(0.55, 0.50, &config);
        assert!(size.is_none());
    }
}
