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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Position entry failed (order rejected/timeout).
    EntryFailed,
    /// Position exit failed (order rejected/timeout), needs retry.
    ExitFailed,
    /// Position is stalled (no updates for extended period), needs investigation.
    Stalled,
}

/// Reason for position failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    /// Order was rejected by the exchange.
    OrderRejected { message: String },
    /// Order timed out waiting for fill.
    OrderTimeout { elapsed_ms: u64 },
    /// Insufficient balance to execute.
    InsufficientBalance,
    /// Market is closed or not tradeable.
    MarketClosed,
    /// Price moved unfavorably before execution.
    PriceSlippage { expected: Decimal, actual: Decimal },
    /// Network or API connectivity issue.
    ConnectivityError { message: String },
    /// Position was stalled for too long without updates.
    StalePosition { last_update_secs: u64 },
    /// Unknown or unclassified failure.
    Unknown { message: String },
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
    /// Failure reason if position failed.
    pub failure_reason: Option<FailureReason>,
    /// Number of retry attempts made.
    pub retry_count: u32,
    /// Last time this position was updated (for stale detection).
    pub last_updated: DateTime<Utc>,
    /// State before entering Stalled (for reliable recovery).
    pub pre_stall_state: Option<PositionState>,
}

/// Maximum retry attempts before giving up.
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Stale threshold in seconds (5 minutes).
pub const STALE_THRESHOLD_SECS: u64 = 300;

