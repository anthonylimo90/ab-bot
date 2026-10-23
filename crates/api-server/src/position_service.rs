//! Centralized service for all position state mutations.
//!
//! Mutations validate a caller snapshot and persist it with an optimistic version
//! check. Stale writes fail and must be reloaded. Live fill receipts and inventory
//! are committed atomically; analytics events are published after persistence.

use anyhow::anyhow;
use polymarket_core::db::positions::{EntryLimits, PositionRepository};
use polymarket_core::types::{
    ArbOpportunity, ExitStrategy, FailureReason, Position, PositionState,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::trade_events::{NewTradeEvent, TradeEventRecorder, TradeEventUpdate};

// ── Supporting types ───────────────────────────────────────────────────

/// Which leg of a position is being operated on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    Yes,
    No,
}

impl std::fmt::Display for Leg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Leg::Yes => write!(f, "yes"),
            Leg::No => write!(f, "no"),
        }
    }
}

/// How a position was finally closed.
#[derive(Debug, Clone)]
pub enum CloseMethod {
    /// Sold held legs on the open market.
    MarketExit { fee: Decimal },
    /// Market resolved; winner is known.
    ResolutionWithWinner { yes_wins: bool, fee: Decimal },
    /// Market resolved; winner unknown. Only equal legs: each pair pays one
    /// dollar whichever side won.
    ResolutionConservative { fee: Decimal },
}

/// Parameters for creating a new position.
#[derive(Debug, Clone)]
pub struct CreatePositionParams {
    pub market_id: String,
    pub yes_entry_price: Decimal,
    pub no_entry_price: Decimal,
    pub quantity: Decimal,
    pub exit_strategy: ExitStrategy,
    pub source: i16,
    pub source_signal_id: Option<Uuid>,
    /// If Some, applies share-based fee model from the arb opportunity.
    pub arb_opportunity: Option<ArbOpportunity>,
    /// Without an arb opportunity, the taker fee terms the position's fills
    /// pay (share-based); `None` keeps the legacy flat model.
    pub taker_fees: Option<polymarket_core::types::FeeSchedule>,
    /// Edge the entry was expected to earn, in the strategy's own unit (net
    /// USD per share for arb, basis points for quant signals). Recorded on
    /// the `entry_requested` event so realized results can be compared
    /// against what each position promised.
    pub expected_edge: Option<Decimal>,
    /// Gross edge observed at entry (arb: gross profit per share).
    pub observed_edge: Option<Decimal>,
    /// Only while no position in the market is active, decided under the
    /// lock every entry takes (across processes).
    pub exclusive_market: bool,
    /// Only while fewer quant positions than this are open, decided under
    /// the same lock.
    pub max_open_quant: Option<i64>,
}

/// Context fields for trade event recording.
#[derive(Debug, Clone)]
pub struct EventContext {
    pub wallet_address: Option<String>,
    pub execution_mode: String,
    pub strategy: String,
    pub source_label: String,
}

// ── PositionService ────────────────────────────────────────────────────

/// Centralized service for all position state mutations.
///
/// Callers pass a repository snapshot. Successful updates advance its version;
/// failures require a reload before retrying. Receipt replay never reapplies a fill.
#[derive(Clone)]
pub struct PositionService {
    repo: Arc<PositionRepository>,
    events: TradeEventRecorder,
}

impl PositionService {
    pub fn new(pool: PgPool, trade_event_tx: broadcast::Sender<TradeEventUpdate>) -> Self {
        Self {
            repo: Arc::new(PositionRepository::new(pool.clone())),
            events: TradeEventRecorder::new(pool, trade_event_tx),
        }
    }

    /// Expose the underlying repository for read-only queries that don't
    /// need the service's mutation guarantees (e.g. bulk listing, stats).
    pub fn repo(&self) -> &PositionRepository {
        &self.repo
    }

    // ── Position creation ──────────────────────────────────────────

