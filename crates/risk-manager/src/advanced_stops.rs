//! Advanced stop-loss strategies.
//!
//! Enhanced stop-loss mechanisms including compound conditions,
//! volatility-based stops, and intelligent exit strategies.

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tracing::debug;
use uuid::Uuid;

/// Advanced stop-loss configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedStopConfig {
    /// Enable break-even stop after reaching target profit.
    pub break_even_enabled: bool,
    /// Profit percentage to trigger break-even.
    pub break_even_trigger_pct: Decimal,
    /// Buffer above entry for break-even stop.
    pub break_even_buffer_pct: Decimal,

    /// Enable step trailing stop.
    pub step_trailing_enabled: bool,
    /// Step size for trailing (e.g., 0.05 = 5 cents).
    pub step_size: Decimal,
    /// Trailing offset per step.
    pub step_offset_pct: Decimal,

    /// Enable volatility-based stop adjustment.
    pub volatility_adjusted: bool,
    /// ATR multiplier for volatility stop.
    pub atr_multiplier: Decimal,
    /// Lookback period for ATR calculation.
    pub atr_period: usize,

    /// Enable time decay (tighter stops as deadline approaches).
    pub time_decay_enabled: bool,
    /// Hours before deadline to start tightening.
    pub time_decay_start_hours: i64,
    /// Final stop tightness multiplier.
    pub time_decay_final_multiplier: Decimal,
}

impl Default for AdvancedStopConfig {
    fn default() -> Self {
        Self {
            break_even_enabled: true,
            break_even_trigger_pct: Decimal::new(5, 2), // 5% profit
            break_even_buffer_pct: Decimal::new(1, 3),  // 0.1% buffer

            step_trailing_enabled: false,
            step_size: Decimal::new(5, 2),       // $0.05 steps
            step_offset_pct: Decimal::new(3, 2), // 3% offset

            volatility_adjusted: true,
            atr_multiplier: Decimal::new(2, 0), // 2x ATR
            atr_period: 14,

            time_decay_enabled: true,
            time_decay_start_hours: 24,
            time_decay_final_multiplier: Decimal::new(5, 1), // 0.5x (tighter)
        }
    }
}

/// Compound stop condition combining multiple triggers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompoundStop {
    pub id: Uuid,
    pub position_id: Uuid,
    pub conditions: Vec<StopCondition>,
    /// How to combine conditions (AND = all must trigger, OR = any triggers).
    pub logic: CompoundLogic,
    pub activated: bool,
    pub triggered: bool,
    pub created_at: DateTime<Utc>,
}

/// Logic for combining stop conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompoundLogic {
    /// All conditions must be true.
    And,
    /// Any condition being true triggers.
    Or,
    /// First N conditions must be true.
    AtLeast(usize),
}

/// Individual stop condition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StopCondition {
    /// Price falls below threshold.
    PriceBelow { price: Decimal },
    /// Price falls by percentage from peak.
    PercentageFromPeak { peak: Decimal, pct: Decimal },
    /// Loss exceeds amount.
    LossExceeds { amount: Decimal },
    /// Time deadline reached.
    TimeReached { deadline: DateTime<Utc> },
    /// Volatility exceeds threshold.
    VolatilityExceeds { threshold: Decimal },
    /// Volume drops below threshold.
    VolumeBelowAvg {
        threshold_pct: Decimal,
        avg_volume: Decimal,
    },
    /// Market hours condition.
    OutsideMarketHours { allowed_hours: Vec<(u8, u8)> },
    /// Consecutive losing candles.
    ConsecutiveDownCandles { count: usize, current: usize },
    /// Break of support level.
    SupportBroken { level: Decimal },
}

impl CompoundStop {
    /// Create a new compound stop.
    pub fn new(position_id: Uuid, conditions: Vec<StopCondition>, logic: CompoundLogic) -> Self {
        Self {
            id: Uuid::new_v4(),
            position_id,
            conditions,
            logic,
            activated: false,
            triggered: false,
            created_at: Utc::now(),
        }
    }

    /// Activate the compound stop.
    pub fn activate(&mut self) {
        self.activated = true;
    }

