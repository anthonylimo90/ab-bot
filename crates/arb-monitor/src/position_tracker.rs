//! Position lifecycle tracking for arbitrage positions.

use anyhow::Result;
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::{
    ArbOpportunity, BinaryMarketBook, ExitStrategy, FailureReason, Position, PositionState,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Tracks arbitrage positions through their lifecycle.
pub struct PositionTracker {
    repo: PositionRepository,
    /// In-memory cache of active positions by market_id.
    active_positions: HashMap<String, Vec<Position>>,
    /// Minimum profit threshold for exit signals.
    exit_threshold: Decimal,
}

#[allow(dead_code)]
impl PositionTracker {
    /// Create a new position tracker.
    pub fn new(pool: PgPool) -> Self {
        Self {
            repo: PositionRepository::new(pool),
            active_positions: HashMap::new(),
            exit_threshold: std::env::var("ARB_EXIT_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| Decimal::new(5, 3)), // default 0.005 = 0.5%
        }
    }

    /// Load active positions from database into cache.
    pub async fn load_active_positions(&mut self) -> Result<()> {
        let positions = self.repo.get_active().await?;
        self.active_positions.clear();

        for position in positions {
            self.active_positions
                .entry(position.market_id.clone())
                .or_default()
                .push(position);
        }

        info!(
            "Loaded {} active positions across {} markets",
            self.active_positions
                .values()
                .map(|v| v.len())
                .sum::<usize>(),
            self.active_positions.len()
        );

        Ok(())
    }

    /// Create a new position from an arbitrage opportunity.
    pub async fn create_position(
        &mut self,
        arb: &ArbOpportunity,
        quantity: Decimal,
        exit_strategy: ExitStrategy,
    ) -> Result<Position> {
        let position = Position::new(
            arb.market_id.clone(),
            arb.yes_ask,
            arb.no_ask,
            quantity,
            exit_strategy,
        );

        self.repo.insert(&position).await?;

        self.active_positions
            .entry(arb.market_id.clone())
            .or_default()
            .push(position.clone());

        info!(
            "Created position {} for market {} with {} shares",
            position.id, arb.market_id, quantity
        );

        Ok(position)
    }

    /// Update P&L for all positions in a market based on current prices.
    pub async fn update_market_positions(
        &mut self,
        market_id: &str,
        book: &BinaryMarketBook,
    ) -> Result<()> {
        let positions = match self.active_positions.get_mut(market_id) {
            Some(p) => p,
            None => return Ok(()),
        };

        let (yes_bid, no_bid, _) = match book.exit_value() {
            Some(v) => v,
            None => return Ok(()),
        };

        for position in positions.iter_mut() {
            if position.is_active() {
                position.update_pnl(yes_bid, no_bid, ArbOpportunity::DEFAULT_FEE);
                debug!(
                    "Position {} unrealized P&L: {}",
                    position.id, position.unrealized_pnl
                );
            }
        }

        Ok(())
    }

    /// Check for exit opportunities on positions in a market.
    pub async fn check_exit_opportunities(
        &mut self,
        market_id: &str,
        book: &BinaryMarketBook,
    ) -> Result<Vec<Uuid>> {
        let positions = match self.active_positions.get_mut(market_id) {
            Some(p) => p,
            None => return Ok(vec![]),
        };

        let (yes_bid, no_bid, _) = match book.exit_value() {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let mut exit_ready = Vec::new();

        for position in positions.iter_mut() {
            if position.state != PositionState::Open {
                continue;
            }

            if position.exit_strategy != ExitStrategy::ExitOnCorrection {
                continue;
            }

            // Calculate potential exit profit
            let exit_value = (yes_bid + no_bid) * position.quantity;
            let entry_cost = position.entry_cost();
            let fees = ArbOpportunity::DEFAULT_FEE * Decimal::TWO * position.quantity;
            let potential_profit = exit_value - entry_cost - fees;

            if potential_profit >= self.exit_threshold * position.quantity {
                if let Err(e) = position.mark_exit_ready() {
                    warn!("Cannot mark position {} exit ready: {}", position.id, e);
                    continue;
                }
                self.repo.update(position).await?;
                exit_ready.push(position.id);

                info!(
                    "EXIT READY: position {} profit={:.4}",
                    position.id, potential_profit
                );
            }
        }

        Ok(exit_ready)
    }

    /// Mark a position as open (execution confirmed).
    pub async fn mark_position_open(&mut self, position_id: Uuid) -> Result<()> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                if let Err(e) = pos.mark_open() {
                    warn!("Cannot mark position {} open: {}", position_id, e);
                    return Ok(());
                }
                self.repo.update(pos).await?;
                info!("Position {} marked as OPEN", position_id);
                return Ok(());
            }
        }
        warn!("Position {} not found in cache", position_id);
        Ok(())
    }

    /// Close a position via market exit.
    pub async fn close_position_exit(
        &mut self,
        position_id: Uuid,
        yes_exit_price: Decimal,
        no_exit_price: Decimal,
    ) -> Result<Option<Decimal>> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                if let Err(e) =
                    pos.close_via_exit(yes_exit_price, no_exit_price, ArbOpportunity::DEFAULT_FEE)
                {
                    warn!("Cannot close position {} via exit: {}", position_id, e);
                    return Ok(None);
                }
                self.repo.update(pos).await?;

                let pnl = pos.realized_pnl;
                info!(
                    "Position {} CLOSED via exit, realized P&L: {:?}",
                    position_id, pnl
                );

                return Ok(pnl);
            }
        }
        Ok(None)
    }

    /// Close a position via market resolution.
    pub async fn close_position_resolution(
        &mut self,
        position_id: Uuid,
    ) -> Result<Option<Decimal>> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                if let Err(e) = pos.close_via_resolution(ArbOpportunity::DEFAULT_FEE) {
                    warn!(
                        "Cannot close position {} via resolution: {}",
                        position_id, e
                    );
                    return Ok(None);
                }
                self.repo.update(pos).await?;

                let pnl = pos.realized_pnl;
                info!(
                    "Position {} CLOSED via resolution, realized P&L: {:?}",
                    position_id, pnl
                );

                return Ok(pnl);
            }
        }
        Ok(None)
    }

    /// Get all active positions.
    pub fn get_active_positions(&self) -> Vec<&Position> {
        self.active_positions
            .values()
            .flat_map(|v| v.iter())
            .filter(|p| p.is_active())
            .collect()
    }

    /// Get positions for a specific market.
    pub fn get_market_positions(&self, market_id: &str) -> Vec<&Position> {
        self.active_positions
            .get(market_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Mark a position entry as failed.
    pub async fn mark_entry_failed(
        &mut self,
        position_id: Uuid,
        reason: FailureReason,
    ) -> Result<()> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                pos.mark_entry_failed(reason.clone());
                self.repo.update(pos).await?;
                error!(
                    position_id = %position_id,
                    reason = ?reason,
                    "Position entry FAILED"
                );
                return Ok(());
            }
        }
        warn!("Position {} not found for entry failure", position_id);
        Ok(())
    }

    /// Mark a position exit as failed (can be retried).
    pub async fn mark_exit_failed(
        &mut self,
        position_id: Uuid,
        reason: FailureReason,
    ) -> Result<bool> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                pos.mark_exit_failed(reason.clone());
                self.repo.update(pos).await?;

                let can_retry = pos.can_retry();
                warn!(
                    position_id = %position_id,
                    reason = ?reason,
                    retry_count = pos.retry_count,
                    can_retry = can_retry,
                    "Position exit FAILED"
                );
                return Ok(can_retry);
            }
        }
        warn!("Position {} not found for exit failure", position_id);
        Ok(false)
    }

    /// Check for stale positions and mark them.
    pub async fn check_stale_positions(&mut self) -> Result<Vec<Uuid>> {
        let mut stale_ids = Vec::new();

        for positions in self.active_positions.values_mut() {
            for pos in positions.iter_mut() {
                if pos.is_stale() {
                    pos.mark_stalled();
                    if let Err(e) = self.repo.update(pos).await {
                        error!(
                            position_id = %pos.id,
                            error = %e,
                            "Failed to update stalled position in DB"
                        );
                    }
                    warn!(
                        position_id = %pos.id,
                        state = ?pos.state,
                        "Position marked as STALLED"
                    );
                    stale_ids.push(pos.id);
                }
            }
        }

        Ok(stale_ids)
    }

    /// Attempt to recover positions that need recovery.
    /// Returns list of position IDs that were recovered and should be re-processed.
    pub async fn recover_failed_positions(&mut self) -> Result<Vec<Uuid>> {
        let mut recovered = Vec::new();

        for positions in self.active_positions.values_mut() {
            for pos in positions.iter_mut() {
                if !pos.needs_recovery() {
                    continue;
                }

                let position_id = pos.id;

                match pos.state {
                    PositionState::ExitFailed => {
                        if pos.attempt_exit_recovery() {
                            if let Err(e) = self.repo.update(pos).await {
                                error!(
                                    position_id = %position_id,
                                    error = %e,
                                    "Failed to update recovered position in DB"
                                );
                                continue;
                            }
                            info!(
                                position_id = %position_id,
                                retry_count = pos.retry_count,
                                "Position recovered from ExitFailed, will retry exit"
                            );
                            recovered.push(position_id);
                        } else {
                            warn!(
                                position_id = %position_id,
                                retry_count = pos.retry_count,
                                "Position ExitFailed max retries exceeded, needs manual intervention"
                            );
                        }
                    }
                    PositionState::Stalled => {
                        if let Some(recovered_state) = pos.attempt_stalled_recovery() {
                            if let Err(e) = self.repo.update(pos).await {
                                error!(
                                    position_id = %position_id,
                                    error = %e,
                                    "Failed to update recovered stalled position in DB"
                                );
                                continue;
                            }
                            info!(
                                position_id = %position_id,
                                recovered_state = ?recovered_state,
                                "Position recovered from Stalled state"
                            );
                            recovered.push(position_id);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(recovered)
    }

    /// Get all positions that need recovery.
    pub fn get_positions_needing_recovery(&self) -> Vec<&Position> {
        self.active_positions
            .values()
            .flat_map(|v| v.iter())
            .filter(|p| p.needs_recovery())
            .collect()
    }

    /// Get count of positions by state.
    pub fn get_state_counts(&self) -> HashMap<PositionState, usize> {
        let mut counts = HashMap::new();
        for positions in self.active_positions.values() {
            for pos in positions {
                *counts.entry(pos.state).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Touch all active positions to update their last_updated timestamp.
    /// Should be called periodically to prevent false stale detection.
    pub async fn touch_active_positions(&mut self) -> Result<()> {
        for positions in self.active_positions.values_mut() {
            for pos in positions.iter_mut() {
                if pos.is_active() && !pos.needs_recovery() {
                    pos.touch();
                    // Note: We don't persist every touch to DB to avoid excessive writes.
                    // The last_updated is primarily for in-memory stale detection.
                }
            }
        }
        Ok(())
    }

    /// Reconcile positions on startup.
    ///
    /// This method performs the following checks:
    /// 1. Loads all active positions from database
    /// 2. Identifies positions in inconsistent states
    /// 3. Attempts automatic recovery where possible
    /// 4. Returns a summary of reconciliation actions
    ///
    /// Should be called once during initialization.
    pub async fn reconcile_on_startup(&mut self) -> Result<ReconciliationResult> {
        info!("Starting position reconciliation...");

        let mut result = ReconciliationResult::default();

        // Load all positions (including failed/stalled ones)
        let all_positions = self.repo.get_active().await?;
        let needing_recovery = self.repo.get_needing_recovery().await?;

        result.total_positions = all_positions.len();
        result.needing_recovery = needing_recovery.len();

        // Clear and rebuild cache
        self.active_positions.clear();
        for position in all_positions {
            self.active_positions
                .entry(position.market_id.clone())
                .or_default()
                .push(position);
        }

        // Check for positions in inconsistent states
        for positions in self.active_positions.values_mut() {
            for pos in positions.iter_mut() {
                match pos.state {
                    PositionState::Pending => {
                        // Position was created but execution never confirmed
                        // Check how old it is
                        if pos.is_stale() {
                            pos.mark_entry_failed(FailureReason::StalePosition {
                                last_update_secs: pos.age_secs(),
                            });
                            self.repo.update(pos).await?;
                            result.stale_pending += 1;
                            warn!(
                                position_id = %pos.id,
                                age_secs = pos.age_secs(),
                                "Stale pending position marked as entry failed"
                            );
                        } else {
                            result.pending_active += 1;
                        }
                    }
                    PositionState::Closing => {
                        // Position was being closed but process was interrupted
                        // Mark as exit failed so it can be retried
                        pos.mark_exit_failed(FailureReason::Unknown {
                            message: "Interrupted during closing - detected on startup".to_string(),
                        });
                        self.repo.update(pos).await?;
                        result.interrupted_closing += 1;
                        warn!(
                            position_id = %pos.id,
                            "Interrupted closing position marked as exit failed for retry"
                        );
                    }
                    PositionState::EntryFailed | PositionState::ExitFailed => {
                        // Already in failed state, will be picked up by recovery
                        result.already_failed += 1;
                    }
                    PositionState::Stalled => {
                        // Stalled positions need investigation
                        result.stalled += 1;
                    }
                    PositionState::Open | PositionState::ExitReady => {
                        // Normal active states
                        result.healthy += 1;
                    }
                    PositionState::Closed => {
                        // Should not appear in active positions query, but handle gracefully
                        result.already_closed += 1;
                    }
                }
            }
        }

        // Attempt automatic recovery
        let recovered = self.recover_failed_positions().await?;
        result.auto_recovered = recovered.len();

        info!(
            total = result.total_positions,
            healthy = result.healthy,
            pending = result.pending_active,
            stale_pending = result.stale_pending,
            interrupted = result.interrupted_closing,
            failed = result.already_failed,
            stalled = result.stalled,
            recovered = result.auto_recovered,
            "Position reconciliation complete"
        );

        Ok(result)
    }

    /// Get a summary of the current position state for monitoring.
    pub fn get_position_summary(&self) -> PositionSummary {
        let mut summary = PositionSummary::default();

        for positions in self.active_positions.values() {
            for pos in positions {
                summary.total += 1;
                match pos.state {
                    PositionState::Pending => summary.pending += 1,
                    PositionState::Open => summary.open += 1,
                    PositionState::ExitReady => summary.exit_ready += 1,
                    PositionState::Closing => summary.closing += 1,
                    PositionState::Closed => summary.closed += 1,
                    PositionState::EntryFailed => summary.entry_failed += 1,
                    PositionState::ExitFailed => summary.exit_failed += 1,
                    PositionState::Stalled => summary.stalled += 1,
                }

                if let Some(pnl) = pos.realized_pnl {
                    summary.total_realized_pnl += pnl;
                }
                summary.total_unrealized_pnl += pos.unrealized_pnl;
            }
        }

        summary
    }
}

/// Result of position reconciliation on startup.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct ReconciliationResult {
    /// Total positions found in database.
    pub total_positions: usize,
    /// Positions that were in healthy/active states.
    pub healthy: usize,
    /// Positions still pending (not yet confirmed).
    pub pending_active: usize,
    /// Stale pending positions marked as failed.
    pub stale_pending: usize,
    /// Positions interrupted during closing.
    pub interrupted_closing: usize,
    /// Positions already in failed state.
    pub already_failed: usize,
    /// Positions in stalled state.
    pub stalled: usize,
    /// Positions that were already closed (shouldn't be in active).
    pub already_closed: usize,
    /// Positions that needed recovery.
    pub needing_recovery: usize,
    /// Positions automatically recovered.
    pub auto_recovered: usize,
}

#[allow(dead_code)]
impl ReconciliationResult {
    /// Check if any issues were found that need attention.
    pub fn has_issues(&self) -> bool {
        self.stale_pending > 0
            || self.interrupted_closing > 0
            || self.stalled > 0
            || self.already_failed > 0
    }

    /// Get count of positions needing manual intervention.
    pub fn needs_manual_intervention(&self) -> usize {
        // Stalled positions and some failed positions may need manual review
        self.stalled
    }
}

/// Summary of current position states.
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct PositionSummary {
    pub total: usize,
    pub pending: usize,
    pub open: usize,
    pub exit_ready: usize,
    pub closing: usize,
    pub closed: usize,
    pub entry_failed: usize,
    pub exit_failed: usize,
    pub stalled: usize,
    pub total_realized_pnl: Decimal,
    pub total_unrealized_pnl: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_position(
        state: PositionState,
        unrealized: Decimal,
        realized: Option<Decimal>,
    ) -> Position {
        let mut pos = Position::new(
            "test-market".to_string(),
            Decimal::new(45, 2), // 0.45
            Decimal::new(50, 2), // 0.50
            Decimal::new(10, 0), // 10 shares
            ExitStrategy::HoldToResolution,
        );
        pos.state = state;
        pos.unrealized_pnl = unrealized;
        pos.realized_pnl = realized;
        pos
    }

    #[test]
    fn test_reconciliation_result_no_issues() {
        let result = ReconciliationResult {
            total_positions: 5,
            healthy: 5,
            ..Default::default()
        };
        assert!(!result.has_issues());
        assert_eq!(result.needs_manual_intervention(), 0);
    }

    #[test]
    fn test_reconciliation_result_with_issues() {
        let result = ReconciliationResult {
            total_positions: 10,
            healthy: 5,
            stale_pending: 2,
            interrupted_closing: 1,
            stalled: 1,
            already_failed: 1,
            ..Default::default()
        };
        assert!(result.has_issues());
        assert_eq!(result.needs_manual_intervention(), 1);
    }

    #[test]
    fn test_position_summary_counts() {
        // Build a tracker's active_positions HashMap manually to test get_position_summary
        let positions = vec![
            make_position(PositionState::Open, Decimal::new(5, 2), None),
            make_position(PositionState::Open, Decimal::new(3, 2), None),
            make_position(PositionState::ExitReady, Decimal::new(10, 2), None),
            make_position(
                PositionState::Closed,
                Decimal::ZERO,
                Some(Decimal::new(15, 2)),
            ),
            make_position(PositionState::EntryFailed, Decimal::ZERO, None),
            make_position(PositionState::ExitFailed, Decimal::new(-2, 2), None),
        ];

        // Simulate what get_position_summary does
        let mut summary = PositionSummary::default();
        for pos in &positions {
            summary.total += 1;
            match pos.state {
                PositionState::Pending => summary.pending += 1,
                PositionState::Open => summary.open += 1,
                PositionState::ExitReady => summary.exit_ready += 1,
                PositionState::Closing => summary.closing += 1,
                PositionState::Closed => summary.closed += 1,
                PositionState::EntryFailed => summary.entry_failed += 1,
                PositionState::ExitFailed => summary.exit_failed += 1,
                PositionState::Stalled => summary.stalled += 1,
            }
            if let Some(pnl) = pos.realized_pnl {
                summary.total_realized_pnl += pnl;
            }
            summary.total_unrealized_pnl += pos.unrealized_pnl;
        }

        assert_eq!(summary.total, 6);
        assert_eq!(summary.open, 2);
        assert_eq!(summary.exit_ready, 1);
        assert_eq!(summary.closed, 1);
        assert_eq!(summary.entry_failed, 1);
        assert_eq!(summary.exit_failed, 1);
        assert_eq!(summary.total_realized_pnl, Decimal::new(15, 2));
        // 0.05 + 0.03 + 0.10 + 0 + 0 + (-0.02) = 0.16
        assert_eq!(summary.total_unrealized_pnl, Decimal::new(16, 2));
    }

    #[test]
    fn test_state_counts() {
        let mut counts: HashMap<PositionState, usize> = HashMap::new();
        let positions = vec![
            make_position(PositionState::Open, Decimal::ZERO, None),
            make_position(PositionState::Open, Decimal::ZERO, None),
            make_position(PositionState::Pending, Decimal::ZERO, None),
            make_position(PositionState::Closed, Decimal::ZERO, Some(Decimal::ONE)),
        ];

        for pos in &positions {
            *counts.entry(pos.state).or_insert(0) += 1;
        }

        assert_eq!(counts[&PositionState::Open], 2);
        assert_eq!(counts[&PositionState::Pending], 1);
        assert_eq!(counts[&PositionState::Closed], 1);
        assert_eq!(counts.get(&PositionState::ExitReady), None);
    }

    #[test]
    fn test_position_active_filtering() {
        let positions = [
            make_position(PositionState::Open, Decimal::ZERO, None),
            make_position(PositionState::Closed, Decimal::ZERO, Some(Decimal::ONE)),
            make_position(PositionState::EntryFailed, Decimal::ZERO, None),
            make_position(PositionState::ExitFailed, Decimal::ZERO, None),
            make_position(PositionState::Pending, Decimal::ZERO, None),
        ];

        let active: Vec<_> = positions.iter().filter(|p| p.is_active()).collect();
        assert_eq!(active.len(), 3); // Open, ExitFailed, Pending

        let needing_recovery: Vec<_> = positions.iter().filter(|p| p.needs_recovery()).collect();
        assert_eq!(needing_recovery.len(), 1); // ExitFailed
    }

    #[test]
    fn test_exit_opportunity_math() {
        // Simulate the exit opportunity calculation from check_exit_opportunities
        let position = make_position(PositionState::Open, Decimal::ZERO, None);
        let exit_threshold = Decimal::new(5, 3); // 0.005

        let yes_bid = Decimal::new(52, 2); // 0.52
        let no_bid = Decimal::new(50, 2); // 0.50

        let exit_value = (yes_bid + no_bid) * position.quantity;
        let entry_cost = position.entry_cost(); // (0.45 + 0.50) * 10 = 9.50
        let fees = ArbOpportunity::DEFAULT_FEE * Decimal::TWO * position.quantity; // 0.02 * 2 * 10 = 0.40
        let potential_profit = exit_value - entry_cost - fees;
        // exit_value = 1.02 * 10 = 10.20
        // potential_profit = 10.20 - 9.50 - 0.40 = 0.30

        assert!(potential_profit >= exit_threshold * position.quantity);
        assert_eq!(potential_profit, Decimal::new(30, 2));
    }
}
