//! Auto-optimizer background service for automatic wallet rotation.
//!
//! This service runs periodically to optimize workspace rosters:
//! - Evaluates wallet performance against configured criteria
//! - Promotes/demotes wallets based on backtest results
//! - Records rotation history for audit trail

use chrono::Utc;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::time::{interval, Duration as TokioDuration};
use uuid::Uuid;

/// Workspace settings for auto-optimization.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkspaceOptimizationSettings {
    pub id: Uuid,
    pub name: String,
    pub auto_optimize_enabled: bool,
    pub optimization_interval_hours: i32,
    pub min_roi_30d: Option<Decimal>,
    pub min_sharpe: Option<Decimal>,
    pub min_win_rate: Option<Decimal>,
    pub min_trades_30d: Option<i32>,
}

/// Wallet metrics from the discovery table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WalletCandidate {
    pub address: String,
    pub roi_30d: Option<Decimal>,
    pub sharpe_30d: Option<Decimal>,
    pub win_rate_30d: Option<Decimal>,
    pub trade_count_30d: Option<i32>,
}

/// Current allocation in a workspace.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CurrentAllocation {
    pub id: Uuid,
    pub wallet_address: String,
    pub tier: String,
    pub allocation_pct: Decimal,
    pub backtest_roi: Option<Decimal>,
    pub backtest_sharpe: Option<Decimal>,
    pub backtest_win_rate: Option<Decimal>,
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

/// Auto-optimizer service.
pub struct AutoOptimizer {
    pool: PgPool,
}

impl AutoOptimizer {
    /// Create a new auto-optimizer.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Start the background optimization loop.
    pub async fn start(self: Arc<Self>) {
        // Run every hour
        let mut ticker = interval(TokioDuration::from_secs(3600));

        tracing::info!("Auto-optimizer started");

        loop {
            ticker.tick().await;

            if let Err(e) = self.run_scheduled_optimizations().await {
                tracing::error!(error = %e, "Auto-optimization cycle failed");
            }
        }
    }

    /// Run optimization for all eligible workspaces.
    pub async fn run_scheduled_optimizations(&self) -> anyhow::Result<()> {
        tracing::info!("Starting auto-optimization cycle");

        // Get workspaces with auto-optimize enabled that are due for optimization
        let workspaces = self.get_eligible_workspaces().await?;

        tracing::info!(count = workspaces.len(), "Found eligible workspaces");

        for workspace in workspaces {
            if let Err(e) = self.optimize_workspace(&workspace).await {
                tracing::error!(
                    workspace_id = %workspace.id,
                    workspace_name = %workspace.name,
                    error = %e,
                    "Failed to optimize workspace"
                );
            }
        }

        tracing::info!("Auto-optimization cycle complete");
        Ok(())
    }

    /// Get workspaces that are due for optimization.
    async fn get_eligible_workspaces(&self) -> anyhow::Result<Vec<WorkspaceOptimizationSettings>> {
        let workspaces: Vec<WorkspaceOptimizationSettings> = sqlx::query_as(
            r#"
            SELECT id, name, auto_optimize_enabled, optimization_interval_hours,
                   min_roi_30d, min_sharpe, min_win_rate, min_trades_30d
            FROM workspaces
            WHERE auto_optimize_enabled = true
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(workspaces)
    }

    /// Optimize a single workspace.
    async fn optimize_workspace(
        &self,
        workspace: &WorkspaceOptimizationSettings,
    ) -> anyhow::Result<()> {
        tracing::info!(
            workspace_id = %workspace.id,
            workspace_name = %workspace.name,
            "Optimizing workspace"
        );

        // Get current allocations
        let current = self.get_current_allocations(workspace.id).await?;
        let active_count = current.iter().filter(|a| a.tier == "active").count();

        // Get candidate wallets that meet criteria
        let candidates = self.get_candidate_wallets(workspace).await?;

        if candidates.is_empty() {
            tracing::info!(
                workspace_id = %workspace.id,
                "No candidates meet criteria"
            );
            return Ok(());
        }

        // Find underperformers in current active roster
        let underperformers = self.find_underperformers(&current, workspace);

        // Find better candidates not in current roster
        let current_addresses: Vec<&str> =
            current.iter().map(|a| a.wallet_address.as_str()).collect();
        let new_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| !current_addresses.contains(&c.address.as_str()))
            .take(5)
            .collect();

        // Perform rotations
        for underperformer in underperformers.iter().take(new_candidates.len()) {
            if let Some(replacement) = new_candidates.get(0) {
                self.rotate_wallet(
                    workspace.id,
                    &underperformer.wallet_address,
                    &replacement.address,
                )
                .await?;
            }
        }

        // If we have empty slots, fill them
        let empty_slots = 5_usize.saturating_sub(active_count);
        for candidate in new_candidates.iter().take(empty_slots) {
            self.add_to_active(workspace.id, &candidate.address).await?;
        }

        // Update last_optimization_at timestamp
        sqlx::query("UPDATE workspaces SET last_optimization_at = NOW() WHERE id = $1")
            .bind(workspace.id)
            .execute(&self.pool)
            .await?;

        tracing::info!(
            workspace_id = %workspace.id,
            "Updated last_optimization_at timestamp"
        );

        Ok(())
    }

