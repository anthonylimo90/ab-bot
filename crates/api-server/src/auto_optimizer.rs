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
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{debug, info, warn};
use uuid::Uuid;

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

    /// Get the grace period in hours for non-immediate triggers
    pub fn grace_period_hours(&self) -> Option<i64> {
        match self {
            DemotionTrigger::NegativeRoi => Some(48),
            DemotionTrigger::LowSharpe => Some(24),
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
pub const DEFAULT_MIN_ROI_30D: f64 = 5.0;
pub const DEFAULT_MIN_SHARPE: f64 = 1.0;
pub const DEFAULT_MIN_WIN_RATE: f64 = 50.0;
pub const DEFAULT_MIN_TRADES: i32 = 10;
pub const DEFAULT_MAX_DRAWDOWN: f64 = 20.0;

/// Default demotion thresholds
pub const DEMOTION_MAX_CONSECUTIVE_LOSSES: i32 = 5;
pub const DEMOTION_MAX_DRAWDOWN_PCT: f64 = 30.0;
pub const DEMOTION_GRACE_ROI_THRESHOLD: f64 = 0.0;
pub const DEMOTION_GRACE_SHARPE_THRESHOLD: f64 = 0.5;

/// Allocation limits
pub const MIN_ALLOCATION_PCT: f64 = 10.0;
pub const MAX_ALLOCATION_PCT: f64 = 35.0;

/// Auto-optimizer service with event-driven automation.
pub struct AutoOptimizer {
    pool: PgPool,
    event_sender: Option<mpsc::Sender<AutomationEvent>>,
}

impl Default for AutoOptimizer {
    fn default() -> Self {
        panic!("AutoOptimizer requires a database pool")
    }
}

impl AutoOptimizer {
    /// Create a new auto-optimizer.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            event_sender: None,
        }
    }

    /// Create with event channel for event-driven automation.
    pub fn with_event_channel(pool: PgPool) -> (Self, mpsc::Receiver<AutomationEvent>) {
        let (tx, rx) = mpsc::channel(100);
        (
            Self {
                pool,
                event_sender: Some(tx),
            },
            rx,
        )
    }

    /// Get event sender for external event emission.
    pub fn event_sender(&self) -> Option<mpsc::Sender<AutomationEvent>> {
        self.event_sender.clone()
    }

    /// Start the background optimization loop with event handling.
    pub async fn start(self: Arc<Self>, mut event_rx: Option<mpsc::Receiver<AutomationEvent>>) {
        // Run scheduled optimization every hour
        let mut ticker = interval(TokioDuration::from_secs(3600));

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

        info!("Auto-optimization cycle complete");
        Ok(())
    }

    /// Get workspaces that are due for optimization.
    async fn get_eligible_workspaces(&self) -> anyhow::Result<Vec<WorkspaceOptimizationSettings>> {
        let workspaces: Vec<WorkspaceOptimizationSettings> = sqlx::query_as(
            r#"
            SELECT id, name, auto_optimize_enabled, optimization_interval_hours,
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
            SELECT id, name, auto_optimize_enabled, optimization_interval_hours,
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
        let active_count = active_wallets.len();

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

        if empty_slots == 0 {
            return Ok(());
        }

        // Get banned wallets for this workspace
        let banned = self.get_banned_wallets(workspace.id).await?;
        let banned_addresses: Vec<&str> =
            banned.iter().map(|b| b.wallet_address.as_str()).collect();

        // Get current roster addresses
        let current_addresses: Vec<&str> =
            current.iter().map(|a| a.wallet_address.as_str()).collect();

        // Get candidate wallets
        let candidates = self.get_candidate_wallets(workspace).await?;

        // Filter out banned and already-in-roster wallets
        let available_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| {
                !banned_addresses.contains(&c.address.as_str())
                    && !current_addresses.contains(&c.address.as_str())
            })
            .collect();

        // Rank candidates by composite score
        let ranked = self.rank_candidates(&available_candidates).await?;

        // Add top candidates to fill empty slots
        let probation_days = workspace.probation_days.unwrap_or(7);

        for candidate in ranked.iter().take(empty_slots) {
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
        // Immediate trigger: 5+ consecutive losses
        if allocation.consecutive_losses.unwrap_or(0) >= DEMOTION_MAX_CONSECUTIVE_LOSSES {
            return Ok(Some(DemotionTrigger::ConsecutiveLosses));
        }

        // Immediate trigger: Max drawdown exceeded
        let max_dd = workspace
            .max_drawdown_pct
            .map(|d| {
                d.to_string()
                    .parse::<f64>()
                    .unwrap_or(DEMOTION_MAX_DRAWDOWN_PCT)
            })
            .unwrap_or(DEMOTION_MAX_DRAWDOWN_PCT);

        if let Some(roi) = allocation.backtest_roi {
            let roi_f64: f64 = roi.to_string().parse().unwrap_or(0.0);
            // Check drawdown (simplified - would need actual drawdown tracking)
            if roi_f64 < -max_dd {
                return Ok(Some(DemotionTrigger::MaxDrawdown));
            }
        }

        // Grace period trigger: ROI < 0%
        if let Some(roi) = allocation.backtest_roi {
            let roi_f64: f64 = roi.to_string().parse().unwrap_or(0.0);
            if roi_f64 < DEMOTION_GRACE_ROI_THRESHOLD {
                return Ok(Some(DemotionTrigger::NegativeRoi));
            }
        }

        // Grace period trigger: Sharpe < 0.5
        if let Some(sharpe) = allocation.backtest_sharpe {
            let sharpe_f64: f64 = sharpe.to_string().parse().unwrap_or(0.0);
            if sharpe_f64 < DEMOTION_GRACE_SHARPE_THRESHOLD {
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

        // ROI grace period: 48 hours
        let roi_expiry = now - Duration::hours(48);

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

        // Sharpe grace period: 24 hours
        let sharpe_expiry = now - Duration::hours(24);

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
    ) -> anyhow::Result<Vec<WalletCandidate>> {
        let min_roi = workspace
            .min_roi_30d
            .unwrap_or(Decimal::new(DEFAULT_MIN_ROI_30D as i64 * 100, 2));
        let min_sharpe = workspace
            .min_sharpe
            .unwrap_or(Decimal::new(DEFAULT_MIN_SHARPE as i64 * 100, 2));
        let min_win_rate = workspace
            .min_win_rate
            .unwrap_or(Decimal::new(DEFAULT_MIN_WIN_RATE as i64, 0));
        let min_trades = workspace.min_trades_30d.unwrap_or(DEFAULT_MIN_TRADES);
        let max_drawdown = workspace
            .max_drawdown_pct
            .unwrap_or(Decimal::new(DEFAULT_MAX_DRAWDOWN as i64, 0));

        let candidates: Vec<WalletCandidate> = sqlx::query_as(
            r#"
            SELECT address, roi_30d, sharpe_30d, win_rate_30d, trades_30d AS trade_count_30d,
                   max_drawdown_30d, last_computed AS last_trade_at
            FROM wallet_success_metrics
            WHERE COALESCE(roi_30d, 0) >= $1
              AND COALESCE(sharpe_30d, 0) >= $2
              AND COALESCE(win_rate_30d, 0) >= $3
              AND COALESCE(trades_30d, 0) >= $4
              AND COALESCE(max_drawdown_30d, 100) <= $5
            ORDER BY COALESCE(roi_30d, 0) DESC
            LIMIT 50
            "#,
        )
        .bind(min_roi)
        .bind(min_sharpe)
        .bind(min_win_rate)
        .bind(min_trades)
        .bind(max_drawdown)
        .fetch_all(&self.pool)
        .await?;

        Ok(candidates)
    }

    /// Rank candidates by composite score.
    async fn rank_candidates(
        &self,
        candidates: &[&WalletCandidate],
    ) -> anyhow::Result<Vec<WalletCompositeScore>> {
        let mut scores = Vec::new();

        for candidate in candidates {
            let roi = candidate
                .roi_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            let sharpe = candidate
                .sharpe_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            let win_rate = candidate
                .win_rate_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);
            let trade_count = candidate.trade_count_30d.unwrap_or(0) as f64;

            // Normalize scores (0-100 scale)
            let roi_score = (roi / 20.0).min(1.0).max(0.0) * 100.0; // 20% = max score
            let sharpe_score = (sharpe / 3.0).min(1.0).max(0.0) * 100.0; // 3.0 = max score
            let win_rate_score = win_rate; // Already 0-100

            // Consistency score based on trade count and drawdown
            let drawdown = candidate
                .max_drawdown_30d
                .map(|d| d.to_string().parse::<f64>().unwrap_or(20.0))
                .unwrap_or(20.0);
            let trade_consistency = (trade_count / 50.0).min(1.0) * 50.0;
            let drawdown_score = (1.0 - drawdown / 30.0).max(0.0) * 50.0;
            let consistency_score = trade_consistency + drawdown_score;

            // Weighted composite score
            // ROI: 30%, Sharpe: 25%, Win Rate: 25%, Consistency: 20%
            let total_score = roi_score * 0.30
                + sharpe_score * 0.25
                + win_rate_score * 0.25
                + consistency_score * 0.20;

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

        if win_rate > 60.0 {
            confidence += 0.2;
        } else if win_rate > 50.0 {
            confidence += 0.1;
        }

        // Cap at 1.0
        confidence.min(1.0)
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
            .unwrap_or(DEFAULT_MIN_ROI_30D);
        let min_sharpe = workspace
            .min_sharpe
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_SHARPE))
            .unwrap_or(DEFAULT_MIN_SHARPE);
        let min_win_rate = workspace
            .min_win_rate
            .map(|d| d.to_string().parse::<f64>().unwrap_or(DEFAULT_MIN_WIN_RATE))
            .unwrap_or(DEFAULT_MIN_WIN_RATE);

        let roi = allocation
            .backtest_roi
            .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let sharpe = allocation
            .backtest_sharpe
            .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);
        let win_rate = allocation
            .backtest_win_rate
            .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
            .unwrap_or(0.0);

        roi >= min_roi && sharpe >= min_sharpe && win_rate >= min_win_rate
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
                let capped = pct.max(MIN_ALLOCATION_PCT).min(MAX_ALLOCATION_PCT);
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

        active
            .iter()
            .map(|a| {
                let roi = a
                    .backtest_roi
                    .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0).max(0.0))
                    .unwrap_or(0.0);
                let pct = (roi / total_roi) * 100.0;
                let capped = pct.max(MIN_ALLOCATION_PCT).min(MAX_ALLOCATION_PCT);
                (a.id, capped)
            })
            .collect()
    }

    /// Rotate a wallet: demote old, promote new.
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
        self.optimize_workspace(&workspace).await
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
                    updated_at = $2
                WHERE id = $3
                "#,
            )
            .bind(probation_until)
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
                 probation_until, probation_allocation_pct, added_at, updated_at)
                VALUES ($1, $2, $3, 20, 'active', true, 'Auto-selected with probation', $4, 50, $5, $5)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(workspace_id)
            .bind(address)
            .bind(probation_until)
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
        triggered_by: Option<Uuid>,
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
                if alloc.consecutive_losses.unwrap_or(0) >= DEMOTION_MAX_CONSECUTIVE_LOSSES {
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

            let candidates = self.get_candidate_wallets(&workspace).await?;
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
