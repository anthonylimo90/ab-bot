//! Circuit breaker for emergency trading halts.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Reason for circuit breaker activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripReason {
    /// Daily loss limit exceeded.
    DailyLossLimit,
    /// Maximum drawdown exceeded.
    MaxDrawdown,
    /// Too many consecutive losses.
    ConsecutiveLosses,
    /// Manual activation.
    Manual,
    /// API/connectivity issues.
    Connectivity,
    /// Unusual market conditions detected.
    MarketConditions,
}

/// Configuration for circuit breaker thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum daily loss before halt (absolute value).
    pub max_daily_loss: Decimal,
    /// Maximum drawdown from peak (percentage, e.g., 0.10 = 10%).
    pub max_drawdown_pct: Decimal,
    /// Number of consecutive losses before halt.
    pub max_consecutive_losses: u32,
    /// Cooldown period after trip (in minutes).
    pub cooldown_minutes: i64,
    /// Whether circuit breaker is enabled.
    pub enabled: bool,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_daily_loss: Decimal::new(1000, 0),    // $1000 daily loss limit
            max_drawdown_pct: Decimal::new(10, 2),    // 10% max drawdown
            max_consecutive_losses: 5,                 // 5 consecutive losses
            cooldown_minutes: 60,                      // 1 hour cooldown
            enabled: true,
        }
    }
}

/// Current state of the circuit breaker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerState {
    /// Whether trading is currently halted.
    pub tripped: bool,
    /// Reason for current trip (if tripped).
    pub trip_reason: Option<TripReason>,
    /// When the circuit breaker was tripped.
    pub tripped_at: Option<DateTime<Utc>>,
    /// When trading can resume (if tripped).
    pub resume_at: Option<DateTime<Utc>>,
    /// Today's realized P&L.
    pub daily_pnl: Decimal,
    /// Peak portfolio value (for drawdown calculation).
    pub peak_value: Decimal,
    /// Current portfolio value.
    pub current_value: Decimal,
    /// Count of consecutive losing trades.
    pub consecutive_losses: u32,
    /// Total trips today.
    pub trips_today: u32,
}

impl Default for CircuitBreakerState {
    fn default() -> Self {
        Self {
            tripped: false,
            trip_reason: None,
            tripped_at: None,
            resume_at: None,
            daily_pnl: Decimal::ZERO,
            peak_value: Decimal::ZERO,
            current_value: Decimal::ZERO,
            consecutive_losses: 0,
            trips_today: 0,
        }
    }
}

