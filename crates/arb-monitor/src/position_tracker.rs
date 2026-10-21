//! Read-only view of active positions for market selection.
//!
//! The API server owns every position lifecycle transition (entries, exit
//! evaluation, exits, reconciliation and recovery). The monitor only needs to
//! know which markets hold positions so their order books stay subscribed; it
//! never writes positions, because a cached snapshot here would race the
//! authoritative writers.

use anyhow::Result;
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::{Position, PositionState};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::info;

/// Cache of active positions, refreshed from the database.
pub struct PositionTracker {
    repo: PositionRepository,
    /// Active positions by market_id.
    active_positions: HashMap<String, Vec<Position>>,
}

impl PositionTracker {
    /// Create a new position tracker.
    pub fn new(pool: PgPool) -> Self {
        Self {
            repo: PositionRepository::new(pool),
            active_positions: HashMap::new(),
        }
    }

    /// Replace the cache with the active positions currently in the database.
    pub async fn load_active_positions(&mut self) -> Result<()> {
        let positions = self.repo.get_active_for_arb_monitor().await?;
        self.active_positions = group_by_market(positions);
        Ok(())
    }

    /// Get all active positions.
    pub fn get_active_positions(&self) -> Vec<&Position> {
        self.active_positions
            .values()
            .flat_map(|v| v.iter())
            .filter(|p| p.is_active())
            .collect()
    }

    /// Get a summary of the current position state for monitoring.
    pub fn get_position_summary(&self) -> PositionSummary {
        summarize(self.active_positions.values().flatten())
    }

    /// Log the cached position summary.
    pub fn log_summary(&self) {
        let summary = self.get_position_summary();
        info!(
            total = summary.total,
            pending = summary.pending,
            open = summary.open,
            exit_ready = summary.exit_ready,
            closing = summary.closing,
            exit_failed = summary.exit_failed,
            stalled = summary.stalled,
            "Loaded active positions (read-only; lifecycle is owned by the API server)"
        );
    }
}

fn group_by_market(positions: Vec<Position>) -> HashMap<String, Vec<Position>> {
    let mut grouped: HashMap<String, Vec<Position>> = HashMap::new();
    for position in positions {
        grouped
            .entry(position.market_id.clone())
            .or_default()
            .push(position);
    }
    grouped
}

fn summarize<'a>(positions: impl Iterator<Item = &'a Position>) -> PositionSummary {
    let mut summary = PositionSummary::default();
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
    summary
}

/// Summary of current position states.
#[derive(Debug, Default, Clone)]
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
    use polymarket_core::types::ExitStrategy;

    fn make_position(
        market: &str,
        state: PositionState,
        unrealized: Decimal,
        realized: Option<Decimal>,
    ) -> Position {
        let mut pos = Position::new(
            market.to_string(),
            Decimal::new(45, 2),
            Decimal::new(50, 2),
            Decimal::new(10, 0),
            ExitStrategy::HoldToResolution,
        );
        pos.state = state;
        pos.unrealized_pnl = unrealized;
        pos.realized_pnl = realized;
        pos
    }

    #[test]
    fn summary_counts_every_state_and_sums_pnl() {
        let positions = [
            make_position("a", PositionState::Open, Decimal::new(5, 2), None),
            make_position("a", PositionState::Open, Decimal::new(3, 2), None),
            make_position("b", PositionState::ExitReady, Decimal::new(10, 2), None),
            make_position(
                "b",
                PositionState::Closed,
                Decimal::ZERO,
                Some(Decimal::new(15, 2)),
            ),
            make_position("c", PositionState::EntryFailed, Decimal::ZERO, None),
            make_position("c", PositionState::ExitFailed, Decimal::new(-2, 2), None),
        ];

        let summary = summarize(positions.iter());
        assert_eq!(summary.total, 6);
        assert_eq!(summary.open, 2);
        assert_eq!(summary.exit_ready, 1);
        assert_eq!(summary.closed, 1);
        assert_eq!(summary.entry_failed, 1);
        assert_eq!(summary.exit_failed, 1);
        assert_eq!(summary.total_realized_pnl, Decimal::new(15, 2));
        assert_eq!(summary.total_unrealized_pnl, Decimal::new(16, 2));
    }

    #[tokio::test]
    async fn active_positions_keep_every_market_with_exposure_subscribed() {
        let tracker = PositionTracker {
            repo: PositionRepository::new(
                sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgres://localhost/unused")
                    .unwrap(),
            ),
            active_positions: group_by_market(vec![
                make_position("a", PositionState::Open, Decimal::ZERO, None),
                make_position("b", PositionState::ExitFailed, Decimal::ZERO, None),
                make_position("c", PositionState::Closed, Decimal::ZERO, None),
                make_position("d", PositionState::EntryFailed, Decimal::ZERO, None),
            ]),
        };

        let mut markets: Vec<_> = tracker
            .get_active_positions()
            .into_iter()
            .map(|p| p.market_id.as_str())
            .collect();
        markets.sort_unstable();
        assert_eq!(markets, ["a", "b"]);
    }
}