    /// Create a new PENDING position, persist it, and record "entry_requested".
    pub async fn create_position(
        &self,
        params: CreatePositionParams,
        ctx: &EventContext,
        metadata: serde_json::Value,
    ) -> anyhow::Result<Position> {
        self.create_position_with_limit(params, ctx, metadata, None)
            .await
    }

    pub async fn create_position_with_limit(
        &self,
        params: CreatePositionParams,
        ctx: &EventContext,
        metadata: serde_json::Value,
        max_exposure: Option<Decimal>,
    ) -> anyhow::Result<Position> {
        let mut position = Position::new(
            params.market_id.clone(),
            params.yes_entry_price,
            params.no_entry_price,
            params.quantity,
            params.exit_strategy,
        );

        if !matches!(ctx.execution_mode.as_str(), "paper" | "live")
            || (ctx.execution_mode == "live" && ctx.wallet_address.is_none())
        {
            return Err(anyhow!("Position requires execution provenance"));
        }
        position.execution_mode = ctx.execution_mode.clone();
        position.wallet_address = if ctx.execution_mode == "live" {
            ctx.wallet_address.clone()
        } else {
            None
        };

        if let Some(ref arb) = params.arb_opportunity {
            position.apply_arb_fee_model(arb);
        } else if let Some(terms) = params.taker_fees {
            position.apply_taker_fee_terms(terms);
        }

        self.repo
            .insert_with_source_and_limit(
                &position,
                params.source,
                params.source_signal_id,
                EntryLimits {
                    max_exposure,
                    exclusive_market: params.exclusive_market,
                    max_open_quant: params.max_open_quant,
                },
            )
            .await
            .map_err(|e| anyhow!("insert position: {}", e))?;

        let mut requested = NewTradeEvent::new(
            &ctx.strategy,
            &ctx.execution_mode,
            &ctx.source_label,
            &params.market_id,
            "entry_requested",
        )
        .with_position(position.id)
        .with_state(None, Some("pending"))
        .with_requested_size(position.entry_cost())
        .with_metadata(metadata);
        requested.expected_edge = params.expected_edge;
        requested.observed_edge = params.observed_edge;
        self.events.record_warn(requested).await;

        Ok(position)
    }

    // ── Entry fills ────────────────────────────────────────────────

