//! Circuit breaker for emergency trading halts.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
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
    /// Hard kill switch — permanent halt requiring manual reset.
    /// Triggered when total drawdown exceeds `hard_kill_drawdown_pct`.
    HardKillSwitch,
}

/// Configuration for circuit breaker thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Hard kill drawdown threshold (e.g. 0.40 = 40%). When total drawdown
    /// exceeds this, trading halts permanently until manual reset.
    /// Set to 0 to disable. Default: 0.40.
    pub hard_kill_drawdown_pct: Decimal,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            max_daily_loss: Decimal::new(2500, 0), // $2500 daily loss limit
            max_drawdown_pct: Decimal::new(20, 2), // 20% max drawdown
            max_consecutive_losses: 8,             // 8 consecutive losses
            cooldown_minutes: 30,                  // 30 min cooldown
            enabled: true,
            gradual_recovery_enabled: true, // Use recovery system instead of hard lockout
            recovery_stages: 3,             // 33%, 66%, 100%
            recovery_stage_minutes: 10,     // 10 minutes per stage
            require_profit_to_advance: false, // Time-based is sufficient for learning
            hard_kill_drawdown_pct: Decimal::new(40, 2), // 40% permanent halt
        }
    }
}

impl CircuitBreakerConfig {
    /// Why these limits cannot guard trading, if they cannot. Drawdowns are
    /// fractions: 20 (meant as 20%) would never trip. A limit of 0 holds
    /// trading back at once, which is safe, so it stands.
    pub fn validate(&self) -> Result<(), String> {
        let fraction = |value: Decimal| value >= Decimal::ZERO && value <= Decimal::ONE;
        if self.max_daily_loss < Decimal::ZERO {
            return Err("max_daily_loss must be at least 0".into());
        }
        if !fraction(self.max_drawdown_pct) {
            return Err(format!(
                "max_drawdown_pct must be a fraction from 0 to 1 (0.20 for 20%), not {}",
                self.max_drawdown_pct
            ));
        }
        if !fraction(self.hard_kill_drawdown_pct) {
            return Err(format!(
                "hard_kill_drawdown_pct must be 0 (off) or a fraction at most 1, not {}",
                self.hard_kill_drawdown_pct
            ));
        }
        if self.max_consecutive_losses < 1 {
            return Err("max_consecutive_losses must be at least 1".into());
        }
        // A longer cooldown is held to a week when scheduled.
        if self.cooldown_minutes < 0 {
            return Err("cooldown_minutes must be at least 0".into());
        }
        Ok(())
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
    /// High-water mark of portfolio equity (reference for the hard kill switch).
    pub peak_value: Decimal,
    /// Reference equity for the soft max-drawdown trip. Rebased when trading
    /// resumes after a drawdown cooldown and on manual reset.
    #[serde(default)]
    pub drawdown_peak: Decimal,
    /// Current portfolio value.
    pub current_value: Decimal,
    /// Count of consecutive losing trades.
    pub consecutive_losses: u32,
    /// Total trips today.
    pub trips_today: u32,
    /// Recovery mode state (if in gradual recovery).
    pub recovery_state: Option<RecoveryState>,
    /// Date of last daily reset (for midnight rollover detection).
    #[serde(default = "today_naive")]
    pub last_reset_date: chrono::NaiveDate,
    /// Whether peak/current values come from real equity snapshots. Drawdown
    /// is only evaluated once seeded; realized P&L alone has no capital base.
    #[serde(default)]
    pub portfolio_value_seeded: bool,
    /// Cash-flow events created up to this time have been applied to the
    /// drawdown references.
    #[serde(default)]
    pub cash_flows_applied_through: Option<DateTime<Utc>>,
}

fn today_naive() -> chrono::NaiveDate {
    Utc::now().date_naive()
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
            next_stage_at: Some(after_minutes(now, stage_minutes)),
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
            drawdown_peak: Decimal::ZERO,
            current_value: Decimal::ZERO,
            consecutive_losses: 0,
            trips_today: 0,
            recovery_state: None,
            last_reset_date: Utc::now().date_naive(),
            portfolio_value_seeded: false,
            cash_flows_applied_through: None,
        }
    }
}

impl CircuitBreakerState {
    /// Clears the per-day counters when the UTC date has changed.
    fn roll_daily_counters(&mut self, today: chrono::NaiveDate) -> bool {
        if self.last_reset_date >= today {
            return false;
        }
        info!(
            last_reset = %self.last_reset_date,
            today = %today,
            "New trading day, resetting daily circuit breaker counters"
        );
        self.daily_pnl = Decimal::ZERO;
        self.consecutive_losses = 0;
        self.trips_today = 0;
        self.last_reset_date = today;
        true
    }
}

/// Trips that halt trading until an operator resets the breaker.
fn latches(reason: &TripReason) -> bool {
    matches!(reason, TripReason::HardKillSwitch | TripReason::Manual)
}

/// The longest wait the breaker schedules: a week. A configured period
/// beyond it (a typo, or one meant as "forever") would overflow the clock.
pub const MAX_WAIT_MINUTES: i64 = 7 * 24 * 60;

/// `now` plus `minutes`, held to between zero and [`MAX_WAIT_MINUTES`].
fn after_minutes(now: DateTime<Utc>, minutes: i64) -> DateTime<Utc> {
    now + Duration::minutes(minutes.clamp(0, MAX_WAIT_MINUTES))
}

/// How strongly a loss limit holds trading back among trips of equal
/// severity: the day's loss (until the UTC day ends), then a run of losses,
/// then a drawdown.
fn loss_rank(reason: &TripReason) -> u8 {
    match reason {
        TripReason::DailyLossLimit => 3,
        TripReason::ConsecutiveLosses => 2,
        TripReason::MaxDrawdown => 1,
        _ => 0,
    }
}

/// Whether a new trip reason supersedes the current one.
fn severity(reason: &TripReason) -> u8 {
    match reason {
        TripReason::HardKillSwitch => 2,
        TripReason::Manual => 1,
        _ => 0,
    }
}

fn drawdown(peak: Decimal, value: Decimal) -> Decimal {
    if peak > Decimal::ZERO {
        (peak - value) / peak
    } else {
        Decimal::ZERO
    }
}

