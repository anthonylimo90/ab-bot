//! Auto-optimizer background service for automatic wallet rotation.
//!
//! This service implements a fully automated wallet selection system:
//! - Auto-Select: Fills empty Active slots with best-performing candidates
//! - Auto-Drop: Demotes wallets failing thresholds (immediate or grace period)
//! - Auto-Swap: Replaces underperformers with better candidates
//! - Confidence-Weighted Allocation: Uses AdvancedPredictor for allocation weights
//! - Probation System: New wallets start at 50% allocation for 7 days
//! - Pin/Ban Support: User overrides for automation behavior

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{debug, info, warn};
use trading_engine::copy_trader::CopyTrader;
use uuid::Uuid;
use wallet_tracker::trade_monitor::TradeMonitor;

use crate::runtime_sync::reconcile_copy_runtime;
use crate::schema::wallet_features_has_strategy_type;

/// Allocation strategy for workspace
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AllocationStrategy {
    /// Equal allocation across all active wallets
    Equal,
    /// Weighted by prediction confidence (default)
    #[default]
    ConfidenceWeighted,
    /// Weighted by recent performance
    Performance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_percent_and_ratio_inputs() {
        assert!((AutoOptimizer::normalize_ratio(50.0) - 0.5).abs() < f64::EPSILON);
        assert!((AutoOptimizer::normalize_ratio(5.0) - 0.05).abs() < f64::EPSILON);
        assert!((AutoOptimizer::normalize_ratio(0.5) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn relax_thresholds_expands_candidate_window() {
        let base = CandidateThresholds {
            min_roi: 0.05,
            min_sharpe: 1.0,
            min_win_rate: 0.55,
            min_trades: 12,
            max_drawdown: 0.20,
        };

        let relaxed = AutoOptimizer::relax_thresholds(&base, 2);
        assert!(relaxed.min_roi <= base.min_roi);
        assert!(relaxed.min_sharpe <= base.min_sharpe);
        assert!(relaxed.min_win_rate <= base.min_win_rate);
        assert!(relaxed.min_trades <= base.min_trades);
        assert!(relaxed.max_drawdown >= base.max_drawdown);
    }

    #[test]
    fn exploration_score_prefers_upside_with_lower_confidence() {
        let conservative = WalletCompositeScore {
            address: "0x1".to_string(),
            total_score: 75.0,
            roi_score: 60.0,
            sharpe_score: 70.0,
            win_rate_score: 65.0,
            consistency_score: 80.0,
            confidence: 0.9,
            strategy_type: None,
        };
        let exploratory = WalletCompositeScore {
            address: "0x2".to_string(),
            total_score: 74.0,
            roi_score: 82.0,
            sharpe_score: 65.0,
            win_rate_score: 62.0,
            consistency_score: 70.0,
            confidence: 0.35,
            strategy_type: None,
        };

        assert!(
            AutoOptimizer::exploration_score(&exploratory)
                > AutoOptimizer::exploration_score(&conservative)
        );
    }

    #[test]
    fn normalize_ratio_edge_cases() {
        // Zero stays zero
        assert_eq!(AutoOptimizer::normalize_ratio(0.0), 0.0);
        // 100.0 => 1.0 (percentage to ratio)
        assert!((AutoOptimizer::normalize_ratio(100.0) - 1.0).abs() < f64::EPSILON);
        // 1000.0 => 10.0 (still divides by 100)
        assert!((AutoOptimizer::normalize_ratio(1000.0) - 10.0).abs() < f64::EPSILON);
        // Negative percentage
        assert!((AutoOptimizer::normalize_ratio(-50.0) - (-0.5)).abs() < f64::EPSILON);
        // Values <= 1.0 stay as-is
        assert!((AutoOptimizer::normalize_ratio(0.99) - 0.99).abs() < f64::EPSILON);
        assert!((AutoOptimizer::normalize_ratio(-0.5) - (-0.5)).abs() < f64::EPSILON);
    }

    #[test]
    fn relax_thresholds_multiple_rounds() {
        let base = CandidateThresholds {
            min_roi: 0.05,
            min_sharpe: 1.0,
            min_win_rate: 0.55,
            min_trades: 12,
            max_drawdown: 0.20,
        };

        // Round 0 (default/catch-all)
        let r0 = AutoOptimizer::relax_thresholds(&base, 0);
        assert!(r0.min_roi < base.min_roi, "Round 0 should relax ROI");
        assert!(
            r0.max_drawdown > base.max_drawdown,
            "Round 0 should relax drawdown"
        );

        // Round 1
        let r1 = AutoOptimizer::relax_thresholds(&base, 1);
        assert!((r1.min_roi - 0.03).abs() < f64::EPSILON);
        assert!((r1.min_sharpe - 0.85).abs() < f64::EPSILON);
        assert_eq!(r1.min_trades, 6);

        // Round 5 (falls into catch-all _ branch)
        let r5 = AutoOptimizer::relax_thresholds(&base, 5);
        assert!(
            r5.min_roi <= r1.min_roi,
            "Higher rounds should be more relaxed than round 1"
        );
        assert!(r5.max_drawdown >= r1.max_drawdown);
        assert_eq!(r5.min_trades, 1);
    }

    #[test]
    fn exploration_score_equal_total_score_tie_break() {
        // Two wallets with equal total_score but different confidence
        let high_conf = WalletCompositeScore {
            address: "0xa".to_string(),
            total_score: 70.0,
            roi_score: 70.0,
            sharpe_score: 70.0,
            win_rate_score: 70.0,
            consistency_score: 70.0,
            confidence: 0.9,
            strategy_type: None,
        };
        let low_conf = WalletCompositeScore {
            confidence: 0.3,
            ..high_conf.clone()
        };

        // Lower confidence should get a higher exploration score
        // because of the (1.0 - confidence) * 10.0 bonus
        assert!(
            AutoOptimizer::exploration_score(&low_conf)
                > AutoOptimizer::exploration_score(&high_conf),
            "Lower confidence should win exploration tie-break"
        );
    }

    #[test]
    fn allocation_strategy_round_trip() {
        use std::str::FromStr;

        let strategies = vec![
            (AllocationStrategy::Equal, "equal"),
            (
                AllocationStrategy::ConfidenceWeighted,
                "confidence_weighted",
            ),
            (AllocationStrategy::Performance, "performance"),
        ];

        for (strategy, expected_str) in strategies {
            let display = strategy.to_string();
            assert_eq!(display, expected_str);

            let parsed = AllocationStrategy::from_str(&display).unwrap();
            assert_eq!(parsed, strategy);
        }

        // Invalid string should error
        assert!(AllocationStrategy::from_str("invalid").is_err());
    }
}

impl std::fmt::Display for AllocationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Equal => write!(f, "equal"),
            Self::ConfidenceWeighted => write!(f, "confidence_weighted"),
            Self::Performance => write!(f, "performance"),
        }
    }
}

impl std::str::FromStr for AllocationStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "equal" => Ok(Self::Equal),
            "confidence_weighted" => Ok(Self::ConfidenceWeighted),
            "performance" => Ok(Self::Performance),
            _ => Err(format!("Invalid allocation strategy: {}", s)),
        }
    }
}

/// Events that can trigger automation actions
#[derive(Debug, Clone)]
pub enum AutomationEvent {
    /// Position closed - update metrics and check thresholds
    PositionClosed {
        workspace_id: Uuid,
        wallet_address: String,
        pnl: Decimal,
        is_win: bool,
    },
    /// Circuit breaker tripped - immediate demotion
    CircuitBreakerTripped {
        workspace_id: Uuid,
        wallet_address: String,
        reason: String,
    },
    /// Metrics updated (hourly batch)
    MetricsUpdated { workspace_id: Uuid },
    /// Manual user action completed
    ManualAction {
        workspace_id: Uuid,
        action: String,
        wallet_address: String,
    },
    /// New workspace created - auto-select initial wallets
    WorkspaceCreated { workspace_id: Uuid },
}

/// Workspace settings for auto-optimization (extended).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkspaceOptimizationSettings {
    pub id: Uuid,
    pub name: String,
    pub copy_trading_enabled: bool,
    // Legacy settings
    pub auto_optimize_enabled: bool,
    pub optimization_interval_hours: i32,
    // Thresholds
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,
    // New automation settings
    pub auto_select_enabled: Option<bool>,
    pub auto_demote_enabled: Option<bool>,
    pub probation_days: Option<i32>,
    pub max_pinned_wallets: Option<i32>,
    pub allocation_strategy: Option<String>,
    pub max_drawdown_pct: Option<Decimal>,
    pub inactivity_days: Option<i32>,
}

/// Wallet metrics from the discovery table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WalletCandidate {
    pub address: String,
    pub roi_30d: Option<Decimal>,
    pub sharpe_30d: Option<Decimal>,
    pub win_rate_30d: Option<Decimal>,
    pub trade_count_30d: Option<i32>,
    pub max_drawdown_30d: Option<Decimal>,
    pub last_trade_at: Option<DateTime<Utc>>,
    /// Recency-adjusted ROI blending 30d (70%) and 90d (30%) metrics.
    pub recency_adjusted_roi: Option<Decimal>,
    /// Days since last metrics computation or trade activity.
    pub staleness_days: Option<f64>,
    /// Copy trade win rate from actual execution history (if available).
    /// When this diverges significantly from reported win_rate_30d, it signals
    /// that the wallet's metrics don't translate well to copy trading.
    pub copy_win_rate: Option<f64>,
    /// Classified trading strategy (e.g., "Arbitrage", "Momentum", "Unknown").
    /// Used for diversity-aware selection to avoid correlated picks.
    pub strategy_type: Option<String>,
}

