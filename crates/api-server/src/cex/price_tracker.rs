//! EMA-based price tracking and divergence detection for CEX feeds.

use std::time;

/// Identifier for a CEX trading pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CexSymbol {
    BtcUsdt,
    EthUsdt,
}

impl CexSymbol {
    pub fn as_str(&self) -> &'static str {
        match self {
            CexSymbol::BtcUsdt => "BTCUSDT",
            CexSymbol::EthUsdt => "ETHUSDT",
        }
    }
}

/// A single price tick from a CEX feed.
#[derive(Debug, Clone)]
pub struct CexPriceTick {
    pub symbol: CexSymbol,
    pub price: f64,
    pub received_at: time::Instant,
}

/// Direction of a detected price movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceDirection {
    Up,
    Down,
}

/// A significant price movement detected by the tracker.
#[derive(Debug, Clone)]
pub struct PriceMovement {
    pub symbol: CexSymbol,
    pub direction: PriceDirection,
    /// Magnitude as a fraction (e.g. 0.005 = 0.5% move).
    pub magnitude_pct: f64,
    /// The EMA before the move (reference price).
    pub ema_before: f64,
    /// Current price that triggered the detection.
    pub price_now: f64,
    /// When the movement was detected (for signal age tracking).
    pub detected_at: time::Instant,
}

/// EMA state for a single symbol.
#[derive(Debug)]
struct EmaState {
    value: f64,
    initialized: bool,
    tick_count: u64,
}

impl EmaState {
    fn new() -> Self {
        Self {
            value: 0.0,
            initialized: false,
            tick_count: 0,
        }
    }

    fn update(&mut self, price: f64, alpha: f64) {
        if !self.initialized {
            self.value = price;
            self.initialized = true;
        } else {
            self.value = alpha * price + (1.0 - alpha) * self.value;
        }
        self.tick_count += 1;
    }
}

/// Configuration for the price tracker.
#[derive(Debug, Clone)]
pub struct PriceTrackerConfig {
    /// EMA smoothing factor (higher = more reactive). Default 0.3.
    pub ema_alpha: f64,
    /// Minimum price divergence from EMA to trigger a signal. Default 0.003 (0.3%).
    pub divergence_threshold: f64,
    /// Minimum ticks before the EMA is considered stable. Default 10.
    pub min_warmup_ticks: u64,
}

impl PriceTrackerConfig {
    pub fn from_env() -> Self {
        Self {
            ema_alpha: std::env::var("LATENCY_ARB_EMA_ALPHA")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.3),
            divergence_threshold: std::env::var("LATENCY_ARB_DIVERGENCE_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.003),
            min_warmup_ticks: std::env::var("LATENCY_ARB_WARMUP_TICKS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
        }
    }
}

/// Tracks EMA per symbol and emits `PriceMovement` on significant divergence.
pub struct PriceTracker {
    config: PriceTrackerConfig,
    btc: EmaState,
    eth: EmaState,
}

impl PriceTracker {
    pub fn new(config: PriceTrackerConfig) -> Self {
        Self {
            config,
            btc: EmaState::new(),
            eth: EmaState::new(),
        }
    }

    /// Process a tick and return a movement if divergence exceeds the threshold.
    pub fn on_tick(&mut self, tick: &CexPriceTick) -> Option<PriceMovement> {
        let state = match tick.symbol {
            CexSymbol::BtcUsdt => &mut self.btc,
            CexSymbol::EthUsdt => &mut self.eth,
        };

        let ema_before = state.value;
        state.update(tick.price, self.config.ema_alpha);

        // Don't emit during warmup period
        if state.tick_count < self.config.min_warmup_ticks {
            return None;
        }

        // Check divergence against the EMA *before* this tick's update
        if ema_before <= 0.0 {
            return None;
        }

        let divergence = (tick.price - ema_before) / ema_before;
        let magnitude = divergence.abs();

        if magnitude >= self.config.divergence_threshold {
            let direction = if divergence > 0.0 {
                PriceDirection::Up
            } else {
                PriceDirection::Down
            };

            Some(PriceMovement {
                symbol: tick.symbol,
                direction,
                magnitude_pct: magnitude,
                ema_before,
                price_now: tick.price,
                detected_at: tick.received_at,
            })
        } else {
            None
        }
    }

    /// Get current EMA value for a symbol.
    pub fn ema(&self, symbol: CexSymbol) -> f64 {
        match symbol {
            CexSymbol::BtcUsdt => self.btc.value,
            CexSymbol::EthUsdt => self.eth.value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tick(symbol: CexSymbol, price: f64) -> CexPriceTick {
        CexPriceTick {
            symbol,
            price,
            received_at: time::Instant::now(),
        }
    }

    #[test]
    fn test_ema_warmup() {
        let config = PriceTrackerConfig {
            ema_alpha: 0.3,
            divergence_threshold: 0.003,
            min_warmup_ticks: 5,
        };
        let mut tracker = PriceTracker::new(config);

        // First 5 ticks at stable price — should not emit during warmup
        for _ in 0..5 {
            assert!(tracker
                .on_tick(&make_tick(CexSymbol::BtcUsdt, 70000.0))
                .is_none());
        }
    }

    #[test]
    fn test_detects_significant_move() {
        let config = PriceTrackerConfig {
            ema_alpha: 0.3,
            divergence_threshold: 0.003, // 0.3%
            min_warmup_ticks: 3,
        };
        let mut tracker = PriceTracker::new(config);

        // Warmup at $70,000
        for _ in 0..5 {
            tracker.on_tick(&make_tick(CexSymbol::BtcUsdt, 70000.0));
        }

        // Big upward move: $70,000 → $70,500 (0.71%) should trigger
        let movement = tracker.on_tick(&make_tick(CexSymbol::BtcUsdt, 70500.0));
        assert!(movement.is_some());
        let m = movement.unwrap();
        assert_eq!(m.direction, PriceDirection::Up);
        assert!(m.magnitude_pct > 0.003);
    }

    #[test]
    fn test_no_signal_on_small_move() {
        let config = PriceTrackerConfig {
            ema_alpha: 0.3,
            divergence_threshold: 0.003,
            min_warmup_ticks: 3,
        };
        let mut tracker = PriceTracker::new(config);

        // Warmup at $70,000
        for _ in 0..5 {
            tracker.on_tick(&make_tick(CexSymbol::BtcUsdt, 70000.0));
        }

        // Small move: $70,000 → $70,050 (0.07%) should NOT trigger
        let movement = tracker.on_tick(&make_tick(CexSymbol::BtcUsdt, 70050.0));
        assert!(movement.is_none());
    }

    #[test]
    fn test_independent_symbols() {
        let config = PriceTrackerConfig {
            ema_alpha: 0.3,
            divergence_threshold: 0.003,
            min_warmup_ticks: 3,
        };
        let mut tracker = PriceTracker::new(config);

        // Warmup BTC
        for _ in 0..5 {
            tracker.on_tick(&make_tick(CexSymbol::BtcUsdt, 70000.0));
        }

        // ETH is still warming up
        tracker.on_tick(&make_tick(CexSymbol::EthUsdt, 3500.0));
        assert!(tracker
            .on_tick(&make_tick(CexSymbol::EthUsdt, 3600.0))
            .is_none()); // still warming

        // BTC big move should fire independently
        let m = tracker.on_tick(&make_tick(CexSymbol::BtcUsdt, 70500.0));
        assert!(m.is_some());
    }
}
