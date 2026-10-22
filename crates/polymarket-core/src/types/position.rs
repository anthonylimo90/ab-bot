//! Position tracking types for arbitrage positions.

use super::market::{ArbOpportunity, FeeSchedule};
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

/// Fee model attached to a position at entry time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionFeeModel {
    /// Legacy flat-fee approximation using `ArbOpportunity::DEFAULT_FEE`.
    LegacyFlat,
    /// Share-based fee accounting using per-leg entry fee shares.
    ShareBased,
}

impl PositionFeeModel {
    pub fn from_i16(value: i16) -> Self {
        match value {
            1 => Self::ShareBased,
            _ => Self::LegacyFlat,
        }
    }

    pub fn as_i16(self) -> i16 {
        match self {
            Self::LegacyFlat => 0,
            Self::ShareBased => 1,
        }
    }
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
    /// Entry filled only one leg and needs dedicated recovery handling.
    OneLeggedEntry { held_leg: String, message: String },
    /// Position was stalled for too long without updates.
    StalePosition { last_update_secs: u64 },
    /// Unknown or unclassified failure.
    Unknown { message: String },
}

impl std::fmt::Display for FailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OrderRejected { message } => write!(f, "order rejected: {message}"),
            Self::OrderTimeout { elapsed_ms } => write!(f, "order timed out after {elapsed_ms} ms"),
            Self::InsufficientBalance => write!(f, "insufficient balance"),
            Self::MarketClosed => write!(f, "market closed"),
            Self::PriceSlippage { expected, actual } => {
                write!(f, "price moved from {expected} to {actual}")
            }
            Self::ConnectivityError { message } => write!(f, "connectivity error: {message}"),
            Self::OneLeggedEntry { held_leg, message } => {
                write!(f, "one-legged entry holding {held_leg}: {message}")
            }
            Self::StalePosition { last_update_secs } => {
                write!(f, "no update for {last_update_secs} s")
            }
            Self::Unknown { message } => write!(f, "{message}"),
        }
    }
}

/// An arbitrage position tracking both YES and NO holdings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Immutable origin of the inventory; unknown legacy rows cannot be traded.
    #[serde(default = "unknown_execution_mode")]
    pub execution_mode: String,
    #[serde(default)]
    pub wallet_address: Option<String>,
    #[serde(default)]
    pub version: i64,
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
    /// Entry-time fee model for P&L calculations.
    pub fee_model: PositionFeeModel,
    /// Worst-case resolution payout per share after entry-time fees.
    pub resolution_payout_per_share: Decimal,
    /// Buy-side fee charged in YES shares at entry.
    pub yes_entry_fee_shares: Decimal,
    /// Buy-side fee charged in NO shares at entry.
    pub no_entry_fee_shares: Decimal,
    /// Taker fee terms of the position's market, when known (arbitrage
    /// positions record those they were priced with).
    #[serde(default)]
    pub taker_fees: Option<FeeSchedule>,
    /// Taker fees (USDC) charged to the position's fills so far, under
    /// `taker_fees`. Deducted from the P&L of share-based positions.
    #[serde(default)]
    pub fees_paid: Decimal,
    /// Quantity of YES tokens currently held (not yet exited).
    pub held_yes_qty: Decimal,
    /// Quantity of NO tokens currently held (not yet exited).
    pub held_no_qty: Decimal,
    /// Total YES tokens sold or resolved out.
    pub exited_yes_qty: Decimal,
    /// Total NO tokens sold or resolved out.
    pub exited_no_qty: Decimal,
    /// "yes" or "no" when the market has resolved; None while open.
    pub resolution_winner: Option<String>,
}

/// Maximum retry attempts before giving up.
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Stale threshold in seconds (5 minutes).
pub const STALE_THRESHOLD_SECS: u64 = 300;