/// Circuit breaker for emergency trading halts.
pub struct CircuitBreaker {
    config: Arc<RwLock<CircuitBreakerConfig>>,
    state: Arc<RwLock<CircuitBreakerState>>,
    /// Fast path flag for checking if tripped.
    is_tripped: AtomicBool,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            state: Arc::new(RwLock::new(CircuitBreakerState::default())),
            is_tripped: AtomicBool::new(false),
        }
    }

    /// Check if trading is currently halted (fast path).
    pub fn is_tripped(&self) -> bool {
        self.is_tripped.load(Ordering::SeqCst)
    }

    /// Check if trading is allowed.
    pub async fn can_trade(&self) -> bool {
        let config = self.config.read().await;
        if !config.enabled {
            return true;
        }
        drop(config);

        if !self.is_tripped.load(Ordering::SeqCst) {
            return true;
        }

        // Check if cooldown has expired
        let state = self.state.read().await;
        if let Some(resume_at) = state.resume_at {
            if Utc::now() >= resume_at {
                drop(state);
                self.reset().await;
                return true;
            }
        }

        false
    }

    /// Record a trade result and check thresholds.
    pub async fn record_trade(&self, pnl: Decimal, is_win: bool) -> Result<Option<TripReason>> {
        let config = self.config.read().await;
        if !config.enabled {
            return Ok(None);
        }

        let mut state = self.state.write().await;

        // Update daily P&L
        state.daily_pnl += pnl;

        // Update consecutive losses
        if is_win {
            state.consecutive_losses = 0;
        } else {
            state.consecutive_losses += 1;
        }

        // Update peak/current value
        state.current_value += pnl;
        if state.current_value > state.peak_value {
            state.peak_value = state.current_value;
        }

        // Check thresholds
        let reason = self.check_thresholds(&config, &state);

        if let Some(ref reason) = reason {
            self.trip_internal(&mut state, reason.clone(), &config).await;
        }

        Ok(reason)
    }

    /// Update portfolio value (for drawdown calculation).
    pub async fn update_portfolio_value(&self, value: Decimal) -> Result<Option<TripReason>> {
        let config = self.config.read().await;
        if !config.enabled {
            return Ok(None);
        }

        let mut state = self.state.write().await;

        state.current_value = value;
        if value > state.peak_value {
            state.peak_value = value;
        }

        // Check drawdown
        if state.peak_value > Decimal::ZERO {
            let drawdown = (state.peak_value - state.current_value) / state.peak_value;
            if drawdown >= config.max_drawdown_pct {
                let reason = TripReason::MaxDrawdown;
                self.trip_internal(&mut state, reason.clone(), &config).await;
                return Ok(Some(reason));
            }
        }

        Ok(None)
    }

    /// Manually trip the circuit breaker.
    pub async fn manual_trip(&self, reason: Option<String>) {
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        warn!(reason = ?reason, "Manual circuit breaker trip");
        self.trip_internal(&mut state, TripReason::Manual, &config).await;
    }

    /// Trip due to connectivity issues.
    pub async fn trip_connectivity(&self, error: &str) {
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        error!(error = %error, "Circuit breaker tripped due to connectivity");
        self.trip_internal(&mut state, TripReason::Connectivity, &config).await;
    }

    /// Reset the circuit breaker.
    pub async fn reset(&self) {
        let mut state = self.state.write().await;

        state.tripped = false;
        state.trip_reason = None;
        state.tripped_at = None;
        state.resume_at = None;
        self.is_tripped.store(false, Ordering::SeqCst);

        info!("Circuit breaker reset");
    }

    /// Reset daily counters (call at start of trading day).
    pub async fn reset_daily(&self) {
        let mut state = self.state.write().await;

        state.daily_pnl = Decimal::ZERO;
        state.consecutive_losses = 0;
        state.trips_today = 0;

        info!("Circuit breaker daily reset");
    }

    /// Get current state.
    pub async fn state(&self) -> CircuitBreakerState {
        self.state.read().await.clone()
    }

    /// Update configuration.
    pub async fn update_config(&self, config: CircuitBreakerConfig) {
        let mut current = self.config.write().await;
        *current = config;
        info!("Circuit breaker config updated");
    }

    /// Get current configuration.
    pub async fn config(&self) -> CircuitBreakerConfig {
        self.config.read().await.clone()
    }

    // Private methods

    fn check_thresholds(
        &self,
        config: &CircuitBreakerConfig,
        state: &CircuitBreakerState,
    ) -> Option<TripReason> {
        // Check daily loss limit
        if state.daily_pnl < Decimal::ZERO && state.daily_pnl.abs() >= config.max_daily_loss {
            return Some(TripReason::DailyLossLimit);
        }

        // Check consecutive losses
        if state.consecutive_losses >= config.max_consecutive_losses {
            return Some(TripReason::ConsecutiveLosses);
        }

        // Check drawdown
        if state.peak_value > Decimal::ZERO {
            let drawdown = (state.peak_value - state.current_value) / state.peak_value;
            if drawdown >= config.max_drawdown_pct {
                return Some(TripReason::MaxDrawdown);
            }
        }

        None
    }

    async fn trip_internal(
        &self,
        state: &mut CircuitBreakerState,
        reason: TripReason,
        config: &CircuitBreakerConfig,
    ) {
        let now = Utc::now();
        let resume_at = now + Duration::minutes(config.cooldown_minutes);

        state.tripped = true;
        state.trip_reason = Some(reason.clone());
        state.tripped_at = Some(now);
        state.resume_at = Some(resume_at);
        state.trips_today += 1;

        self.is_tripped.store(true, Ordering::SeqCst);

        error!(
            reason = ?reason,
            resume_at = %resume_at,
            daily_pnl = %state.daily_pnl,
            consecutive_losses = %state.consecutive_losses,
            "Circuit breaker TRIPPED - trading halted"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_daily_loss_trip() {
        let config = CircuitBreakerConfig {
            max_daily_loss: Decimal::new(100, 0),
            max_consecutive_losses: 100, // High limit so consecutive losses don't trip first
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Record losses up to limit
        for _ in 0..9 {
            let reason = breaker.record_trade(Decimal::new(-10, 0), false).await.unwrap();
            assert!(reason.is_none());
        }

        // This should trip
        let reason = breaker.record_trade(Decimal::new(-10, 0), false).await.unwrap();
        assert_eq!(reason, Some(TripReason::DailyLossLimit));
        assert!(breaker.is_tripped());
    }

    #[tokio::test]
    async fn test_consecutive_losses_trip() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 3,
            max_daily_loss: Decimal::new(100000, 0), // High limit
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 2 losses - no trip
        breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();
        assert!(!breaker.is_tripped());

        // 3rd loss - trip
        let reason = breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();
        assert_eq!(reason, Some(TripReason::ConsecutiveLosses));
    }

    #[tokio::test]
    async fn test_win_resets_consecutive_losses() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 3,
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // 2 losses
        breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();

        // 1 win - resets counter
        breaker.record_trade(Decimal::new(1, 0), true).await.unwrap();

        // 2 more losses - should not trip yet
        breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0), false).await.unwrap();
        assert!(!breaker.is_tripped());
    }

    #[tokio::test]
    async fn test_drawdown_trip() {
        let config = CircuitBreakerConfig {
            max_drawdown_pct: Decimal::new(10, 2), // 10%
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Set initial value
        breaker.update_portfolio_value(Decimal::new(1000, 0)).await.unwrap();

        // 9% drawdown - no trip
        let reason = breaker.update_portfolio_value(Decimal::new(910, 0)).await.unwrap();
        assert!(reason.is_none());

        // 10% drawdown - trip
        let reason = breaker.update_portfolio_value(Decimal::new(900, 0)).await.unwrap();
        assert_eq!(reason, Some(TripReason::MaxDrawdown));
    }

    #[tokio::test]
    async fn test_manual_trip_and_reset() {
        let config = CircuitBreakerConfig::default();
        let breaker = CircuitBreaker::new(config);

        assert!(!breaker.is_tripped());
        assert!(breaker.can_trade().await);

        breaker.manual_trip(Some("Test".to_string())).await;
        assert!(breaker.is_tripped());
        assert!(!breaker.can_trade().await);

        breaker.reset().await;
        assert!(!breaker.is_tripped());
        assert!(breaker.can_trade().await);
    }

    #[tokio::test]
    async fn test_disabled_breaker() {
        let config = CircuitBreakerConfig {
            enabled: false,
            max_daily_loss: Decimal::new(1, 0), // Very low
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Should not trip when disabled
        breaker.record_trade(Decimal::new(-1000, 0), false).await.unwrap();
        assert!(!breaker.is_tripped());
        assert!(breaker.can_trade().await);
    }
}