    /// Check if the compound stop is triggered.
    pub fn check(&self, context: &StopContext) -> bool {
        if !self.activated || self.triggered {
            return false;
        }

        let results: Vec<bool> = self
            .conditions
            .iter()
            .map(|c| c.evaluate(context))
            .collect();

        match self.logic {
            CompoundLogic::And => results.iter().all(|&r| r),
            CompoundLogic::Or => results.iter().any(|&r| r),
            CompoundLogic::AtLeast(n) => results.iter().filter(|&&r| r).count() >= n,
        }
    }
}

impl StopCondition {
    /// Evaluate if this condition is triggered.
    pub fn evaluate(&self, ctx: &StopContext) -> bool {
        match self {
            StopCondition::PriceBelow { price } => ctx.current_price <= *price,
            StopCondition::PercentageFromPeak { peak, pct } => {
                let trigger = *peak * (Decimal::ONE - *pct);
                ctx.current_price <= trigger
            }
            StopCondition::LossExceeds { amount } => ctx.unrealized_pnl <= -*amount,
            StopCondition::TimeReached { deadline } => Utc::now() >= *deadline,
            StopCondition::VolatilityExceeds { threshold } => ctx
                .current_volatility
                .map(|v| v >= *threshold)
                .unwrap_or(false),
            StopCondition::VolumeBelowAvg {
                threshold_pct,
                avg_volume,
            } => ctx
                .current_volume
                .map(|v| v < *avg_volume * *threshold_pct)
                .unwrap_or(false),
            StopCondition::OutsideMarketHours { allowed_hours } => {
                let hour = Utc::now().time().hour() as u8;
                !allowed_hours
                    .iter()
                    .any(|(start, end)| hour >= *start && hour < *end)
            }
            StopCondition::ConsecutiveDownCandles { count, current } => current >= count,
            StopCondition::SupportBroken { level } => ctx.current_price < *level,
        }
    }
}

/// Context for evaluating stop conditions.
#[derive(Debug, Clone)]
pub struct StopContext {
    pub current_price: Decimal,
    pub entry_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub current_volatility: Option<Decimal>,
    pub current_volume: Option<Decimal>,
    pub position_age_hours: i64,
}

/// Volatility-based stop calculator.
#[derive(Debug, Clone)]
pub struct VolatilityStop {
    /// Price history for ATR calculation.
    price_history: VecDeque<PriceBar>,
    /// ATR period.
    period: usize,
    /// ATR multiplier.
    multiplier: Decimal,
    /// Current ATR value.
    current_atr: Option<Decimal>,
}

/// Price bar for volatility calculation.
#[derive(Debug, Clone)]
pub struct PriceBar {
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub timestamp: DateTime<Utc>,
}

impl VolatilityStop {
    /// Create a new volatility stop calculator.
    pub fn new(period: usize, multiplier: Decimal) -> Self {
        Self {
            price_history: VecDeque::with_capacity(period + 1),
            period,
            multiplier,
            current_atr: None,
        }
    }

    /// Add a new price bar.
    pub fn add_bar(&mut self, bar: PriceBar) {
        self.price_history.push_back(bar);
        if self.price_history.len() > self.period + 1 {
            self.price_history.pop_front();
        }
        self.calculate_atr();
    }

    /// Calculate Average True Range.
    fn calculate_atr(&mut self) {
        if self.price_history.len() < 2 {
            return;
        }

        let mut true_ranges: Vec<Decimal> = Vec::new();
        let bars: Vec<_> = self.price_history.iter().collect();

        for i in 1..bars.len() {
            let current = bars[i];
            let prev = bars[i - 1];

            // True Range = max(H-L, |H-Pc|, |L-Pc|)
            let hl = current.high - current.low;
            let hpc = (current.high - prev.close).abs();
            let lpc = (current.low - prev.close).abs();

            let tr = hl.max(hpc).max(lpc);
            true_ranges.push(tr);
        }

        if true_ranges.len() >= self.period {
            let sum: Decimal = true_ranges.iter().rev().take(self.period).sum();
            self.current_atr = Some(sum / Decimal::from(self.period));
        }
    }

    /// Get the current volatility-based stop level.
    pub fn get_stop_level(&self, entry_price: Decimal) -> Option<Decimal> {
        self.current_atr
            .map(|atr| entry_price - (atr * self.multiplier))
    }

    /// Get current ATR value.
    pub fn current_atr(&self) -> Option<Decimal> {
        self.current_atr
    }
}