fn unknown_execution_mode() -> String {
    "unknown".into()
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
        let now = Utc::now();
        let legacy_entry_fee_drag =
            (yes_entry_price + no_entry_price) * ArbOpportunity::DEFAULT_FEE;
        Self {
            execution_mode: "unknown".into(),
            wallet_address: None,
            version: 0,
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
            fee_model: PositionFeeModel::LegacyFlat,
            resolution_payout_per_share: (Decimal::ONE - legacy_entry_fee_drag).max(Decimal::ZERO),
            yes_entry_fee_shares: Decimal::ZERO,
            no_entry_fee_shares: Decimal::ZERO,
            taker_fees: None,
            fees_paid: Decimal::ZERO,
            held_yes_qty: Decimal::ZERO,
            held_no_qty: Decimal::ZERO,
            exited_yes_qty: Decimal::ZERO,
            exited_no_qty: Decimal::ZERO,
            resolution_winner: None,
        }
    }

    /// Total entry cost for this position.
    pub fn entry_cost(&self) -> Decimal {
        let yes_qty = self.held_yes_qty + self.exited_yes_qty;
        let no_qty = self.held_no_qty + self.exited_no_qty;
        if yes_qty > Decimal::ZERO || no_qty > Decimal::ZERO {
            self.yes_entry_price * yes_qty + self.no_entry_price * no_qty
        } else {
            (self.yes_entry_price + self.no_entry_price) * self.quantity
        }
    }

    /// Apply share-based fee terms from an arbitrage opportunity.
    pub fn apply_arb_fee_model(&mut self, arb: &ArbOpportunity) {
        self.fee_model = PositionFeeModel::ShareBased;
        self.resolution_payout_per_share = arb.worst_case_payout.max(Decimal::ZERO);
        self.yes_entry_fee_shares = arb.yes_fee_shares.max(Decimal::ZERO);
        self.no_entry_fee_shares = arb.no_fee_shares.max(Decimal::ZERO);
        self.taker_fees = arb.fee_schedule;
    }

    /// Charge the taker fee of a fill of `quantity` shares at `price` under
    /// the position's fee terms (none when they are unknown). Returns it.
    pub fn charge_taker_fee(&mut self, price: Decimal, quantity: Decimal) -> Decimal {
        let fee = self
            .taker_fees
            .map(|terms| terms.fee_per_share(price) * quantity.max(Decimal::ZERO))
            .unwrap_or_default();
        self.fees_paid += fee;
        fee
    }

    /// What is deducted from the P&L for fees: the fees charged to the fills
    /// of a share-based position, or the flat `rate` on `volume` otherwise.
    pub fn fee_charges(&self, rate: Decimal, volume: Decimal) -> Decimal {
        match self.fee_model {
            PositionFeeModel::ShareBased => self.fees_paid,
            PositionFeeModel::LegacyFlat => rate * volume,
        }
    }

    fn net_yes_shares(&self) -> Decimal {
        self.quantity * (Decimal::ONE - self.yes_entry_fee_shares).max(Decimal::ZERO)
    }

    fn net_no_shares(&self) -> Decimal {
        self.quantity * (Decimal::ONE - self.no_entry_fee_shares).max(Decimal::ZERO)
    }

    /// Update unrealized P&L based on current market prices.
    /// `fee` is only used for legacy flat-fee positions.
    pub fn update_pnl(&mut self, yes_bid: Decimal, no_bid: Decimal, fee: Decimal) {
        let entry_cost = self.entry_cost();
        if self.execution_mode != "unknown" && self.has_open_exposure() {
            let proceeds = self.yes_exit_price.unwrap_or_default() * self.exited_yes_qty
                + self.no_exit_price.unwrap_or_default() * self.exited_no_qty;
            let remaining = match self.exit_strategy {
                ExitStrategy::ExitOnCorrection => {
                    yes_bid * self.held_yes_qty + no_bid * self.held_no_qty
                }
                ExitStrategy::HoldToResolution => self.held_yes_qty.min(self.held_no_qty),
            };
            self.unrealized_pnl = proceeds + remaining
                - entry_cost
                - self.fee_charges(fee, entry_cost + proceeds + remaining);
            self.last_updated = Utc::now();
            return;
        }
        match self.exit_strategy {
            ExitStrategy::ExitOnCorrection => {
                self.unrealized_pnl = match self.fee_model {
                    PositionFeeModel::LegacyFlat => {
                        let exit_value = (yes_bid + no_bid) * self.quantity;
                        let entry_fees = fee * entry_cost;
                        let exit_fees = fee * exit_value;
                        exit_value - entry_cost - entry_fees - exit_fees
                    }
                    // Share-based fees are taken at entry; live exit value reflects the
                    // smaller YES/NO share balances that remain after those fees.
                    PositionFeeModel::ShareBased => {
                        let exit_value =
                            (yes_bid * self.net_yes_shares()) + (no_bid * self.net_no_shares());
                        exit_value - entry_cost
                    }
                };
            }
            ExitStrategy::HoldToResolution => {
                self.unrealized_pnl = match self.fee_model {
                    PositionFeeModel::LegacyFlat => {
                        let guaranteed_return = Decimal::ONE * self.quantity;
                        let entry_fees = fee * entry_cost;
                        guaranteed_return - entry_cost - entry_fees
                    }
                    PositionFeeModel::ShareBased => {
                        (self.resolution_payout_per_share * self.quantity) - entry_cost
                    }
                };
            }
        }
        self.last_updated = Utc::now();
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

    /// Record the realized exit fill for the YES leg while the position is still active.
    pub fn record_yes_exit_fill(
        &mut self,
        yes_exit_price: Decimal,
    ) -> std::result::Result<(), String> {
        if self.yes_entry_price <= Decimal::ZERO {
            return Err("Position does not hold a YES leg".to_string());
        }
        if self.yes_exit_price.is_some() {
            return Err("YES leg is already recorded as exited".to_string());
        }
        if self.state == PositionState::Closed {
            return Err("Cannot record an exit fill on a closed position".to_string());
        }
        if self.state == PositionState::EntryFailed {
            return Err("Cannot record an exit fill on an entry-failed position".to_string());
        }

        self.yes_exit_price = Some(yes_exit_price);
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Record the realized exit fill for the NO leg while the position is still active.
    pub fn record_no_exit_fill(
        &mut self,
        no_exit_price: Decimal,
    ) -> std::result::Result<(), String> {
        if self.no_entry_price <= Decimal::ZERO {
            return Err("Position does not hold a NO leg".to_string());
        }
        if self.no_exit_price.is_some() {
            return Err("NO leg is already recorded as exited".to_string());
        }
        if self.state == PositionState::Closed {
            return Err("Cannot record an exit fill on a closed position".to_string());
        }
        if self.state == PositionState::EntryFailed {
            return Err("Cannot record an exit fill on an entry-failed position".to_string());
        }

        self.no_exit_price = Some(no_exit_price);
        self.last_updated = Utc::now();
        Ok(())
    }

    pub fn held_outcomes(&self) -> (bool, bool) {
        if self.state == PositionState::Closed {
            return (false, false);
        }

        let has_yes = self.held_yes_qty > Decimal::ZERO;
        let has_no = self.held_no_qty > Decimal::ZERO;
        (has_yes, has_no)
    }

    /// True if both YES and NO legs have explicit remaining inventory.
    pub fn has_full_pair_exposure(&self) -> bool {
        self.held_yes_qty > Decimal::ZERO && self.held_no_qty > Decimal::ZERO
    }

    // ── Per-leg exposure tracking (explicit qty fields) ─────────────

    /// Record a YES leg entry fill. Increments held_yes_qty.
    pub fn apply_yes_entry_fill(&mut self, fill_qty: Decimal) {
        self.held_yes_qty += fill_qty;
        self.last_updated = Utc::now();
    }

    /// Record a NO leg entry fill. Increments held_no_qty.
    pub fn apply_no_entry_fill(&mut self, fill_qty: Decimal) {
        self.held_no_qty += fill_qty;
        self.last_updated = Utc::now();
    }

    /// Record a YES exit fill; moves qty from held to exited.
    pub fn apply_yes_exit_fill(&mut self, fill_qty: Decimal) -> std::result::Result<(), String> {
        if fill_qty > self.held_yes_qty {
            return Err(format!(
                "YES exit fill {} exceeds held qty {}",
                fill_qty, self.held_yes_qty
            ));
        }
        self.held_yes_qty -= fill_qty;
        self.exited_yes_qty += fill_qty;
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Record a NO exit fill; moves qty from held to exited.
    pub fn apply_no_exit_fill(&mut self, fill_qty: Decimal) -> std::result::Result<(), String> {
        if fill_qty > self.held_no_qty {
            return Err(format!(
                "NO exit fill {} exceeds held qty {}",
                fill_qty, self.held_no_qty
            ));
        }
        self.held_no_qty -= fill_qty;
        self.exited_no_qty += fill_qty;
        self.last_updated = Utc::now();
        Ok(())
    }

    /// Returns (has_yes, has_no) from explicit qty fields.
    /// Explicit counterpart to [`held_outcomes`] which infers from prices.
    pub fn held_legs(&self) -> (bool, bool) {
        (
            self.held_yes_qty > Decimal::ZERO,
            self.held_no_qty > Decimal::ZERO,
        )
    }

    /// True if any exposure exists (YES or NO still held).
    pub fn has_open_exposure(&self) -> bool {
        self.held_yes_qty > Decimal::ZERO || self.held_no_qty > Decimal::ZERO
    }

    /// Write off `quantity` of the YES (or NO) holding that the wallet does
    /// not have. Returns the quantity written off (at most the holding).
    ///
    /// The shares count as exited for no proceeds, so their cost lands in the
    /// realized P&L. The V1 exchange kept a buy's fee in shares, which this
    /// books as the fee it was; the V2 exchange (since 2026-04-28) charges
    /// every fee in collateral, so a shortfall is never a fee already in
    /// `fees_paid`, and none is taken back.
    pub fn write_off_missing_shares(&mut self, yes: bool, quantity: Decimal) -> Decimal {
        let (held, exited, price) = if yes {
            (
                &mut self.held_yes_qty,
                &mut self.exited_yes_qty,
                &mut self.yes_exit_price,
            )
        } else {
            (
                &mut self.held_no_qty,
                &mut self.exited_no_qty,
                &mut self.no_exit_price,
            )
        };
        let missing = quantity.max(Decimal::ZERO).min(*held);
        if missing.is_zero() {
            return Decimal::ZERO;
        }
        let proceeds = price.unwrap_or_default() * *exited;
        *exited += missing;
        *held -= missing;
        *price = Some(proceeds / *exited);
        self.last_updated = Utc::now();
        missing
    }

    /// Writes off per-leg remainders below the smallest sellable quantity.
    ///
    /// Market sells carry whole hundredths of a share, so a remainder such as
    /// 0.0033 shares can never be sold. It is recorded as exited at zero
    /// proceeds: the weighted exit price is diluted so the leg's recorded
    /// proceeds are unchanged. Returns the written-off `(yes, no)` quantities.
    pub fn write_off_unsellable_dust(&mut self) -> (Decimal, Decimal) {
        fn write_off(
            held: &mut Decimal,
            exited: &mut Decimal,
            price: &mut Option<Decimal>,
        ) -> Decimal {
            let dust = *held;
            if dust <= Decimal::ZERO || dust >= super::order::MIN_SELLABLE_SHARES {
                return Decimal::ZERO;
            }
            let proceeds = price.unwrap_or_default() * *exited;
            *exited += dust;
            *held = Decimal::ZERO;
            *price = Some(proceeds / *exited);
            dust
        }

        let yes = write_off(
            &mut self.held_yes_qty,
            &mut self.exited_yes_qty,
            &mut self.yes_exit_price,
        );
        let no = write_off(
            &mut self.held_no_qty,
            &mut self.exited_no_qty,
            &mut self.no_exit_price,
        );
        if yes > Decimal::ZERO || no > Decimal::ZERO {
            self.last_updated = Utc::now();
        }
        (yes, no)
    }

    /// Requeues a failed exit for another attempt, regardless of how many
    /// attempts failed; callers pace retries (see the exit handler backoff).
    pub fn requeue_failed_exit(&mut self) -> bool {
        if self.state != PositionState::ExitFailed {
            return false;
        }
        self.state = PositionState::ExitReady;
        self.failure_reason = None;
        self.last_updated = Utc::now();
        true
    }

    /// Sends a stalled position that still holds shares straight to the exit
    /// queue. Without holdings there is nothing to sell, so nothing changes.
    pub fn requeue_stalled_for_exit(&mut self) -> bool {
        if self.state != PositionState::Stalled || !self.has_open_exposure() {
            return false;
        }
        self.state = PositionState::ExitReady;
        self.pre_stall_state = None;
        self.failure_reason = None;
        self.last_updated = Utc::now();
        true
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

    /// Whether this row should stay visible as an open position in the API.
    ///
    /// Most non-closed states remain open. The one exception is `EntryFailed`,
    /// which is normally terminal unless it represents a one-legged arb entry
    /// that still holds live market exposure.
    pub fn should_persist_as_open(&self) -> bool {
        match self.state {
            PositionState::Closed => false,
            PositionState::EntryFailed => self.is_one_legged_entry_fail(),
            _ => true,
        }
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

    /// Check if this is a one-legged entry failure (YES filled but NO failed).
    pub fn is_one_legged_entry_fail(&self) -> bool {
        if self.state != PositionState::EntryFailed {
            return false;
        }
        match &self.failure_reason {
            Some(FailureReason::OneLeggedEntry { .. }) => true,
            Some(FailureReason::OrderRejected { message })
            | Some(FailureReason::ConnectivityError { message }) => {
                let lower = message.to_lowercase();
                lower.contains("one-legged") || lower.contains("one_legged")
            }
            _ => false,
        }
    }

    /// Recover a one-legged entry-failed position back to Open state.
    /// Should only be called after the missing NO leg has been successfully placed.
    pub fn recover_one_legged_to_open(&mut self) -> std::result::Result<(), String> {
        if self.state != PositionState::EntryFailed {
            return Err(format!(
                "Cannot recover one-legged from {:?} (expected EntryFailed)",
                self.state
            ));
        }
        if !self.is_one_legged_entry_fail() {
            return Err("Position is not a one-legged entry failure".to_string());
        }
        if self.retry_count >= MAX_RETRY_ATTEMPTS {
            return Err(format!(
                "Max retries exceeded ({}/{})",
                self.retry_count, MAX_RETRY_ATTEMPTS
            ));
        }
        self.state = PositionState::Open;
        self.failure_reason = None;
        self.retry_count += 1;
        self.last_updated = Utc::now();
        Ok(())
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
                    format!("Entry failed: {reason}")
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
                    format!("Exit failed: {reason}{retry_msg}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsellable_dust_is_written_off_without_changing_proceeds() {
        let mut p = Position::new(
            "m".into(),
            Decimal::new(48, 2),
            Decimal::ZERO,
            Decimal::ONE,
            ExitStrategy::ExitOnCorrection,
        );
        p.held_yes_qty = "52.0833".parse().unwrap();
        p.state = PositionState::Closing;
        p.held_yes_qty -= "52.08".parse::<Decimal>().unwrap();
        p.exited_yes_qty = "52.08".parse().unwrap();
        p.yes_exit_price = Some(Decimal::new(50, 2));

        let (yes, no) = p.write_off_unsellable_dust();
        assert_eq!(yes, "0.0033".parse().unwrap());
        assert_eq!(no, Decimal::ZERO);
        assert!(!p.has_open_exposure());
        assert_eq!(p.exited_yes_qty, "52.0833".parse().unwrap());
        let proceeds = p.yes_exit_price.unwrap() * p.exited_yes_qty;
        assert_eq!(proceeds.round_dp(8), "26.04".parse().unwrap());

        // A whole sellable lot is never written off.
        p.held_no_qty = Decimal::new(1, 2);
        assert_eq!(
            p.write_off_unsellable_dust(),
            (Decimal::ZERO, Decimal::ZERO)
        );
        assert!(p.has_open_exposure());
    }

    #[test]
    fn failed_exits_requeue_past_the_retry_count() {
        let mut p = Position::new(
            "m".into(),
            Decimal::new(48, 2),
            Decimal::ZERO,
            Decimal::ONE,
            ExitStrategy::ExitOnCorrection,
        );
        p.held_yes_qty = Decimal::ONE;
        for _ in 0..(MAX_RETRY_ATTEMPTS + 2) {
            p.mark_exit_failed(FailureReason::Unknown {
                message: "no bids".into(),
            });
        }
        assert!(p.requeue_failed_exit());
        assert_eq!(p.state, PositionState::ExitReady);
        assert!(!p.requeue_failed_exit());
    }

    #[test]
    fn stalled_positions_requeue_for_exit_only_with_holdings() {
        let mut p = Position::new(
            "m".into(),
            Decimal::new(48, 2),
            Decimal::ZERO,
            Decimal::ONE,
            ExitStrategy::ExitOnCorrection,
        );
        p.mark_stalled();
        assert!(!p.requeue_stalled_for_exit());
        assert_eq!(p.state, PositionState::Stalled);

        p.held_yes_qty = Decimal::ONE;
        assert!(p.requeue_stalled_for_exit());
        assert_eq!(p.state, PositionState::ExitReady);
        assert_eq!(p.pre_stall_state, None);
    }

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

        pos.apply_yes_entry_fill(pos.quantity);
        pos.apply_no_entry_fill(pos.quantity);
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
    }

    #[test]
    fn test_share_fee_model_resolution_pnl_uses_stored_payout() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::HoldToResolution,
        );
        let arb = ArbOpportunity {
            market_id: "market123".to_string(),
            timestamp: Utc::now(),
            yes_ask: Decimal::new(48, 2),
            no_ask: Decimal::new(46, 2),
            total_cost: Decimal::new(94, 2),
            gross_profit: Decimal::new(6, 2),
            net_profit: Decimal::new(4, 2),
            fee_drag: Decimal::new(2, 2),
            worst_case_payout: Decimal::new(98, 2),
            yes_fee_shares: Decimal::new(1, 2),
            no_fee_shares: Decimal::new(2, 2),
            fee_schedule: None,
        };
        pos.apply_arb_fee_model(&arb);
        pos.mark_open().unwrap();

        pos.update_pnl(
            Decimal::new(10, 2),
            Decimal::new(10, 2),
            Decimal::new(99, 2),
        );
        assert_eq!(pos.fee_model, PositionFeeModel::ShareBased);
        assert_eq!(pos.unrealized_pnl, Decimal::new(400, 2));
    }

    #[test]
    fn test_held_outcomes_ignore_recorded_exit_legs() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );
        pos.apply_yes_entry_fill(pos.quantity);
        pos.apply_no_entry_fill(pos.quantity);
        pos.mark_open().unwrap();
        pos.record_yes_exit_fill(Decimal::new(52, 2)).unwrap();

        pos.apply_yes_exit_fill(pos.quantity).unwrap();
        assert_eq!(pos.held_outcomes(), (false, true));
    }

    #[test]
    fn test_share_fee_model_exit_on_correction_uses_net_shares() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );
        let arb = ArbOpportunity {
            market_id: "market123".to_string(),
            timestamp: Utc::now(),
            yes_ask: Decimal::new(48, 2),
            no_ask: Decimal::new(46, 2),
            total_cost: Decimal::new(94, 2),
            gross_profit: Decimal::new(6, 2),
            net_profit: Decimal::new(4, 2),
            fee_drag: Decimal::new(2, 2),
            worst_case_payout: Decimal::new(98, 2),
            yes_fee_shares: Decimal::new(1, 2),
            no_fee_shares: Decimal::new(2, 2),
            fee_schedule: None,
        };
        pos.apply_arb_fee_model(&arb);
        pos.apply_yes_entry_fill(pos.quantity);
        pos.apply_no_entry_fill(pos.quantity);
        pos.mark_open().unwrap();
        pos.mark_exit_ready().unwrap();

        pos.update_pnl(
            Decimal::new(50, 2),
            Decimal::new(50, 2),
            Decimal::new(99, 2),
        );
        assert_eq!(pos.unrealized_pnl, Decimal::new(450, 2));
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
    fn test_partial_exit_recovery_preserves_recorded_leg_fill() {
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
        pos.record_yes_exit_fill(Decimal::new(52, 2)).unwrap();
        pos.mark_exit_failed(FailureReason::ConnectivityError {
            message: "NO sell error".to_string(),
        });

        assert_eq!(pos.yes_exit_price, Some(Decimal::new(52, 2)));
        assert_eq!(pos.no_exit_price, None);

        assert!(pos.attempt_exit_recovery());
        pos.mark_closing().unwrap();
        pos.record_no_exit_fill(Decimal::new(49, 2)).unwrap();

        assert_eq!(pos.yes_exit_price, Some(Decimal::new(52, 2)));
        assert_eq!(pos.no_exit_price, Some(Decimal::new(49, 2)));
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
    fn test_update_pnl_refreshes_last_updated() {
        let mut pos = Position::new(
            "market123".to_string(),
            Decimal::new(48, 2),
            Decimal::new(46, 2),
            Decimal::new(100, 0),
            ExitStrategy::ExitOnCorrection,
        );
        pos.mark_open().unwrap();
        let old_last_updated = Utc::now() - chrono::Duration::minutes(10);
        pos.last_updated = old_last_updated;

        pos.update_pnl(Decimal::new(50, 2), Decimal::new(50, 2), Decimal::new(2, 2));

        assert!(pos.last_updated > old_last_updated);
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

    #[test]
    fn failure_reasons_read_as_text() {
        let reason = FailureReason::OrderRejected {
            message: "not enough balance".into(),
        };
        assert_eq!(reason.to_string(), "order rejected: not enough balance");
        let one_legged = FailureReason::OneLeggedEntry {
            held_leg: "yes".into(),
            message: "NO leg unfilled".into(),
        };
        assert!(one_legged.to_string().contains("one-legged"));
    }

    #[test]
    fn missing_shares_count_as_exited_for_nothing() {
        for model in [PositionFeeModel::ShareBased, PositionFeeModel::LegacyFlat] {
            let mut pos = Position::new(
                "m".into(),
                Decimal::new(50, 2),
                Decimal::ZERO,
                Decimal::from(100),
                ExitStrategy::ExitOnCorrection,
            );
            pos.fee_model = model;
            pos.taker_fees = Some(FeeSchedule::CONSERVATIVE);
            pos.fees_paid = Decimal::from(2);
            // 100 bought, then a partial sale of 10 at 0.60.
            pos.held_yes_qty = Decimal::from(90);
            pos.exited_yes_qty = Decimal::from(10);
            pos.yes_exit_price = Some(Decimal::new(60, 2));

            let written_off = pos.write_off_missing_shares(true, Decimal::new(35, 1));
            assert_eq!(written_off, Decimal::new(35, 1));
            assert_eq!(pos.held_yes_qty, Decimal::new(865, 1));
            assert_eq!(pos.exited_yes_qty, Decimal::new(135, 1));
            // Proceeds unchanged (10 × 0.60 = 6), spread over the larger quantity.
            assert_eq!(
                (pos.yes_exit_price.unwrap() * pos.exited_yes_qty).round_dp(20),
                Decimal::from(6)
            );
            // Fees charged in collateral stay charged.
            assert_eq!(pos.fees_paid, Decimal::from(2));
            // Never more than is held, never a negative amount, other leg untouched.
            assert_eq!(
                pos.write_off_missing_shares(true, Decimal::from(1000)),
                Decimal::new(865, 1)
            );
            assert_eq!(pos.held_yes_qty, Decimal::ZERO);
            assert_eq!(
                pos.write_off_missing_shares(false, Decimal::ONE),
                Decimal::ZERO
            );
            assert_eq!(
                pos.write_off_missing_shares(true, -Decimal::ONE),
                Decimal::ZERO
            );
        }
    }
}