/// Circuit breaker for emergency trading halts.
pub struct CircuitBreaker {
    config: Arc<RwLock<CircuitBreakerConfig>>,
    state: Arc<RwLock<CircuitBreakerState>>,
    /// Fast path flag for checking if tripped.
    is_tripped: AtomicBool,
    /// Consecutive equity snapshots at or beyond the hard-kill drawdown.
    hard_kill_breaches: AtomicU32,
    /// Database repository for persistence.
    repo: Option<CircuitBreakerRepository>,
    /// Whether this process keeps the persisted state. One process trades
    /// and saves it; another (an API process beside it, during a deploy)
    /// follows the stored state and stores only operator changes.
    leader: AtomicBool,
    /// The operator epoch of the stored state this process last saw.
    operator_epoch: AtomicI64,
    /// A save of the leader's state failed and is still owed.
    unsaved: AtomicBool,
    /// When the stored state was last checked by [`Self::sync`].
    synced_at: std::sync::Mutex<Option<std::time::Instant>>,
}

/// How often a follower re-reads the stored state.
const FOLLOWER_SYNC: std::time::Duration = std::time::Duration::from_secs(2);
/// How often the leader looks for an operator change made elsewhere.
pub(crate) const LEADER_SYNC: std::time::Duration = std::time::Duration::from_secs(5);

/// Take an operator's change (trip, reset) from `stored` into `state`,
/// keeping the counters the leader has kept since.
fn adopt_operator_change(state: &mut CircuitBreakerState, stored: CircuitBreakerState) {
    state.tripped = stored.tripped;
    state.trip_reason = stored.trip_reason;
    state.tripped_at = stored.tripped_at;
    state.resume_at = stored.resume_at;
    state.recovery_state = stored.recovery_state;
    state.peak_value = stored.peak_value;
    state.drawdown_peak = stored.drawdown_peak;
    state.trips_today = stored.trips_today;
}

/// Consecutive equity snapshots beyond the hard-kill drawdown required before
/// the latch engages. The first breach halts trading with a max-drawdown trip,
/// so a single bad mark cannot latch the breaker but cannot be traded through.
const HARD_KILL_CONFIRMATIONS: u32 = 2;