    /// Get current allocations for a workspace.
    async fn get_current_allocations(
        &self,
        workspace_id: Uuid,
    ) -> anyhow::Result<Vec<CurrentAllocation>> {
        let allocations: Vec<CurrentAllocation> = sqlx::query_as(
            r#"
            SELECT id, wallet_address, tier, allocation_pct,
                   backtest_roi, backtest_sharpe, backtest_win_rate
            FROM workspace_wallet_allocations
            WHERE workspace_id = $1
            "#,
        )
        .bind(workspace_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(allocations)
    }

    /// Get candidate wallets that meet the workspace criteria.
    async fn get_candidate_wallets(
        &self,
        workspace: &WorkspaceOptimizationSettings,
    ) -> anyhow::Result<Vec<WalletCandidate>> {
        // Build dynamic query based on criteria
        let min_roi = workspace.min_roi_30d.unwrap_or(Decimal::ZERO);
        let min_sharpe = workspace.min_sharpe.unwrap_or(Decimal::ZERO);
        let min_win_rate = workspace.min_win_rate.unwrap_or(Decimal::ZERO);
        let min_trades = workspace.min_trades_30d.unwrap_or(0);

        let candidates: Vec<WalletCandidate> = sqlx::query_as(
            r#"
            SELECT address, roi_30d, sharpe_30d, win_rate_30d, trade_count_30d
            FROM wallet_success_metrics
            WHERE enabled = true
              AND COALESCE(roi_30d, 0) >= $1
              AND COALESCE(sharpe_30d, 0) >= $2
              AND COALESCE(win_rate_30d, 0) >= $3
              AND COALESCE(trade_count_30d, 0) >= $4
            ORDER BY COALESCE(roi_30d, 0) DESC
            LIMIT 20
            "#,
        )
        .bind(min_roi)
        .bind(min_sharpe)
        .bind(min_win_rate)
        .bind(min_trades)
        .fetch_all(&self.pool)
        .await?;

        Ok(candidates)
    }

    /// Find underperforming wallets in the current roster.
    fn find_underperformers<'a>(
        &self,
        current: &'a [CurrentAllocation],
        workspace: &WorkspaceOptimizationSettings,
    ) -> Vec<&'a CurrentAllocation> {
        let min_roi = workspace.min_roi_30d.unwrap_or(Decimal::ZERO);
        let min_sharpe = workspace.min_sharpe.unwrap_or(Decimal::ZERO);
        let min_win_rate = workspace.min_win_rate.unwrap_or(Decimal::ZERO);

        current
            .iter()
            .filter(|a| a.tier == "active")
            .filter(|a| {
                let roi_ok = a.backtest_roi.unwrap_or(Decimal::ZERO) >= min_roi;
                let sharpe_ok = a.backtest_sharpe.unwrap_or(Decimal::ZERO) >= min_sharpe;
                let win_rate_ok = a.backtest_win_rate.unwrap_or(Decimal::ZERO) >= min_win_rate;
                !roi_ok || !sharpe_ok || !win_rate_ok
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
        let workspace: WorkspaceOptimizationSettings = sqlx::query_as(
            r#"
            SELECT id, name, auto_optimize_enabled, optimization_interval_hours,
                   min_roi_30d, min_sharpe, min_win_rate, min_trades_30d
            FROM workspaces
            WHERE id = $1
            "#,
        )
        .bind(workspace_id)
        .fetch_one(&self.pool)
        .await?;

        self.optimize_workspace(&workspace).await
    }
}
