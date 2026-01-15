//! Circuit breaker for emergency trading halts.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::circuit_breaker_repo::CircuitBreakerRepository;

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
    /// Enable gradual recovery mode.
    pub gradual_recovery_enabled: bool,
    /// Number of recovery stages (e.g., 4 stages = 25%, 50%, 75%, 100%).
    pub recovery_stages: u32,
    /// Minutes between recovery stages.
    pub recovery_stage_minutes: i64,
    /// Require a profitable trade before advancing to next stage.
    pub require_profit_to_advance: bool,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_daily_loss: Decimal::new(1000, 0), // $1000 daily loss limit
            max_drawdown_pct: Decimal::new(10, 2), // 10% max drawdown
            max_consecutive_losses: 5,             // 5 consecutive losses
            cooldown_minutes: 60,                  // 1 hour cooldown
            enabled: true,
            gradual_recovery_enabled: false, // Off by default for backward compatibility
            recovery_stages: 4,              // 25%, 50%, 75%, 100%
            recovery_stage_minutes: 15,      // 15 minutes per stage
            require_profit_to_advance: true, // Need a win to advance
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
    /// Recovery mode state (if in gradual recovery).
    pub recovery_state: Option<RecoveryState>,
}

/// State for gradual recovery mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryState {
    /// Current recovery stage (1 to recovery_stages).
    pub current_stage: u32,
    /// Total number of stages.
    pub total_stages: u32,
    /// When recovery mode started.
    pub started_at: DateTime<Utc>,
    /// When current stage started.
    pub stage_started_at: DateTime<Utc>,
    /// When we can advance to next stage (time-based).
    pub next_stage_at: Option<DateTime<Utc>>,
    /// Whether we've had a profitable trade in current stage.
    pub had_profit_this_stage: bool,
    /// Trades executed in current stage.
    pub trades_this_stage: u32,
    /// P&L accumulated during recovery.
    pub recovery_pnl: Decimal,
}

impl RecoveryState {
    /// Create a new recovery state.
    pub fn new(total_stages: u32, stage_minutes: i64) -> Self {
        let now = Utc::now();
        Self {
            current_stage: 1,
            total_stages,
            started_at: now,
            stage_started_at: now,
            next_stage_at: Some(now + Duration::minutes(stage_minutes)),
            had_profit_this_stage: false,
            trades_this_stage: 0,
            recovery_pnl: Decimal::ZERO,
        }
    }

    /// Get the current capacity as a percentage (0.0 to 1.0).
    pub fn capacity_pct(&self) -> Decimal {
        Decimal::from(self.current_stage) / Decimal::from(self.total_stages)
    }

    /// Check if fully recovered.
    pub fn is_fully_recovered(&self) -> bool {
        self.current_stage >= self.total_stages
    }
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
            recovery_state: None,
        }
    }
}