/// Step trailing stop implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTrailingStop {
    pub id: Uuid,
    pub position_id: Uuid,
    pub entry_price: Decimal,
    pub step_size: Decimal,
    pub offset_pct: Decimal,
    pub current_step: u32,
    pub highest_step: u32,
    pub stop_price: Decimal,
    pub activated: bool,
}

impl StepTrailingStop {
    /// Create a new step trailing stop.
    pub fn new(
        position_id: Uuid,
        entry_price: Decimal,
        step_size: Decimal,
        offset_pct: Decimal,
    ) -> Self {
        let initial_stop = entry_price * (Decimal::ONE - offset_pct);
        Self {
            id: Uuid::new_v4(),
            position_id,
            entry_price,
            step_size,
            offset_pct,
            current_step: 0,
            highest_step: 0,
            stop_price: initial_stop,
            activated: false,
        }
    }

    /// Update with new price and return if stop triggered.
    pub fn update(&mut self, current_price: Decimal) -> bool {
        if !self.activated {
            return false;
        }

        // Calculate current step level
        let price_move = current_price - self.entry_price;
        if price_move > Decimal::ZERO {
            let steps = (price_move / self.step_size).floor();
            let new_step = steps.to_string().parse::<u32>().unwrap_or(0);

            if new_step > self.highest_step {
                self.highest_step = new_step;
                // Move stop up by one step
                let step_up = self.entry_price + (self.step_size * Decimal::from(new_step - 1));
                self.stop_price = step_up.max(self.stop_price);

                debug!(
                    stop_id = %self.id,
                    new_step = new_step,
                    new_stop = %self.stop_price,
                    "Step trailing stop moved up"
                );
            }
        }

        self.current_step = self.highest_step;

        // Check if triggered
        current_price <= self.stop_price
    }

    /// Activate the stop.
    pub fn activate(&mut self) {
        self.activated = true;
    }
}

/// Break-even stop implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakEvenStop {
    pub id: Uuid,
    pub position_id: Uuid,
    pub entry_price: Decimal,
    pub trigger_profit_pct: Decimal,
    pub buffer_pct: Decimal,
    pub triggered_to_break_even: bool,
    pub stop_price: Option<Decimal>,
    pub activated: bool,
}

impl BreakEvenStop {
    /// Create a new break-even stop.
    pub fn new(
        position_id: Uuid,
        entry_price: Decimal,
        trigger_profit_pct: Decimal,
        buffer_pct: Decimal,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            position_id,
            entry_price,
            trigger_profit_pct,
            buffer_pct,
            triggered_to_break_even: false,
            stop_price: None,
            activated: false,
        }
    }

    /// Update with new price.
    pub fn update(&mut self, current_price: Decimal) -> bool {
        if !self.activated {
            return false;
        }

        // Check if we should move to break-even
        if !self.triggered_to_break_even {
            let profit_pct = (current_price - self.entry_price) / self.entry_price;
            if profit_pct >= self.trigger_profit_pct {
                // Move stop to break-even + buffer
                self.stop_price = Some(self.entry_price * (Decimal::ONE + self.buffer_pct));
                self.triggered_to_break_even = true;

                debug!(
                    stop_id = %self.id,
                    stop_price = ?self.stop_price,
                    "Break-even stop activated"
                );
            }
        }

        // Check if triggered
        if let Some(stop) = self.stop_price {
            return current_price <= stop;
        }

        false
    }

    /// Activate the stop.
    pub fn activate(&mut self) {
        self.activated = true;
    }
}

/// Time-decay stop that tightens as deadline approaches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeDecayStop {
    pub id: Uuid,
    pub position_id: Uuid,
    pub base_stop_pct: Decimal,
    pub deadline: DateTime<Utc>,
    pub decay_start_hours: i64,
    pub final_multiplier: Decimal,
    pub entry_price: Decimal,
    pub activated: bool,
}