#[derive(Debug, Clone)]
struct CandidateThresholds {
    min_roi: f64,
    min_sharpe: f64,
    min_win_rate: f64,
    min_trades: i32,
    max_drawdown: f64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct WalletMetricSnapshot {
    roi_30d: Option<Decimal>,
    sharpe_30d: Option<Decimal>,
    win_rate_30d: Option<Decimal>,
    confidence_score: Option<Decimal>,
}

/// Current allocation in a workspace (extended).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CurrentAllocation {
    pub id: Uuid,
    pub wallet_address: String,
    pub tier: String,
    pub allocation_pct: Decimal,
    pub backtest_roi: Option<Decimal>,
    pub backtest_sharpe: Option<Decimal>,
    pub backtest_win_rate: Option<Decimal>,
    // Pin status
    pub pinned: Option<bool>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub pinned_by: Option<Uuid>,
    // Probation status
    pub probation_until: Option<DateTime<Utc>>,
    pub probation_allocation_pct: Option<Decimal>,
    // Loss tracking
    pub consecutive_losses: Option<i32>,
    pub last_loss_at: Option<DateTime<Utc>>,
    // Confidence score
    pub confidence_score: Option<Decimal>,
    // Grace period tracking
    pub grace_period_started_at: Option<DateTime<Utc>>,
    pub grace_period_reason: Option<String>,
    // Auto-assignment
    pub auto_assigned: bool,
}

