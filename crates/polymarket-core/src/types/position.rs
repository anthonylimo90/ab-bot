//! Position tracking types for arbitrage positions.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Exit strategy for an arbitrage position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitStrategy {
    /// Hold both positions until market resolves and collect $1.00.
    HoldToResolution,
    /// Exit when spread normalizes back to ~$1.00.
    ExitOnCorrection,
}

/// Current state of a position in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionState {
    /// Entry signal detected, awaiting execution.
    Pending,
    /// Both sides purchased, actively monitoring.
    Open,
    /// Spread normalized, exit opportunity available.
    ExitReady,
    /// Exit initiated, awaiting confirmation.
    Closing,
    /// Position fully closed or resolved.
    Closed,
}

/// An arbitrage position tracking both YES and NO holdings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Unique identifier for this position.
    pub id: Uuid,
    /// Polymarket market identifier.
    pub market_id: String,
    /// Price paid for YES outcome.
    pub yes_entry_price: Decimal,
    /// Price paid for NO outcome.
    pub no_entry_price: Decimal,
    /// Number of shares purchased.
    pub quantity: Decimal,
    /// When the position was opened.
    pub entry_timestamp: DateTime<Utc>,
    /// Strategy for exiting this position.
    pub exit_strategy: ExitStrategy,
    /// Current lifecycle state.
    pub state: PositionState,
    /// Current unrealized P&L based on live prices.
    pub unrealized_pnl: Decimal,
    /// Final P&L after position is closed.
    pub realized_pnl: Option<Decimal>,
    /// When the position was closed (if applicable).
    pub exit_timestamp: Option<DateTime<Utc>>,
    /// Exit prices if position was sold (not held to resolution).
    pub yes_exit_price: Option<Decimal>,
    pub no_exit_price: Option<Decimal>,
}

impl Position {
    /// Create a new pending position.
    pub fn new(
        market_id: String,
        yes_entry_price: Decimal,
        no_entry_price: Decimal,
        quantity: Decimal,
        exit_strategy: ExitStrategy,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            market_id,
            yes_entry_price,
            no_entry_price,
            quantity,
            entry_timestamp: Utc::now(),
            exit_strategy,
            state: PositionState::Pending,
            unrealized_pnl: Decimal::ZERO,
            realized_pnl: None,
            exit_timestamp: None,
            yes_exit_price: None,
            no_exit_price: None,
        }
    }

    /// Total entry cost for this position.
    pub fn entry_cost(&self) -> Decimal {
        (self.yes_entry_price + self.no_entry_price) * self.quantity
    }

    /// Update unrealized P&L based on current market prices.
    pub fn update_pnl(&mut self, yes_bid: Decimal, no_bid: Decimal, fee: Decimal) {
        match self.exit_strategy {
            ExitStrategy::ExitOnCorrection => {
                let exit_value = (yes_bid + no_bid) * self.quantity;
                let entry_cost = self.entry_cost();
                // Fee on both entry and exit
                let total_fees = fee * Decimal::TWO * self.quantity;
                self.unrealized_pnl = exit_value - entry_cost - total_fees;
            }
            ExitStrategy::HoldToResolution => {
                let guaranteed_return = Decimal::ONE * self.quantity;
                let entry_cost = self.entry_cost();
                let total_fees = fee * self.quantity;
                self.unrealized_pnl = guaranteed_return - entry_cost - total_fees;
            }
        }
    }

    /// Mark position as open (both sides purchased).
    pub fn mark_open(&mut self) {
        self.state = PositionState::Open;
    }

    /// Mark position as ready to exit.
    pub fn mark_exit_ready(&mut self) {
        self.state = PositionState::ExitReady;
    }

    /// Mark position as closing.
    pub fn mark_closing(&mut self) {
        self.state = PositionState::Closing;
    }

    /// Close the position via market exit.
    pub fn close_via_exit(
        &mut self,
        yes_exit_price: Decimal,
        no_exit_price: Decimal,
        fee: Decimal,
    ) {
        self.yes_exit_price = Some(yes_exit_price);
        self.no_exit_price = Some(no_exit_price);
        self.exit_timestamp = Some(Utc::now());
        self.state = PositionState::Closed;

        let exit_value = (yes_exit_price + no_exit_price) * self.quantity;
        let entry_cost = self.entry_cost();
        let total_fees = fee * Decimal::TWO * self.quantity;
        self.realized_pnl = Some(exit_value - entry_cost - total_fees);
        self.unrealized_pnl = Decimal::ZERO;
    }

    /// Close the position via market resolution.
    pub fn close_via_resolution(&mut self, fee: Decimal) {
        self.exit_timestamp = Some(Utc::now());
        self.state = PositionState::Closed;

        let guaranteed_return = Decimal::ONE * self.quantity;
        let entry_cost = self.entry_cost();
        let total_fees = fee * self.quantity;
        self.realized_pnl = Some(guaranteed_return - entry_cost - total_fees);
        self.unrealized_pnl = Decimal::ZERO;
    }

    /// Check if position is still active (not closed).
    pub fn is_active(&self) -> bool {
        self.state != PositionState::Closed
    }
}