impl Position {
    /// Create a new pending position.
    pub fn new(
        market_id: String,
        yes_entry_price: Decimal,
        no_entry_price: Decimal,
        quantity: Decimal,
        exit_strategy: ExitStrategy,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            market_id,
            yes_entry_price,
            no_entry_price,
            quantity,
            entry_timestamp: now,
            exit_strategy,
            state: PositionState::Pending,
            unrealized_pnl: Decimal::ZERO,
            realized_pnl: None,
            exit_timestamp: None,
            yes_exit_price: None,
            no_exit_price: None,
            failure_reason: None,
            retry_count: 0,
            last_updated: now,
            pre_stall_state: None,
        }
    }

    /// Total entry cost for this position.
    pub fn entry_cost(&self) -> Decimal {
        (self.yes_entry_price + self.no_entry_price) * self.quantity
    }

    /// Update unrealized P&L based on current market prices.
    /// `fee` is the fee **rate** (e.g. 0.02 = 2%), applied to notional value of each leg.
    pub fn update_pnl(&mut self, yes_bid: Decimal, no_bid: Decimal, fee: Decimal) {
        let entry_cost = self.entry_cost();
        match self.exit_strategy {
            ExitStrategy::ExitOnCorrection => {
                let exit_value = (yes_bid + no_bid) * self.quantity;
                // Fee = rate * notional on entry + rate * notional on exit
                let entry_fees = fee * entry_cost;
                let exit_fees = fee * exit_value;
                self.unrealized_pnl = exit_value - entry_cost - entry_fees - exit_fees;
            }
            ExitStrategy::HoldToResolution => {
                let guaranteed_return = Decimal::ONE * self.quantity;
                // Fee = rate * notional on entry only (resolution has no trading fee)
                let entry_fees = fee * entry_cost;
                self.unrealized_pnl = guaranteed_return - entry_cost - entry_fees;
            }
        }
    }

    /// Mark position as open (both sides purchased).
    /// Only valid from Pending state.
    pub fn mark_open(&mut self) -> std::result::Result<(), String> {
        if self.state != PositionState::Pending {
            return Err(format!(
                "Cannot transition to Open from {:?} (expected Pending)",
                self.state
            ));
        }
        self.state = PositionState::Open;
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Mark position as ready to exit.
    /// Only valid from Open state.
    pub fn mark_exit_ready(&mut self) -> std::result::Result<(), String> {
        if self.state != PositionState::Open {
            return Err(format!(
                "Cannot transition to ExitReady from {:?} (expected Open)",
                self.state
            ));
        }
        self.state = PositionState::ExitReady;
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Mark position as closing.
    /// Only valid from ExitReady state.
    pub fn mark_closing(&mut self) -> std::result::Result<(), String> {
        if self.state != PositionState::ExitReady {
            return Err(format!(
                "Cannot transition to Closing from {:?} (expected ExitReady)",
                self.state
            ));
        }
        self.state = PositionState::Closing;
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Close the position via market exit (selling both sides).
    /// `fee` is the fee **rate** (e.g. 0.02 = 2%), applied to notional value.
    ///
    /// Returns an error if the position is already closed or in a terminal state.
    pub fn close_via_exit(
        &mut self,
        yes_exit_price: Decimal,
        no_exit_price: Decimal,
        fee: Decimal,
    ) -> std::result::Result<(), String> {
        if self.state == PositionState::Closed {
            return Err("Position is already closed".to_string());
        }
        if self.state == PositionState::EntryFailed {
            return Err("Cannot close a position that failed to enter".to_string());
        }

        self.yes_exit_price = Some(yes_exit_price);
        self.no_exit_price = Some(no_exit_price);
        self.exit_timestamp = Some(Utc::now());
        self.state = PositionState::Closed;

        let exit_value = (yes_exit_price + no_exit_price) * self.quantity;
        let entry_cost = self.entry_cost();
        // Fee = rate * notional on entry + rate * notional on exit
        let entry_fees = fee * entry_cost;
        let exit_fees = fee * exit_value;
        self.realized_pnl = Some(exit_value - entry_cost - entry_fees - exit_fees);
        self.unrealized_pnl = Decimal::ZERO;
        Ok(())
    }

    /// Close the position via market resolution (guaranteed $1.00 per share).
    /// `fee` is the fee **rate** (e.g. 0.02 = 2%), applied to notional value.
    ///
    /// Returns an error if the position is already closed or in a terminal state.
    pub fn close_via_resolution(&mut self, fee: Decimal) -> std::result::Result<(), String> {
        if self.state == PositionState::Closed {
            return Err("Position is already closed".to_string());
        }
        if self.state == PositionState::EntryFailed {
            return Err("Cannot close a position that failed to enter".to_string());
        }

        self.exit_timestamp = Some(Utc::now());
        self.state = PositionState::Closed;

        let guaranteed_return = Decimal::ONE * self.quantity;
        let entry_cost = self.entry_cost();
        // Fee = rate * notional on entry only (resolution has no trading fee)
        let entry_fees = fee * entry_cost;
        self.realized_pnl = Some(guaranteed_return - entry_cost - entry_fees);
        self.unrealized_pnl = Decimal::ZERO;
        Ok(())
    }

    /// Check if position is still active (not closed or permanently failed).
    pub fn is_active(&self) -> bool {
        !matches!(
            self.state,
            PositionState::Closed | PositionState::EntryFailed
        )
    }

    /// Check if position needs recovery action.
    pub fn needs_recovery(&self) -> bool {
        matches!(
            self.state,
            PositionState::ExitFailed | PositionState::Stalled
        )
    }

    /// Check if position can be retried.
    pub fn can_retry(&self) -> bool {
        self.retry_count < MAX_RETRY_ATTEMPTS
    }

    /// Check if position is stale (no updates for extended period).
    pub fn is_stale(&self) -> bool {
        self.age_secs() > STALE_THRESHOLD_SECS
            && matches!(
                self.state,
                PositionState::Pending | PositionState::Open | PositionState::Closing
            )
    }

    /// Get the age of the position in seconds since last update.
    pub fn age_secs(&self) -> u64 {
        Utc::now()
            .signed_duration_since(self.last_updated)
            .num_seconds()
            .max(0) as u64
    }

    /// Mark position entry as failed.
    pub fn mark_entry_failed(&mut self, reason: FailureReason) {
        self.state = PositionState::EntryFailed;
        self.failure_reason = Some(reason);
        self.last_updated = Utc::now();
    }

    /// Mark position exit as failed (can be retried).
    pub fn mark_exit_failed(&mut self, reason: FailureReason) {
        self.state = PositionState::ExitFailed;
        self.failure_reason = Some(reason);
        self.retry_count += 1;
        self.last_updated = Utc::now();
    }

    /// Mark position as stalled, preserving the current state for recovery.
    pub fn mark_stalled(&mut self) {
        let elapsed = Utc::now()
            .signed_duration_since(self.last_updated)
            .num_seconds() as u64;
        self.pre_stall_state = Some(self.state);
        self.state = PositionState::Stalled;
        self.failure_reason = Some(FailureReason::StalePosition {
            last_update_secs: elapsed,
        });
        self.last_updated = Utc::now();
    }

    /// Attempt to recover from ExitFailed state by retrying exit.
    /// Returns true if recovery should be attempted, false if max retries exceeded.
    pub fn attempt_exit_recovery(&mut self) -> bool {
        if self.state != PositionState::ExitFailed {
            return false;
        }

        if self.retry_count >= MAX_RETRY_ATTEMPTS {
            return false;
        }

        // Move back to ExitReady to trigger exit again
        self.state = PositionState::ExitReady;
        self.failure_reason = None;
        self.last_updated = Utc::now();
        true
    }

    /// Attempt to recover from Stalled state.
    /// Returns the previous state the position should return to.
    ///
    /// Uses the saved `pre_stall_state` if available (set by `mark_stalled`).
    /// Falls back to heuristic detection for positions stalled before this field
    /// was introduced.
    pub fn attempt_stalled_recovery(&mut self) -> Option<PositionState> {
        if self.state != PositionState::Stalled {
            return None;
        }

        // Prefer the explicitly saved pre-stall state
        let recovered_state = if let Some(prev) = self.pre_stall_state.take() {
            prev
        } else {
            // Fallback heuristic for legacy positions without pre_stall_state.
            // is_stale() only fires for Pending, Open, and Closing states,
            // so the recovery target must be one of those three.
            if self.yes_exit_price.is_some() || self.no_exit_price.is_some() {
                // Had exit prices set → was in the middle of closing
                PositionState::ExitReady
            } else if self.yes_entry_price > Decimal::ZERO && self.no_entry_price > Decimal::ZERO {
                // Has valid entry prices → was likely open
                // (even if unrealized_pnl happens to be zero)
                PositionState::Open
            } else {
                PositionState::Pending
            }
        };

        self.state = recovered_state;
        self.failure_reason = None;
        self.retry_count += 1;
        self.last_updated = Utc::now();

        Some(recovered_state)
    }

    /// Touch the position to update last_updated timestamp.
    pub fn touch(&mut self) {
        self.last_updated = Utc::now();
    }

    /// Get a human-readable status message.
    pub fn status_message(&self) -> String {
        match &self.state {
            PositionState::Pending => "Awaiting entry execution".to_string(),
            PositionState::Open => format!("Open, P&L: {:.4}", self.unrealized_pnl),
            PositionState::ExitReady => "Ready to exit".to_string(),
            PositionState::Closing => "Exit in progress".to_string(),
            PositionState::Closed => {
                if let Some(pnl) = self.realized_pnl {
                    format!("Closed, realized P&L: {:.4}", pnl)
                } else {
                    "Closed".to_string()
                }
            }
            PositionState::EntryFailed => {
                if let Some(ref reason) = self.failure_reason {
                    format!("Entry failed: {:?}", reason)
                } else {
                    "Entry failed".to_string()
                }
            }
            PositionState::ExitFailed => {
                let retry_msg = if self.can_retry() {
                    format!(" (retry {}/{})", self.retry_count, MAX_RETRY_ATTEMPTS)
                } else {
                    " (max retries exceeded)".to_string()
                };
                if let Some(ref reason) = self.failure_reason {
                    format!("Exit failed: {:?}{}", reason, retry_msg)
                } else {
                    format!("Exit failed{}", retry_msg)
                }
            }
            PositionState::Stalled => {
                if let Some(FailureReason::StalePosition { last_update_secs }) =
                    &self.failure_reason
                {
                    format!("Stalled for {}s, needs investigation", last_update_secs)
                } else {
                    "Stalled, needs investigation".to_string()
                }
            }
        }
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

        pos.mark_open().unwrap();
        assert_eq!(pos.state, PositionState::Open);

        // Update P&L with current prices
        let fee = Decimal::new(2, 2); // 0.02
        pos.update_pnl(Decimal::new(50, 2), Decimal::new(50, 2), fee);
        // Exit value: (0.50 + 0.50) * 100 = 100
        // Entry cost: 94
        // Entry fees: 0.02 * 94 = 1.88
        // Exit fees: 0.02 * 100 = 2.00
        // Unrealized P&L: 100 - 94 - 1.88 - 2.00 = 2.12
        assert_eq!(pos.unrealized_pnl, Decimal::new(212, 2));

        pos.mark_exit_ready().unwrap();
        assert_eq!(pos.state, PositionState::ExitReady);

        pos.close_via_exit(Decimal::new(50, 2), Decimal::new(50, 2), fee)
            .unwrap();
        assert_eq!(pos.state, PositionState::Closed);
        assert_eq!(pos.realized_pnl, Some(Decimal::new(212, 2)));
        assert!(!pos.is_active());

        // Double-close should fail
        assert!(pos
            .close_via_exit(Decimal::new(50, 2), Decimal::new(50, 2), fee)
            .is_err());
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

        pos.mark_open().unwrap();
        let fee = Decimal::new(2, 2); // 0.02

        // For hold strategy, unrealized P&L is based on guaranteed $1 return
        pos.update_pnl(Decimal::new(40, 2), Decimal::new(40, 2), fee);
        // Guaranteed return: 1.00 * 100 = 100
        // Entry cost: 94
        // Entry fees: 0.02 * 94 = 1.88
        // Unrealized P&L: 100 - 94 - 1.88 = 4.12
        assert_eq!(pos.unrealized_pnl, Decimal::new(412, 2));

        pos.close_via_resolution(fee).unwrap();
        assert_eq!(pos.realized_pnl, Some(Decimal::new(412, 2)));

        // Double-close via resolution should fail
        assert!(pos.close_via_resolution(fee).is_err());
    }

    #[test]
    fn test_entry_failure() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        assert_eq!(pos.state, PositionState::Pending);
        assert!(pos.is_active());

        pos.mark_entry_failed(FailureReason::OrderRejected {
            message: "Insufficient balance".to_string(),
        });

        assert_eq!(pos.state, PositionState::EntryFailed);
        assert!(!pos.is_active()); // Entry failed positions are not active
        assert!(pos.failure_reason.is_some());
    }

    #[test]
    fn test_exit_failure_and_recovery() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        pos.mark_open().unwrap();
        pos.mark_exit_ready().unwrap();

        // First exit attempt fails
        pos.mark_exit_failed(FailureReason::OrderTimeout { elapsed_ms: 5000 });
        assert_eq!(pos.state, PositionState::ExitFailed);
        assert_eq!(pos.retry_count, 1);
        assert!(pos.needs_recovery());
        assert!(pos.can_retry());

        // Recovery attempt
        assert!(pos.attempt_exit_recovery());
        assert_eq!(pos.state, PositionState::ExitReady);
        assert!(pos.failure_reason.is_none());

        // Second exit attempt fails
        pos.mark_exit_failed(FailureReason::ConnectivityError {
            message: "Connection reset".to_string(),
        });
        assert_eq!(pos.retry_count, 2);

        // Third recovery
        assert!(pos.attempt_exit_recovery());
        pos.mark_exit_failed(FailureReason::OrderTimeout { elapsed_ms: 5000 });
        assert_eq!(pos.retry_count, 3);

        // Max retries reached
        assert!(!pos.can_retry());
        assert!(!pos.attempt_exit_recovery());
    }

    #[test]
    fn test_stalled_recovery() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        pos.mark_open().unwrap();
        let fee = Decimal::new(2, 2);
        pos.update_pnl(Decimal::new(50, 2), Decimal::new(50, 2), fee);

        // Simulate stall
        pos.mark_stalled();
        assert_eq!(pos.state, PositionState::Stalled);
        assert!(pos.needs_recovery());
        // Pre-stall state should be saved
        assert_eq!(pos.pre_stall_state, Some(PositionState::Open));

        // Recovery should return to Open state via pre_stall_state
        let recovered = pos.attempt_stalled_recovery();
        assert_eq!(recovered, Some(PositionState::Open));
        assert_eq!(pos.state, PositionState::Open);
        // pre_stall_state should be consumed
        assert_eq!(pos.pre_stall_state, None);
    }

    #[test]
    fn test_stalled_recovery_with_zero_pnl() {
        // Regression: open position with unrealized_pnl == 0 should still
        // recover to Open (not Pending).
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        pos.mark_open().unwrap();
        // Don't call update_pnl — unrealized_pnl stays at ZERO

        pos.mark_stalled();
        let recovered = pos.attempt_stalled_recovery();
        assert_eq!(recovered, Some(PositionState::Open));
        assert_eq!(pos.state, PositionState::Open);
    }

    #[test]
    fn test_stalled_recovery_from_closing() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        pos.mark_open().unwrap();
        pos.mark_exit_ready().unwrap();
        pos.mark_closing().unwrap();

        pos.mark_stalled();
        assert_eq!(pos.pre_stall_state, Some(PositionState::Closing));

        let recovered = pos.attempt_stalled_recovery();
        assert_eq!(recovered, Some(PositionState::Closing));
        assert_eq!(pos.state, PositionState::Closing);
    }

    #[test]
    fn test_stalled_recovery_from_pending() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        // Stall while still pending
        pos.mark_stalled();
        assert_eq!(pos.pre_stall_state, Some(PositionState::Pending));

        let recovered = pos.attempt_stalled_recovery();
        assert_eq!(recovered, Some(PositionState::Pending));
        assert_eq!(pos.state, PositionState::Pending);
    }

    #[test]
    fn test_invalid_state_transitions() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        // Cannot go directly to ExitReady from Pending
        assert!(pos.mark_exit_ready().is_err());
        // Cannot go directly to Closing from Pending
        assert!(pos.mark_closing().is_err());

        pos.mark_open().unwrap();
        // Cannot mark_open again
        assert!(pos.mark_open().is_err());
        // Cannot go to Closing from Open (must go through ExitReady)
        assert!(pos.mark_closing().is_err());
    }

    #[test]
    fn test_status_messages() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );

        assert!(pos.status_message().contains("Awaiting"));

        pos.mark_open().unwrap();
        assert!(pos.status_message().contains("Open"));

        pos.mark_exit_failed(FailureReason::OrderTimeout { elapsed_ms: 5000 });
        let msg = pos.status_message();
        assert!(msg.contains("Exit failed"));
        assert!(msg.contains("retry 1/3"));
    }
}