    /// Record a single-leg entry fill. Updates held_*_qty and entry price.
    /// Records "entry_filled" event for that leg.
    ///
    /// This does NOT transition state — the caller must call `mark_open`
    /// after all expected fills are recorded.
    pub async fn record_entry_fill(
        &self,
        position: &mut Position,
        leg: Leg,
        fill_price: Decimal,
        fill_qty: Decimal,
        ctx: &EventContext,
        receipt: Option<Uuid>,
    ) -> anyhow::Result<()> {
        if fill_qty <= Decimal::ZERO || fill_price <= Decimal::ZERO {
            return Err(anyhow!("Entry fill must have positive quantity and price"));
        }
        match leg {
            Leg::Yes => {
                position.yes_entry_price = (position.yes_entry_price * position.held_yes_qty
                    + fill_price * fill_qty)
                    / (position.held_yes_qty + fill_qty);
                position.apply_yes_entry_fill(fill_qty);
            }
            Leg::No => {
                position.no_entry_price = (position.no_entry_price * position.held_no_qty
                    + fill_price * fill_qty)
                    / (position.held_no_qty + fill_qty);
                position.apply_no_entry_fill(fill_qty);
            }
        }

        position.quantity = (position.held_yes_qty + position.exited_yes_qty)
            .max(position.held_no_qty + position.exited_no_qty);
        // Market orders take liquidity: the fill pays the taker fee.
        position.charge_taker_fee(fill_price, fill_qty);
        if let Some(id) = receipt {
            self.repo
                .update_with_receipt(position, id, fill_qty)
                .await?;
        } else {
            self.repo.update(position).await?;
        }

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "entry_filled",
                )
                .with_position(position.id)
                .with_fill_price(fill_price)
                .with_filled_size(fill_qty * fill_price)
                .with_metadata(serde_json::json!({
                    "leg": leg.to_string(),
                    "fill_qty": fill_qty.to_string(),
                    "fill_price": fill_price.to_string(),
                })),
            )
            .await;

        Ok(())
    }

    // ── State transitions ──────────────────────────────────────────

    /// Transition Pending → Open. Records "position_open" event.
    pub async fn mark_open(
        &self,
        position: &mut Position,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        let from = format!("{:?}", position.state).to_lowercase();
        position
            .mark_open()
            .map_err(|e| anyhow!("mark_open: {}", e))?;

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after mark_open: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "position_open",
                )
                .with_position(position.id)
                .with_state(Some(&from), Some("open")),
            )
            .await;

        Ok(())
    }

    /// Transition Pending → EntryFailed. Records "entry_failed" event.
    pub async fn mark_entry_failed(
        &self,
        position: &mut Position,
        reason: FailureReason,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }

        let from = format!("{:?}", position.state).to_lowercase();
        let reason_str = reason.to_string();
        position.mark_entry_failed(reason);

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after mark_entry_failed: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "entry_failed",
                )
                .with_position(position.id)
                .with_state(Some(&from), Some("entry_failed"))
                .with_reason(Some(&reason_str)),
            )
            .await;

        Ok(())
    }

    /// Transition Open → ExitReady. Records "exit_marked_ready" event.
    pub async fn mark_exit_ready(
        &self,
        position: &mut Position,
        reason: &str,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        let from = format!("{:?}", position.state).to_lowercase();
        position
            .mark_exit_ready()
            .map_err(|e| anyhow!("mark_exit_ready: {}", e))?;

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after mark_exit_ready: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_marked_ready",
                )
                .with_position(position.id)
                .with_state(Some(&from), Some("exit_ready"))
                .with_reason(Some(reason)),
            )
            .await;

        Ok(())
    }

    /// Transition ExitReady → Closing. Records "exit_requested" event.
    pub async fn mark_closing(
        &self,
        position: &mut Position,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }

        let from = format!("{:?}", position.state).to_lowercase();
        position
            .mark_closing()
            .map_err(|e| anyhow!("mark_closing: {}", e))?;

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after mark_closing: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_requested",
                )
                .with_position(position.id)
                .with_state(Some(&from), Some("closing")),
            )
            .await;

        Ok(())
    }

    // ── Exit fills ─────────────────────────────────────────────────

    /// Record a single-leg exit fill.
    ///
    /// Accumulates a weighted exit price and moves only confirmed quantities
    /// from held to exited. A partial exit leaves the remaining inventory intact.
    pub async fn record_exit_fill(
        &self,
        position: &mut Position,
        leg: Leg,
        fill_price: Decimal,
        fill_qty: Decimal,
        ctx: &EventContext,
        receipt: Option<Uuid>,
    ) -> anyhow::Result<()> {
        if fill_qty <= Decimal::ZERO || fill_price < Decimal::ZERO {
            return Err(anyhow!("Invalid exit fill"));
        }
        let (held, exited, price) = match leg {
            Leg::Yes => (
                &mut position.held_yes_qty,
                &mut position.exited_yes_qty,
                &mut position.yes_exit_price,
            ),
            Leg::No => (
                &mut position.held_no_qty,
                &mut position.exited_no_qty,
                &mut position.no_exit_price,
            ),
        };
        if fill_qty > *held {
            return Err(anyhow!("Exit fill exceeds holdings"));
        }
        *price = Some(
            (price.unwrap_or_default() * *exited + fill_price * fill_qty) / (*exited + fill_qty),
        );
        *held -= fill_qty;
        *exited += fill_qty;
        position.charge_taker_fee(fill_price, fill_qty);
        // A confirmed sale is progress: retry pacing restarts from the first step.
        position.retry_count = 0;
        position.last_updated = chrono::Utc::now();

        if let Some(id) = receipt {
            self.repo
                .update_with_receipt(position, id, fill_qty)
                .await?;
        } else {
            self.repo.update(position).await?;
        }

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_fill",
                )
                .with_position(position.id)
                .with_fill_price(fill_price)
                .with_filled_size(fill_qty * fill_price)
                .with_metadata(serde_json::json!({
                    "leg": leg.to_string(),
                    "fill_qty": fill_qty.to_string(),
                    "fill_price": fill_price.to_string(),
                })),
            )
            .await;

        Ok(())
    }

    /// Writes off per-leg remainders too small to sell (see
    /// [`Position::write_off_unsellable_dust`]). Records "exit_dust_written_off".
    pub async fn write_off_unsellable_dust(
        &self,
        position: &mut Position,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        let (yes, no) = position.write_off_unsellable_dust();
        if yes.is_zero() && no.is_zero() {
            return Ok(());
        }
        self.repo.update(position).await?;
        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_dust_written_off",
                )
                .with_position(position.id)
                .with_metadata(serde_json::json!({
                    "yes_qty": yes.to_string(),
                    "no_qty": no.to_string(),
                })),
            )
            .await;
        Ok(())
    }

    /// Writes off the part of a leg's holding the wallet does not have (see
    /// [`Position::write_off_missing_shares`]). Records
    /// "entry_fee_shares_written_off".
    pub async fn write_off_missing_shares(
        &self,
        position: &mut Position,
        leg: Leg,
        quantity: Decimal,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        let written_off = position.write_off_missing_shares(leg == Leg::Yes, quantity);
        if written_off.is_zero() {
            return Ok(());
        }
        self.repo.update(position).await?;
        self.record_shares_written_off(position, leg, written_off, ctx)
            .await;
        Ok(())
    }

    /// Write off what the wallet lacks from several positions at once: each
    /// `(position, quantity, context)` is persisted with all the others or
    /// none is, and each write-off is then told under its own context.
    /// Returns the positions as written.
    pub async fn write_off_missing_shares_together(
        &self,
        leg: Leg,
        write_offs: Vec<(Position, Decimal, EventContext)>,
    ) -> anyhow::Result<Vec<Position>> {
        let mut positions = Vec::new();
        let mut told = Vec::new();
        for (mut position, quantity, ctx) in write_offs {
            let written_off = position.write_off_missing_shares(leg == Leg::Yes, quantity);
            if !written_off.is_zero() {
                told.push((positions.len(), written_off, ctx));
            }
            positions.push(position);
        }
        let mut changed: Vec<Position> =
            told.iter().map(|(i, _, _)| positions[*i].clone()).collect();
        self.repo.update_all(&mut changed).await?;
        for ((i, written_off, ctx), position) in told.into_iter().zip(changed) {
            self.record_shares_written_off(&position, leg, written_off, &ctx)
                .await;
            positions[i] = position;
        }
        Ok(positions)
    }

    async fn record_shares_written_off(
        &self,
        position: &Position,
        leg: Leg,
        written_off: Decimal,
        ctx: &EventContext,
    ) {
        // The position stays where it is: its state goes with the event.
        let state = crate::handlers::positions::position_state_label(position.state);
        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "entry_fee_shares_written_off",
                )
                .with_position(position.id)
                .with_state(Some(state), Some(state))
                .with_metadata(serde_json::json!({
                    "leg": leg.to_string(),
                    "quantity": written_off.to_string(),
                })),
            )
            .await;
    }

    // ── Position close ─────────────────────────────────────────────

    /// Final close: computes PnL, sets state=Closed, records event.
    pub async fn close_position(
        &self,
        position: &mut Position,
        method: CloseMethod,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        self.close_position_noting(position, method, ctx, None)
            .await
    }

    /// [`Self::close_position`], with `note` as the close event's metadata.
    pub async fn close_position_noting(
        &self,
        target: &mut Position,
        method: CloseMethod,
        ctx: &EventContext,
        note: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        // Closed on a copy: the caller's position changes only once the close
        // is stored, so a failed store leaves nothing half-closed for a
        // later write (an exit marked failed) to persist.
        let mut closed = target.clone();
        let position = &mut closed;
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }

        if matches!(
            position.state,
            PositionState::Closed | PositionState::EntryFailed
        ) {
            return Err(anyhow!("Cannot close a closed or entry-failed position"));
        }
        if position.held_yes_qty
            + position.held_no_qty
            + position.exited_yes_qty
            + position.exited_no_qty
            <= Decimal::ZERO
        {
            return Err(anyhow!(
                "Cannot close a position without confirmed inventory"
            ));
        }
        let from = format!("{:?}", position.state).to_lowercase();

        let event_type = match method {
            CloseMethod::MarketExit { fee } => {
                if position.has_open_exposure() {
                    return Err(anyhow!("Cannot close while shares remain held"));
                }
                let proceeds = position.yes_exit_price.unwrap_or_default()
                    * position.exited_yes_qty
                    + position.no_exit_price.unwrap_or_default() * position.exited_no_qty;
                let basis = position.yes_entry_price * position.exited_yes_qty
                    + position.no_entry_price * position.exited_no_qty;
                position.realized_pnl =
                    Some(proceeds - basis - position.fee_charges(fee, basis + proceeds));
                position.unrealized_pnl = Decimal::ZERO;
                position.state = PositionState::Closed;
                position.exit_timestamp = Some(chrono::Utc::now());
                "closed_via_exit"
            }
            CloseMethod::ResolutionWithWinner { yes_wins, fee } => {
                let proceeds = position.yes_exit_price.unwrap_or_default()
                    * position.exited_yes_qty
                    + position.no_exit_price.unwrap_or_default() * position.exited_no_qty;
                let payout = if yes_wins {
                    position.held_yes_qty
                } else {
                    position.held_no_qty
                };
                let basis = position.entry_cost();
                position.realized_pnl =
                    Some(proceeds + payout - basis - position.fee_charges(fee, basis + proceeds));
                position.resolution_winner = Some(if yes_wins { "yes" } else { "no" }.into());
                position.exited_yes_qty += position.held_yes_qty;
                position.exited_no_qty += position.held_no_qty;
                position.held_yes_qty = Decimal::ZERO;
                position.held_no_qty = Decimal::ZERO;
                position.state = PositionState::Closed;
                position.exit_timestamp = Some(chrono::Utc::now());
                position.unrealized_pnl = Decimal::ZERO;
                "closed_via_resolution"
            }
            CloseMethod::ResolutionConservative { fee } => {
                if !position.has_full_pair_exposure()
                    || position.held_yes_qty != position.held_no_qty
                {
                    return Err(anyhow!(
                        "Winner required for unequal remaining resolution holdings"
                    ));
                }
                let pairs = position.held_yes_qty;
                let proceeds = position.yes_exit_price.unwrap_or_default()
                    * position.exited_yes_qty
                    + position.no_exit_price.unwrap_or_default() * position.exited_no_qty;
                let basis = position.entry_cost();
                position.realized_pnl =
                    Some(proceeds + pairs - basis - position.fee_charges(fee, basis + proceeds));
                position.exited_yes_qty += position.held_yes_qty;
                position.exited_no_qty += position.held_no_qty;
                position.held_yes_qty = Decimal::ZERO;
                position.held_no_qty = Decimal::ZERO;
                position.state = PositionState::Closed;
                position.exit_timestamp = Some(chrono::Utc::now());
                position.unrealized_pnl = Decimal::ZERO;
                "closed_via_resolution"
            }
        };

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after close: {}", e))?;

        let mut event = NewTradeEvent::new(
            &ctx.strategy,
            &ctx.execution_mode,
            &ctx.source_label,
            &position.market_id,
            event_type,
        )
        .with_position(position.id)
        .with_state(Some(&from), Some("closed"));

        if let Some(pnl) = position.realized_pnl {
            event = event.with_realized_pnl(pnl);
        }
        if let Some(note) = note {
            event = event.with_metadata(note);
        }

        self.events.record_warn(event).await;
        *target = closed;

        Ok(())
    }

    // ── Failure and recovery ───────────────────────────────────────

    /// Mark exit as failed. Records "exit_failed" event.
    pub async fn mark_exit_failed(
        &self,
        position: &mut Position,
        reason: FailureReason,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }

        let from = format!("{:?}", position.state).to_lowercase();
        let reason_str = reason.to_string();
        position.mark_exit_failed(reason);

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after mark_exit_failed: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_failed",
                )
                .with_position(position.id)
                .with_state(Some(&from), Some("exit_failed"))
                .with_reason(Some(&reason_str)),
            )
            .await;

        Ok(())
    }

    /// Closing → ExitReady for a sale that was certainly not sent. Records
    /// "exit_deferred". Returns false when the position is not closing.
    pub async fn defer_exit(
        &self,
        position: &mut Position,
        reason: &str,
        ctx: &EventContext,
    ) -> anyhow::Result<bool> {
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }
        if !position.defer_exit() {
            return Ok(false);
        }

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after deferring an exit: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_deferred",
                )
                .with_position(position.id)
                .with_state(Some("closing"), Some("exit_ready"))
                .with_reason(Some(reason)),
            )
            .await;

        Ok(true)
    }

    /// ExitFailed → ExitReady. Records "exit_recovery_requeued" event.
    /// Returns false when the position is not in ExitFailed. Retries are never
    /// exhausted while shares are held; callers pace them.
    pub async fn attempt_exit_recovery(
        &self,
        position: &mut Position,
        ctx: &EventContext,
    ) -> anyhow::Result<bool> {
        let recovered = position.requeue_failed_exit();
        if !recovered {
            return Ok(false);
        }

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after exit recovery: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "exit_recovery_requeued",
                )
                .with_position(position.id)
                .with_state(Some("exit_failed"), Some("exit_ready"))
                .with_metadata(serde_json::json!({
                    "retry_count": position.retry_count,
                })),
            )
            .await;

        Ok(true)
    }

    /// Stalled → ExitReady for a stalled position that still holds shares.
    /// Records "stall_requeued_for_exit". Returns false without any change
    /// otherwise.
    pub async fn requeue_stalled_for_exit(
        &self,
        position: &mut Position,
        ctx: &EventContext,
    ) -> anyhow::Result<bool> {
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }
        if !position.requeue_stalled_for_exit() {
            return Ok(false);
        }
        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after stalled requeue: {}", e))?;
        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "stall_requeued_for_exit",
                )
                .with_position(position.id)
                .with_state(Some("stalled"), Some("exit_ready")),
            )
            .await;
        Ok(true)
    }

    /// Stalled → pre_stall_state. Records "stall_recovery_attempted" event.
    /// Returns the recovered state if successful, None if not stalled or
    /// no pre_stall_state was saved.
    pub async fn attempt_stalled_recovery(
        &self,
        position: &mut Position,
        ctx: &EventContext,
    ) -> anyhow::Result<Option<PositionState>> {
        let recovered_state = position.attempt_stalled_recovery();
        if recovered_state.is_none() {
            return Ok(None);
        }

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after stalled recovery: {}", e))?;

        let to_state = format!("{:?}", position.state).to_lowercase();
        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "stall_recovery_attempted",
                )
                .with_position(position.id)
                .with_state(Some("stalled"), Some(&to_state)),
            )
            .await;

        Ok(recovered_state)
    }

    // ── One-legged recovery helpers ────────────────────────────────

    /// Transition a Pending one-legged position to Open then ExitReady
    /// for immediate exit of the held leg.
    ///
    /// This is used by the arb executor when only YES fills and NO fails.
    /// The position MUST be in Pending state (mark_open rejects other states).
    pub async fn transition_one_legged_to_exit_ready(
        &self,
        position: &mut Position,
        held_leg: &str,
        failure_msg: &str,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        if position.execution_mode == "live" && self.repo.has_unsettled_orders(position.id).await? {
            return Err(anyhow!(
                "Position has an unresolved order; reconcile it before changing lifecycle"
            ));
        }

        // Accepts Pending (arb executor: YES filled, NO failed immediately)
        // or EntryFailed (exit handler: one-legged recovery from prior failure).
        if !matches!(
            position.state,
            PositionState::Pending | PositionState::EntryFailed
        ) {
            return Err(anyhow!(
                "transition_one_legged_to_exit_ready requires Pending or EntryFailed state, got {:?}",
                position.state
            ));
        }

        // Set the missing-leg price and qty to zero
        if (held_leg == "yes" && position.held_no_qty > Decimal::ZERO)
            || (held_leg == "no" && position.held_yes_qty > Decimal::ZERO)
            || !matches!(held_leg, "yes" | "no")
        {
            return Err(anyhow!(
                "One-legged recovery conflicts with explicit holdings"
            ));
        }
        if held_leg == "yes" {
            position.no_entry_price = Decimal::ZERO;
            position.held_no_qty = Decimal::ZERO;
        } else {
            position.yes_entry_price = Decimal::ZERO;
            position.held_yes_qty = Decimal::ZERO;
        }

        // Force through Pending → Open → ExitReady regardless of current state,
        // since both Pending and EntryFailed need the same target.
        position.state = PositionState::Pending; // reset so mark_open() accepts it
        position
            .mark_open()
            .map_err(|e| anyhow!("one-legged mark_open: {}", e))?;

        // mark_exit_ready expects Open
        position
            .mark_exit_ready()
            .map_err(|e| anyhow!("one-legged mark_exit_ready: {}", e))?;

        position.failure_reason = Some(FailureReason::OneLeggedEntry {
            held_leg: held_leg.to_string(),
            message: failure_msg.to_string(),
        });

        self.repo
            .update(position)
            .await
            .map_err(|e| anyhow!("update after one-legged transition: {}", e))?;

        self.events
            .record_warn(
                NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    "one_legged_exit_ready",
                )
                .with_position(position.id)
                .with_state(Some("pending"), Some("exit_ready"))
                .with_reason(Some(failure_msg))
                .with_metadata(serde_json::json!({
                    "held_leg": held_leg,
                })),
            )
            .await;

        Ok(())
    }

    // ── Utility ────────────────────────────────────────────────────

    /// Load a position by ID.
    pub async fn load(&self, id: Uuid) -> anyhow::Result<Position> {
        self.repo
            .get(id)
            .await?
            .ok_or_else(|| anyhow!("position {} not found", id))
    }
}