/// Summary statistics for position performance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PositionStats {
    pub total_positions: u64,
    pub open_positions: u64,
    pub closed_positions: u64,
    pub total_realized_pnl: Decimal,
    pub total_unrealized_pnl: Decimal,
    pub win_count: u64,
    pub loss_count: u64,
}

impl PositionStats {
    pub fn win_rate(&self) -> Option<f64> {
        let total = self.win_count + self.loss_count;
        if total == 0 {
            None
        } else {
            Some(self.win_count as f64 / total as f64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_lifecycle() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2), // 0.48
            Decimal::new(46, 2), // 0.46
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        // Entry cost: (0.48 + 0.46) * 100 = 94
        assert_eq!(pos.entry_cost(), Decimal::new(94, 0));
        assert_eq!(pos.state, PositionState::Pending);

        pos.mark_open();
        assert_eq!(pos.state, PositionState::Open);

        // Update P&L with current prices
        let fee = Decimal::new(2, 2); // 0.02
        pos.update_pnl(Decimal::new(50, 2), Decimal::new(50, 2), fee);
        // Exit value: (0.50 + 0.50) * 100 = 100
        // Entry cost: 94
        // Fees: 0.02 * 2 * 100 = 4
        // Unrealized P&L: 100 - 94 - 4 = 2
        assert_eq!(pos.unrealized_pnl, Decimal::new(2, 0));

        pos.mark_exit_ready();
        assert_eq!(pos.state, PositionState::ExitReady);

        pos.close_via_exit(Decimal::new(50, 2), Decimal::new(50, 2), fee);
        assert_eq!(pos.state, PositionState::Closed);
        assert_eq!(pos.realized_pnl, Some(Decimal::new(2, 0)));
        assert!(!pos.is_active());
    }

    #[test]
    fn test_hold_to_resolution() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2), // 0.48
            Decimal::new(46, 2), // 0.46
            Decimal::new(100, 0),
            ExitStrategy::HoldToResolution,
        );

        pos.mark_open();
        let fee = Decimal::new(2, 2); // 0.02

        // For hold strategy, unrealized P&L is based on guaranteed $1 return
        pos.update_pnl(Decimal::new(40, 2), Decimal::new(40, 2), fee);
        // Guaranteed return: 1.00 * 100 = 100
        // Entry cost: 94
        // Fees: 0.02 * 100 = 2
        // Unrealized P&L: 100 - 94 - 2 = 4
        assert_eq!(pos.unrealized_pnl, Decimal::new(4, 0));

        pos.close_via_resolution(fee);
        assert_eq!(pos.realized_pnl, Some(Decimal::new(4, 0)));
    }
}