/// Wallet ban record
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WalletBan {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub wallet_address: String,
    pub reason: Option<String>,
    pub banned_by: Option<Uuid>,
    pub banned_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Demotion trigger types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemotionTrigger {
    /// 5+ consecutive losses
    ConsecutiveLosses,
    /// Max drawdown > 30%
    MaxDrawdown,
    /// Circuit breaker trip
    CircuitBreaker,
    /// ROI < 0% for 48h
    NegativeRoi,
    /// Sharpe < 0.5 for 24h
    LowSharpe,
    /// No trades in 14 days
    Inactivity,
    /// Manual user action
    ManualDemote,
    /// Probation failed
    ProbationFailed,
}

impl DemotionTrigger {
    /// Whether this trigger requires immediate action (no grace period)
    pub fn is_immediate(&self) -> bool {
        matches!(
            self,
            DemotionTrigger::ConsecutiveLosses
                | DemotionTrigger::MaxDrawdown
                | DemotionTrigger::CircuitBreaker
                | DemotionTrigger::ManualDemote
        )
    }

    /// Get the grace period in hours for non-immediate triggers.
    /// Returns defaults; use AutoOptimizerConfig for configurable values.
    pub fn grace_period_hours(&self) -> Option<i64> {
        match self {
            DemotionTrigger::NegativeRoi => Some(72),
            DemotionTrigger::LowSharpe => Some(48),
            DemotionTrigger::Inactivity => None, // No grace period, but not immediate
            _ => None,
        }
    }
}

/// Composite score for wallet ranking
#[derive(Debug, Clone, Serialize)]
pub struct WalletCompositeScore {
    pub address: String,
    pub total_score: f64,
    pub roi_score: f64,         // 30% weight
    pub sharpe_score: f64,      // 25% weight
    pub win_rate_score: f64,    // 25% weight
    pub consistency_score: f64, // 20% weight
    pub confidence: f64,
    /// Classified strategy type for diversity-aware selection.
    pub strategy_type: Option<String>,
}

/// Automation action result for history logging
#[derive(Debug, Clone, Serialize)]
pub struct AutomationAction {
    pub action: String,
    pub wallet_in: Option<String>,
    pub wallet_out: Option<String>,
    pub reason: String,
    pub trigger: Option<DemotionTrigger>,
    pub metrics_snapshot: serde_json::Value,
    pub undo_expires_at: Option<DateTime<Utc>>,
}

/// Rotation action to record.
#[derive(Debug, Clone, Serialize)]
pub struct RotationRecord {
    pub workspace_id: Uuid,
    pub action: String,
    pub wallet_in: Option<String>,
    pub wallet_out: Option<String>,
    pub reason: String,
    pub evidence: serde_json::Value,
}

/// Default thresholds for promotion criteria
pub const DEFAULT_MIN_ROI_30D: f64 = 0.05;
pub const DEFAULT_MIN_SHARPE: f64 = 1.0;
pub const DEFAULT_MIN_WIN_RATE: f64 = 0.50;
pub const DEFAULT_MIN_TRADES: i32 = 10;
pub const DEFAULT_MAX_DRAWDOWN: f64 = 0.20;

/// Configuration for auto-optimizer demotion thresholds and allocation limits.
#[derive(Debug, Clone)]
pub struct AutoOptimizerConfig {
    /// Max consecutive losses before immediate demotion.
    pub demotion_max_consecutive_losses: i32,
    /// Max drawdown percentage before immediate demotion.
    pub demotion_max_drawdown_pct: f64,
    /// ROI threshold below which grace period starts.
    pub demotion_grace_roi_threshold: f64,
    /// Sharpe threshold below which grace period starts.
    pub demotion_grace_sharpe_threshold: f64,
    /// Grace period hours for negative ROI trigger.
    pub grace_period_negative_roi_hours: i64,
    /// Grace period hours for low Sharpe trigger.
    pub grace_period_low_sharpe_hours: i64,
    /// Minimum allocation percentage per wallet.
    pub min_allocation_pct: f64,
    /// Maximum allocation percentage per wallet.
    pub max_allocation_pct: f64,
    /// Whether to progressively relax thresholds when candidate pool is thin.
    pub auto_relax_thresholds: bool,
    /// Number of relaxation rounds to apply.
    pub max_relaxation_rounds: usize,
    /// Number of slots reserved for high-upside exploration candidates.
    pub exploration_slots: usize,
    /// Minimum confidence required for exploration picks.
    pub min_exploration_confidence: f64,
}

impl Default for AutoOptimizerConfig {
    fn default() -> Self {
        Self {
            demotion_max_consecutive_losses: 8,
            demotion_max_drawdown_pct: 0.40,
            demotion_grace_roi_threshold: -0.05,
            demotion_grace_sharpe_threshold: 0.3,
            grace_period_negative_roi_hours: 72,
            grace_period_low_sharpe_hours: 48,
            min_allocation_pct: 5.0,
            max_allocation_pct: 50.0,
            auto_relax_thresholds: true,
            max_relaxation_rounds: 3,
            exploration_slots: 1,
            min_exploration_confidence: 0.15,
        }
    }
}

/// Auto-optimizer service with event-driven automation.
pub struct AutoOptimizer {
    pool: PgPool,
    config: AutoOptimizerConfig,
    event_sender: Option<mpsc::Sender<AutomationEvent>>,
    trade_monitor: Option<Arc<TradeMonitor>>,
    copy_trader: Option<Arc<RwLock<CopyTrader>>>,
}

impl Default for AutoOptimizer {
    fn default() -> Self {
        panic!("AutoOptimizer requires a database pool")
    }
}

impl AutoOptimizer {
    /// Create a new auto-optimizer with default config.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            config: AutoOptimizerConfig::default(),
            event_sender: None,
            trade_monitor: None,
            copy_trader: None,
        }
    }

    /// Create a new auto-optimizer with custom config.
    pub fn new_with_config(pool: PgPool, config: AutoOptimizerConfig) -> Self {
        Self {
            pool,
            config,
            event_sender: None,
            trade_monitor: None,
            copy_trader: None,
        }
    }

    /// Create with event channel for event-driven automation.
    pub fn with_event_channel(pool: PgPool) -> (Self, mpsc::Receiver<AutomationEvent>) {
        let (tx, rx) = mpsc::channel(100);
        (
            Self {
                pool,
                config: AutoOptimizerConfig::default(),
                event_sender: Some(tx),
                trade_monitor: None,
                copy_trader: None,
            },
            rx,
        )
    }

    /// Attach live runtime handles so optimizer mutations can sync monitor/trader state.
    pub fn with_runtime_handles(
        mut self,
        trade_monitor: Option<Arc<TradeMonitor>>,
        copy_trader: Option<Arc<RwLock<CopyTrader>>>,
    ) -> Self {
        self.trade_monitor = trade_monitor;
        self.copy_trader = copy_trader;
        self
    }

    /// Get event sender for external event emission.
    pub fn event_sender(&self) -> Option<mpsc::Sender<AutomationEvent>> {
        self.event_sender.clone()
    }

    async fn reconcile_runtime_if_attached(&self) -> anyhow::Result<()> {
        if self.trade_monitor.is_none() && self.copy_trader.is_none() {
            return Ok(());
        }

        reconcile_copy_runtime(
            &self.pool,
            self.trade_monitor.as_ref(),
            self.copy_trader.as_ref(),
        )
        .await
        .map(|_| ())
    }

    /// Start the background optimization loop with event handling.
    pub async fn start(self: Arc<Self>, mut event_rx: Option<mpsc::Receiver<AutomationEvent>>) {
        // Run scheduled optimization on a configurable interval (default 15 min)
        let interval_secs = std::env::var("AUTO_ROTATION_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(900u64);
        let mut ticker = interval(TokioDuration::from_secs(interval_secs));

        info!("Auto-optimizer started");

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = self.run_scheduled_optimizations().await {
                        warn!(error = %e, "Auto-optimization cycle failed");
                    }
                }
                Some(event) = async {
                    if let Some(ref mut rx) = event_rx {
                        rx.recv().await
                    } else {
                        std::future::pending::<Option<AutomationEvent>>().await
                    }
                } => {
                    if let Err(e) = self.handle_event(event).await {
                        warn!(error = %e, "Failed to handle automation event");
                    }
                }
            }
        }
    }

    /// Handle an automation event.
    pub async fn handle_event(&self, event: AutomationEvent) -> anyhow::Result<()> {
        match event {
            AutomationEvent::PositionClosed {
                workspace_id,
                wallet_address,
                pnl,
                is_win,
            } => {
                self.handle_position_closed(workspace_id, &wallet_address, pnl, is_win)
                    .await?;
            }
            AutomationEvent::CircuitBreakerTripped {
                workspace_id,
                wallet_address,
                reason,
            } => {
                self.handle_circuit_breaker_trip(workspace_id, &wallet_address, &reason)
                    .await?;
            }
            AutomationEvent::MetricsUpdated { workspace_id } => {
                let workspace = self.get_workspace_settings(workspace_id).await?;
                self.optimize_workspace(&workspace).await?;
            }
            AutomationEvent::ManualAction { workspace_id, .. } => {
                // Manual actions might trigger fill-up of empty slots
                let workspace = self.get_workspace_settings(workspace_id).await?;
                self.fill_empty_slots(&workspace).await?;
            }
            AutomationEvent::WorkspaceCreated { workspace_id } => {
                self.handle_workspace_created(workspace_id).await?;
            }
        }
        self.reconcile_runtime_if_attached().await?;
        Ok(())
    }

    /// Run optimization for all eligible workspaces.
    pub async fn run_scheduled_optimizations(&self) -> anyhow::Result<()> {
        info!("Starting auto-optimization cycle");

        let workspaces = self.get_eligible_workspaces().await?;
        info!(count = workspaces.len(), "Found eligible workspaces");

        for workspace in workspaces {
            if let Err(e) = self.optimize_workspace(&workspace).await {
                warn!(
                    workspace_id = %workspace.id,
                    workspace_name = %workspace.name,
                    error = %e,
                    "Failed to optimize workspace"
                );
            }
        }

        // Process probation graduations
        self.process_probation_graduations().await?;

        // Process grace period expirations
        self.process_grace_period_expirations().await?;

        self.reconcile_runtime_if_attached().await?;

        info!("Auto-optimization cycle complete");
        Ok(())
    }

    /// Get workspaces that are due for optimization.
    async fn get_eligible_workspaces(&self) -> anyhow::Result<Vec<WorkspaceOptimizationSettings>> {
        let workspaces: Vec<WorkspaceOptimizationSettings> = sqlx::query_as(
            r#"
            SELECT id, name, COALESCE(copy_trading_enabled, TRUE) as copy_trading_enabled,
                   auto_optimize_enabled, optimization_interval_hours,
                   min_roi_30d, min_sharpe, min_win_rate, min_trades_30d,
                   auto_select_enabled, auto_demote_enabled, probation_days,
                   max_pinned_wallets, allocation_strategy, max_drawdown_pct, inactivity_days
            FROM workspaces
            WHERE auto_optimize_enabled = true
               OR COALESCE(auto_select_enabled, true) = true
               OR COALESCE(auto_demote_enabled, true) = true
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(workspaces)
    }

    /// Get settings for a specific workspace.
    async fn get_workspace_settings(
        &self,
        workspace_id: Uuid,
    ) -> anyhow::Result<WorkspaceOptimizationSettings> {
        let workspace: WorkspaceOptimizationSettings = sqlx::query_as(
            r#"
            SELECT id, name, COALESCE(copy_trading_enabled, TRUE) as copy_trading_enabled,
                   auto_optimize_enabled, optimization_interval_hours,
                   min_roi_30d, min_sharpe, min_win_rate, min_trades_30d,
                   auto_select_enabled, auto_demote_enabled, probation_days,
                   max_pinned_wallets, allocation_strategy, max_drawdown_pct, inactivity_days
            FROM workspaces
            WHERE id = $1
            "#,
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(workspace)
    }

    /// Optimize a single workspace.
    async fn optimize_workspace(
        &self,
        workspace: &WorkspaceOptimizationSettings,
    ) -> anyhow::Result<()> {
        if !workspace.copy_trading_enabled {
            debug!(
                workspace_id = %workspace.id,
                workspace_name = %workspace.name,
                "Skipping optimization because copy_trading_enabled=false"
            );
            return Ok(());
        }

        debug!(
            workspace_id = %workspace.id,
            workspace_name = %workspace.name,
            "Optimizing workspace"
        );

        let auto_select = workspace.auto_select_enabled.unwrap_or(true);
        let auto_demote = workspace.auto_demote_enabled.unwrap_or(true);

        // Get current allocations
        let current = self.get_current_allocations(workspace.id).await?;
        let active_wallets: Vec<_> = current.iter().filter(|a| a.tier == "active").collect();
        let _active_count = active_wallets.len();

        // Step 1: Check for wallets that need demotion (if auto_demote enabled)
        if auto_demote {
            for allocation in &active_wallets {
                // Skip pinned wallets
                if allocation.pinned.unwrap_or(false) {
                    debug!(
                        wallet = %allocation.wallet_address,
                        "Skipping pinned wallet for demotion check"
                    );
                    continue;
                }

                // Check for demotion triggers
                if let Some(trigger) = self.check_demotion_triggers(allocation, workspace).await? {
                    if trigger.is_immediate() {
                        self.demote_wallet(workspace.id, &allocation.wallet_address, trigger, None)
                            .await?;
                    } else if allocation.grace_period_started_at.is_none() {
                        // Start grace period
                        self.start_grace_period(workspace.id, &allocation.wallet_address, trigger)
                            .await?;
                    }
                }
            }
        }

        // Step 2: Fill empty slots (if auto_select enabled)
        if auto_select {
            self.fill_empty_slots(workspace).await?;
        }

        // Step 3: Recalculate allocations based on strategy
        self.recalculate_allocations(workspace).await?;

        // Update last_optimization_at timestamp
        sqlx::query("UPDATE workspaces SET last_optimization_at = NOW() WHERE id = $1")
            .bind(workspace.id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Fill empty active slots with best candidates.
    async fn fill_empty_slots(
        &self,
        workspace: &WorkspaceOptimizationSettings,
    ) -> anyhow::Result<()> {
        let current = self.get_current_allocations(workspace.id).await?;
        let active_count = current.iter().filter(|a| a.tier == "active").count();
        let empty_slots = 5_usize.saturating_sub(active_count);

        info!(
            workspace_id = %workspace.id,
            active_count = active_count,
            empty_slots = empty_slots,
            "Checking for empty slots to fill"
        );

        if empty_slots == 0 {
            info!(workspace_id = %workspace.id, "No empty slots - roster is full");
            return Ok(());
        }

        // Get banned wallets for this workspace
        let banned = self.get_banned_wallets(workspace.id).await?;
        let banned_addresses: Vec<&str> =
            banned.iter().map(|b| b.wallet_address.as_str()).collect();

        // Get current roster addresses
        let current_addresses: Vec<&str> =
            current.iter().map(|a| a.wallet_address.as_str()).collect();

        // Get candidate wallets with adaptive relaxation when needed.
        let candidate_target = empty_slots + self.config.exploration_slots + 2;
        let candidates = self
            .get_candidate_wallets(workspace, candidate_target)
            .await?;
        info!(
            workspace_id = %workspace.id,
            candidate_count = candidates.len(),
            "Found candidate wallets for slot filling"
        );

        // Filter out banned and already-in-roster wallets
        let available_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| {
                !banned_addresses.contains(&c.address.as_str())
                    && !current_addresses.contains(&c.address.as_str())
            })
            .collect();

        info!(
            workspace_id = %workspace.id,
            available_count = available_candidates.len(),
            banned_count = banned.len(),
            roster_count = current.len(),
            "Filtered available candidates"
        );

        // Rank candidates by composite score
        let ranked = self.rank_candidates(&available_candidates).await?;

        // Add top candidates to fill empty slots. Reserve a small number of slots
        // for exploration picks when appetite is higher.
        let probation_days = workspace.probation_days.unwrap_or(7);

        if ranked.is_empty() {
            warn!(
                workspace_id = %workspace.id,
                "No candidates available to fill empty slots"
            );
        }

        let exploration_slots = self
            .config
            .exploration_slots
            .min(empty_slots.saturating_sub(1));
        let core_slots = empty_slots.saturating_sub(exploration_slots);

        // Apply strategy diversity penalty: penalize candidates that duplicate
        // a strategy type already selected. This ensures the final roster covers
        // at least 2 distinct strategies when possible.
        let mut selected: Vec<&WalletCompositeScore> = Vec::with_capacity(core_slots);
        let mut strategy_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for candidate in ranked.iter() {
            if selected.len() >= core_slots {
                break;
            }

            let strategy = candidate
                .strategy_type
                .as_deref()
                .unwrap_or("Unknown")
                .to_string();
            let count = strategy_counts.get(&strategy).copied().unwrap_or(0);

            // Apply a 20% diversity penalty per duplicate strategy type.
            // The first wallet of each strategy gets no penalty.
            let diversity_factor = 1.0 - (count as f64 * 0.20).min(0.60);
            let adjusted_score = candidate.total_score * diversity_factor;

            // Accept the candidate if its adjusted score still beats the
            // threshold of 50% of the top score (prevents very weak picks).
            let top_score = ranked.first().map(|r| r.total_score).unwrap_or(1.0);
            if adjusted_score >= top_score * 0.50 || selected.is_empty() {
                selected.push(candidate);
                *strategy_counts.entry(strategy).or_insert(0) += 1;
            }
        }

        if exploration_slots > 0 {
            let mut exploration_pool: Vec<&WalletCompositeScore> = ranked
                .iter()
                .skip(core_slots)
                .filter(|candidate| candidate.confidence >= self.config.min_exploration_confidence)
                .collect();

            exploration_pool.sort_by(|a, b| {
                let score_a = Self::exploration_score(a);
                let score_b = Self::exploration_score(b);
                score_b
                    .partial_cmp(&score_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            selected.extend(exploration_pool.into_iter().take(exploration_slots));
        }

        if selected.is_empty() {
            return Ok(());
        }

        for candidate in selected.into_iter().take(empty_slots) {
            info!(
                workspace_id = %workspace.id,
                wallet = %candidate.address,
                score = candidate.total_score,
                confidence = candidate.confidence,
                "Adding wallet to active with probation"
            );
            self.add_to_active_with_probation(workspace.id, &candidate.address, probation_days)
                .await?;
        }

        Ok(())
    }

    /// Check for demotion triggers on a wallet.
    async fn check_demotion_triggers(
        &self,
        allocation: &CurrentAllocation,
        workspace: &WorkspaceOptimizationSettings,
    ) -> anyhow::Result<Option<DemotionTrigger>> {
        // Immediate trigger: consecutive losses
        if allocation.consecutive_losses.unwrap_or(0) >= self.config.demotion_max_consecutive_losses
        {
            return Ok(Some(DemotionTrigger::ConsecutiveLosses));
        }

        // Immediate trigger: Max drawdown exceeded
        let max_dd = workspace
            .max_drawdown_pct
            .map(|d| {
                d.to_string()
                    .parse::<f64>()
                    .unwrap_or(self.config.demotion_max_drawdown_pct)
            })
            .unwrap_or(self.config.demotion_max_drawdown_pct);
        let max_dd = Self::normalize_ratio(max_dd);

        if let Some(roi) = allocation.backtest_roi {
            let roi_f64 = Self::normalize_ratio(roi.to_string().parse().unwrap_or(0.0));
            // Check drawdown (simplified - would need actual drawdown tracking)
            if roi_f64 < -max_dd {
                return Ok(Some(DemotionTrigger::MaxDrawdown));
            }
        }

        // Grace period trigger: ROI below threshold
        if let Some(roi) = allocation.backtest_roi {
            let roi_f64 = Self::normalize_ratio(roi.to_string().parse().unwrap_or(0.0));
            let roi_threshold = Self::normalize_ratio(self.config.demotion_grace_roi_threshold);
            if roi_f64 < roi_threshold {
                return Ok(Some(DemotionTrigger::NegativeRoi));
            }
        }

        // Grace period trigger: Sharpe below threshold
        if let Some(sharpe) = allocation.backtest_sharpe {
            let sharpe_f64: f64 = sharpe.to_string().parse().unwrap_or(0.0);
            if sharpe_f64 < self.config.demotion_grace_sharpe_threshold {
                return Ok(Some(DemotionTrigger::LowSharpe));
            }
        }

        // Inactivity check - would need last_trade_at from wallet metrics
        // This would typically be checked via a join with wallet_success_metrics

        Ok(None)
    }

    /// Start grace period for a wallet.
    async fn start_grace_period(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        trigger: DemotionTrigger,
    ) -> anyhow::Result<()> {
        let now = Utc::now();
        let reason = format!("{:?}", trigger);

        sqlx::query(
            r#"
            UPDATE workspace_wallet_allocations
            SET grace_period_started_at = $1, grace_period_reason = $2, updated_at = $1
            WHERE workspace_id = $3 AND wallet_address = $4
            "#,
        )
        .bind(now)
        .bind(&reason)
        .bind(workspace_id)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;

        // Log to history
        self.log_rotation_action(
            workspace_id,
            "grace_period_start",
            None,
            Some(wallet_address),
            &format!("Grace period started: {}", reason),
            serde_json::json!({ "trigger": reason }),
            None,
        )
        .await?;

        info!(
            workspace_id = %workspace_id,
            wallet = wallet_address,
            trigger = ?trigger,
            "Started grace period for wallet"
        );

        Ok(())
    }

    /// Process expired grace periods.
    async fn process_grace_period_expirations(&self) -> anyhow::Result<()> {
        // Find wallets with expired grace periods
        let now = Utc::now();

        // ROI grace period
        let roi_expiry = now - Duration::hours(self.config.grace_period_negative_roi_hours);

        let expired_roi: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT workspace_id, wallet_address
            FROM workspace_wallet_allocations
            WHERE tier = 'active'
              AND grace_period_started_at IS NOT NULL
              AND grace_period_reason = 'NegativeRoi'
              AND grace_period_started_at <= $1
              AND COALESCE(pinned, false) = false
            "#,
        )
        .bind(roi_expiry)
        .fetch_all(&self.pool)
        .await?;

        for (workspace_id, wallet_address) in expired_roi {
            self.demote_wallet(
                workspace_id,
                &wallet_address,
                DemotionTrigger::NegativeRoi,
                None,
            )
            .await?;
        }

        // Sharpe grace period
        let sharpe_expiry = now - Duration::hours(self.config.grace_period_low_sharpe_hours);

        let expired_sharpe: Vec<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT workspace_id, wallet_address
            FROM workspace_wallet_allocations
            WHERE tier = 'active'
              AND grace_period_started_at IS NOT NULL
              AND grace_period_reason = 'LowSharpe'
              AND grace_period_started_at <= $1
              AND COALESCE(pinned, false) = false
            "#,
        )
        .bind(sharpe_expiry)
        .fetch_all(&self.pool)
        .await?;

        for (workspace_id, wallet_address) in expired_sharpe {
            self.demote_wallet(
                workspace_id,
                &wallet_address,
                DemotionTrigger::LowSharpe,
                None,
            )
            .await?;
        }

        Ok(())
    }

    /// Process probation graduations.
    async fn process_probation_graduations(&self) -> anyhow::Result<()> {
        let now = Utc::now();

        // Find wallets that have completed probation
        let graduated: Vec<(Uuid, String, Uuid)> = sqlx::query_as(
            r#"
            SELECT workspace_id, wallet_address, id
            FROM workspace_wallet_allocations
            WHERE tier = 'active'
              AND probation_until IS NOT NULL
              AND probation_until <= $1
            "#,
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;

        for (workspace_id, wallet_address, allocation_id) in graduated {
            // Check if wallet still meets criteria before graduating
            let workspace = self.get_workspace_settings(workspace_id).await?;
            let allocation = self
                .get_allocation_by_id(allocation_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Allocation not found"))?;

            let meets_criteria = self.check_promotion_criteria(&allocation, &workspace);

            if meets_criteria {
                // Graduate: remove probation status
                sqlx::query(
                    r#"
                    UPDATE workspace_wallet_allocations
                    SET probation_until = NULL, updated_at = $1
                    WHERE id = $2
                    "#,
                )
                .bind(now)
                .bind(allocation_id)
                .execute(&self.pool)
                .await?;

                self.log_rotation_action(
                    workspace_id,
                    "probation_graduate",
                    Some(&wallet_address),
                    None,
                    "Wallet graduated from probation - full allocation enabled",
                    serde_json::json!({ "allocation_id": allocation_id.to_string() }),
                    None,
                )
                .await?;

                info!(
                    workspace_id = %workspace_id,
                    wallet = wallet_address,
                    "Wallet graduated from probation"
                );
            } else {
                // Failed probation - demote
                self.demote_wallet(
                    workspace_id,
                    &wallet_address,
                    DemotionTrigger::ProbationFailed,
                    None,
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Get current allocations for a workspace (extended).
    async fn get_current_allocations(
        &self,
        workspace_id: Uuid,
    ) -> anyhow::Result<Vec<CurrentAllocation>> {
        let allocations: Vec<CurrentAllocation> = sqlx::query_as(
            r#"
            SELECT id, wallet_address, tier, allocation_pct,
                   backtest_roi, backtest_sharpe, backtest_win_rate,
                   pinned, pinned_at, pinned_by,
                   probation_until, probation_allocation_pct,
                   consecutive_losses, last_loss_at,
                   confidence_score,
                   grace_period_started_at, grace_period_reason,
                   auto_assigned
            FROM workspace_wallet_allocations
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(allocations)
    }

    /// Get a specific allocation by ID.
    async fn get_allocation_by_id(
        &self,
        allocation_id: Uuid,
    ) -> anyhow::Result<Option<CurrentAllocation>> {
        let allocation: Option<CurrentAllocation> = sqlx::query_as(
            r#"
            SELECT id, wallet_address, tier, allocation_pct,
                   backtest_roi, backtest_sharpe, backtest_win_rate,
                   pinned, pinned_at, pinned_by,
                   probation_until, probation_allocation_pct,
                   consecutive_losses, last_loss_at,
                   confidence_score,
                   grace_period_started_at, grace_period_reason,
                   auto_assigned
            FROM workspace_wallet_allocations
            WHERE id = $1
            "#,
        )
        .bind(allocation_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(allocation)
    }

    /// Get banned wallets for a workspace.
    async fn get_banned_wallets(&self, workspace_id: Uuid) -> anyhow::Result<Vec<WalletBan>> {
        let bans: Vec<WalletBan> = sqlx::query_as(
            r#"
            SELECT id, workspace_id, wallet_address, reason, banned_by, banned_at, expires_at
            FROM workspace_wallet_bans
            WHERE workspace_id = $1
              AND (expires_at IS NULL OR expires_at > NOW())
            "#,
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(bans)
    }

    /// Check if a wallet is banned.
    pub async fn is_wallet_banned(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
    ) -> anyhow::Result<bool> {
        let exists: Option<(i32,)> = sqlx::query_as(
            r#"
            SELECT 1
            FROM workspace_wallet_bans
            WHERE workspace_id = $1
              AND wallet_address = $2
              AND (expires_at IS NULL OR expires_at > NOW())
            "#,
        )
        .bind(workspace_id)
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(exists.is_some())
    }

    /// Get candidate wallets that meet the workspace criteria.
    async fn get_candidate_wallets(
        &self,
        workspace: &WorkspaceOptimizationSettings,
        target_count: usize,
    ) -> anyhow::Result<Vec<WalletCandidate>> {
        let base_thresholds = self.base_candidate_thresholds(workspace);
        let mut merged: HashMap<String, WalletCandidate> = HashMap::new();

        let strict = self.query_candidate_wallets(&base_thresholds, 50).await?;
        let strict_count = strict.len();
        for candidate in strict {
            merged.insert(candidate.address.clone(), candidate);
        }

        if self.config.auto_relax_thresholds && merged.len() < target_count {
            for round in 1..=self.config.max_relaxation_rounds {
                let relaxed = Self::relax_thresholds(&base_thresholds, round);
                let relaxed_candidates = self.query_candidate_wallets(&relaxed, 100).await?;
                for candidate in relaxed_candidates {
                    merged.entry(candidate.address.clone()).or_insert(candidate);
                }

                if merged.len() >= target_count {
                    break;
                }
            }
        }

        let mut candidates: Vec<WalletCandidate> = merged.into_values().collect();
        candidates.sort_by(|a, b| {
            let a_roi = a
                .roi_30d
                .as_ref()
                .and_then(|d| d.to_string().parse::<f64>().ok())
                .unwrap_or(0.0);
            let b_roi = b
                .roi_30d
                .as_ref()
                .and_then(|d| d.to_string().parse::<f64>().ok())
                .unwrap_or(0.0);
            b_roi
                .partial_cmp(&a_roi)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!(
            workspace_id = %workspace.id,
            strict_count = strict_count,
            total_count = candidates.len(),
            min_roi = base_thresholds.min_roi,
            min_sharpe = base_thresholds.min_sharpe,
            min_win_rate = base_thresholds.min_win_rate,
            min_trades = base_thresholds.min_trades,
            max_drawdown = base_thresholds.max_drawdown,
            "Candidate query complete"
        );

        Ok(candidates)
    }

    fn base_candidate_thresholds(
        &self,
        workspace: &WorkspaceOptimizationSettings,
    ) -> CandidateThresholds {
        let min_roi = workspace
            .min_roi_30d
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_ROI_30D))
            .map(Self::normalize_ratio)
            .unwrap_or(DEFAULT_MIN_ROI_30D);
        let min_sharpe = workspace
            .min_sharpe
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_SHARPE))
            .unwrap_or(DEFAULT_MIN_SHARPE);
        let min_win_rate = workspace
            .min_win_rate
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_WIN_RATE))
            .map(Self::normalize_ratio)
            .unwrap_or(DEFAULT_MIN_WIN_RATE);
        let min_trades = workspace.min_trades_30d.unwrap_or(DEFAULT_MIN_TRADES);
        let max_drawdown = workspace
            .max_drawdown_pct
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MAX_DRAWDOWN))
            .map(Self::normalize_ratio)
            .unwrap_or(DEFAULT_MAX_DRAWDOWN);

        CandidateThresholds {
            min_roi,
            min_sharpe,
            min_win_rate,
            min_trades,
            max_drawdown,
        }
    }

    fn relax_thresholds(base: &CandidateThresholds, round: usize) -> CandidateThresholds {
        match round {
            1 => CandidateThresholds {
                min_roi: (base.min_roi - 0.02).max(-0.10),
                min_sharpe: (base.min_sharpe - 0.15).max(0.0),
                min_win_rate: (base.min_win_rate - 0.05).max(0.45),
                min_trades: (base.min_trades / 2).max(5),
                max_drawdown: (base.max_drawdown + 0.10).min(0.60),
            },
            2 => CandidateThresholds {
                min_roi: (base.min_roi - 0.05).max(-0.15),
                min_sharpe: (base.min_sharpe - 0.30).max(0.0),
                min_win_rate: (base.min_win_rate - 0.10).max(0.42),
                min_trades: (base.min_trades / 3).max(3),
                max_drawdown: (base.max_drawdown + 0.20).min(0.70),
            },
            _ => CandidateThresholds {
                min_roi: (base.min_roi - 0.10).max(-0.25),
                min_sharpe: (base.min_sharpe - 0.50).max(0.0),
                min_win_rate: (base.min_win_rate - 0.15).max(0.40),
                min_trades: 1,
                max_drawdown: (base.max_drawdown + 0.30).min(0.80),
            },
        }
    }

    async fn query_candidate_wallets(
        &self,
        thresholds: &CandidateThresholds,
        limit: i64,
    ) -> anyhow::Result<Vec<WalletCandidate>> {
        let has_strategy_type = wallet_features_has_strategy_type(&self.pool).await;
        let strategy_type_select = if has_strategy_type {
            "wf_st.strategy_type"
        } else {
            "NULL::TEXT AS strategy_type"
        };
        let strategy_type_join = if has_strategy_type {
            "LEFT JOIN wallet_features wf_st ON wf_st.address = cm.address"
        } else {
            ""
        };

        let sql = format!(
            r#"
            WITH candidate_metrics AS (
                SELECT
                    COALESCE(wsm.address, wf.address) AS address,
                    CASE
                        WHEN ABS(COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)) > 1
                            THEN COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0) / 100
                        ELSE COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)
                    END AS roi_30d,
                    COALESCE(wsm.sharpe_30d, 0) AS sharpe_30d,
                    CASE
                        WHEN ABS(COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)) > 1
                            THEN COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0) / 100
                        ELSE COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)
                    END AS win_rate_30d,
                    COALESCE(wsm.trades_30d, wf.total_trades::integer, 0) AS trade_count_30d,
                    CASE
                        WHEN ABS(COALESCE(wsm.max_drawdown_30d, 0.2)) > 1
                            THEN COALESCE(wsm.max_drawdown_30d, 0.2) / 100
                        ELSE COALESCE(wsm.max_drawdown_30d, 0.2)
                    END AS max_drawdown_30d,
                    COALESCE(wsm.last_computed, wf.last_trade, NOW()) AS last_trade_at,
                    -- Recency-adjusted ROI: blend 30d (70%) and 90d (30%) metrics
                    (0.70 * COALESCE(
                        CASE WHEN ABS(COALESCE(wsm.roi_30d, 0)) > 1 THEN wsm.roi_30d / 100 ELSE COALESCE(wsm.roi_30d, 0) END,
                        0
                    ) + 0.30 * COALESCE(
                        CASE WHEN ABS(COALESCE(wsm.roi_90d, wsm.roi_30d, 0)) > 1 THEN COALESCE(wsm.roi_90d, wsm.roi_30d, 0) / 100 ELSE COALESCE(wsm.roi_90d, wsm.roi_30d, 0) END,
                        0
                    )) AS recency_adjusted_roi,
                    -- Staleness: days since last data update
                    (
                        EXTRACT(EPOCH FROM (NOW() - COALESCE(wsm.last_computed, wf.last_trade, NOW())))
                        / 86400.0
                    )::FLOAT8 AS staleness_days
                FROM wallet_success_metrics wsm
                FULL OUTER JOIN wallet_features wf ON wf.address = wsm.address
                WHERE COALESCE(wsm.address, wf.address) IS NOT NULL
            ),
            copy_performance AS (
                SELECT
                    source_wallet AS address,
                    AVG(CASE WHEN pnl > 0 THEN 1.0 ELSE 0.0 END)::FLOAT8 AS copy_win_rate
                FROM copy_trade_history
                WHERE created_at > NOW() - INTERVAL '30 days'
                GROUP BY source_wallet
            )
            SELECT
                cm.address, cm.roi_30d, cm.sharpe_30d, cm.win_rate_30d,
                cm.trade_count_30d, cm.max_drawdown_30d, cm.last_trade_at,
                cm.recency_adjusted_roi, cm.staleness_days,
                cp.copy_win_rate,
                {strategy_type_select}
            FROM candidate_metrics cm
            LEFT JOIN copy_performance cp ON cp.address = cm.address
            {strategy_type_join}
            WHERE cm.roi_30d >= $1::numeric
              AND cm.sharpe_30d >= $2::numeric
              AND cm.win_rate_30d >= $3::numeric
              AND cm.trade_count_30d >= $4
              AND cm.max_drawdown_30d <= $5::numeric
            ORDER BY cm.recency_adjusted_roi DESC
            LIMIT $6
            "#,
        );

        let candidates: Vec<WalletCandidate> = sqlx::query_as(&sql)
            .bind(thresholds.min_roi)
            .bind(thresholds.min_sharpe)
            .bind(thresholds.min_win_rate)
            .bind(thresholds.min_trades)
            .bind(thresholds.max_drawdown)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        Ok(candidates)
    }

    /// Rank candidates by composite score with recency weighting.
    async fn rank_candidates(
        &self,
        candidates: &[&WalletCandidate],
    ) -> anyhow::Result<Vec<WalletCompositeScore>> {
        let mut scores = Vec::new();

        for candidate in candidates {
            // Prefer recency-adjusted ROI when available, fall back to raw 30d ROI
            let roi = candidate
                .recency_adjusted_roi
                .or(candidate.roi_30d)
                .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
                .map(Self::normalize_ratio)
                .unwrap_or(0.0);
            let sharpe = candidate
                .sharpe_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            let win_rate = candidate
                .win_rate_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
                .map(Self::normalize_ratio)
                .unwrap_or(0.0);
            let trade_count = candidate.trade_count_30d.unwrap_or(0) as f64;

            // Normalize scores (0-100 scale)
            let roi_score = (roi / 0.20).clamp(0.0, 1.0) * 100.0; // 20% monthly = max score
            let sharpe_score = (sharpe / 3.0).clamp(0.0, 1.0) * 100.0; // 3.0 = max score
            let win_rate_score = (win_rate * 100.0).clamp(0.0, 100.0);

            // Consistency score based on trade count and drawdown
            let drawdown = candidate
                .max_drawdown_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(20.0))
                .map(Self::normalize_ratio)
                .unwrap_or(DEFAULT_MAX_DRAWDOWN);
            let trade_consistency = (trade_count / 50.0).min(1.0) * 50.0;
            let drawdown_score = (1.0 - drawdown / 0.30).max(0.0) * 50.0;
            let consistency_score = trade_consistency + drawdown_score;

            // Weighted composite score
            // ROI: 30%, Sharpe: 25%, Win Rate: 25%, Consistency: 20%
            let raw_score = roi_score * 0.30
                + sharpe_score * 0.25
                + win_rate_score * 0.25
                + consistency_score * 0.20;

            // Apply staleness penalty: score decays linearly over 60 days, floored at 50%
            let staleness = candidate.staleness_days.unwrap_or(0.0).max(0.0);
            let staleness_multiplier = (1.0 - staleness / 60.0).max(0.5);

            // Apply copy trade divergence penalty: if actual copy performance is
            // significantly worse than reported metrics, penalize the score by 10%.
            // This closes the feedback loop between paper metrics and live execution.
            let copy_penalty = if let Some(copy_wr) = candidate.copy_win_rate {
                let reported_wr = win_rate;
                if reported_wr - copy_wr > 0.15 {
                    // Copy win rate is >15pp below reported — apply 10% penalty
                    0.90
                } else {
                    1.0
                }
            } else {
                1.0 // No copy history — no penalty
            };

            let total_score = raw_score * staleness_multiplier * copy_penalty;

            // Confidence based on data quality
            let confidence = self.calculate_data_confidence(trade_count as i32, sharpe, win_rate);

            scores.push(WalletCompositeScore {
                address: candidate.address.clone(),
                total_score,
                roi_score,
                sharpe_score,
                win_rate_score,
                consistency_score,
                confidence,
                strategy_type: candidate.strategy_type.clone(),
            });
        }

        // Sort by total score descending
        scores.sort_by(|a, b| {
            b.total_score
                .partial_cmp(&a.total_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(scores)
    }

    /// Calculate data confidence score.
    fn calculate_data_confidence(&self, trade_count: i32, sharpe: f64, win_rate: f64) -> f64 {
        let mut confidence: f64 = 0.0;

        // More trades = higher confidence
        if trade_count >= 100 {
            confidence += 0.4;
        } else if trade_count >= 50 {
            confidence += 0.3;
        } else if trade_count >= 20 {
            confidence += 0.2;
        } else if trade_count >= 10 {
            confidence += 0.1;
        }

        // Consistent metrics boost confidence
        if sharpe > 1.5 {
            confidence += 0.2;
        } else if sharpe > 1.0 {
            confidence += 0.1;
        }

        if win_rate > 0.60 {
            confidence += 0.2;
        } else if win_rate > 0.50 {
            confidence += 0.1;
        }

        // Cap at 1.0
        confidence.min(1.0)
    }

    fn normalize_ratio(value: f64) -> f64 {
        if value.abs() > 1.0 {
            value / 100.0
        } else {
            value
        }
    }

    fn exploration_score(candidate: &WalletCompositeScore) -> f64 {
        candidate.roi_score * 0.45
            + candidate.sharpe_score * 0.20
            + candidate.win_rate_score * 0.15
            + candidate.consistency_score * 0.10
            + ((1.0 - candidate.confidence).max(0.0) * 10.0)
    }

    /// Check if allocation meets promotion criteria.
    fn check_promotion_criteria(
        &self,
        allocation: &CurrentAllocation,
        workspace: &WorkspaceOptimizationSettings,
    ) -> bool {
        let min_roi = workspace
            .min_roi_30d
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_ROI_30D))
            .map(Self::normalize_ratio)
            .unwrap_or(DEFAULT_MIN_ROI_30D);
        let min_sharpe = workspace
            .min_sharpe
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_SHARPE))
            .unwrap_or(DEFAULT_MIN_SHARPE);
        let min_win_rate = workspace
            .min_win_rate
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_WIN_RATE))
            .map(Self::normalize_ratio)
            .unwrap_or(DEFAULT_MIN_WIN_RATE);

        let roi = allocation
            .backtest_roi
            .map(|d| Self::normalize_ratio(d.to_string().parse::<f64>().unwrap_or(0.0)));
        let sharpe = allocation
            .backtest_sharpe
            .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0));
        let win_rate = allocation
            .backtest_win_rate
            .map(|d| Self::normalize_ratio(d.to_string().parse::<f64>().unwrap_or(0.0)));

        // If no metrics are available yet, do not fail probation purely on missing data.
        if roi.is_none() && sharpe.is_none() && win_rate.is_none() {
            return true;
        }

        let mut checks = 0;
        let mut passed = 0;

        if let Some(roi) = roi {
            checks += 1;
            if roi >= min_roi {
                passed += 1;
            }
        }
        if let Some(sharpe) = sharpe {
            checks += 1;
            if sharpe >= min_sharpe {
                passed += 1;
            }
        }
        if let Some(win_rate) = win_rate {
            checks += 1;
            if win_rate >= min_win_rate {
                passed += 1;
            }
        }

        checks == passed
    }

    /// Recalculate allocations based on strategy.
    async fn recalculate_allocations(
        &self,
        workspace: &WorkspaceOptimizationSettings,
    ) -> anyhow::Result<()> {
        let strategy = workspace
            .allocation_strategy
            .as_ref()
            .and_then(|s| s.parse::<AllocationStrategy>().ok())
            .unwrap_or(AllocationStrategy::ConfidenceWeighted);

        let allocations = self.get_current_allocations(workspace.id).await?;
        let active: Vec<_> = allocations.iter().filter(|a| a.tier == "active").collect();

        if active.is_empty() {
            return Ok(());
        }

        let new_allocations = match strategy {
            AllocationStrategy::Equal => {
                let pct = 100.0 / active.len() as f64;
                active.iter().map(|a| (a.id, pct)).collect::<Vec<_>>()
            }
            AllocationStrategy::ConfidenceWeighted => {
                self.calculate_confidence_weighted_allocations(&active)
            }
            AllocationStrategy::Performance => {
                self.calculate_performance_weighted_allocations(&active)
            }
        };

        // Update allocations in database
        let now = Utc::now();
        for (allocation_id, pct) in new_allocations {
            // Apply probation scaling if applicable
            let allocation = active.iter().find(|a| a.id == allocation_id);
            let effective_pct = if let Some(a) = allocation {
                if a.probation_until.is_some() && a.probation_until.unwrap() > now {
                    let probation_pct = a.probation_allocation_pct.unwrap_or(Decimal::new(50, 0));
                    let probation_f64: f64 = probation_pct.to_string().parse().unwrap_or(50.0);
                    pct * (probation_f64 / 100.0)
                } else {
                    pct
                }
            } else {
                pct
            };

            sqlx::query(
                "UPDATE workspace_wallet_allocations SET allocation_pct = $1, updated_at = $2 WHERE id = $3",
            )
            .bind(Decimal::new((effective_pct * 100.0) as i64, 2))
            .bind(now)
            .bind(allocation_id)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Calculate confidence-weighted allocations.
    fn calculate_confidence_weighted_allocations(
        &self,
        active: &[&CurrentAllocation],
    ) -> Vec<(Uuid, f64)> {
        let base_allocation = 100.0 / active.len() as f64;

        // Calculate raw allocations with confidence multiplier
        let raw_allocations: Vec<(Uuid, f64)> = active
            .iter()
            .map(|a| {
                let confidence = a
                    .confidence_score
                    .map(|d| d.to_string().parse::<f64>().unwrap_or(0.5))
                    .unwrap_or(0.5);
                // Confidence multiplier: 0.5 + (confidence * 1.0), range: 0.5x to 1.5x
                let multiplier = 0.5 + confidence;
                (a.id, base_allocation * multiplier)
            })
            .collect();

        // Normalize to ensure total = 100%
        let total: f64 = raw_allocations.iter().map(|(_, v)| v).sum();
        let normalized: Vec<(Uuid, f64)> = raw_allocations
            .into_iter()
            .map(|(id, v)| {
                let pct = (v / total) * 100.0;
                // Apply min/max caps
                let capped = pct
                    .max(self.config.min_allocation_pct)
                    .min(self.config.max_allocation_pct);
                (id, capped)
            })
            .collect();

        // Re-normalize after capping
        let total_capped: f64 = normalized.iter().map(|(_, v)| v).sum();
        normalized
            .into_iter()
            .map(|(id, v)| (id, (v / total_capped) * 100.0))
            .collect()
    }

    /// Calculate performance-weighted allocations.
    fn calculate_performance_weighted_allocations(
        &self,
        active: &[&CurrentAllocation],
    ) -> Vec<(Uuid, f64)> {
        // Weight by ROI
        let total_roi: f64 = active
            .iter()
            .map(|a| {
                a.backtest_roi
                    .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0).max(0.0))
                    .unwrap_or(0.0)
            })
            .sum();

        if total_roi <= 0.0 {
            // Fall back to equal if no positive ROI
            let pct = 100.0 / active.len() as f64;
            return active.iter().map(|a| (a.id, pct)).collect();
        }

        let capped: Vec<(Uuid, f64)> = active
            .iter()
            .map(|a| {
                let roi = a
                    .backtest_roi
                    .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0).max(0.0))
                    .unwrap_or(0.0);
                let pct = (roi / total_roi) * 100.0;
                let bounded = pct
                    .max(self.config.min_allocation_pct)
                    .min(self.config.max_allocation_pct);
                (a.id, bounded)
            })
            .collect();

        let total_capped: f64 = capped.iter().map(|(_, v)| v).sum();
        if total_capped <= 0.0 {
            let pct = 100.0 / active.len() as f64;
            return active.iter().map(|a| (a.id, pct)).collect();
        }

        capped
            .into_iter()
            .map(|(id, v)| (id, (v / total_capped) * 100.0))
            .collect()
    }

    /// Rotate a wallet: demote old, promote new.
    #[allow(dead_code)]
    async fn rotate_wallet(
        &self,
        workspace_id: Uuid,
        old_address: &str,
        new_address: &str,
    ) -> anyhow::Result<()> {
        let now = Utc::now();

        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Demote old wallet to bench
        sqlx::query(
            r#"
            UPDATE workspace_wallet_allocations
            SET tier = 'bench', updated_at = $1
            WHERE workspace_id = $2 AND wallet_address = $3
            "#,
        )
        .bind(now)
        .bind(workspace_id)
        .bind(old_address)
        .execute(&mut *tx)
        .await?;

        // Check if new wallet is already in roster
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
        )
        .bind(workspace_id)
        .bind(new_address)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((id,)) = existing {
            // Promote existing wallet
            sqlx::query(
                r#"
                UPDATE workspace_wallet_allocations
                SET tier = 'active', allocation_pct = 20, updated_at = $1
                WHERE id = $2
                "#,
            )
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        } else {
            // Add new wallet to active roster
            sqlx::query(
                r#"
                INSERT INTO workspace_wallet_allocations
                (id, workspace_id, wallet_address, allocation_pct, tier, auto_assigned, auto_assigned_reason, added_at, updated_at)
                VALUES ($1, $2, $3, 20, 'active', true, 'Auto-optimizer rotation', $4, $4)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(workspace_id)
            .bind(new_address)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        // Record rotation history
        sqlx::query(
            r#"
            INSERT INTO auto_rotation_history
            (id, workspace_id, action, wallet_in, wallet_out, reason, evidence, notification_sent, acknowledged, created_at)
            VALUES ($1, $2, 'replace', $3, $4, $5, $6, false, false, $7)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(workspace_id)
        .bind(new_address)
        .bind(old_address)
        .bind("Underperforming wallet replaced by better candidate")
        .bind(serde_json::json!({"action": "auto_rotation"}))
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        tracing::info!(
            workspace_id = %workspace_id,
            old_wallet = old_address,
            new_wallet = new_address,
            "Rotated wallet"
        );

        Ok(())
    }

    /// Add a new wallet to the active roster.
    #[allow(dead_code)]
    async fn add_to_active(&self, workspace_id: Uuid, address: &str) -> anyhow::Result<()> {
        let now = Utc::now();

        // Check if already in roster
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
        )
        .bind(workspace_id)
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id,)) = existing {
            // Promote to active
            sqlx::query(
                r#"
                UPDATE workspace_wallet_allocations
                SET tier = 'active', allocation_pct = 20, updated_at = $1
                WHERE id = $2
                "#,
            )
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        } else {
            // Add new
            sqlx::query(
                r#"
                INSERT INTO workspace_wallet_allocations
                (id, workspace_id, wallet_address, allocation_pct, tier, auto_assigned, auto_assigned_reason, added_at, updated_at)
                VALUES ($1, $2, $3, 20, 'active', true, 'Auto-optimizer selection', $4, $4)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(workspace_id)
            .bind(address)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }

        // Record history
        sqlx::query(
            r#"
            INSERT INTO auto_rotation_history
            (id, workspace_id, action, wallet_in, reason, evidence, notification_sent, acknowledged, created_at)
            VALUES ($1, $2, 'add', $3, $4, $5, false, false, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(workspace_id)
        .bind(address)
        .bind("Wallet added by auto-optimizer")
        .bind(serde_json::json!({"action": "auto_add"}))
        .bind(now)
        .execute(&self.pool)
        .await?;

        tracing::info!(
            workspace_id = %workspace_id,
            wallet = address,
            "Added wallet to active roster"
        );

        Ok(())
    }

    /// Run optimization for a specific workspace (triggered manually).
    pub async fn optimize_workspace_by_id(&self, workspace_id: Uuid) -> anyhow::Result<()> {
        let workspace = self.get_workspace_settings(workspace_id).await?;
        self.optimize_workspace(&workspace).await?;
        self.reconcile_runtime_if_attached().await?;
        Ok(())
    }

    async fn get_wallet_metric_snapshot(
        &self,
        address: &str,
    ) -> anyhow::Result<Option<WalletMetricSnapshot>> {
        let snapshot: Option<WalletMetricSnapshot> = sqlx::query_as(
            r#"
            SELECT
                CASE
                    WHEN ABS(COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)) > 1
                        THEN COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0) / 100
                    ELSE COALESCE(wsm.roi_30d, ((COALESCE(wf.win_rate, 0.5) - 0.5) * 2)::numeric, 0)
                END AS roi_30d,
                COALESCE(wsm.sharpe_30d, 0) AS sharpe_30d,
                CASE
                    WHEN ABS(COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)) > 1
                        THEN COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0) / 100
                    ELSE COALESCE(wsm.win_rate_30d, wf.win_rate::numeric, 0)
                END AS win_rate_30d,
                CASE
                    WHEN ABS(COALESCE(wsm.predicted_success_prob, 0.5)) > 1
                        THEN COALESCE(wsm.predicted_success_prob, 0.5) / 100
                    ELSE COALESCE(wsm.predicted_success_prob, 0.5)
                END AS confidence_score
            FROM wallet_success_metrics wsm
            FULL OUTER JOIN wallet_features wf ON wf.address = wsm.address
            WHERE LOWER(COALESCE(wsm.address, wf.address)) = LOWER($1)
            LIMIT 1
            "#,
        )
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        Ok(snapshot)
    }

    /// Add wallet to active roster with probation period.
    async fn add_to_active_with_probation(
        &self,
        workspace_id: Uuid,
        address: &str,
        probation_days: i32,
    ) -> anyhow::Result<()> {
        let now = Utc::now();
        let probation_until = now + Duration::days(probation_days as i64);
        let snapshot = self.get_wallet_metric_snapshot(address).await?;

        let backtest_roi = snapshot.as_ref().and_then(|s| s.roi_30d.as_ref().cloned());
        let backtest_sharpe = snapshot
            .as_ref()
            .and_then(|s| s.sharpe_30d.as_ref().cloned());
        let backtest_win_rate = snapshot
            .as_ref()
            .and_then(|s| s.win_rate_30d.as_ref().cloned());
        let confidence = snapshot
            .as_ref()
            .and_then(|s| s.confidence_score.as_ref().cloned())
            .unwrap_or(Decimal::new(5, 1)); // 0.5 default

        // Check if already in roster
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
        )
        .bind(workspace_id)
        .bind(address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id,)) = existing {
            // Promote to active with probation
            sqlx::query(
                r#"
                UPDATE workspace_wallet_allocations
                SET tier = 'active', allocation_pct = 20,
                    probation_until = $1, probation_allocation_pct = 50,
                    backtest_roi = $2, backtest_sharpe = $3, backtest_win_rate = $4,
                    confidence_score = $5,
                    updated_at = $6
                WHERE id = $7
                "#,
            )
            .bind(probation_until)
            .bind(backtest_roi)
            .bind(backtest_sharpe)
            .bind(backtest_win_rate)
            .bind(confidence)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        } else {
            // Add new with probation
            sqlx::query(
                r#"
                INSERT INTO workspace_wallet_allocations
                (id, workspace_id, wallet_address, allocation_pct, tier, auto_assigned, auto_assigned_reason,
                 probation_until, probation_allocation_pct,
                 backtest_roi, backtest_sharpe, backtest_win_rate, confidence_score,
                 added_at, updated_at)
                VALUES ($1, $2, $3, 20, 'active', true, 'Auto-selected with probation',
                        $4, 50, $5, $6, $7, $8, $9, $9)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(workspace_id)
            .bind(address)
            .bind(probation_until)
            .bind(backtest_roi)
            .bind(backtest_sharpe)
            .bind(backtest_win_rate)
            .bind(confidence)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }

        // Log to history
        self.log_rotation_action(
            workspace_id,
            "probation_start",
            Some(address),
            None,
            &format!(
                "Wallet added with {}-day probation period (50% allocation)",
                probation_days
            ),
            serde_json::json!({
                "probation_days": probation_days,
                "probation_until": probation_until.to_rfc3339()
            }),
            Some(now + Duration::hours(1)), // Undo available for 1 hour
        )
        .await?;

        info!(
            workspace_id = %workspace_id,
            wallet = address,
            probation_days = probation_days,
            "Added wallet with probation"
        );

        Ok(())
    }

    /// Demote a wallet to bench.
    async fn demote_wallet(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        trigger: DemotionTrigger,
        _triggered_by: Option<Uuid>,
    ) -> anyhow::Result<()> {
        let now = Utc::now();
        let action = if trigger.is_immediate() {
            "emergency_demote"
        } else {
            "grace_period_demote"
        };

        // Demote to bench and clear grace period
        sqlx::query(
            r#"
            UPDATE workspace_wallet_allocations
            SET tier = 'bench',
                grace_period_started_at = NULL,
                grace_period_reason = NULL,
                updated_at = $1
            WHERE workspace_id = $2 AND wallet_address = $3
            "#,
        )
        .bind(now)
        .bind(workspace_id)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;

        // Log to history
        self.log_rotation_action(
            workspace_id,
            action,
            None,
            Some(wallet_address),
            &format!("Wallet demoted: {:?}", trigger),
            serde_json::json!({
                "trigger": format!("{:?}", trigger),
                "immediate": trigger.is_immediate()
            }),
            Some(now + Duration::hours(1)), // Undo available for 1 hour
        )
        .await?;

        info!(
            workspace_id = %workspace_id,
            wallet = wallet_address,
            trigger = ?trigger,
            "Demoted wallet"
        );

        // Try to fill the now-empty slot
        let workspace = self.get_workspace_settings(workspace_id).await?;
        self.fill_empty_slots(&workspace).await?;

        Ok(())
    }

    /// Handle position closed event - update loss tracking.
    async fn handle_position_closed(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        _pnl: Decimal,
        is_win: bool,
    ) -> anyhow::Result<()> {
        let now = Utc::now();

        if is_win {
            // Reset consecutive losses on win
            sqlx::query(
                r#"
                UPDATE workspace_wallet_allocations
                SET consecutive_losses = 0, updated_at = $1
                WHERE workspace_id = $2 AND wallet_address = $3
                "#,
            )
            .bind(now)
            .bind(workspace_id)
            .bind(wallet_address)
            .execute(&self.pool)
            .await?;
        } else {
            // Increment consecutive losses
            sqlx::query(
                r#"
                UPDATE workspace_wallet_allocations
                SET consecutive_losses = COALESCE(consecutive_losses, 0) + 1,
                    last_loss_at = $1,
                    updated_at = $1
                WHERE workspace_id = $2 AND wallet_address = $3
                "#,
            )
            .bind(now)
            .bind(workspace_id)
            .bind(wallet_address)
            .execute(&self.pool)
            .await?;

            // Check if we need to trigger immediate demotion
            let allocation: Option<CurrentAllocation> = sqlx::query_as(
                r#"
                SELECT id, wallet_address, tier, allocation_pct,
                       backtest_roi, backtest_sharpe, backtest_win_rate,
                       pinned, pinned_at, pinned_by,
                       probation_until, probation_allocation_pct,
                       consecutive_losses, last_loss_at,
                       confidence_score,
                       grace_period_started_at, grace_period_reason,
                       auto_assigned
                FROM workspace_wallet_allocations
                WHERE workspace_id = $1 AND wallet_address = $2 AND tier = 'active'
                "#,
            )
            .bind(workspace_id)
            .bind(wallet_address)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(alloc) = allocation {
                // Skip if pinned
                if alloc.pinned.unwrap_or(false) {
                    return Ok(());
                }

                // Check consecutive losses threshold
                if alloc.consecutive_losses.unwrap_or(0)
                    >= self.config.demotion_max_consecutive_losses
                {
                    self.demote_wallet(
                        workspace_id,
                        wallet_address,
                        DemotionTrigger::ConsecutiveLosses,
                        None,
                    )
                    .await?;
                }
            }
        }

        // Trigger immediate single-wallet metric recompute for faster feedback.
        // This updates wallet_success_metrics so the next optimization cycle
        // has fresh data instead of waiting up to 1 hour for the batch job.
        let pool = self.pool.clone();
        let address = wallet_address.to_string();
        tokio::spawn(async move {
            let config = crate::metrics_calculator::MetricsCalculatorConfig {
                enabled: true,
                ..Default::default()
            };
            let calculator = crate::metrics_calculator::MetricsCalculator::new(pool, config);
            if let Err(e) = calculator.compute_single_wallet(&address).await {
                warn!(
                    address = %address,
                    error = %e,
                    "Failed to recompute metrics after position close"
                );
            } else {
                debug!(address = %address, "Recomputed metrics after position close");
            }
        });

        Ok(())
    }

    /// Handle circuit breaker trip - immediate demotion.
    async fn handle_circuit_breaker_trip(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        // Check if wallet is pinned
        let is_pinned: Option<(bool,)> = sqlx::query_as(
            r#"
            SELECT COALESCE(pinned, false)
            FROM workspace_wallet_allocations
            WHERE workspace_id = $1 AND wallet_address = $2 AND tier = 'active'
            "#,
        )
        .bind(workspace_id)
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((pinned,)) = is_pinned {
            if pinned {
                warn!(
                    workspace_id = %workspace_id,
                    wallet = wallet_address,
                    "Circuit breaker tripped for pinned wallet - not demoting"
                );
                return Ok(());
            }
        }

        info!(
            workspace_id = %workspace_id,
            wallet = wallet_address,
            reason = reason,
            "Demoting wallet due to circuit breaker trip"
        );

        self.demote_wallet(
            workspace_id,
            wallet_address,
            DemotionTrigger::CircuitBreaker,
            None,
        )
        .await
    }

    /// Handle new workspace creation - auto-select initial wallets.
    async fn handle_workspace_created(&self, workspace_id: Uuid) -> anyhow::Result<()> {
        let workspace = self.get_workspace_settings(workspace_id).await?;

        // Only auto-select if enabled
        if !workspace.auto_select_enabled.unwrap_or(true) {
            return Ok(());
        }

        info!(
            workspace_id = %workspace_id,
            "Auto-selecting initial wallets for new workspace"
        );

        self.fill_empty_slots(&workspace).await?;

        Ok(())
    }

    /// Log rotation action to history table.
    #[allow(clippy::too_many_arguments)]
    async fn log_rotation_action(
        &self,
        workspace_id: Uuid,
        action: &str,
        wallet_in: Option<&str>,
        wallet_out: Option<&str>,
        reason: &str,
        evidence: serde_json::Value,
        undo_expires_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<()> {
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO auto_rotation_history
            (id, workspace_id, action, wallet_in, wallet_out, reason, evidence,
             notification_sent, acknowledged, undo_expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, false, false, $8, $9)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(workspace_id)
        .bind(action)
        .bind(wallet_in)
        .bind(wallet_out)
        .bind(reason)
        .bind(evidence)
        .bind(undo_expires_at)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Undo a recent rotation action.
    pub async fn undo_rotation(
        &self,
        workspace_id: Uuid,
        rotation_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        let now = Utc::now();

        // Find the rotation record
        #[allow(clippy::type_complexity)]
        let rotation: Option<(
            String,
            Option<String>,
            Option<String>,
            Option<DateTime<Utc>>,
        )> = sqlx::query_as(
            r#"
                SELECT action, wallet_in, wallet_out, undo_expires_at
                FROM auto_rotation_history
                WHERE id = $1 AND workspace_id = $2 AND undone = false
                "#,
        )
        .bind(rotation_id)
        .bind(workspace_id)
        .fetch_optional(&self.pool)
        .await?;

        let (action, wallet_in, wallet_out, undo_expires_at) = match rotation {
            Some(r) => r,
            None => return Ok(false),
        };

        // Check if undo is still available
        if let Some(expires) = undo_expires_at {
            if now > expires {
                return Ok(false);
            }
        } else {
            return Ok(false); // No undo available for this action
        }

        // Perform the undo based on action type
        match action.as_str() {
            "probation_start" | "add" => {
                // Undo add: remove the wallet
                if let Some(ref addr) = wallet_in {
                    sqlx::query(
                        "DELETE FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
                    )
                    .bind(workspace_id)
                    .bind(addr)
                    .execute(&self.pool)
                    .await?;
                }
            }
            "emergency_demote" | "grace_period_demote" | "demote" => {
                // Undo demote: promote back to active
                if let Some(ref addr) = wallet_out {
                    sqlx::query(
                        r#"
                        UPDATE workspace_wallet_allocations
                        SET tier = 'active', updated_at = $1
                        WHERE workspace_id = $2 AND wallet_address = $3
                        "#,
                    )
                    .bind(now)
                    .bind(workspace_id)
                    .bind(addr)
                    .execute(&self.pool)
                    .await?;
                }
            }
            _ => return Ok(false),
        }

        // Mark as undone
        sqlx::query(
            r#"
            UPDATE auto_rotation_history
            SET undone = true, undone_at = $1, undone_by = $2
            WHERE id = $3
            "#,
        )
        .bind(now)
        .bind(user_id)
        .bind(rotation_id)
        .execute(&self.pool)
        .await?;

        // Log the undo
        self.log_rotation_action(
            workspace_id,
            "undo",
            wallet_out.as_deref(),
            wallet_in.as_deref(),
            &format!("Undone: {}", action),
            serde_json::json!({ "original_action_id": rotation_id.to_string() }),
            None,
        )
        .await?;

        info!(
            workspace_id = %workspace_id,
            rotation_id = %rotation_id,
            action = action,
            "Rotation action undone"
        );

        Ok(true)
    }

    /// Pin a wallet to prevent auto-demotion.
    pub async fn pin_wallet(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE workspace_wallet_allocations
            SET pinned = true, pinned_at = $1, pinned_by = $2, updated_at = $1
            WHERE workspace_id = $3 AND wallet_address = $4 AND tier = 'active'
            "#,
        )
        .bind(now)
        .bind(user_id)
        .bind(workspace_id)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() > 0 {
            self.log_rotation_action(
                workspace_id,
                "pin",
                Some(wallet_address),
                None,
                "Wallet pinned by user",
                serde_json::json!({ "user_id": user_id.to_string() }),
                None,
            )
            .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Unpin a wallet.
    pub async fn unpin_wallet(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE workspace_wallet_allocations
            SET pinned = false, pinned_at = NULL, pinned_by = NULL, updated_at = $1
            WHERE workspace_id = $2 AND wallet_address = $3
            "#,
        )
        .bind(now)
        .bind(workspace_id)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() > 0 {
            self.log_rotation_action(
                workspace_id,
                "unpin",
                None,
                Some(wallet_address),
                "Wallet unpinned by user",
                serde_json::json!({ "user_id": user_id.to_string() }),
                None,
            )
            .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Ban a wallet from auto-promotion.
    pub async fn ban_wallet(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        reason: Option<&str>,
        user_id: Uuid,
        expires_at: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Uuid> {
        let ban_id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO workspace_wallet_bans
            (id, workspace_id, wallet_address, reason, banned_by, banned_at, expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (workspace_id, wallet_address) DO UPDATE SET
                reason = EXCLUDED.reason,
                banned_by = EXCLUDED.banned_by,
                banned_at = EXCLUDED.banned_at,
                expires_at = EXCLUDED.expires_at
            "#,
        )
        .bind(ban_id)
        .bind(workspace_id)
        .bind(wallet_address)
        .bind(reason)
        .bind(user_id)
        .bind(now)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;

        self.log_rotation_action(
            workspace_id,
            "ban",
            None,
            Some(wallet_address),
            reason.unwrap_or("User banned wallet"),
            serde_json::json!({
                "user_id": user_id.to_string(),
                "expires_at": expires_at.map(|t| t.to_rfc3339())
            }),
            None,
        )
        .await?;

        Ok(ban_id)
    }

    /// Unban a wallet.
    pub async fn unban_wallet(
        &self,
        workspace_id: Uuid,
        wallet_address: &str,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "DELETE FROM workspace_wallet_bans WHERE workspace_id = $1 AND wallet_address = $2",
        )
        .bind(workspace_id)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() > 0 {
            self.log_rotation_action(
                workspace_id,
                "unban",
                Some(wallet_address),
                None,
                "User unbanned wallet",
                serde_json::json!({ "user_id": user_id.to_string() }),
                None,
            )
            .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get automation preview - what would happen on next run.
    pub async fn get_automation_preview(
        &self,
        workspace_id: Uuid,
    ) -> anyhow::Result<Vec<AutomationAction>> {
        let mut actions = Vec::new();
        let workspace = self.get_workspace_settings(workspace_id).await?;

        let current = self.get_current_allocations(workspace_id).await?;
        let active_wallets: Vec<_> = current.iter().filter(|a| a.tier == "active").collect();

        // Check for potential demotions
        for allocation in &active_wallets {
            if allocation.pinned.unwrap_or(false) {
                continue;
            }

            if let Some(trigger) = self.check_demotion_triggers(allocation, &workspace).await? {
                actions.push(AutomationAction {
                    action: if trigger.is_immediate() {
                        "emergency_demote".to_string()
                    } else {
                        "grace_period_start".to_string()
                    },
                    wallet_in: None,
                    wallet_out: Some(allocation.wallet_address.clone()),
                    reason: format!("Trigger: {:?}", trigger),
                    trigger: Some(trigger),
                    metrics_snapshot: serde_json::json!({
                        "roi": allocation.backtest_roi,
                        "sharpe": allocation.backtest_sharpe,
                        "consecutive_losses": allocation.consecutive_losses
                    }),
                    undo_expires_at: None,
                });
            }
        }

        // Check for potential promotions
        let active_count = active_wallets.len();
        let empty_slots = 5_usize.saturating_sub(active_count);

        if empty_slots > 0 {
            let banned = self.get_banned_wallets(workspace_id).await?;
            let banned_addresses: Vec<&str> =
                banned.iter().map(|b| b.wallet_address.as_str()).collect();
            let current_addresses: Vec<&str> =
                current.iter().map(|a| a.wallet_address.as_str()).collect();

            let candidate_target = empty_slots + self.config.exploration_slots + 2;
            let candidates = self
                .get_candidate_wallets(&workspace, candidate_target)
                .await?;
            let available: Vec<_> = candidates
                .iter()
                .filter(|c| {
                    !banned_addresses.contains(&c.address.as_str())
                        && !current_addresses.contains(&c.address.as_str())
                })
                .collect();

            let ranked = self.rank_candidates(&available).await?;

            for candidate in ranked.iter().take(empty_slots) {
                actions.push(AutomationAction {
                    action: "probation_start".to_string(),
                    wallet_in: Some(candidate.address.clone()),
                    wallet_out: None,
                    reason: format!(
                        "Auto-select: score {:.1}, confidence {:.0}%",
                        candidate.total_score,
                        candidate.confidence * 100.0
                    ),
                    trigger: None,
                    metrics_snapshot: serde_json::json!({
                        "total_score": candidate.total_score,
                        "roi_score": candidate.roi_score,
                        "confidence": candidate.confidence
                    }),
                    undo_expires_at: None,
                });
            }
        }

        Ok(actions)
    }
}