/// Circuit breaker for emergency trading halts.
pub struct CircuitBreaker {
    config: Arc<RwLock<CircuitBreakerConfig>>,
    state: Arc<RwLock<CircuitBreakerState>>,
    /// Fast path flag for checking if tripped.
    is_tripped: AtomicBool,
    /// Database repository for persistence.
    repo: Option<CircuitBreakerRepository>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker without database persistence.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            state: Arc::new(RwLock::new(CircuitBreakerState::default())),
            is_tripped: AtomicBool::new(false),
            repo: None,
        }
    }

    /// Create a new circuit breaker with database persistence.
    pub fn with_persistence(config: CircuitBreakerConfig, pool: PgPool) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            state: Arc::new(RwLock::new(CircuitBreakerState::default())),
            is_tripped: AtomicBool::new(false),
            repo: Some(CircuitBreakerRepository::new(pool)),
        }
    }

    /// Load state from database on startup.
    /// This should be called once during initialization.
    pub async fn load_state(&self) -> Result<bool> {
        let repo = match &self.repo {
            Some(r) => r,
            None => {
                warn!("Cannot load state: no database connection configured");
                return Ok(false);
            }
        };

        // Check if we need a daily reset
        if let Some(last_reset) = repo.get_last_reset_date().await? {
            let today = Utc::now().date_naive();
            if last_reset < today {
                info!(
                    last_reset = %last_reset,
                    today = %today,
                    "New trading day detected, performing daily reset"
                );
                self.reset_daily().await;
                repo.update_last_reset_date(today).await?;
                return Ok(true);
            }
        }

        // Load existing state
        if let Some(loaded_state) = repo.load().await? {
            let mut state = self.state.write().await;
            *state = loaded_state;

            // Update atomic flag
            self.is_tripped.store(state.tripped, Ordering::SeqCst);

            // Check if we should auto-reset from cooldown
            if state.tripped {
                if let Some(resume_at) = state.resume_at {
                    if Utc::now() >= resume_at {
                        info!("Cooldown expired during downtime, resetting circuit breaker");
                        drop(state);
                        self.reset().await;
                    }
                }
            }

            return Ok(true);
        }

        Ok(false)
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

        // If in recovery mode, trading is allowed (at reduced capacity)
        {
            let state = self.state.read().await;
            if state.recovery_state.is_some() {
                return true;
            }
        }

        if !self.is_tripped.load(Ordering::SeqCst) {
            return true;
        }

        // Check if cooldown has expired
        let state = self.state.read().await;
        if let Some(resume_at) = state.resume_at {
            if Utc::now() >= resume_at {
                drop(state);
                // Start recovery mode if enabled
                if config.gradual_recovery_enabled {
                    self.start_recovery().await;
                    return true;
                } else {
                    self.reset().await;
                    return true;
                }
            }
        }

        false
    }

    /// Get current trading capacity (0.0 to 1.0).
    /// Returns 1.0 when not in recovery mode or when recovery is disabled.
    pub async fn trading_capacity(&self) -> Decimal {
        let state = self.state.read().await;
        match &state.recovery_state {
            Some(recovery) => recovery.capacity_pct(),
            None => Decimal::ONE,
        }
    }

    /// Check if currently in recovery mode.
    pub async fn is_in_recovery(&self) -> bool {
        let state = self.state.read().await;
        state.recovery_state.is_some()
    }

    /// Get recovery state if in recovery mode.
    pub async fn recovery_state(&self) -> Option<RecoveryState> {
        let state = self.state.read().await;
        state.recovery_state.clone()
    }

    /// Start gradual recovery mode.
    async fn start_recovery(&self) {
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        let recovery = RecoveryState::new(config.recovery_stages, config.recovery_stage_minutes);

        info!(
            total_stages = recovery.total_stages,
            capacity_pct = %recovery.capacity_pct(),
            "Starting gradual recovery mode"
        );

        state.tripped = false;
        state.trip_reason = None;
        state.recovery_state = Some(recovery);
        self.is_tripped.store(false, Ordering::SeqCst);

        // Persist state to database
        self.persist_state(&state).await;
    }

    /// Advance recovery to next stage if conditions are met.
    /// Returns true if advanced, false if not ready.
    pub async fn try_advance_recovery(&self) -> bool {
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        let recovery = match &mut state.recovery_state {
            Some(r) => r,
            None => return false,
        };

        // Already fully recovered
        if recovery.is_fully_recovered() {
            info!("Recovery complete, exiting recovery mode");
            state.recovery_state = None;
            self.persist_state(&state).await;
            return true;
        }

        let now = Utc::now();

        // Check time requirement
        let time_ready = recovery.next_stage_at.map(|t| now >= t).unwrap_or(true);

        // Check profit requirement if enabled
        let profit_ready = !config.require_profit_to_advance || recovery.had_profit_this_stage;

        if time_ready && profit_ready {
            recovery.current_stage += 1;
            recovery.stage_started_at = now;
            recovery.next_stage_at = Some(now + Duration::minutes(config.recovery_stage_minutes));
            recovery.had_profit_this_stage = false;
            recovery.trades_this_stage = 0;

            info!(
                stage = recovery.current_stage,
                total_stages = recovery.total_stages,
                capacity_pct = %recovery.capacity_pct(),
                "Advanced to next recovery stage"
            );

            // Check if now fully recovered
            if recovery.is_fully_recovered() {
                info!(
                    recovery_pnl = %recovery.recovery_pnl,
                    "Gradual recovery complete"
                );
                state.recovery_state = None;
            }

            self.persist_state(&state).await;
            return true;
        }

        false
    }

    /// Force exit from recovery mode (either complete or abort).
    pub async fn exit_recovery(&self, complete: bool) {
        let mut state = self.state.write().await;

        if let Some(recovery) = &state.recovery_state {
            if complete {
                info!(
                    recovery_pnl = %recovery.recovery_pnl,
                    stages_completed = recovery.current_stage,
                    "Recovery mode completed early"
                );
            } else {
                warn!(
                    recovery_pnl = %recovery.recovery_pnl,
                    stages_completed = recovery.current_stage,
                    "Recovery mode aborted"
                );
            }
        }

        state.recovery_state = None;
        self.persist_state(&state).await;
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

        // Update recovery state if in recovery mode
        if state.recovery_state.is_some() {
            // Extract values we need before mutating
            let daily_pnl = state.daily_pnl;
            let consecutive_losses = state.consecutive_losses;

            let recovery = state.recovery_state.as_mut().unwrap();
            recovery.trades_this_stage += 1;
            recovery.recovery_pnl += pnl;

            if is_win {
                recovery.had_profit_this_stage = true;
            }

            // During recovery, use stricter thresholds (scaled by capacity)
            let capacity = recovery.capacity_pct();
            let recovery_stage = recovery.current_stage;
            let scaled_max_loss = config.max_daily_loss * capacity;
            let capacity_f64: f64 = capacity.to_string().parse().unwrap_or(1.0);
            let scaled_consecutive =
                (config.max_consecutive_losses as f64 * capacity_f64).max(1.0) as u32;

            // Check if we should re-trip during recovery
            if daily_pnl < Decimal::ZERO && daily_pnl.abs() >= scaled_max_loss {
                warn!(
                    daily_pnl = %daily_pnl,
                    scaled_limit = %scaled_max_loss,
                    recovery_stage = recovery_stage,
                    "Re-tripping during recovery: daily loss exceeded scaled limit"
                );
                state.recovery_state = None;
                drop(state);
                drop(config);
                return self.record_trade_trip(TripReason::DailyLossLimit).await;
            }

            if consecutive_losses >= scaled_consecutive {
                warn!(
                    consecutive_losses = consecutive_losses,
                    scaled_limit = scaled_consecutive,
                    recovery_stage = recovery_stage,
                    "Re-tripping during recovery: consecutive losses exceeded scaled limit"
                );
                state.recovery_state = None;
                drop(state);
                drop(config);
                return self.record_trade_trip(TripReason::ConsecutiveLosses).await;
            }

            // Persist state to database
            self.persist_state(&state).await;
            return Ok(None);
        }

        // Check thresholds (normal mode)
        let reason = self.check_thresholds(&config, &state);

        if let Some(ref reason) = reason {
            self.trip_internal(&mut state, reason.clone(), &config)
                .await;
        }

        // Persist state to database
        self.persist_state(&state).await;

        Ok(reason)
    }

    /// Helper to trip from within record_trade (avoids holding locks).
    async fn record_trade_trip(&self, reason: TripReason) -> Result<Option<TripReason>> {
        let config = self.config.read().await;
        let mut state = self.state.write().await;
        self.trip_internal(&mut state, reason.clone(), &config)
            .await;
        self.persist_state(&state).await;
        Ok(Some(reason))
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
                self.trip_internal(&mut state, reason.clone(), &config)
                    .await;
                // Persist state to database
                self.persist_state(&state).await;
                return Ok(Some(reason));
            }
        }

        // Persist state to database
        self.persist_state(&state).await;

        Ok(None)
    }

    /// Manually trip the circuit breaker.
    pub async fn manual_trip(&self, reason: Option<String>) {
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        warn!(reason = ?reason, "Manual circuit breaker trip");
        self.trip_internal(&mut state, TripReason::Manual, &config)
            .await;

        // Persist state to database
        self.persist_state(&state).await;
    }

    /// Trip due to connectivity issues.
    pub async fn trip_connectivity(&self, error: &str) {
        let config = self.config.read().await;
        let mut state = self.state.write().await;

        error!(error = %error, "Circuit breaker tripped due to connectivity");
        self.trip_internal(&mut state, TripReason::Connectivity, &config)
            .await;

        // Persist state to database
        self.persist_state(&state).await;
    }

    /// Reset the circuit breaker.
    pub async fn reset(&self) {
        let mut state = self.state.write().await;

        state.tripped = false;
        state.trip_reason = None;
        state.tripped_at = None;
        state.resume_at = None;
        self.is_tripped.store(false, Ordering::SeqCst);

        // Persist state to database
        self.persist_state(&state).await;

        info!("Circuit breaker reset");
    }

    /// Reset daily counters (call at start of trading day).
    pub async fn reset_daily(&self) {
        let mut state = self.state.write().await;

        state.daily_pnl = Decimal::ZERO;
        state.consecutive_losses = 0;
        state.trips_today = 0;

        // Persist state to database
        self.persist_state(&state).await;

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

    /// Persist current state to database (if configured).
    async fn persist_state(&self, state: &CircuitBreakerState) {
        if let Some(repo) = &self.repo {
            if let Err(e) = repo.save(state).await {
                error!(error = %e, "Failed to persist circuit breaker state");
            }
        }
    }

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
            let reason = breaker
                .record_trade(Decimal::new(-10, 0), false)
                .await
                .unwrap();
            assert!(reason.is_none());
        }

        // This should trip
        let reason = breaker
            .record_trade(Decimal::new(-10, 0), false)
            .await
            .unwrap();
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
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        assert!(!breaker.is_tripped());

        // 3rd loss - trip
        let reason = breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
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
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();

        // 1 win - resets counter
        breaker
            .record_trade(Decimal::new(1, 0), true)
            .await
            .unwrap();

        // 2 more losses - should not trip yet
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
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
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();

        // 9% drawdown - no trip
        let reason = breaker
            .update_portfolio_value(Decimal::new(910, 0))
            .await
            .unwrap();
        assert!(reason.is_none());

        // 10% drawdown - trip
        let reason = breaker
            .update_portfolio_value(Decimal::new(900, 0))
            .await
            .unwrap();
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
        breaker
            .record_trade(Decimal::new(-1000, 0), false)
            .await
            .unwrap();
        assert!(!breaker.is_tripped());
        assert!(breaker.can_trade().await);
    }

    #[tokio::test]
    async fn test_recovery_state_capacity() {
        let recovery = RecoveryState::new(4, 15);

        // Stage 1 of 4 = 25% capacity
        assert_eq!(recovery.current_stage, 1);
        assert_eq!(recovery.capacity_pct(), Decimal::new(25, 2));
        assert!(!recovery.is_fully_recovered());
    }

    #[tokio::test]
    async fn test_gradual_recovery_mode_enabled() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 2,
            max_daily_loss: Decimal::new(100000, 0), // High limit
            cooldown_minutes: 0,                     // Immediate cooldown for testing
            gradual_recovery_enabled: true,
            recovery_stages: 4,
            recovery_stage_minutes: 0, // Immediate stage advancement
            require_profit_to_advance: false,
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Trip the breaker
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        let reason = breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        assert_eq!(reason, Some(TripReason::ConsecutiveLosses));
        assert!(breaker.is_tripped());

        // After cooldown, should enter recovery mode
        assert!(breaker.can_trade().await);
        assert!(breaker.is_in_recovery().await);

        // Check capacity starts at 25%
        let capacity = breaker.trading_capacity().await;
        assert_eq!(capacity, Decimal::new(25, 2));
    }

    #[tokio::test]
    async fn test_recovery_stage_advancement() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 2,
            max_daily_loss: Decimal::new(100000, 0),
            cooldown_minutes: 0,
            gradual_recovery_enabled: true,
            recovery_stages: 4,
            recovery_stage_minutes: 0, // Immediate
            require_profit_to_advance: false,
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Trip and enter recovery
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker.can_trade().await;

        // Advance through stages
        assert!(breaker.try_advance_recovery().await); // Stage 2
        assert_eq!(breaker.trading_capacity().await, Decimal::new(50, 2));

        assert!(breaker.try_advance_recovery().await); // Stage 3
        assert_eq!(breaker.trading_capacity().await, Decimal::new(75, 2));

        assert!(breaker.try_advance_recovery().await); // Stage 4
        assert_eq!(breaker.trading_capacity().await, Decimal::ONE);

        // Should be fully recovered now
        assert!(!breaker.is_in_recovery().await);
    }

    #[tokio::test]
    async fn test_recovery_requires_profit() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 2,
            max_daily_loss: Decimal::new(100000, 0),
            cooldown_minutes: 0,
            gradual_recovery_enabled: true,
            recovery_stages: 4,
            recovery_stage_minutes: 0,       // Immediate time
            require_profit_to_advance: true, // Must have profit
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Trip and enter recovery
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker.can_trade().await;

        // Cannot advance without profit
        assert!(!breaker.try_advance_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::new(25, 2));

        // Record a winning trade
        breaker
            .record_trade(Decimal::new(1, 0), true)
            .await
            .unwrap();

        // Now can advance
        assert!(breaker.try_advance_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::new(50, 2));
    }

    #[tokio::test]
    async fn test_recovery_retrip_on_loss() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 100,          // Won't trip on consecutive
            max_daily_loss: Decimal::new(100, 0), // $100 daily loss
            cooldown_minutes: 0,
            gradual_recovery_enabled: true,
            recovery_stages: 4,
            recovery_stage_minutes: 0,
            require_profit_to_advance: false,
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Trigger initial trip via daily loss
        for _ in 0..10 {
            breaker
                .record_trade(Decimal::new(-10, 0), false)
                .await
                .unwrap();
        }
        assert!(breaker.is_tripped());

        // Enter recovery mode
        breaker.can_trade().await;
        assert!(breaker.is_in_recovery().await);

        // At stage 1 (25% capacity), scaled limit is $25
        // Current daily P&L is -$100, which exceeds the scaled limit
        // But the check happens on new trades, so record another loss
        let reason = breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();

        // Should re-trip
        assert_eq!(reason, Some(TripReason::DailyLossLimit));
        assert!(breaker.is_tripped());
        assert!(!breaker.is_in_recovery().await);
    }

    #[tokio::test]
    async fn test_exit_recovery_early() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 2,
            max_daily_loss: Decimal::new(100000, 0),
            cooldown_minutes: 0,
            gradual_recovery_enabled: true,
            recovery_stages: 4,
            recovery_stage_minutes: 15, // Won't naturally advance
            require_profit_to_advance: true,
            enabled: true,
            ..Default::default()
        };
        let breaker = CircuitBreaker::new(config);

        // Trip and enter recovery
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker
            .record_trade(Decimal::new(-1, 0), false)
            .await
            .unwrap();
        breaker.can_trade().await;
        assert!(breaker.is_in_recovery().await);

        // Force exit recovery (complete)
        breaker.exit_recovery(true).await;
        assert!(!breaker.is_in_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::ONE);
    }
}