impl TimeDecayStop {
    /// Create a new time-decay stop.
    pub fn new(
        position_id: Uuid,
        entry_price: Decimal,
        base_stop_pct: Decimal,
        deadline: DateTime<Utc>,
        decay_start_hours: i64,
        final_multiplier: Decimal,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            position_id,
            base_stop_pct,
            deadline,
            decay_start_hours,
            final_multiplier,
            entry_price,
            activated: false,
        }
    }

    /// Get current stop level based on time decay.
    pub fn current_stop_level(&self) -> Decimal {
        let now = Utc::now();
        let hours_to_deadline = self.deadline.signed_duration_since(now).num_hours();

        let decay_factor = if hours_to_deadline >= self.decay_start_hours {
            Decimal::ONE
        } else if hours_to_deadline <= 0 {
            self.final_multiplier
        } else {
            // Linear decay from 1.0 to final_multiplier
            let progress = Decimal::from(self.decay_start_hours - hours_to_deadline)
                / Decimal::from(self.decay_start_hours);
            Decimal::ONE - (progress * (Decimal::ONE - self.final_multiplier))
        };

        let adjusted_stop_pct = self.base_stop_pct * decay_factor;
        self.entry_price * (Decimal::ONE - adjusted_stop_pct)
    }

    /// Check if triggered.
    pub fn is_triggered(&self, current_price: Decimal) -> bool {
        if !self.activated {
            return false;
        }
        current_price <= self.current_stop_level()
    }

    /// Activate the stop.
    pub fn activate(&mut self) {
        self.activated = true;
    }
}

/// Market session-aware stop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStop {
    pub id: Uuid,
    pub position_id: Uuid,
    /// Close position at session end.
    pub close_at_session_end: bool,
    /// Session end hour (UTC).
    pub session_end_hour: u8,
    /// Days to trade (empty = all days).
    pub trading_days: Vec<Weekday>,
    /// Tighten stops outside prime hours.
    pub tighten_outside_prime: bool,
    /// Prime hours (UTC).
    pub prime_hours: (u8, u8),
    /// Tightening multiplier.
    pub outside_prime_multiplier: Decimal,
    pub activated: bool,
}