// ── Builder extensions on NewTradeEvent ─────────────────────────────────
//
// These mirror the per-module extensions in arb_executor.rs / exit_handler.rs
// but are shared by the service for all callers.

trait TradeEventExt {
    fn with_position(self, id: Uuid) -> Self;
    fn with_state(self, from: Option<&str>, to: Option<&str>) -> Self;
    fn with_reason(self, reason: Option<&str>) -> Self;
    fn with_fill_price(self, price: Decimal) -> Self;
    fn with_filled_size(self, size: Decimal) -> Self;
    fn with_requested_size(self, size: Decimal) -> Self;
    fn with_realized_pnl(self, pnl: Decimal) -> Self;
    fn with_metadata(self, metadata: serde_json::Value) -> Self;
}

impl TradeEventExt for NewTradeEvent {
    fn with_position(mut self, id: Uuid) -> Self {
        self.position_id = Some(id);
        self
    }
    fn with_state(mut self, from: Option<&str>, to: Option<&str>) -> Self {
        self.state_from = from.map(String::from);
        self.state_to = to.map(String::from);
        self
    }
    fn with_reason(mut self, reason: Option<&str>) -> Self {
        self.reason = reason.map(String::from);
        self
    }
    fn with_fill_price(mut self, price: Decimal) -> Self {
        self.fill_price = Some(price);
        self
    }
    fn with_filled_size(mut self, size: Decimal) -> Self {
        self.filled_size_usd = Some(size);
        self
    }
    fn with_requested_size(mut self, size: Decimal) -> Self {
        self.requested_size_usd = Some(size);
        self
    }
    fn with_realized_pnl(mut self, pnl: Decimal) -> Self {
        self.realized_pnl = Some(pnl);
        self
    }
    fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}