impl CircuitBreaker {
    /// Create a new circuit breaker without database persistence.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            state: Arc::new(RwLock::new(CircuitBreakerState::default())),
            is_tripped: AtomicBool::new(false),
            hard_kill_breaches: AtomicU32::new(0),
            repo: None,
            leader: AtomicBool::new(true),
            operator_epoch: AtomicI64::new(0),
            unsaved: AtomicBool::new(false),
            synced_at: std::sync::Mutex::new(None),
        }
    }

    /// Create a new circuit breaker with database persistence.
    pub fn with_persistence(config: CircuitBreakerConfig, pool: PgPool) -> Self {
        Self {
            repo: Some(CircuitBreakerRepository::new(pool)),
            ..Self::new(config)
        }
    }

    /// Start as a follower of the stored state: until [`Self::lead`], this
    /// process saves only operator changes and re-reads the rest.
    pub fn following(self) -> Self {
        self.leader.store(false, Ordering::SeqCst);
        self
    }

    /// Stop keeping the persisted state (another process may lead now).
    pub fn step_down(&self) {
        self.leader.store(false, Ordering::SeqCst);
    }

    /// Whether this process keeps the persisted state.
    pub fn is_leader(&self) -> bool {
        self.leader.load(Ordering::SeqCst)
    }

    /// Take over the persisted state: reload it (the previous leader's last
    /// save included) and keep it from now on.
    pub async fn lead(&self) -> Result<bool> {
        self.leader.store(true, Ordering::SeqCst);
        self.load_state().await
    }

    /// Bring this process's view up to date with the stored state, at most
    /// every few seconds. A follower takes the whole stored state; the leader
    /// takes an operator change made through another process, and retries a
    /// save that failed. Errors leave the current view in place.
    pub async fn sync(&self) {
        let Some(repo) = &self.repo else { return };
        let leader = self.is_leader();
        {
            let mut synced_at = self.synced_at.lock().unwrap_or_else(|e| e.into_inner());
            let every = if leader { LEADER_SYNC } else { FOLLOWER_SYNC };
            if synced_at.is_some_and(|at| at.elapsed() < every) {
                return;
            }
            *synced_at = Some(std::time::Instant::now());
        }
        if !leader {
            if let Err(error) = self.mirror().await {
                warn!(%error, "Could not re-read the circuit breaker's stored state");
            }
            return;
        }
        let stored_epoch = match repo.operator_epoch().await {
            Ok(epoch) => epoch,
            Err(error) => {
                warn!(%error, "Could not check the circuit breaker for operator changes");
                return;
            }
        };
        let mut state = self.state.write().await;
        if stored_epoch.is_some_and(|epoch| epoch != self.operator_epoch.load(Ordering::SeqCst))
            || self.unsaved.load(Ordering::SeqCst)
        {
            // persist_state adopts an operator change it finds first.
            self.persist_state(&mut state).await;
        }
    }

    /// A follower's view: the whole stored state.
    async fn mirror(&self) -> Result<()> {
        let Some(repo) = &self.repo else {
            return Ok(());
        };
        if let Some((mut stored, epoch)) = repo.load().await? {
            stored.roll_daily_counters(Utc::now().date_naive());
            let mut state = self.state.write().await;
            self.is_tripped.store(stored.tripped, Ordering::SeqCst);
            *state = stored;
            self.operator_epoch.store(epoch, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Load state from database on startup.
    ///
    /// The persisted trip, latch, drawdown references and recovery stage are
    /// always restored; a new UTC day only clears the per-day counters.
    pub async fn load_state(&self) -> Result<bool> {
        let repo = match &self.repo {
            Some(r) => r,
            None => {
                warn!("Cannot load state: no database connection configured");
                return Ok(false);
            }
        };

        let Some((mut loaded, epoch)) = repo.load().await? else {
            return Ok(false);
        };
        loaded.roll_daily_counters(Utc::now().date_naive());

        let mut state = self.state.write().await;
        *state = loaded;
        self.operator_epoch.store(epoch, Ordering::SeqCst);
        self.is_tripped.store(state.tripped, Ordering::SeqCst);
        if state.trip_reason == Some(TripReason::HardKillSwitch) {
            warn!("Hard kill switch is active — trading halted until manual reset");
        }
        self.persist_state(&mut state).await;
        Ok(true)
    }

    /// Check if trading is currently halted (fast path).
    pub fn is_tripped(&self) -> bool {
        self.is_tripped.load(Ordering::SeqCst)
    }

    /// Check if trading is allowed.
    ///
    /// A trip always blocks, including during gradual recovery. Once a
    /// non-latching trip's cooldown has elapsed, trading resumes (in gradual
    /// recovery when enabled); recovery stages advance as their time comes.
    pub async fn can_trade(&self) -> bool {
        self.sync().await;
        if !self.config.read().await.enabled {
            // Switching the breaker off stops its limits, not a latch the
            // hard kill or an operator set: only a reset lifts that.
            let state = self.state.read().await;
            return !(state.tripped && state.trip_reason.as_ref().is_some_and(latches));
        }

        if self.is_tripped.load(Ordering::SeqCst) {
            return self.resume_if_cooldown_elapsed().await;
        }

        if self.state.read().await.recovery_state.is_some() {
            self.try_advance_recovery().await;
        }
        true
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

    /// Cash-flow events created up to this time have already been applied.
    pub async fn cash_flows_applied_through(&self) -> Option<DateTime<Utc>> {
        self.state.read().await.cash_flows_applied_through
    }

    /// Resumes trading when a non-latching trip's cooldown has elapsed.
    /// A daily loss limit holds for the rest of its UTC day: the trip lasts
    /// while the day's loss still breaches it. Returns whether trading is
    /// allowed.
    async fn resume_if_cooldown_elapsed(&self) -> bool {
        let config = self.config.read().await.clone();
        let mut state = self.state.write().await;

        let due = state.tripped
            && !state.trip_reason.as_ref().is_some_and(latches)
            && state
                .resume_at
                .is_some_and(|resume_at| Utc::now() >= resume_at);
        if !due {
            return !state.tripped;
        }
        // A day's loss over the limit (losses recorded during this trip
        // included) holds the trip for the rest of its UTC day.
        let rolled = state.roll_daily_counters(Utc::now().date_naive());
        if Self::check_loss_thresholds(&config, &state) == Some(TripReason::DailyLossLimit) {
            if state.trip_reason != Some(TripReason::DailyLossLimit) || rolled {
                state.trip_reason = Some(TripReason::DailyLossLimit);
                self.persist_state(&mut state).await;
            }
            return false;
        }

        // Measure any further soft drawdown from the equity trading resumes at;
        // the hard kill keeps measuring from the high-water mark.
        if state.trip_reason == Some(TripReason::MaxDrawdown) && state.portfolio_value_seeded {
            state.drawdown_peak = state.current_value;
        }
        state.tripped = false;
        state.trip_reason = None;
        state.tripped_at = None;
        state.resume_at = None;
        self.is_tripped.store(false, Ordering::SeqCst);

        if config.gradual_recovery_enabled {
            let recovery =
                RecoveryState::new(config.recovery_stages, config.recovery_stage_minutes);
            info!(
                total_stages = recovery.total_stages,
                capacity_pct = %recovery.capacity_pct(),
                "Cooldown elapsed, starting gradual recovery mode"
            );
            state.recovery_state = Some(recovery);
        } else {
            info!("Cooldown elapsed, circuit breaker reset");
        }

        self.persist_state(&mut state).await;
        true
    }

    /// Advance recovery to next stage if conditions are met.
    /// Returns true if advanced, false if not ready.
    pub async fn try_advance_recovery(&self) -> bool {
        let config = self.config.read().await.clone();
        let mut state = self.state.write().await;

        let recovery = match &mut state.recovery_state {
            Some(r) => r,
            None => return false,
        };

        // Already fully recovered
        if recovery.is_fully_recovered() {
            info!("Recovery complete, exiting recovery mode");
            state.recovery_state = None;
            self.persist_state(&mut state).await;
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
            recovery.next_stage_at = Some(after_minutes(now, config.recovery_stage_minutes));
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

            self.persist_state(&mut state).await;
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
        self.persist_state(&mut state).await;
    }

    /// Record a realized trade result and check the loss thresholds.
    ///
    /// Realized P&L never moves the equity values: drawdown is evaluated only
    /// from equity snapshots in [`Self::update_portfolio_value`].
    pub async fn record_trade(&self, pnl: Decimal) -> Result<Option<TripReason>> {
        let config = self.config.read().await.clone();
        if !config.enabled {
            return Ok(None);
        }

        let mut state = self.state.write().await;
        state.roll_daily_counters(Utc::now().date_naive());

        state.daily_pnl += pnl;
        // A loss extends the streak and a win ends it; a break-even trade
        // does neither.
        let is_win = pnl > Decimal::ZERO;
        if is_win {
            state.consecutive_losses = 0;
        } else if pnl < Decimal::ZERO {
            state.consecutive_losses += 1;
        }

        let reason = if let Some(recovery) = state.recovery_state.as_mut() {
            recovery.trades_this_stage += 1;
            recovery.recovery_pnl += pnl;
            if is_win {
                recovery.had_profit_this_stage = true;
            }

            // During recovery, use stricter thresholds (scaled by capacity)
            let capacity = recovery.capacity_pct();
            let recovery_stage = recovery.current_stage;
            let scaled_max_loss = config.max_daily_loss * capacity;
            let capacity_f64 = capacity.to_f64().unwrap_or(1.0);
            let scaled_consecutive =
                (config.max_consecutive_losses as f64 * capacity_f64).max(1.0) as u32;

            if state.daily_pnl < Decimal::ZERO && state.daily_pnl.abs() >= scaled_max_loss {
                warn!(
                    daily_pnl = %state.daily_pnl,
                    scaled_limit = %scaled_max_loss,
                    recovery_stage,
                    "Re-tripping during recovery: daily loss exceeded scaled limit"
                );
                Some(TripReason::DailyLossLimit)
            } else if state.consecutive_losses >= scaled_consecutive {
                warn!(
                    consecutive_losses = state.consecutive_losses,
                    scaled_limit = scaled_consecutive,
                    recovery_stage,
                    "Re-tripping during recovery: consecutive losses exceeded scaled limit"
                );
                Some(TripReason::ConsecutiveLosses)
            } else {
                None
            }
        } else {
            Self::check_loss_thresholds(&config, &state)
        };

        if let Some(reason) = &reason {
            self.trip_internal(&mut state, reason.clone(), &config);
        }
        self.persist_state(&mut state).await;
        Ok(reason)
    }

    /// Update portfolio value (for drawdown calculation).
    pub async fn update_portfolio_value(&self, value: Decimal) -> Result<Option<TripReason>> {
        self.update_portfolio_value_with_flows(value, Decimal::ZERO, None)
            .await
    }

    /// Update portfolio equity, first applying withdrawals recorded since the
    /// previous update so moving cash out is not treated as a trading loss.
    ///
    /// `withdrawals` is the (non-positive) sum of withdrawal amounts created up
    /// to `flows_through`, which must precede the balance read behind `value`.
    /// Deposits are not applied: they raise equity and the high-water mark on
    /// their own, while applying them at record time could fabricate drawdown.
    pub async fn update_portfolio_value_with_flows(
        &self,
        value: Decimal,
        withdrawals: Decimal,
        flows_through: Option<DateTime<Utc>>,
    ) -> Result<Option<TripReason>> {
        let config = self.config.read().await.clone();
        if !config.enabled {
            return Ok(None);
        }

        let mut state = self.state.write().await;
        state.roll_daily_counters(Utc::now().date_naive());

        if let Some(through) = flows_through {
            if state.portfolio_value_seeded && withdrawals < Decimal::ZERO {
                state.peak_value = (state.peak_value + withdrawals).max(Decimal::ZERO);
                state.drawdown_peak = (state.drawdown_peak + withdrawals).max(Decimal::ZERO);
                info!(
                    withdrawals = %withdrawals,
                    peak_value = %state.peak_value,
                    drawdown_peak = %state.drawdown_peak,
                    "Applied withdrawals to circuit breaker drawdown references"
                );
            }
            state.cash_flows_applied_through = Some(through);
        }

        if !state.portfolio_value_seeded {
            // Replace any reference not derived from equity snapshots.
            state.peak_value = value;
            state.drawdown_peak = value;
            state.portfolio_value_seeded = true;
        }
        state.current_value = value;
        state.peak_value = state.peak_value.max(value);
        state.drawdown_peak = state.drawdown_peak.max(value);

        let hard_breach = config.hard_kill_drawdown_pct > Decimal::ZERO
            && drawdown(state.peak_value, value) >= config.hard_kill_drawdown_pct;
        let breaches = if hard_breach {
            self.hard_kill_breaches.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.hard_kill_breaches.store(0, Ordering::SeqCst);
            0
        };

        let reason = if hard_breach && breaches >= HARD_KILL_CONFIRMATIONS {
            Some(TripReason::HardKillSwitch)
        } else if hard_breach || drawdown(state.drawdown_peak, value) >= config.max_drawdown_pct {
            Some(TripReason::MaxDrawdown)
        } else {
            None
        };

        if let Some(reason) = &reason {
            self.trip_internal(&mut state, reason.clone(), &config);
        }
        self.persist_state(&mut state).await;
        Ok(reason)
    }

    /// Check drawdown while an equity reading is held back (cash may have
    /// left the wallet without the flow being applied yet): `best_case` is
    /// the most equity can be, the reading plus every outflow that may have
    /// left. It trips only on a drawdown even that much equity shows, and it
    /// changes no reference, so it can neither raise a peak nor trip falsely,
    /// and misses a loss by no more than the outflows in doubt.
    pub async fn check_drawdown_bound(&self, best_case: Decimal) -> Result<Option<TripReason>> {
        let config = self.config.read().await.clone();
        if !config.enabled {
            return Ok(None);
        }
        let mut state = self.state.write().await;
        if !state.portfolio_value_seeded {
            return Ok(None);
        }
        let hard_breach = config.hard_kill_drawdown_pct > Decimal::ZERO
            && drawdown(state.peak_value, best_case) >= config.hard_kill_drawdown_pct;
        // A best case short of a breach does not show the equity recovered,
        // so it leaves a run of confirmed breaches as it is.
        let breaches = if hard_breach {
            self.hard_kill_breaches.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        let reason = if hard_breach && breaches >= HARD_KILL_CONFIRMATIONS {
            Some(TripReason::HardKillSwitch)
        } else if hard_breach || drawdown(state.drawdown_peak, best_case) >= config.max_drawdown_pct
        {
            Some(TripReason::MaxDrawdown)
        } else {
            None
        };
        if let Some(reason) = &reason {
            warn!(%best_case, ?reason, "Drawdown breached even counting withdrawals in doubt");
            self.trip_internal(&mut state, reason.clone(), &config);
            self.persist_state(&mut state).await;
        }
        Ok(reason)
    }

    /// Manually trip the circuit breaker. A manual trip latches until reset.
    /// It holds in this process at once; an error means it could not be
    /// stored, so neither a restart nor another API process would keep it.
    pub async fn manual_trip(&self, reason: Option<String>) -> Result<()> {
        let config = self.config.read().await.clone();
        warn!(reason = ?reason, "Manual circuit breaker trip");
        self.operator_change(|breaker, state| {
            breaker.trip_internal(state, TripReason::Manual, &config)
        })
        .await
    }

    /// Apply an operator's change to the current state (the stored one, for
    /// a follower) and store it, counting the operator epoch up.
    async fn operator_change(
        &self,
        change: impl FnOnce(&Self, &mut CircuitBreakerState),
    ) -> Result<()> {
        if !self.is_leader() {
            self.mirror().await?;
        }
        let mut state = self.state.write().await;
        change(self, &mut state);
        let Some(repo) = &self.repo else {
            return Ok(());
        };
        match repo.save_operator_change(&state).await {
            Ok(epoch) => {
                self.operator_epoch.store(epoch, Ordering::SeqCst);
                Ok(())
            }
            Err(error) => {
                error!(%error, "Failed to store an operator change to the circuit breaker");
                // The leader's next save carries it, without the epoch.
                self.unsaved.store(true, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    /// Trip due to connectivity issues.
    pub async fn trip_connectivity(&self, error: &str) {
        let config = self.config.read().await.clone();
        let mut state = self.state.write().await;

        error!(error = %error, "Circuit breaker tripped due to connectivity");
        self.trip_internal(&mut state, TripReason::Connectivity, &config);
        self.persist_state(&mut state).await;
    }

    /// Operator reset: clears any trip (including latches) and recovery, and
    /// accepts current equity as the new drawdown baseline. An error means
    /// the reset holds in this process only.
    pub async fn reset(&self) -> Result<()> {
        self.operator_change(|breaker, state| {
            state.tripped = false;
            state.trip_reason = None;
            state.tripped_at = None;
            state.resume_at = None;
            state.recovery_state = None;
            if state.portfolio_value_seeded {
                state.peak_value = state.current_value;
                state.drawdown_peak = state.current_value;
            }
            breaker.hard_kill_breaches.store(0, Ordering::SeqCst);
            breaker.is_tripped.store(false, Ordering::SeqCst);
            info!(baseline = %state.current_value, "Circuit breaker reset");
        })
        .await
    }

    /// Reset daily counters (call at start of trading day).
    pub async fn reset_daily(&self) {
        let mut state = self.state.write().await;

        state.daily_pnl = Decimal::ZERO;
        state.consecutive_losses = 0;
        state.trips_today = 0;
        state.last_reset_date = Utc::now().date_naive();

        self.persist_state(&mut state).await;

        info!("Circuit breaker daily reset");
    }

    /// Get current state.
    pub async fn state(&self) -> CircuitBreakerState {
        self.sync().await;
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

    /// Persist current state to database (if configured and this process
    /// is the leader). An operator change stored through another process
    /// since this one last looked is adopted into `state` first, then saved
    /// with what this process counted since; a failed save is retried by
    /// [`Self::sync`].
    async fn persist_state(&self, state: &mut CircuitBreakerState) {
        let Some(repo) = &self.repo else { return };
        if !self.is_leader() {
            return;
        }
        let saved = match repo
            .save(state, self.operator_epoch.load(Ordering::SeqCst))
            .await
        {
            Ok(false) => match repo.load().await {
                Ok(Some((stored, epoch))) => {
                    warn!(
                        tripped = stored.tripped,
                        trip_reason = ?stored.trip_reason,
                        "Adopting a circuit breaker change made through another process"
                    );
                    if !stored.tripped {
                        self.hard_kill_breaches.store(0, Ordering::SeqCst);
                    }
                    adopt_operator_change(state, stored);
                    self.is_tripped.store(state.tripped, Ordering::SeqCst);
                    self.operator_epoch.store(epoch, Ordering::SeqCst);
                    repo.save(state, epoch).await
                }
                Ok(None) => Ok(false),
                Err(error) => Err(error),
            },
            other => other,
        };
        match saved {
            Ok(true) => self.unsaved.store(false, Ordering::SeqCst),
            Ok(false) => {
                warn!("Circuit breaker state changed again while saving; retrying later");
                self.unsaved.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                error!(error = %e, "Failed to persist circuit breaker state");
                self.unsaved.store(true, Ordering::SeqCst);
            }
        }
    }

    fn check_loss_thresholds(
        config: &CircuitBreakerConfig,
        state: &CircuitBreakerState,
    ) -> Option<TripReason> {
        if state.daily_pnl < Decimal::ZERO && state.daily_pnl.abs() >= config.max_daily_loss {
            return Some(TripReason::DailyLossLimit);
        }
        if state.consecutive_losses >= config.max_consecutive_losses {
            return Some(TripReason::ConsecutiveLosses);
        }
        None
    }

    /// Trips the breaker. Re-evaluating an active trip is a no-op, so repeated
    /// threshold breaches cannot postpone resumption; only a more severe reason
    /// (manual, then hard kill) replaces the current one. A loss limit
    /// breached during a lesser trip (a run of losses during a drawdown
    /// cooldown, the day's loss during either) takes the trip over once, with
    /// a fresh cooldown, so losses recorded while halted are never dropped.
    /// Any trip ends gradual recovery.
    fn trip_internal(
        &self,
        state: &mut CircuitBreakerState,
        reason: TripReason,
        config: &CircuitBreakerConfig,
    ) {
        let was_tripped = state.tripped;
        if was_tripped {
            let current = state.trip_reason.clone();
            let outranks = current
                .as_ref()
                .is_none_or(|current| severity(&reason) > severity(current));
            let escalates = current.as_ref().is_some_and(|current| {
                severity(&reason) == severity(current)
                    && !latches(current)
                    && loss_rank(&reason) > loss_rank(current)
            });
            if escalates {
                let resume_at = after_minutes(Utc::now(), config.cooldown_minutes);
                state.resume_at = Some(state.resume_at.map_or(resume_at, |at| at.max(resume_at)));
                state.trip_reason = Some(reason.clone());
                warn!(
                    reason = ?reason,
                    previous = ?current,
                    daily_pnl = %state.daily_pnl,
                    consecutive_losses = state.consecutive_losses,
                    "A loss limit was breached while tripped; the trip takes its reason"
                );
                return;
            }
            if !outranks {
                return;
            }
        }

        let now = Utc::now();
        state.tripped = true;
        state.trip_reason = Some(reason.clone());
        state.tripped_at = Some(now);
        state.recovery_state = None;
        if !was_tripped {
            state.trips_today += 1;
        }
        self.is_tripped.store(true, Ordering::SeqCst);

        if latches(&reason) {
            state.resume_at = None;
            error!(
                reason = ?reason,
                daily_pnl = %state.daily_pnl,
                peak_value = %state.peak_value,
                current_value = %state.current_value,
                "Circuit breaker LATCHED - trading halted until manual reset"
            );
        } else {
            let resume_at = after_minutes(now, config.cooldown_minutes);
            state.resume_at = Some(resume_at);
            error!(
                reason = ?reason,
                resume_at = %resume_at,
                daily_pnl = %state.daily_pnl,
                consecutive_losses = %state.consecutive_losses,
                "Circuit breaker TRIPPED - trading halted"
            );
        }
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
            let reason = breaker.record_trade(Decimal::new(-10, 0)).await.unwrap();
            assert!(reason.is_none());
        }

        // This should trip
        let reason = breaker.record_trade(Decimal::new(-10, 0)).await.unwrap();
        assert_eq!(reason, Some(TripReason::DailyLossLimit));
        assert!(breaker.is_tripped());
    }

    #[tokio::test]
    async fn a_daily_loss_trip_holds_for_the_rest_of_its_day() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_daily_loss: Decimal::new(100, 0),
            max_consecutive_losses: 100,
            cooldown_minutes: 0,
            enabled: true,
            ..Default::default()
        });
        assert_eq!(
            breaker.record_trade(Decimal::new(-150, 0)).await.unwrap(),
            Some(TripReason::DailyLossLimit)
        );
        // The cooldown has passed, but the day's loss still breaches.
        assert!(!breaker.can_trade().await);
        assert!(breaker.is_tripped());
        // A new UTC day clears the day's loss: trading resumes.
        breaker.state.write().await.last_reset_date =
            Utc::now().date_naive() - chrono::Days::new(1);
        assert!(breaker.can_trade().await);
        assert!(!breaker.is_tripped());
        assert_eq!(breaker.state().await.daily_pnl, Decimal::ZERO);
    }

    #[tokio::test]
    async fn losses_recorded_during_another_trip_are_weighed_before_resuming() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_daily_loss: Decimal::new(2500, 0),
            max_consecutive_losses: 3,
            cooldown_minutes: 0,
            enabled: true,
            ..Default::default()
        });
        // Tripped for drawdown; three losing closes come in meanwhile.
        {
            let mut state = breaker.state.write().await;
            let config = breaker.config.read().await.clone();
            breaker.trip_internal(&mut state, TripReason::MaxDrawdown, &config);
        }
        for loss in [-1000, -900, -800] {
            breaker.record_trade(Decimal::from(loss)).await.unwrap();
        }
        // Over the day's limit: the trip holds, under its true reason.
        assert!(!breaker.can_trade().await);
        assert_eq!(
            breaker.state().await.trip_reason,
            Some(TripReason::DailyLossLimit)
        );

        // Within the day's limit but a run of losses: its own cooldown.
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_daily_loss: Decimal::new(100000, 0),
            max_consecutive_losses: 3,
            cooldown_minutes: 30,
            enabled: true,
            ..Default::default()
        });
        {
            let mut state = breaker.state.write().await;
            let config = breaker.config.read().await.clone();
            breaker.trip_internal(&mut state, TripReason::MaxDrawdown, &config);
            state.resume_at = Some(Utc::now());
        }
        for _ in 0..3 {
            breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        }
        assert!(!breaker.can_trade().await);
        let state = breaker.state().await;
        assert_eq!(state.trip_reason, Some(TripReason::ConsecutiveLosses));
        assert!(state.resume_at.unwrap() > Utc::now() + Duration::minutes(29));

        // A run already served is not served again after a later trip.
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_daily_loss: Decimal::new(100000, 0),
            max_consecutive_losses: 2,
            cooldown_minutes: 0,
            enabled: true,
            ..Default::default()
        });
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        assert!(breaker.can_trade().await);
        {
            let mut state = breaker.state.write().await;
            let config = breaker.config.read().await.clone();
            breaker.trip_internal(&mut state, TripReason::MaxDrawdown, &config);
        }
        assert!(breaker.can_trade().await);
    }

    #[tokio::test]
    async fn a_break_even_trade_neither_extends_nor_ends_a_losing_streak() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_consecutive_losses: 2,
            max_daily_loss: Decimal::new(100000, 0),
            enabled: true,
            ..Default::default()
        });
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        breaker.record_trade(Decimal::ZERO).await.unwrap();
        assert_eq!(breaker.state().await.consecutive_losses, 1);
        assert!(!breaker.is_tripped());
        breaker.record_trade(Decimal::ONE).await.unwrap();
        assert_eq!(breaker.state().await.consecutive_losses, 0);
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
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        assert!(!breaker.is_tripped());

        // 3rd loss - trip
        let reason = breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
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
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();

        // 1 win - resets counter
        breaker.record_trade(Decimal::new(1, 0)).await.unwrap();

        // 2 more losses - should not trip yet
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
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

        breaker.manual_trip(Some("Test".to_string())).await.unwrap();
        assert!(breaker.is_tripped());
        assert!(!breaker.can_trade().await);

        breaker.reset().await.unwrap();
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
        breaker.record_trade(Decimal::new(-1000, 0)).await.unwrap();
        assert!(!breaker.is_tripped());
        assert!(breaker.can_trade().await);
    }

    #[tokio::test]
    async fn switching_the_breaker_off_does_not_lift_a_latch() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());
        breaker.manual_trip(None).await.unwrap();
        let mut config = breaker.config().await;
        config.enabled = false;
        breaker.update_config(config).await;
        assert!(!breaker.can_trade().await);
        breaker.reset().await.unwrap();
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
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        let reason = breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
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
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
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
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.can_trade().await;

        // Cannot advance without profit
        assert!(!breaker.try_advance_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::new(25, 2));

        // Record a winning trade
        breaker.record_trade(Decimal::new(1, 0)).await.unwrap();

        // Now can advance
        assert!(breaker.try_advance_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::new(50, 2));
    }

    #[tokio::test]
    async fn test_recovery_retrip_on_loss() {
        let config = CircuitBreakerConfig {
            max_consecutive_losses: 3,
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

        // Trigger the initial trip by a run of losses ($30, within the day's limit)
        for _ in 0..3 {
            breaker.record_trade(Decimal::new(-10, 0)).await.unwrap();
        }
        assert!(breaker.is_tripped());

        // Enter recovery mode
        breaker.can_trade().await;
        assert!(breaker.is_in_recovery().await);

        // At stage 1 (25% capacity), the scaled daily limit is $25, which the
        // day's -$30 already exceeds: the next loss re-trips.
        let reason = breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();

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
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.record_trade(Decimal::new(-1, 0)).await.unwrap();
        breaker.can_trade().await;
        assert!(breaker.is_in_recovery().await);

        // Force exit recovery (complete)
        breaker.exit_recovery(true).await;
        assert!(!breaker.is_in_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::ONE);
    }

    fn latching_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            max_drawdown_pct: Decimal::new(20, 2),
            hard_kill_drawdown_pct: Decimal::new(40, 2),
            max_consecutive_losses: 2,
            max_daily_loss: Decimal::new(100000, 0),
            cooldown_minutes: 0,
            gradual_recovery_enabled: true,
            recovery_stages: 3,
            recovery_stage_minutes: 60,
            enabled: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_held_reading_trips_only_on_a_loss_its_best_case_shows() {
        let breaker = CircuitBreaker::new(latching_config());
        // Nothing to compare with before equity is first seen.
        assert_eq!(
            breaker.check_drawdown_bound(Decimal::ZERO).await.unwrap(),
            None
        );
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        // 780 read while 50 may have left: at best 830, a 17% drawdown.
        assert_eq!(
            breaker
                .check_drawdown_bound(Decimal::new(830, 0))
                .await
                .unwrap(),
            None
        );
        // A best case above the peak raises nothing.
        breaker
            .check_drawdown_bound(Decimal::new(5000, 0))
            .await
            .unwrap();
        let state = breaker.state().await;
        assert_eq!(state.peak_value, Decimal::new(1000, 0));
        assert_eq!(state.current_value, Decimal::new(1000, 0));
        assert!(!state.tripped);
        // At best 790: a 21% loss whatever the withdrawals did.
        assert_eq!(
            breaker
                .check_drawdown_bound(Decimal::new(790, 0))
                .await
                .unwrap(),
            Some(TripReason::MaxDrawdown)
        );
        assert!(breaker.state().await.tripped);
    }

    #[tokio::test]
    async fn a_held_reading_does_not_reset_hard_kill_confirmation() {
        let breaker = CircuitBreaker::new(latching_config());
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        // A 45% loss once: one confirmation of the 40% hard kill.
        breaker
            .update_portfolio_value(Decimal::new(550, 0))
            .await
            .unwrap();
        // A held reading whose best case shows no breach proves nothing.
        breaker
            .check_drawdown_bound(Decimal::new(900, 0))
            .await
            .unwrap();
        // The next breach confirms the hard kill.
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(550, 0))
                .await
                .unwrap(),
            Some(TripReason::HardKillSwitch)
        );
    }

    async fn enter_recovery(breaker: &CircuitBreaker) {
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        assert!(breaker.is_tripped());
        assert!(breaker.can_trade().await);
        assert!(breaker.is_in_recovery().await);
    }

    #[tokio::test]
    async fn trips_block_trading_during_recovery() {
        let breaker = CircuitBreaker::new(latching_config());
        enter_recovery(&breaker).await;

        breaker.manual_trip(None).await.unwrap();
        assert!(!breaker.can_trade().await);
        assert!(!breaker.is_in_recovery().await);

        breaker.reset().await.unwrap();
        enter_recovery(&breaker).await;
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        let reason = breaker
            .update_portfolio_value(Decimal::new(750, 0))
            .await
            .unwrap();
        assert_eq!(reason, Some(TripReason::MaxDrawdown));
        assert!(!breaker.is_in_recovery().await);
        // Zero cooldown: trading resumes in a fresh recovery, never mid-trip.
        assert!(breaker.can_trade().await);
    }

    #[tokio::test]
    async fn manual_trip_latches_until_reset() {
        let breaker = CircuitBreaker::new(latching_config());
        breaker.manual_trip(Some("operator".into())).await.unwrap();

        let state = breaker.state().await;
        assert_eq!(state.resume_at, None);
        assert!(!breaker.can_trade().await);

        // A milder breach cannot downgrade the latch to an auto-resuming trip.
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        assert_eq!(breaker.state().await.trip_reason, Some(TripReason::Manual));
        assert!(!breaker.can_trade().await);

        breaker.reset().await.unwrap();
        assert!(breaker.can_trade().await);
    }

    #[tokio::test]
    async fn hard_kill_needs_confirmation_and_then_stays_latched() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            cooldown_minutes: 30,
            ..latching_config()
        });
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();

        // First breach halts with a max-drawdown trip.
        let first = breaker
            .update_portfolio_value(Decimal::new(550, 0))
            .await
            .unwrap();
        assert_eq!(first, Some(TripReason::MaxDrawdown));
        assert!(!breaker.can_trade().await);

        // A second consecutive breach latches.
        let second = breaker
            .update_portfolio_value(Decimal::new(560, 0))
            .await
            .unwrap();
        assert_eq!(second, Some(TripReason::HardKillSwitch));
        assert_eq!(breaker.state().await.resume_at, None);

        // Partial recovery to a 30% drawdown cannot replace the latch.
        breaker
            .update_portfolio_value(Decimal::new(700, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(990, 0))
            .await
            .unwrap();
        let state = breaker.state().await;
        assert_eq!(state.trip_reason, Some(TripReason::HardKillSwitch));
        assert!(!breaker.can_trade().await);
    }

    #[tokio::test]
    async fn single_bad_mark_does_not_latch() {
        let breaker = CircuitBreaker::new(latching_config());
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(100, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(100, 0))
            .await
            .unwrap();
        assert_eq!(
            breaker.state().await.trip_reason,
            Some(TripReason::MaxDrawdown)
        );
    }

    #[tokio::test]
    async fn repeated_breaches_do_not_extend_the_cooldown() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            cooldown_minutes: 30,
            ..latching_config()
        });
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(780, 0))
            .await
            .unwrap();
        let first = breaker.state().await;
        // The same breach again neither extends nor restarts the trip.
        breaker
            .update_portfolio_value(Decimal::new(770, 0))
            .await
            .unwrap();
        let again = breaker.state().await;
        assert_eq!(again.trip_reason, Some(TripReason::MaxDrawdown));
        assert_eq!(again.resume_at, first.resume_at);
        assert_eq!(again.tripped_at, first.tripped_at);

        // A run of losses reaching its limit meanwhile takes the trip over,
        // once, with its own cooldown; more losses do not extend it.
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        let taken = breaker.state().await;
        assert_eq!(taken.trip_reason, Some(TripReason::ConsecutiveLosses));
        assert!(taken.resume_at >= first.resume_at);
        breaker.record_trade(Decimal::NEGATIVE_ONE).await.unwrap();
        let later = breaker.state().await;
        assert_eq!(later.resume_at, taken.resume_at);
        assert_eq!(later.tripped_at, first.tripped_at);
        assert_eq!(later.trips_today, 1);
    }

    #[test]
    fn limits_that_cannot_guard_trading_are_refused() {
        assert!(CircuitBreakerConfig::default().validate().is_ok());
        let percent = CircuitBreakerConfig {
            max_drawdown_pct: Decimal::from(20),
            ..Default::default()
        };
        assert!(percent.validate().is_err());
        let hard_kill_percent = CircuitBreakerConfig {
            hard_kill_drawdown_pct: Decimal::from(40),
            ..Default::default()
        };
        assert!(hard_kill_percent.validate().is_err());
        let hard_kill_off = CircuitBreakerConfig {
            hard_kill_drawdown_pct: Decimal::ZERO,
            ..Default::default()
        };
        assert!(hard_kill_off.validate().is_ok());
        let no_losses = CircuitBreakerConfig {
            max_consecutive_losses: 0,
            ..Default::default()
        };
        assert!(no_losses.validate().is_err());
        let negative = CircuitBreakerConfig {
            cooldown_minutes: -1,
            ..Default::default()
        };
        assert!(negative.validate().is_err());
    }

    #[tokio::test]
    async fn an_absurd_cooldown_is_held_to_a_week() {
        let now = Utc::now();
        assert_eq!(
            after_minutes(now, i64::MAX),
            now + Duration::minutes(MAX_WAIT_MINUTES)
        );
        assert_eq!(after_minutes(now, -5), now);
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            cooldown_minutes: 1_000_000_000_000,
            ..latching_config()
        });
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(780, 0))
            .await
            .unwrap();
        let state = breaker.state().await;
        assert!(state.tripped);
        assert!(state.resume_at.unwrap() <= Utc::now() + Duration::minutes(MAX_WAIT_MINUTES));
    }

    #[tokio::test]
    async fn drawdown_resume_rebases_soft_reference_but_not_hard_kill() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            gradual_recovery_enabled: false,
            ..latching_config()
        });
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(780, 0))
                .await
                .unwrap(),
            Some(TripReason::MaxDrawdown)
        );
        assert!(breaker.can_trade().await);

        // 780 is the new soft reference: 10% further is not a soft trip...
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(705, 0))
                .await
                .unwrap(),
            None
        );
        // ...but the hard kill still measures from the 1000 high-water mark.
        breaker
            .update_portfolio_value(Decimal::new(590, 0))
            .await
            .unwrap();
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(590, 0))
                .await
                .unwrap(),
            Some(TripReason::HardKillSwitch)
        );
    }

    #[tokio::test]
    async fn manual_reset_accepts_current_equity_as_baseline() {
        let breaker = CircuitBreaker::new(latching_config());
        breaker
            .update_portfolio_value(Decimal::new(1000, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(500, 0))
            .await
            .unwrap();
        breaker
            .update_portfolio_value(Decimal::new(500, 0))
            .await
            .unwrap();
        assert_eq!(
            breaker.state().await.trip_reason,
            Some(TripReason::HardKillSwitch)
        );

        breaker.reset().await.unwrap();
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(500, 0))
                .await
                .unwrap(),
            None
        );
        assert!(breaker.can_trade().await);
        let state = breaker.state().await;
        assert_eq!(state.peak_value, Decimal::new(500, 0));
        assert_eq!(state.drawdown_peak, Decimal::new(500, 0));
    }

    #[tokio::test]
    async fn withdrawals_are_not_drawdown() {
        let breaker = CircuitBreaker::new(latching_config());
        let t0 = Utc::now();
        breaker
            .update_portfolio_value_with_flows(Decimal::new(1000, 0), Decimal::ZERO, Some(t0))
            .await
            .unwrap();

        let reason = breaker
            .update_portfolio_value_with_flows(
                Decimal::new(550, 0),
                Decimal::new(-450, 0),
                Some(t0 + Duration::minutes(5)),
            )
            .await
            .unwrap();
        assert_eq!(reason, None);
        let state = breaker.state().await;
        assert_eq!(state.peak_value, Decimal::new(550, 0));
        assert_eq!(
            state.cash_flows_applied_through,
            Some(t0 + Duration::minutes(5))
        );

        // A later 25% trading loss still trips.
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(410, 0))
                .await
                .unwrap(),
            Some(TripReason::MaxDrawdown)
        );
    }

    #[tokio::test]
    async fn realized_pnl_without_equity_never_trips_drawdown() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_consecutive_losses: 100,
            ..latching_config()
        });
        breaker.record_trade(Decimal::new(5, 1)).await.unwrap();
        let reason = breaker.record_trade(Decimal::new(-2, 1)).await.unwrap();
        assert_eq!(reason, None);
        assert!(breaker.can_trade().await);
        let state = breaker.state().await;
        assert_eq!(state.peak_value, Decimal::ZERO);
        assert_eq!(state.current_value, Decimal::ZERO);
    }

    #[tokio::test]
    async fn first_equity_snapshot_replaces_unseeded_references() {
        let breaker = CircuitBreaker::new(latching_config());
        {
            let mut state = breaker.state.write().await;
            state.peak_value = Decimal::new(5000, 0);
            state.drawdown_peak = Decimal::new(5000, 0);
        }
        assert_eq!(
            breaker
                .update_portfolio_value(Decimal::new(500, 0))
                .await
                .unwrap(),
            None
        );
        assert_eq!(breaker.state().await.peak_value, Decimal::new(500, 0));
    }

    #[tokio::test]
    async fn recovery_advances_as_trading_continues() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            recovery_stage_minutes: 0,
            ..latching_config()
        });
        enter_recovery(&breaker).await;
        assert_eq!(
            breaker.trading_capacity().await,
            Decimal::ONE / Decimal::from(3)
        );

        for _ in 0..3 {
            assert!(breaker.can_trade().await);
        }
        assert!(!breaker.is_in_recovery().await);
        assert_eq!(breaker.trading_capacity().await, Decimal::ONE);
    }
}