impl SessionStop {
    /// Create a new session stop.
    pub fn new(position_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            position_id,
            close_at_session_end: false,
            session_end_hour: 21, // 9 PM UTC
            trading_days: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            tighten_outside_prime: true,
            prime_hours: (13, 21), // 1 PM - 9 PM UTC (US market hours)
            outside_prime_multiplier: Decimal::new(5, 1), // 0.5x (tighter)
            activated: false,
        }
    }

    /// Check if should close at session end.
    pub fn should_close_session_end(&self) -> bool {
        if !self.activated || !self.close_at_session_end {
            return false;
        }

        let now = Utc::now();
        let hour = now.time().hour() as u8;
        let weekday = now.weekday();

        // Check if it's a trading day
        if !self.trading_days.is_empty() && !self.trading_days.contains(&weekday) {
            return true; // Close on non-trading days
        }

        // Check if session ended
        hour >= self.session_end_hour
    }

    /// Get stop multiplier based on current session.
    pub fn get_stop_multiplier(&self) -> Decimal {
        if !self.activated || !self.tighten_outside_prime {
            return Decimal::ONE;
        }

        let hour = Utc::now().time().hour() as u8;
        let (start, end) = self.prime_hours;

        if hour >= start && hour < end {
            Decimal::ONE
        } else {
            self.outside_prime_multiplier
        }
    }

    /// Activate the stop.
    pub fn activate(&mut self) {
        self.activated = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compound_stop_and_logic() {
        let conditions = vec![
            StopCondition::PriceBelow {
                price: Decimal::new(45, 2),
            },
            StopCondition::LossExceeds {
                amount: Decimal::new(100, 0),
            },
        ];

        let mut stop = CompoundStop::new(Uuid::new_v4(), conditions, CompoundLogic::And);
        stop.activate();

        let ctx = StopContext {
            current_price: Decimal::new(44, 2),
            entry_price: Decimal::new(50, 2),
            unrealized_pnl: Decimal::new(-150, 0),
            current_volatility: None,
            current_volume: None,
            position_age_hours: 24,
        };

        // Both conditions met
        assert!(stop.check(&ctx));

        // Only one condition met
        let ctx2 = StopContext {
            current_price: Decimal::new(44, 2),
            unrealized_pnl: Decimal::new(-50, 0), // Loss not exceeded
            ..ctx
        };
        assert!(!stop.check(&ctx2));
    }

    #[test]
    fn test_compound_stop_or_logic() {
        let conditions = vec![
            StopCondition::PriceBelow {
                price: Decimal::new(45, 2),
            },
            StopCondition::LossExceeds {
                amount: Decimal::new(100, 0),
            },
        ];

        let mut stop = CompoundStop::new(Uuid::new_v4(), conditions, CompoundLogic::Or);
        stop.activate();

        let ctx = StopContext {
            current_price: Decimal::new(44, 2),
            entry_price: Decimal::new(50, 2),
            unrealized_pnl: Decimal::new(-50, 0), // Loss not exceeded
            current_volatility: None,
            current_volume: None,
            position_age_hours: 24,
        };

        // Only price condition met, but OR logic
        assert!(stop.check(&ctx));
    }

    #[test]
    fn test_volatility_stop_atr() {
        let mut vol_stop = VolatilityStop::new(3, Decimal::new(2, 0));

        // Add price bars
        vol_stop.add_bar(PriceBar {
            high: Decimal::new(52, 2),
            low: Decimal::new(48, 2),
            close: Decimal::new(50, 2),
            timestamp: Utc::now(),
        });
        vol_stop.add_bar(PriceBar {
            high: Decimal::new(53, 2),
            low: Decimal::new(49, 2),
            close: Decimal::new(51, 2),
            timestamp: Utc::now(),
        });
        vol_stop.add_bar(PriceBar {
            high: Decimal::new(54, 2),
            low: Decimal::new(50, 2),
            close: Decimal::new(52, 2),
            timestamp: Utc::now(),
        });
        vol_stop.add_bar(PriceBar {
            high: Decimal::new(55, 2),
            low: Decimal::new(51, 2),
            close: Decimal::new(53, 2),
            timestamp: Utc::now(),
        });

        assert!(vol_stop.current_atr().is_some());
        let stop_level = vol_stop.get_stop_level(Decimal::new(50, 2));
        assert!(stop_level.is_some());
    }

    #[test]
    fn test_step_trailing_stop() {
        let mut stop = StepTrailingStop::new(
            Uuid::new_v4(),
            Decimal::new(50, 2), // Entry at 0.50
            Decimal::new(5, 2),  // 0.05 steps
            Decimal::new(3, 2),  // 3% offset
        );
        stop.activate();

        // Price moves up to 0.55 (one step)
        assert!(!stop.update(Decimal::new(55, 2)));
        assert_eq!(stop.highest_step, 1);

        // Price moves up to 0.60 (two steps)
        assert!(!stop.update(Decimal::new(60, 2)));
        assert_eq!(stop.highest_step, 2);

        // Price falls to stop level
        assert!(stop.update(Decimal::new(50, 2)));
    }

    #[test]
    fn test_break_even_stop() {
        let mut stop = BreakEvenStop::new(
            Uuid::new_v4(),
            Decimal::new(50, 2), // Entry at 0.50
            Decimal::new(5, 2),  // 5% profit trigger
            Decimal::new(1, 3),  // 0.1% buffer
        );
        stop.activate();

        // Price at entry - no break-even yet
        assert!(!stop.update(Decimal::new(50, 2)));
        assert!(!stop.triggered_to_break_even);

        // Price up 5% - should trigger break-even
        assert!(!stop.update(Decimal::new(525, 3))); // 0.525
        assert!(stop.triggered_to_break_even);
        assert!(stop.stop_price.is_some());

        // Price falls to stop
        assert!(stop.update(Decimal::new(50, 2)));
    }

    #[test]
    fn test_time_decay_stop() {
        let deadline = Utc::now() + chrono::Duration::hours(12);
        let stop = TimeDecayStop::new(
            Uuid::new_v4(),
            Decimal::new(50, 2), // Entry at 0.50
            Decimal::new(10, 2), // 10% base stop
            deadline,
            24,                 // Start decay 24h before
            Decimal::new(5, 1), // Final 0.5x multiplier
        );

        // Since we're 12 hours out and decay starts at 24h, we should have some decay
        let stop_level = stop.current_stop_level();
        // Base stop would be 0.45 (0.50 - 10%)
        // With 50% progress toward deadline, stop should be tighter
        assert!(stop_level > Decimal::new(45, 2));
    }

    #[test]
    fn test_session_stop_multiplier() {
        let mut stop = SessionStop::new(Uuid::new_v4());
        stop.activated = true;

        let multiplier = stop.get_stop_multiplier();
        // Depending on current time, either 1.0 or 0.5
        assert!(multiplier == Decimal::ONE || multiplier == Decimal::new(5, 1));
    }
}
