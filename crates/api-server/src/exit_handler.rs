//! Exit handler — closes positions via market exit or resolution.
//!
//! Companion to `arb_executor.rs`: the arb executor opens positions but nothing
//! closes them. This module handles two exit paths:
//! - **ExitOnCorrection**: mark an open position `ExitReady` once its spread
//!   normalizes, then sell its holdings
//! - **HoldToResolution**: wait for market to resolve ($1 payout)

use chrono::Utc;
use polymarket_core::api::ClobClient;
use polymarket_core::db::positions::{PositionRepository, SOURCE_ARBITRAGE, SOURCE_RECOMMENDATION};
use polymarket_core::error::Error as PolymarketError;
use polymarket_core::types::signal::{QuantSignalKind, SignalDirection};
use polymarket_core::types::{
    sellable_shares, FailureReason, Market, MarketOrder, OrderSide, Position,
};
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;

use crate::position_service::{CloseMethod, EventContext, Leg};
use crate::trade_events::{NewTradeEvent, TradeEventRecorder};
use crate::wallet_inventory::recover_wallet_orphan_inventory;
use crate::websocket::{SignalType, SignalUpdate};

enum ExitBidStatus {
    Available(Decimal, Decimal),
    Resolved,
    Unavailable,
}

/// Whether the exit handler is there to act on what a live entry may leave
/// behind: a pair whose second leg failed must be flattened, and a quant
/// position needs its take-profit, stop-loss and maximum hold.
#[derive(Clone)]
pub struct ExitReadiness {
    config: Arc<RwLock<ExitHandlerConfig>>,
    heartbeat: Arc<AtomicI64>,
    pool: Option<PgPool>,
    /// The last look for live exits that keep failing, and its answer.
    stuck_exits: Arc<std::sync::Mutex<Option<(std::time::Instant, bool)>>>,
}

/// How old the exit handler's heartbeat may be before the handler counts as
/// not running: three exit polls, and never under five minutes (a pass with
/// many exits and resolutions to check takes time).
pub(crate) fn heartbeat_stale_after_secs(cfg: &ExitHandlerConfig) -> i64 {
    (cfg.exit_poll_interval_secs.saturating_mul(3)).max(300) as i64
}

/// A live exit failing this many times (since its last fill) counts as stuck.
const STUCK_EXIT_FAILURES: i32 = 3;
/// Stuck live exits at once that count as exits not working. One market with
/// no bids, or a delisted one, keeps its own position stuck; a failure of
/// selling itself (an allowance, the wallet, the venue) shows up across
/// positions.
const STUCK_EXIT_POSITIONS: i64 = 3;
/// A live exit waiting this long without a step counts as stuck.
const STUCK_EXIT_MINUTES: i32 = 30;
/// How long the answer to the stuck-exit look is reused.
const STUCK_EXIT_RECHECK: std::time::Duration = std::time::Duration::from_secs(30);

impl ExitReadiness {
    pub fn new(config: Arc<RwLock<ExitHandlerConfig>>, heartbeat: Arc<AtomicI64>) -> Self {
        Self {
            config,
            heartbeat,
            pool: None,
            stuck_exits: Default::default(),
        }
    }

    /// Also refuse live entries while live exits keep failing (see
    /// [`Self::refusal`]), read from `pool`.
    pub fn watching_exits(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Whether [`STUCK_EXIT_POSITIONS`] live positions are stuck at once,
    /// each having failed to exit [`STUCK_EXIT_FAILURES`] times or waited
    /// [`STUCK_EXIT_MINUTES`] without a step: the handler runs, but its exits
    /// are not getting out.
    async fn exits_stuck(&self) -> bool {
        let Some(pool) = &self.pool else {
            return false;
        };
        if let Some((at, stuck)) = *self.stuck_exits.lock().unwrap_or_else(|e| e.into_inner()) {
            if at.elapsed() < STUCK_EXIT_RECHECK {
                return stuck;
            }
        }
        let stuck = match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM positions WHERE execution_mode = 'live' \
             AND ((state = 6 AND retry_count >= $1) \
                  OR (state IN (2, 3) AND last_updated < NOW() - make_interval(mins => $2)))",
        )
        .bind(STUCK_EXIT_FAILURES)
        .bind(STUCK_EXIT_MINUTES)
        .fetch_one(pool)
        .await
        {
            Ok(stuck) => stuck >= STUCK_EXIT_POSITIONS,
            Err(error) => {
                // Unknown is not "working": entries wait for the next look.
                warn!(%error, "Could not check for stuck live exits");
                true
            }
        };
        if stuck {
            warn!("Live exits keep failing; new live entries wait until they get out");
        }
        *self.stuck_exits.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((std::time::Instant::now(), stuck));
        stuck
    }

    /// Why a live entry may not start now, if it may not.
    pub async fn refusal(&self) -> Option<&'static str> {
        let cfg = self.config.read().await;
        if !cfg.enabled {
            return Some("exit_handler_disabled");
        }
        // The handler marks itself alive at least every exit poll; a pass
        // that panicked clears the mark.
        let stale_after = heartbeat_stale_after_secs(&cfg);
        let age = Utc::now().timestamp() - self.heartbeat.load(Ordering::Relaxed);
        if age > stale_after {
            return Some("exit_handler_not_running");
        }
        drop(cfg);
        self.exits_stuck().await.then_some("live_exits_failing")
    }

    /// The loss, as a fraction of cost, at which a quant position is sold.
    pub async fn quant_stop_loss(&self) -> Decimal {
        self.config.read().await.quant_stop_loss_pct
    }
}

/// Configuration for the exit handler (env-var driven).
#[derive(Debug, Clone)]
pub struct ExitHandlerConfig {
    /// Whether the exit handler is enabled.
    pub enabled: bool,
    /// How often to check ExitReady positions (seconds).
    pub exit_poll_interval_secs: u64,
    /// How often to check market resolutions (seconds).
    pub resolution_check_secs: u64,
    /// Unrealized profit target for generic quant exits.
    /// Fractional return: 0.15 means 15%.
    pub quant_take_profit_pct: Decimal,
    /// Unrealized loss threshold for generic quant exits.
    /// Fractional loss: 0.10 means 10%.
    pub quant_stop_loss_pct: Decimal,
    /// Maximum holding period for generic quant exits.
    pub quant_max_hold_hours: i64,
    /// Minimum cooldown before retrying an ExitFailed position.
    pub failed_exit_retry_backoff_secs: u64,
}

impl Default for ExitHandlerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            exit_poll_interval_secs: 30,
            resolution_check_secs: 300,
            quant_take_profit_pct: Decimal::new(15, 2),
            quant_stop_loss_pct: Decimal::new(10, 2),
            quant_max_hold_hours: 24,
            failed_exit_retry_backoff_secs: 300,
        }
    }
}

impl ExitHandlerConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_switch("EXIT_HANDLER_ENABLED"),
            exit_poll_interval_secs: polymarket_core::config::env_count(
                "EXIT_POLL_INTERVAL_SECS",
                30,
            ),
            resolution_check_secs: polymarket_core::config::env_count(
                "EXIT_RESOLUTION_CHECK_SECS",
                300,
            ),
            // A gain can exceed the cost: 1.5 takes profit at +150%.
            quant_take_profit_pct: polymarket_core::config::env_number(
                "QUANT_TAKE_PROFIT_PCT",
                Decimal::new(15, 2),
            ),
            quant_stop_loss_pct: polymarket_core::config::env_fraction(
                "QUANT_STOP_LOSS_PCT",
                Decimal::new(10, 2),
            ),
            quant_max_hold_hours: polymarket_core::config::env_count("QUANT_MAX_HOLD_HOURS", 24),
            failed_exit_retry_backoff_secs: polymarket_core::config::env_count(
                "EXIT_FAILED_RETRY_BACKOFF_SECS",
                300,
            ),
        }
    }
}

// Token cache is shared with arb_executor (saves ~57MB of duplicated market data).
use crate::market_catalog::OutcomeTokenCache;

/// Exit handler service — closes positions via sell orders or resolution detection.
pub struct ExitHandler {
    config: Arc<RwLock<ExitHandlerConfig>>,
    position_repo: PositionRepository,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    token_cache: Arc<OutcomeTokenCache>,
    trade_event_recorder: TradeEventRecorder,
    /// Centralized service for position state mutations.
    position_service: crate::position_service::PositionService,
    /// Heartbeat timestamp (epoch secs) — updated every tick to prove liveness.
    heartbeat: Arc<AtomicI64>,
    /// Whether the latest passes ran to their end; while not, the heartbeat
    /// is held at zero.
    passes_clean: std::sync::atomic::AtomicBool,
    /// Since when a resolved position's token balance of a leg (true for
    /// YES) has been unreadable, without a read succeeding since.
    balance_unreadable_since: std::sync::Mutex<HashMap<(uuid::Uuid, bool), std::time::Instant>>,
    /// When a position's sales started being deferred unsent, and when last.
    deferred_since: std::sync::Mutex<HashMap<uuid::Uuid, (std::time::Instant, std::time::Instant)>>,
}

#[derive(Debug, Clone)]
struct QuantExitContext {
    #[allow(dead_code)] // Kept for future trade event enrichment
    signal_id: uuid::Uuid,
    kind: QuantSignalKind,
    direction: SignalDirection,
    metadata: serde_json::Value,
}

impl ExitHandler {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config: Arc<RwLock<ExitHandlerConfig>>,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
        pool: PgPool,
        heartbeat: Arc<AtomicI64>,
        token_cache: Arc<OutcomeTokenCache>,
    ) -> Self {
        let position_service =
            crate::position_service::PositionService::new(pool.clone(), trade_event_tx.clone());
        Self {
            config,
            position_repo: PositionRepository::new(pool.clone()),
            pool: pool.clone(),
            order_executor,
            circuit_breaker,
            clob_client: clob_client.clone(),
            signal_tx,
            token_cache,
            trade_event_recorder: TradeEventRecorder::new(pool.clone(), trade_event_tx),
            position_service,
            heartbeat,
            passes_clean: std::sync::atomic::AtomicBool::new(true),
            balance_unreadable_since: Default::default(),
            deferred_since: Default::default(),
        }
    }

    /// Snapshot the current config for use during a single tick.
    async fn snapshot_config(&self) -> ExitHandlerConfig {
        self.config.read().await.clone()
    }

    fn touch_heartbeat(&self) {
        let now = if self.passes_clean.load(Ordering::Relaxed) {
            Utc::now().timestamp()
        } else {
            0
        };
        self.heartbeat.store(now, Ordering::Relaxed);
    }

    /// Main run loop with two tickers.
    pub async fn run(self) -> anyhow::Result<()> {
        let startup_cfg = self.snapshot_config().await;

        info!(
            enabled = startup_cfg.enabled,
            exit_poll_secs = startup_cfg.exit_poll_interval_secs,
            resolution_check_secs = startup_cfg.resolution_check_secs,
            quant_take_profit_pct = %startup_cfg.quant_take_profit_pct,
            quant_stop_loss_pct = %startup_cfg.quant_stop_loss_pct,
            quant_max_hold_hours = startup_cfg.quant_max_hold_hours,
            failed_exit_retry_backoff_secs = startup_cfg.failed_exit_retry_backoff_secs,
            "Starting exit handler (always-on, per-tick guard)"
        );

        // Mark liveness before the initial cache load, which can block on network I/O.
        self.touch_heartbeat();

        // Token cache is shared with arb_executor — no separate refresh needed.
        // The arb_executor refreshes every cache_refresh_secs (default 300s).
        let cache_size = self.token_cache.len().await;
        info!(
            markets = cache_size,
            "Exit handler: using shared token cache from arb executor"
        );

        // A zero period (from the environment) would panic the ticker.
        let mut exit_ticker = tokio::time::interval(tokio::time::Duration::from_secs(
            startup_cfg.exit_poll_interval_secs.max(1),
        ));

        // After an overrun, wait a full period instead of catching up.

        exit_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut resolution_ticker = tokio::time::interval(tokio::time::Duration::from_secs(
            startup_cfg.resolution_check_secs.max(1),
        ));
        // After an overrun, wait a full period instead of catching up.
        resolution_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // Skip the first immediate ticks
        exit_ticker.tick().await;
        resolution_ticker.tick().await;

        // Whether the latest exit and resolution passes ran to their end. A
        // pass that panicked clears the heartbeat until one runs cleanly, so
        // the handler does not read as alive (and live entries wait).
        let (mut exits_ok, mut resolutions_ok) = (true, true);
        loop {
            self.passes_clean
                .store(exits_ok && resolutions_ok, Ordering::Relaxed);
            self.touch_heartbeat();

            tokio::select! {
                _ = exit_ticker.tick() => {
                    // Per-tick guard: skip when disabled at runtime
                    let cfg = self.snapshot_config().await;
                    if !cfg.enabled {
                        continue;
                    }
                    exits_ok = self.exit_pass(&cfg).await;
                }
                _ = resolution_ticker.tick() => {
                    // Settled whether or not exits are enabled: booking a
                    // resolved market sends no order, and an unsettled one
                    // keeps its exposure counted and its result from the
                    // circuit breaker.
                    resolutions_ok = crate::background::pass("market resolutions", async {
                        if let Err(e) = self.check_market_resolutions().await {
                            error!(error = %e, "Failed to check market resolutions");
                        }
                    })
                    .await;
                }
            }
        }
    }

    /// One pass over every position the exit handler moves. Each step is
    /// guarded on its own, so a step that panics does not keep the others
    /// from running. Returns whether every step ran to its end.
    async fn exit_pass(&self, cfg: &ExitHandlerConfig) -> bool {
        use crate::background::pass;
        let steps = [
            pass("exit evaluation", async {
                if let Err(e) = self.evaluate_open_positions(cfg).await {
                    error!(error = %e, "Failed to evaluate open positions for exit");
                }
            })
            .await,
            pass("failed exits", async {
                if let Err(e) = self.process_failed_exits(cfg).await {
                    error!(error = %e, "Failed to process failed exits");
                }
            })
            .await,
            pass("exit-ready positions", async {
                if let Err(e) = self.process_exit_ready().await {
                    error!(error = %e, "Failed to process exit-ready positions");
                }
            })
            .await,
            pass("one-legged recovery", async {
                if let Err(e) = self.process_one_legged_recovery().await {
                    error!(error = %e, "Failed to process one-legged recovery");
                }
            })
            .await,
            pass("orphan inventory recovery", async {
                if let Err(e) = self.process_orphan_inventory_recovery(cfg).await {
                    error!(error = %e, "Failed to process orphan inventory recovery");
                }
            })
            .await,
        ];
        steps.into_iter().all(|finished| finished)
    }

    /// Evaluate open ExitOnCorrection positions and promote exit-eligible ones.
    async fn evaluate_open_positions(&self, cfg: &ExitHandlerConfig) -> anyhow::Result<()> {
        let mut candidates = self.position_repo.get_open_exit_candidates().await?;
        if candidates.is_empty() {
            return Ok(());
        }

        let quant_contexts = self.load_quant_exit_contexts(&candidates).await?;

        debug!(count = candidates.len(), "Evaluating open exit candidates");

        for candidate in &mut candidates {
            self.touch_heartbeat();
            let position_id = candidate.position.id;
            // One failing row (e.g. a stale version written by another worker)
            // must not starve every later candidate of exit evaluation.
            if let Err(error) = self
                .evaluate_open_candidate(candidate, quant_contexts.get(&position_id), cfg)
                .await
            {
                warn!(%position_id, %error, "Failed to evaluate open position for exit");
            }
        }

        Ok(())
    }

    async fn evaluate_open_candidate(
        &self,
        candidate: &mut polymarket_core::db::positions::ExitCandidate,
        quant_ctx: Option<&QuantExitContext>,
        cfg: &ExitHandlerConfig,
    ) -> anyhow::Result<()> {
        let fee = Decimal::new(2, 2);
        let (yes_bid, no_bid) = match self.current_exit_bids(&candidate.position).await? {
            ExitBidStatus::Available(yes_bid, no_bid) => (yes_bid, no_bid),
            ExitBidStatus::Resolved => {
                self.close_open_position_via_resolution(
                    &mut candidate.position,
                    candidate.source,
                    quant_ctx,
                )
                .await?;
                return Ok(());
            }
            ExitBidStatus::Unavailable => return Ok(()),
        };

        let previous_pnl = candidate.position.unrealized_pnl;
        candidate.position.update_pnl(yes_bid, no_bid, fee);
        // Most ticks move nothing; an unchanged mark is not rewritten.
        if candidate.position.unrealized_pnl != previous_pnl {
            self.position_repo.update(&mut candidate.position).await?;
        }

        if self
            .should_mark_exit_ready(
                &candidate.position,
                candidate.source,
                quant_ctx,
                yes_bid,
                no_bid,
                cfg,
            )
            .await?
        {
            let execution_mode = self.current_execution_mode().await;
            let ctx = Self::event_context(&execution_mode, candidate.source, quant_ctx);
            if let Err(e) = self
                .position_service
                .mark_exit_ready(&mut candidate.position, "spread_normalized", &ctx)
                .await
            {
                warn!(error = %e, "Failed to mark exit ready");
            }
        }

        Ok(())
    }

    /// Process ExitReady positions (ExitOnCorrection strategy).
    async fn process_exit_ready(&self) -> anyhow::Result<()> {
        let positions = self.position_repo.get_exit_ready().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Processing exit-ready positions");

        for mut position in positions {
            self.touch_heartbeat();
            if let Err(e) = self.execute_exit(&mut position).await {
                warn!(
                    position_id = %position.id,
                    market_id = %position.market_id,
                    error = %e,
                    "Exit execution failed for position"
                );
            }
        }

        Ok(())
    }

    /// Puts a closing position whose sale was certainly not sent back in the
    /// exit queue for the next pass, without counting a failed exit. False,
    /// leaving the caller to fail the exit (and alert), once its sales have
    /// gone unsent for `MAX_EXIT_DEFERRAL` in a row: whatever holds them
    /// (an order stuck unresolved, a book that cannot be read) is not passing;
    /// or when the position could not be put back.
    async fn defer_sale(&self, position: &mut Position, reason: &str, ctx: &EventContext) -> bool {
        // A run lasts while sales go unsent pass after pass: the gap that
        // ends one is several passes, however long a pass is set to be.
        let poll_secs = self.config.read().await.exit_poll_interval_secs;
        let gap = DEFERRAL_GAP.max(std::time::Duration::from_secs(poll_secs.saturating_mul(3)));
        let now = std::time::Instant::now();
        let first = {
            let mut deferred = self
                .deferred_since
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            deferred.retain(|_, (_, last)| now.duration_since(*last) <= gap);
            let run = deferred.entry(position.id).or_insert((now, now));
            run.1 = now;
            run.0
        };
        if now.duration_since(first) >= MAX_EXIT_DEFERRAL {
            self.deferred_since
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&position.id);
            warn!(
                position_id = %position.id,
                reason,
                "Exit sales have gone unsent for ten minutes; counting a failed exit"
            );
            return false;
        }
        info!(position_id = %position.id, reason, "Exit sale not sent; it goes again next pass");
        if let Err(error) = self
            .position_service
            .defer_exit(position, reason, ctx)
            .await
        {
            // Still Closing: the caller records a failed exit instead, which
            // also takes it out of Closing for a retry.
            warn!(position_id = %position.id, %error, "Could not return the position to the exit queue");
            return false;
        }
        true
    }

    /// Execute the sell orders for a single ExitReady position.
    async fn execute_exit(&self, position: &mut Position) -> anyhow::Result<()> {
        // Sell every leg of the exit or none of it; a shutdown leaves the
        // position ExitReady for the next process.
        let Some(_operation) = self.order_executor.begin_operation() else {
            return Ok(());
        };
        self.order_executor
            .validate_position_execution(position)
            .await?;
        let market_id = position.market_id.clone();
        self.touch_heartbeat();
        let execution_mode = position.execution_mode.clone();
        // Always try to load quant context — dual-leg quant positions have both
        // prices set, so a zero-price gate would misclassify them as arb.
        let quant_ctx = self
            .load_quant_exit_context(position.id)
            .await
            .ok()
            .flatten();

        // Mark Closing
        let source = if quant_ctx.is_some() {
            SOURCE_RECOMMENDATION
        } else {
            SOURCE_ARBITRAGE
        };
        let ctx = Self::event_context(&execution_mode, source, quant_ctx.as_ref());
        // Token IDs are read before the position is marked Closing: a lookup
        // that fails leaves it ExitReady for the next pass, not Closing until
        // housekeeping finds it.
        let tokens = match self.resolve_market_tokens(&market_id).await {
            Ok(tokens) => tokens,
            Err(error) => {
                warn!(position_id = %position.id, %error, "Could not read the market's tokens; the exit goes again next pass");
                return Ok(());
            }
        };
        if let Err(e) = self.position_service.mark_closing(position, &ctx).await {
            warn!(error = %e, "mark_closing failed");
            return Ok(());
        }

        let (yes_token_id, no_token_id) = match tokens {
            Some(ids) => ids,
            None => {
                let _ = self
                    .position_service
                    .mark_exit_failed(
                        position,
                        FailureReason::ConnectivityError {
                            message: "No token IDs for market".to_string(),
                        },
                        &ctx,
                    )
                    .await;
                self.publish_alert(&market_id, "exit_failed", "No token IDs for market");
                return Ok(());
            }
        };

        // Both legs' exchanges before either is sold: a lookup failing only
        // for the second leg would leave the exit one-legged.
        if let Err(error) = self
            .order_executor
            .resolve_exchanges(&[yes_token_id.as_str(), no_token_id.as_str()])
            .await
        {
            let _ = self
                .position_service
                .mark_exit_failed(
                    position,
                    FailureReason::ConnectivityError {
                        message: error.to_string(),
                    },
                    &ctx,
                )
                .await;
            self.publish_alert(&market_id, "exit_failed", &error.to_string());
            return Ok(());
        }

        // The fees of these sales are charged under the market's terms.
        self.attach_fee_terms(position).await;
        // Sell what the wallet has: a buy whose fee the V1 exchange kept in
        // shares left the recorded holding above the balance by that fee.
        for (leg, token_id) in [(Leg::Yes, &yes_token_id), (Leg::No, &no_token_id)] {
            if let Err(error) = self
                .align_leg_with_wallet(position, leg, token_id, &ctx)
                .await
            {
                // Nothing was written off; the exit is retried, not left closing.
                warn!(position_id = %position.id, %error, "Could not measure the holding before a sale");
                let _ = self
                    .position_service
                    .mark_exit_failed(
                        position,
                        FailureReason::ConnectivityError {
                            message: error.to_string(),
                        },
                        &ctx,
                    )
                    .await;
                return Ok(());
            }
        }

        // Market sells carry whole hundredths of a share; a smaller remainder
        // is written off below rather than retried forever.
        let (has_yes, has_no) = held_outcomes(position);
        let yes_qty = sellable_shares(position.held_yes_qty);
        let no_qty = sellable_shares(position.held_no_qty);
        if has_yes && yes_qty > Decimal::ZERO {
            let yes_report = match self
                .execute_sell_order_with_refresh(
                    &market_id,
                    "YES",
                    &yes_token_id,
                    OrderSide::Sell,
                    yes_qty,
                    position,
                )
                .await
            {
                Ok(report) => report,
                Err(e) => {
                    if self
                        .close_via_resolution_if_market_resolved(position, quant_ctx.as_ref())
                        .await?
                    {
                        return Ok(());
                    }
                    if nothing_was_sent(&e.to_string())
                        && self.defer_sale(position, &e.to_string(), &ctx).await
                    {
                        return Ok(());
                    }
                    error!(error = %e, market_id = %market_id, "YES sell order error");
                    let _ = self
                        .position_service
                        .mark_exit_failed(
                            position,
                            FailureReason::ConnectivityError {
                                message: format!("YES sell error: {e}"),
                            },
                            &ctx,
                        )
                        .await;
                    self.publish_alert(&market_id, "exit_failed", "YES sell order error");
                    return Ok(());
                }
            };

            if yes_report.status == polymarket_core::types::OrderStatus::Pending {
                return Ok(());
            }
            if !yes_report.is_success() {
                let msg = yes_report
                    .error_message
                    .unwrap_or_else(|| "YES sell not filled".to_string());
                if nothing_was_sent(&msg) && self.defer_sale(position, &msg, &ctx).await {
                    return Ok(());
                }
                let _ = self
                    .position_service
                    .mark_exit_failed(
                        position,
                        FailureReason::OrderRejected {
                            message: format!("YES sell failed: {msg}"),
                        },
                        &ctx,
                    )
                    .await;
                self.publish_alert(&market_id, "exit_failed", "YES sell order rejected");
                return Ok(());
            }

            if let Err(error) = self
                .position_service
                .record_exit_fill(
                    position,
                    Leg::Yes,
                    yes_report.average_price,
                    yes_report.filled_quantity,
                    &ctx,
                    (ctx.execution_mode == "live").then_some(yes_report.order_id),
                )
                .await
            {
                warn!(
                    position_id = %position.id,
                    market_id = %market_id,
                    error = %error,
                    "Failed to record YES exit fill"
                );
                let _ = self
                    .position_service
                    .mark_exit_failed(
                        position,
                        FailureReason::Unknown {
                            message: format!("failed to record YES exit fill: {error}"),
                        },
                        &ctx,
                    )
                    .await;
                self.publish_alert(
                    &market_id,
                    "exit_failed",
                    "YES exit fill bookkeeping failed",
                );
                return Ok(());
            }
        }

        if has_no && no_qty > Decimal::ZERO {
            let no_report = match self
                .execute_sell_order_with_refresh(
                    &market_id,
                    "NO",
                    &no_token_id,
                    OrderSide::Sell,
                    no_qty,
                    position,
                )
                .await
            {
                Ok(report) => report,
                Err(e) => {
                    if self
                        .close_via_resolution_if_market_resolved(position, quant_ctx.as_ref())
                        .await?
                    {
                        return Ok(());
                    }
                    if nothing_was_sent(&e.to_string())
                        && self.defer_sale(position, &e.to_string(), &ctx).await
                    {
                        return Ok(());
                    }
                    error!(error = %e, market_id = %market_id, "NO sell order error");
                    let _ = self
                        .position_service
                        .mark_exit_failed(
                            position,
                            FailureReason::ConnectivityError {
                                message: format!("NO sell error: {e}"),
                            },
                            &ctx,
                        )
                        .await;
                    self.publish_alert(&market_id, "exit_failed", "NO sell order error");
                    return Ok(());
                }
            };

            if no_report.status == polymarket_core::types::OrderStatus::Pending {
                return Ok(());
            }
            if !no_report.is_success() {
                let msg = no_report
                    .error_message
                    .unwrap_or_else(|| "NO sell not filled".to_string());
                if nothing_was_sent(&msg) && self.defer_sale(position, &msg, &ctx).await {
                    return Ok(());
                }
                let _ = self
                    .position_service
                    .mark_exit_failed(
                        position,
                        FailureReason::OrderRejected {
                            message: format!("NO sell failed: {msg}"),
                        },
                        &ctx,
                    )
                    .await;
                self.publish_alert(&market_id, "exit_failed", "NO sell order rejected");
                return Ok(());
            }

            if let Err(error) = self
                .position_service
                .record_exit_fill(
                    position,
                    Leg::No,
                    no_report.average_price,
                    no_report.filled_quantity,
                    &ctx,
                    (ctx.execution_mode == "live").then_some(no_report.order_id),
                )
                .await
            {
                warn!(
                    position_id = %position.id,
                    market_id = %market_id,
                    error = %error,
                    "Failed to record NO exit fill"
                );
                let _ = self
                    .position_service
                    .mark_exit_failed(
                        position,
                        FailureReason::Unknown {
                            message: format!("failed to record NO exit fill: {error}"),
                        },
                        &ctx,
                    )
                    .await;
                self.publish_alert(&market_id, "exit_failed", "NO exit fill bookkeeping failed");
                return Ok(());
            }
        }

        let fee = Decimal::new(2, 2); // 2%

        if let Err(error) = self
            .position_service
            .write_off_unsellable_dust(position, &ctx)
            .await
        {
            warn!(position_id = %position.id, %error, "Failed to write off unsellable dust");
            return Ok(());
        }
        if position.has_open_exposure() {
            self.position_service
                .mark_exit_failed(
                    position,
                    FailureReason::Unknown {
                        message: "Partial exit; remaining shares require another confirmed sale"
                            .into(),
                    },
                    &ctx,
                )
                .await?;
            return Ok(());
        }
        // Close position
        if let Err(e) = self
            .position_service
            .close_position(position, CloseMethod::MarketExit { fee }, &ctx)
            .await
        {
            warn!(position_id = %position.id, error = %e, "close_position failed");
            return Ok(());
        }

        let yes_price = position.yes_exit_price.unwrap_or(Decimal::ZERO);
        let no_price = position.no_exit_price.unwrap_or(Decimal::ZERO);

        let realized_pnl = position.realized_pnl.unwrap_or_default();
        record_close_with_breaker(&self.pool, &self.circuit_breaker, position, true).await;

        // Publish success signal
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: market_id.clone(),
            outcome_id: "both".to_string(),
            action: "closed_via_exit".to_string(),
            confidence: 1.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "position_id": position.id.to_string(),
                "yes_exit_price": yes_price.to_string(),
                "no_exit_price": no_price.to_string(),
                "realized_pnl": realized_pnl.to_string(),
            }),
        };
        let _ = self.signal_tx.send(signal);

        info!(
            market_id = %market_id,
            position_id = %position.id,
            realized_pnl = %realized_pnl,
            "Position closed via exit"
        );

        Ok(())
    }

    /// Give a share-based position that records no fee terms (one opened
    /// before migration 092) its market's terms, as the exchange charges them,
    /// so the fees of its sales are charged. Its buys were made on the V1
    /// exchange, which kept their fees in shares: the wallet alignment books
    /// those. When the terms cannot be read the sale goes ahead uncharged.
    async fn attach_fee_terms(&self, position: &mut Position) {
        if position.fee_model != polymarket_core::types::PositionFeeModel::ShareBased
            || position.taker_fees.is_some()
        {
            return;
        }
        match self.clob_client.get_fee_schedule(&position.market_id).await {
            Ok(terms) => {
                position.taker_fees = Some(terms);
                // Kept at once, so a sale settled later elsewhere (the intent
                // reconciler, after an unclear submission) charges its fee too.
                if let Err(error) = self.position_service.repo().update(position).await {
                    warn!(position_id = %position.id, %error, "Could not record the market's fee terms");
                }
            }
            Err(error) => warn!(
                position_id = %position.id,
                %error,
                "Could not read the market's fee terms; this exit's fees go uncharged"
            ),
        }
    }

    /// Before a live sale, write off what the wallet lacks of a token when
    /// the gap is small (see [`fee_shortfall`]): buy fees the V1 exchange
    /// kept in shares, or rounding dust. A larger gap is left for
    /// reconciliation and the sale goes ahead as recorded, for the balance
    /// preflight to refuse. See [`Self::measure_leg`].
    async fn align_leg_with_wallet(
        &self,
        position: &mut Position,
        leg: Leg,
        token_id: &str,
        ctx: &EventContext,
    ) -> anyhow::Result<()> {
        match self.measure_leg(position, leg, token_id, ctx).await? {
            Measured::Unmeasured(why)
            | Measured::Waiting(why @ (BALANCE_UNREADABLE | SIGNER_NOT_LOADED)) => warn!(
                position_id = %position.id,
                leg = %leg,
                reason = why,
                "Holding not measured against the wallet; nothing written off"
            ),
            Measured::Matches | Measured::Waiting(_) => {}
        }
        Ok(())
    }

    /// Measure `position`'s `leg` against the wallet. The balance is
    /// compared with everything the wallet's live positions and manual
    /// orders record of the token; a small gap (see [`fee_shortfall`]) is
    /// shared out among all its positions in the market at once, each by
    /// its holding, so no measurement ever hides another holder's part.
    /// Nothing is measured while an order of the wallet on the market has an
    /// unknown outcome or a buy may still be settling on chain: a balance
    /// read then can be short by shares on their way. A far larger gap
    /// means the shares were redeemed or moved, and the record stands.
    async fn measure_leg(
        &self,
        position: &mut Position,
        leg: Leg,
        token_id: &str,
        ctx: &EventContext,
    ) -> anyhow::Result<Measured> {
        let held = match leg {
            Leg::Yes => position.held_yes_qty,
            Leg::No => position.held_no_qty,
        };
        if position.execution_mode != "live" || held <= Decimal::ZERO {
            return Ok(Measured::Matches);
        }
        let Some(wallet) = position.wallet_address.clone() else {
            return Ok(Measured::Matches);
        };
        // The balance read below is the executor's wallet, which is loaded
        // only at startup. A paper executor never has one; a live one whose
        // signer failed to load may have it after a restart, so the close
        // gives that the unreadable-balance grace. Another wallet's holding
        // can never be measured.
        match self.order_executor.wallet_address().await {
            None if self.order_executor.is_live() => {
                return Ok(Measured::Waiting(SIGNER_NOT_LOADED));
            }
            None => return Ok(Measured::Unmeasured(SIGNER_NOT_LOADED)),
            Some(executor) if !executor.eq_ignore_ascii_case(&wallet) => {
                return Ok(Measured::Unmeasured(
                    "the position's wallet is not the trading wallet",
                ));
            }
            Some(_) => {}
        }
        let (others, unsettled): (Decimal, bool) = sqlx::query_as(OTHER_HOLDERS_SQL)
            .bind(position.id)
            .bind(&position.market_id)
            .bind(&wallet)
            .bind(leg.to_string())
            .bind(token_id)
            .bind(FEE_WRITE_OFF_SETTLE_MINUTES)
            .fetch_one(&self.pool)
            .await?;
        if unsettled {
            return Ok(Measured::Waiting(
                "an order of the wallet on the market may still settle",
            ));
        }
        let balance = match self
            .order_executor
            .get_live_balance_allowance("CONDITIONAL", Some(token_id))
            .await
        {
            Ok(snapshot) => snapshot.balance_in_units(),
            Err(error) => {
                warn!(position_id = %position.id, %error, "Could not read the token balance");
                None
            }
        };
        let Some(balance) = balance else {
            return Ok(Measured::Waiting(BALANCE_UNREADABLE));
        };
        let recorded = held + others.max(Decimal::ZERO);
        let Some(shortfall) = fee_shortfall(recorded, balance) else {
            return Ok(if balance < recorded {
                Measured::Unmeasured(
                    "the wallet holds far fewer shares than recorded: redeemed or moved",
                )
            } else {
                Measured::Matches
            });
        };
        info!(
            position_id = %position.id,
            leg = %leg,
            %recorded,
            %balance,
            %shortfall,
            "Writing off shares the wallet lacks"
        );
        let siblings: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM positions WHERE id <> $1 AND market_id = $2 AND execution_mode = 'live' \
             AND LOWER(wallet_address) = LOWER($3) \
             AND CASE WHEN $4 = 'yes' THEN held_yes_qty ELSE held_no_qty END > 0",
        )
        .bind(position.id)
        .bind(&position.market_id)
        .bind(&wallet)
        .bind(leg.to_string())
        .fetch_all(&self.pool)
        .await?;
        let mut write_offs = Vec::new();
        for id in siblings {
            let sibling = self.position_service.load(id).await?;
            let sibling_held = match leg {
                Leg::Yes => sibling.held_yes_qty,
                Leg::No => sibling.held_no_qty,
            };
            // Each sibling's write-off is told under its own strategy.
            let quant = self.load_quant_exit_context(id).await.ok().flatten();
            let source = if quant.is_some() {
                SOURCE_RECOMMENDATION
            } else {
                SOURCE_ARBITRAGE
            };
            let sibling_ctx = Self::event_context(&sibling.execution_mode, source, quant.as_ref());
            write_offs.push((sibling, shortfall * sibling_held / recorded, sibling_ctx));
        }
        write_offs.push((position.clone(), shortfall * held / recorded, ctx.clone()));
        // All of them are written off together, or none is.
        let written = self
            .position_service
            .write_off_missing_shares_together(leg, write_offs)
            .await?;
        if let Some(own) = written
            .into_iter()
            .find(|written| written.id == position.id)
        {
            *position = own;
        }
        Ok(Measured::Matches)
    }

    async fn current_exit_bids(&self, position: &Position) -> anyhow::Result<ExitBidStatus> {
        let market_id = position.market_id.as_str();
        let Some((yes_token_id, no_token_id)) = self.resolve_market_tokens(market_id).await? else {
            warn!(market_id = %market_id, "No token IDs for exit price evaluation");
            return Ok(ExitBidStatus::Unavailable);
        };

        // Both books at once: a leg not held is worth nothing to exit.
        let (has_yes, has_no) = held_outcomes(position);
        let bid = |held: bool, token_id: String, side: &'static str| async move {
            if held {
                self.load_exit_best_bid(market_id, &token_id, side).await
            } else {
                Ok(Some(Decimal::ZERO))
            }
        };
        let (yes_bid, no_bid) = tokio::join!(
            bid(has_yes, yes_token_id, "YES"),
            bid(has_no, no_token_id, "NO")
        );
        match (yes_bid?, no_bid?) {
            (Some(yes_bid), Some(no_bid)) => Ok(ExitBidStatus::Available(yes_bid, no_bid)),
            _ => self.exit_bid_fallback_status(market_id).await,
        }
    }

    async fn load_exit_best_bid(
        &self,
        market_id: &str,
        token_id: &str,
        side: &str,
    ) -> anyhow::Result<Option<Decimal>> {
        match self
            .order_executor
            .clob_client()
            .get_order_book(token_id)
            .await
        {
            // An empty bid side is no price at all, not a zero mark that
            // would read as a total loss and trigger a stop-loss exit.
            Ok(book) => Ok(book.best_bid()),
            Err(polymarket_core::error::Error::Api {
                status: Some(404), ..
            }) => {
                warn!(
                    market_id = %market_id,
                    token_id = %token_id,
                    side = side,
                    "Exit evaluation token returned 404"
                );
                Ok(None)
            }
            Err(error) => {
                warn!(
                    market_id = %market_id,
                    token_id = %token_id,
                    side = side,
                    error = %error,
                    "Failed loading orderbook for exit evaluation"
                );
                Ok(None)
            }
        }
    }

    async fn exit_bid_fallback_status(&self, market_id: &str) -> anyhow::Result<ExitBidStatus> {
        match self.clob_client.get_market_by_id(market_id).await {
            Ok(market) if market.resolved => Ok(ExitBidStatus::Resolved),
            Ok(_) => Ok(ExitBidStatus::Unavailable),
            Err(error) => {
                warn!(
                    market_id = %market_id,
                    error = %error,
                    "Failed to fetch market after exit-orderbook miss"
                );
                Ok(ExitBidStatus::Unavailable)
            }
        }
    }

    /// How long the token balance of a resolved position has been
    /// unreadable, counting from the first look that found it so.
    fn unreadable_for(&self, position_id: uuid::Uuid, leg: Leg) -> std::time::Duration {
        self.balance_unreadable_since
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry((position_id, leg == Leg::Yes))
            .or_insert_with(std::time::Instant::now)
            .elapsed()
    }

    /// A read of the leg's balance answered: its grace starts afresh.
    fn balance_read(&self, position_id: uuid::Uuid, leg: Leg) {
        self.balance_unreadable_since
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(position_id, leg == Leg::Yes));
    }

    async fn finalize_resolution_close(
        &self,
        position: &mut Position,
        source: i16,
        quant_ctx: Option<&QuantExitContext>,
        resolved_yes_winner: Option<bool>,
    ) -> anyhow::Result<bool> {
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM position_order_links WHERE position_id=$1 AND NOT settled)")
            .bind(position.id).fetch_one(&self.pool).await?;
        if pending {
            return Ok(false);
        }
        let market_id = position.market_id.clone();
        let fee = Decimal::new(2, 2);
        let execution_mode = position.execution_mode.clone();
        let ctx = Self::event_context(&execution_mode, source, quant_ctx);

        // Only the wallet's shares pay out: a buy whose fee the V1 exchange
        // kept in shares left the recorded holding above them. The legs that
        // pay are measured against the wallet first: the winner's, or both
        // without one (a loser pays nothing either way). While that cannot be
        // done (an order on the market still settling, the balance
        // unreadable) the close waits for a later look.
        let tokens = self.resolve_market_tokens(&market_id).await?;
        let mut unmeasured: Vec<(Leg, &'static str)> = Vec::new();
        match &tokens {
            Some((yes_token_id, no_token_id)) => {
                let legs = match resolved_yes_winner {
                    Some(true) => vec![(Leg::Yes, yes_token_id)],
                    Some(false) => vec![(Leg::No, no_token_id)],
                    None => vec![(Leg::Yes, yes_token_id), (Leg::No, no_token_id)],
                };
                // Every leg is looked at before waiting, so each unreadable
                // leg's grace runs from the same look.
                let mut waits = None;
                for (leg, token_id) in legs {
                    let measured = self.measure_leg(position, leg, token_id, &ctx).await?;
                    if !matches!(measured, Measured::Waiting(why) if waits_out_its_grace(why)) {
                        self.balance_read(position.id, leg);
                    }
                    match measured {
                        Measured::Matches => {}
                        Measured::Unmeasured(why) => unmeasured.push((leg, why)),
                        // A balance that stays unreadable leaves the record
                        // standing once the grace has passed.
                        Measured::Waiting(why)
                            if waits_out_its_grace(why)
                                && self.unreadable_for(position.id, leg)
                                    >= UNREADABLE_BALANCE_GRACE =>
                        {
                            unmeasured.push((leg, why))
                        }
                        Measured::Waiting(why) => {
                            waits.get_or_insert(why);
                        }
                    }
                }
                if let Some(why) = waits {
                    warn!(
                        position_id = %position.id,
                        market_id = %market_id,
                        reason = why,
                        "A resolved position waits to be measured against the wallet"
                    );
                    return Ok(false);
                }
            }
            None if position.execution_mode == "live" => {
                let legs = match resolved_yes_winner {
                    Some(true) => vec![Leg::Yes],
                    Some(false) => vec![Leg::No],
                    None => vec![Leg::Yes, Leg::No],
                };
                let held = |leg: &Leg| match leg {
                    Leg::Yes => position.held_yes_qty > Decimal::ZERO,
                    Leg::No => position.held_no_qty > Decimal::ZERO,
                };
                // A failed lookup of the market may pass: the record stands
                // only once it has failed as long as an unreadable balance.
                let mut waiting = false;
                for leg in legs.into_iter().filter(held) {
                    if self.unreadable_for(position.id, leg) >= UNREADABLE_BALANCE_GRACE {
                        unmeasured.push((leg, "the market's tokens are unknown"));
                    } else {
                        waiting = true;
                    }
                }
                if waiting {
                    warn!(
                        position_id = %position.id,
                        market_id = %market_id,
                        "A resolved position waits for its market's tokens to be looked up"
                    );
                    return Ok(false);
                }
            }
            None => {}
        }
        // A position from before execution provenance was recorded sends no
        // order, but settles at its record once its market resolves: no
        // wallet is known to measure it against.
        if position.execution_mode == "unknown" {
            let held = |leg: &Leg| match leg {
                Leg::Yes => position.held_yes_qty > Decimal::ZERO,
                Leg::No => position.held_no_qty > Decimal::ZERO,
            };
            let paying = match resolved_yes_winner {
                Some(true) => vec![Leg::Yes],
                Some(false) => vec![Leg::No],
                None => vec![Leg::Yes, Leg::No],
            };
            unmeasured.extend(
                paying
                    .into_iter()
                    .filter(held)
                    .map(|leg| (leg, "the position's execution provenance is unknown")),
            );
        }

        let mut note = serde_json::Map::new();
        let method = if let Some(yes_wins) = resolved_yes_winner {
            CloseMethod::ResolutionWithWinner { yes_wins, fee }
        } else if position.has_full_pair_exposure() && position.held_yes_qty == position.held_no_qty
        {
            // Equal legs pay one dollar a pair whichever side won.
            CloseMethod::ResolutionConservative { fee }
        } else {
            // No single winner named and the legs differ: a resolution not
            // final yet (trading may stop days before it), flags not set, or
            // a split payout. Never booked at a guessed payout: the position
            // waits, the first such look recorded for an operator.
            let waited = self.waited_for_winner(position, &ctx).await?;
            warn!(
                position_id = %position.id,
                market_id = %market_id,
                waited_hours = waited.num_hours(),
                "Resolved market has no single winner and the position's legs differ; waiting for the winner"
            );
            return Ok(false);
        };

        // Where the wallet could not confirm the record, the close says so.
        if !unmeasured.is_empty() {
            note.insert(
                "unmeasured".into(),
                unmeasured
                    .iter()
                    .map(|(leg, why)| serde_json::json!({"leg": leg.to_string(), "reason": why}))
                    .collect(),
            );
        }
        let note = (!note.is_empty()).then_some(serde_json::Value::Object(note));
        if let Err(e) = self
            .position_service
            .close_position_noting(position, method, &ctx, note)
            .await
        {
            warn!(
                position_id = %position.id,
                market_id = %market_id,
                error = %e,
                "resolution close failed"
            );
            return Ok(false);
        }
        for leg in [Leg::Yes, Leg::No] {
            self.balance_read(position.id, leg);
        }
        for (leg, why) in &unmeasured {
            warn!(
                position_id = %position.id,
                leg = %leg,
                reason = why,
                "Resolved position closed at its recorded holding, not measured against the wallet"
            );
        }

        let realized_pnl = position.realized_pnl.unwrap_or_default();
        record_close_with_breaker(
            &self.pool,
            &self.circuit_breaker,
            position,
            unmeasured.is_empty(),
        )
        .await;

        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: market_id.clone(),
            outcome_id: "both".to_string(),
            action: "closed_via_resolution".to_string(),
            confidence: 1.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "position_id": position.id.to_string(),
                "realized_pnl": realized_pnl.to_string(),
                "source": source,
            }),
        };
        let _ = self.signal_tx.send(signal);

        info!(
            market_id = %market_id,
            position_id = %position.id,
            realized_pnl = %realized_pnl,
            "Position closed via resolution fallback"
        );

        Ok(true)
    }

    async fn close_open_position_via_resolution(
        &self,
        position: &mut Position,
        source: i16,
        quant_ctx: Option<&QuantExitContext>,
    ) -> anyhow::Result<bool> {
        let market = match self.clob_client.get_market_by_id(&position.market_id).await {
            Ok(market) => market,
            Err(error) => {
                warn!(
                    market_id = %position.market_id,
                    error = %error,
                    "Failed to load resolved market details for fallback close"
                );
                return Ok(false);
            }
        };

        if !market.resolved {
            return Ok(false);
        }

        let yes_winner = self.resolved_yes_winner(&market).await;
        // A failed attempt leaves the exit to be marked failed, and retried
        // through resolution, rather than stuck closing.
        match self
            .finalize_resolution_close(position, source, quant_ctx, yes_winner)
            .await
        {
            Ok(closed) => Ok(closed),
            Err(error) => {
                warn!(position_id = %position.id, %error, "Resolution close after a failed sale did not complete");
                Ok(false)
            }
        }
    }

    /// Winner of a resolved market from the YES leg's point of view, matched
    /// by the token ids positions trade (see [`resolved_yes_winner_for_tokens`]).
    async fn resolved_yes_winner(&self, market: &Market) -> Option<bool> {
        let tokens = self.resolve_market_tokens(&market.id).await.ok().flatten();
        resolved_yes_winner_for_tokens(market, tokens.as_ref())
    }

    async fn resolve_market_tokens(
        &self,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        // Fast path: already in shared cache
        if let Some(ids) = self.token_cache.get(market_id).await {
            return Ok(Some(ids));
        }

        // Single-market hydration via CLOB API
        if let Some(ids) = self.token_cache.hydrate_market(market_id).await? {
            return Ok(Some(ids));
        }

        // If market_id is actually a token_id (asset_id), look up its condition_id
        if let Some(condition_id) = self.lookup_condition_id_for_token(market_id).await? {
            if let Some(ids) = self.token_cache.get(&condition_id).await {
                self.token_cache.alias(market_id, &ids).await;
                return Ok(Some(ids));
            }

            if let Some(ids) = self.token_cache.hydrate_market(&condition_id).await? {
                self.token_cache.alias(market_id, &ids).await;
                return Ok(Some(ids));
            }
        }

        Ok(None)
    }

    async fn lookup_condition_id_for_token(
        &self,
        token_id: &str,
    ) -> anyhow::Result<Option<String>> {
        let condition_id: Option<String> = sqlx::query_scalar(
            r#"
            SELECT condition_id
            FROM token_condition_cache
            WHERE token_id = $1
            LIMIT 1
            "#,
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(condition_id)
    }

    async fn load_quant_exit_contexts(
        &self,
        candidates: &[polymarket_core::db::positions::ExitCandidate],
    ) -> anyhow::Result<HashMap<uuid::Uuid, QuantExitContext>> {
        let position_ids: Vec<uuid::Uuid> = candidates
            .iter()
            .filter(|candidate| candidate.source == SOURCE_RECOMMENDATION)
            .map(|candidate| candidate.position.id)
            .collect();

        if position_ids.is_empty() {
            return Ok(HashMap::new());
        }

        #[derive(sqlx::FromRow)]
        struct QuantContextRow {
            position_id: uuid::Uuid,
            signal_id: uuid::Uuid,
            kind: String,
            direction: String,
            metadata: serde_json::Value,
        }

        let rows: Vec<QuantContextRow> = sqlx::query_as(
            r#"
            SELECT
                p.id AS position_id,
                qs.id AS signal_id,
                qs.kind,
                qs.direction,
                COALESCE(qs.metadata, '{}'::jsonb) AS metadata
            FROM positions p
            JOIN quant_signals qs
              ON qs.id = p.source_signal_id
            WHERE p.id = ANY($1)
            "#,
        )
        .bind(&position_ids)
        .fetch_all(&self.pool)
        .await?;

        let mut map = HashMap::new();
        for row in rows {
            let Some(kind) = parse_quant_kind(&row.kind) else {
                continue;
            };
            let Some(direction) = parse_signal_direction(&row.direction) else {
                continue;
            };
            map.insert(
                row.position_id,
                QuantExitContext {
                    signal_id: row.signal_id,
                    kind,
                    direction,
                    metadata: row.metadata,
                },
            );
        }

        Ok(map)
    }

    async fn load_quant_exit_context(
        &self,
        position_id: uuid::Uuid,
    ) -> anyhow::Result<Option<QuantExitContext>> {
        #[derive(sqlx::FromRow)]
        struct QuantContextRow {
            signal_id: uuid::Uuid,
            kind: String,
            direction: String,
            metadata: serde_json::Value,
        }

        let row: Option<QuantContextRow> = sqlx::query_as(
            r#"
            SELECT
                qs.id AS signal_id,
                qs.kind,
                qs.direction,
                COALESCE(qs.metadata, '{}'::jsonb) AS metadata
            FROM positions p
            JOIN quant_signals qs
              ON qs.id = p.source_signal_id
            WHERE p.id = $1
            "#,
        )
        .bind(position_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.and_then(|row| {
            Some(QuantExitContext {
                signal_id: row.signal_id,
                kind: parse_quant_kind(&row.kind)?,
                direction: parse_signal_direction(&row.direction)?,
                metadata: row.metadata,
            })
        }))
    }

    fn generic_quant_exit(&self, position: &Position, cfg: &ExitHandlerConfig) -> bool {
        let entry_cost = position.entry_cost();
        let pnl_pct = if entry_cost > Decimal::ZERO {
            position.unrealized_pnl / entry_cost
        } else {
            Decimal::ZERO
        };
        let held_hours = Utc::now()
            .signed_duration_since(position.entry_timestamp)
            .num_hours();

        pnl_pct >= cfg.quant_take_profit_pct
            || pnl_pct <= -cfg.quant_stop_loss_pct
            || held_hours >= cfg.quant_max_hold_hours
    }

    async fn flow_strategy_exit(
        &self,
        position: &Position,
        quant_ctx: &QuantExitContext,
        _cfg: &ExitHandlerConfig,
    ) -> anyhow::Result<bool> {
        let window_minutes = quant_ctx
            .metadata
            .get("window_minutes")
            .and_then(|value| value.as_i64())
            .unwrap_or(60) as i32;
        let entry_imbalance =
            json_decimal_abs(&quant_ctx.metadata, "imbalance_ratio").unwrap_or(Decimal::ZERO);

        let latest_row: Option<(Decimal,)> = sqlx::query_as(
            r#"
            SELECT imbalance_ratio
            FROM market_flow_features
            WHERE condition_id = $1
              AND window_minutes = $2
            ORDER BY window_end DESC
            LIMIT 1
            "#,
        )
        .bind(&position.market_id)
        .bind(window_minutes)
        .fetch_optional(&self.pool)
        .await?;

        let Some((imbalance_ratio,)) = latest_row else {
            return Ok(false);
        };

        let original_sign = direction_sign(quant_ctx.direction);
        let current_sign = if imbalance_ratio > Decimal::ZERO {
            1
        } else if imbalance_ratio < Decimal::ZERO {
            -1
        } else {
            0
        };
        let min_supported_imbalance = entry_imbalance
            .checked_div(Decimal::new(2, 0))
            .unwrap_or(Decimal::ZERO)
            .max(Decimal::new(15, 2));

        Ok((current_sign != 0 && current_sign != original_sign)
            || imbalance_ratio.abs() < min_supported_imbalance)
    }

    async fn should_mark_exit_ready(
        &self,
        position: &Position,
        source: i16,
        quant_ctx: Option<&QuantExitContext>,
        yes_bid: Decimal,
        no_bid: Decimal,
        cfg: &ExitHandlerConfig,
    ) -> anyhow::Result<bool> {
        if source != SOURCE_RECOMMENDATION {
            return Ok(position.unrealized_pnl > Decimal::ZERO);
        }

        let generic_exit = self.generic_quant_exit(position, cfg);
        if generic_exit {
            return Ok(true);
        }

        let Some(quant_ctx) = quant_ctx else {
            return Ok(false);
        };

        let current_yes = infer_yes_price(yes_bid, no_bid);
        let current_no = infer_no_price(yes_bid, no_bid);

        let strategy_exit = match quant_ctx.kind {
            QuantSignalKind::Flow => self
                .flow_strategy_exit(position, quant_ctx, cfg)
                .await
                .unwrap_or(false),
            QuantSignalKind::MeanReversion => {
                mean_reversion_target_hit(quant_ctx, current_yes, current_no)
            }
            QuantSignalKind::CrossMarket => cross_market_target_hit(quant_ctx, current_yes),
            QuantSignalKind::ResolutionProximity => resolution_lean_decay(quant_ctx, current_yes),
        };

        Ok(strategy_exit)
    }

    /// How long a resolved position has waited for its market to name a
    /// winner, from the first look that found none (recorded as a trade
    /// event, so the wait survives a restart).
    async fn waited_for_winner(
        &self,
        position: &Position,
        ctx: &EventContext,
    ) -> anyhow::Result<chrono::Duration> {
        let since: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
            "SELECT MIN(occurred_at) FROM trade_events \
             WHERE position_id = $1 AND event_type = $2",
        )
        .bind(position.id)
        .bind(WAITING_FOR_WINNER)
        .fetch_one(&self.pool)
        .await?;
        if let Some(since) = since {
            return Ok(Utc::now() - since);
        }
        self.trade_event_recorder
            .record(NewTradeEvent {
                position_id: Some(position.id),
                reason: Some(
                    "The market resolved without naming a winner and the legs differ".into(),
                ),
                metadata: serde_json::json!({
                    "held_yes": position.held_yes_qty.to_string(),
                    "held_no": position.held_no_qty.to_string(),
                }),
                ..NewTradeEvent::new(
                    &ctx.strategy,
                    &ctx.execution_mode,
                    &ctx.source_label,
                    &position.market_id,
                    WAITING_FOR_WINNER,
                )
            })
            .await?;
        Ok(chrono::Duration::zero())
    }

    /// Settle every position still holding shares in a resolved market,
    /// including exits that failed or stalled, whatever their exit strategy.
    async fn check_market_resolutions(&self) -> anyhow::Result<()> {
        let positions = self.position_repo.get_resolution_candidates().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Checking market resolutions");

        // Collect unique market IDs and check them individually. This avoids
        // pulling the entire CLOB market universe every resolution tick.
        let market_ids: HashSet<String> = positions.iter().map(|p| p.market_id.clone()).collect();
        let mut resolved_markets = HashMap::new();
        for market_id in &market_ids {
            self.touch_heartbeat();
            match self.clob_client.get_market_by_id(market_id).await {
                Ok(market) if market.resolved => {
                    let yes_winner = self.resolved_yes_winner(&market).await;
                    resolved_markets.insert(market_id.clone(), yes_winner);
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(market_id = %market_id, error = %e, "Failed to fetch market for resolution check");
                }
            }
        }

        if resolved_markets.is_empty() {
            debug!("No resolved markets found for held positions");
            return Ok(());
        }

        info!(
            resolved = resolved_markets.len(),
            "Found resolved markets with open positions"
        );

        for mut position in positions {
            self.touch_heartbeat();
            let Some(yes_winner) = resolved_markets.get(&position.market_id).copied() else {
                continue;
            };

            let quant_ctx = self
                .load_quant_exit_context(position.id)
                .await
                .ok()
                .flatten();
            let source = if quant_ctx.is_some() {
                SOURCE_RECOMMENDATION
            } else {
                SOURCE_ARBITRAGE
            };
            if let Err(error) = self
                .finalize_resolution_close(&mut position, source, quant_ctx.as_ref(), yes_winner)
                .await
            {
                warn!(position_id = %position.id, %error, "Failed to settle resolved position");
            }
        }

        Ok(())
    }

    async fn current_execution_mode(&self) -> String {
        if self.order_executor.is_live_ready().await {
            "live".to_string()
        } else {
            "paper".to_string()
        }
    }

    /// Build an EventContext for PositionService calls from position source and quant context.
    fn event_context(
        execution_mode: &str,
        source: i16,
        quant_ctx: Option<&QuantExitContext>,
    ) -> EventContext {
        let (strategy, source_label) = if let Some(qctx) = quant_ctx {
            (qctx.kind.as_str().to_string(), "quant".to_string())
        } else if source == SOURCE_RECOMMENDATION {
            ("quant".to_string(), "quant".to_string())
        } else {
            ("arb".to_string(), "arb".to_string())
        };
        EventContext {
            wallet_address: None,
            execution_mode: execution_mode.to_string(),
            strategy,
            source_label,
        }
    }

    async fn execute_sell_order_with_refresh(
        &self,
        market_id: &str,
        leg: &str,
        token_id: &str,
        side: OrderSide,
        quantity: Decimal,
        position: &Position,
    ) -> anyhow::Result<polymarket_core::types::ExecutionReport> {
        let order = MarketOrder::new(market_id.to_string(), token_id.to_string(), side, quantity);
        let first_attempt = order.id;

        match self
            .order_executor
            .execute_market_order_for_position(order, position, &leg.to_ascii_lowercase())
            .await
        {
            Ok(report) => Ok(report),
            Err(error) if is_not_found_error(&error) => {
                // Only a sale that never reached the exchange is tried again:
                // one journaled may have been placed.
                let journaled: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM execution_intents WHERE id = $1)",
                )
                .bind(first_attempt)
                .fetch_one(&self.pool)
                .await?;
                if journaled {
                    return Err(error);
                }
                warn!(
                    market_id = %market_id,
                    token_id = %token_id,
                    error = %error,
                    "Sell leg hit 404; refreshing token cache and retrying once"
                );

                self.touch_heartbeat();
                if let Err(refresh_error) = self.token_cache.refresh_market(market_id).await {
                    warn!(
                        market_id = %market_id,
                        error = %refresh_error,
                        "Failed to refresh exit token cache after sell-side 404"
                    );
                }

                let Some((yes_token_id, no_token_id)) =
                    self.resolve_market_tokens(market_id).await?
                else {
                    return Err(error);
                };
                let refreshed_token_id = if leg == "YES" {
                    yes_token_id
                } else {
                    no_token_id
                };

                if refreshed_token_id == token_id {
                    return Err(error);
                }

                let retry_order =
                    MarketOrder::new(market_id.to_string(), refreshed_token_id, side, quantity);
                self.order_executor
                    .execute_market_order_for_position(
                        retry_order,
                        position,
                        &leg.to_ascii_lowercase(),
                    )
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn close_via_resolution_if_market_resolved(
        &self,
        position: &mut Position,
        quant_ctx: Option<&QuantExitContext>,
    ) -> anyhow::Result<bool> {
        let market = match self.clob_client.get_market_by_id(&position.market_id).await {
            Ok(market) => market,
            Err(error) => {
                warn!(
                    market_id = %position.market_id,
                    error = %error,
                    "Failed to check market resolution after sell failure"
                );
                return Ok(false);
            }
        };

        if !market.resolved {
            return Ok(false);
        }

        let source = if quant_ctx.is_some() {
            SOURCE_RECOMMENDATION
        } else {
            SOURCE_ARBITRAGE
        };
        let yes_winner = self.resolved_yes_winner(&market).await;
        // A failed attempt leaves the exit to be marked failed, and retried
        // through resolution, rather than stuck closing.
        match self
            .finalize_resolution_close(position, source, quant_ctx, yes_winner)
            .await
        {
            Ok(closed) => Ok(closed),
            Err(error) => {
                warn!(position_id = %position.id, %error, "Resolution close after a failed sale did not complete");
                Ok(false)
            }
        }
    }

    /// Process one-legged entry failures by flattening the filled YES leg.
    async fn process_one_legged_recovery(&self) -> anyhow::Result<()> {
        let positions = self.position_repo.get_one_legged_entry_failed().await?;
        if positions.is_empty() {
            return Ok(());
        }

        info!(
            count = positions.len(),
            "Processing one-legged entry failures for recovery"
        );

        for mut position in positions {
            self.touch_heartbeat();
            let market_id = position.market_id.clone();
            let held_leg = position.one_legged_held_leg();
            let entry_price = if held_leg == "no" {
                position.no_entry_price
            } else {
                position.yes_entry_price
            };
            if entry_price <= Decimal::ZERO {
                warn!(
                    market_id = %market_id,
                    position_id = %position.id,
                    leg = held_leg,
                    "Cannot flatten one-legged position without a recorded entry price"
                );
                continue;
            }

            // Transition EntryFailed → ExitReady via PositionService so
            // mark_closing succeeds and a trade event is recorded.
            let execution_mode = self.current_execution_mode().await;
            let ctx = Self::event_context(&execution_mode, SOURCE_ARBITRAGE, None);
            if let Err(error) = self
                .position_service
                .transition_one_legged_to_exit_ready(
                    &mut position,
                    held_leg,
                    &format!("one-legged recovery: flattening held {held_leg} leg"),
                    &ctx,
                )
                .await
            {
                warn!(
                    market_id = %market_id,
                    position_id = %position.id,
                    error = %error,
                    "Failed to transition one-legged position for flattening"
                );
                continue;
            }

            if let Err(error) = self.execute_exit(&mut position).await {
                warn!(
                    market_id = %market_id,
                    position_id = %position.id,
                    error = %error,
                    "One-legged flattening attempt failed"
                );
            } else {
                info!(
                    market_id = %market_id,
                    position_id = %position.id,
                    "One-legged position sent through exit flow for flattening"
                );
            }
        }

        Ok(())
    }

    async fn process_orphan_inventory_recovery(
        &self,
        cfg: &ExitHandlerConfig,
    ) -> anyhow::Result<()> {
        let Some(wallet_address) = self.order_executor.wallet_address().await else {
            return Ok(());
        };

        let result = recover_wallet_orphan_inventory(
            &self.pool,
            self.order_executor.as_ref(),
            self.clob_client.as_ref(),
            &wallet_address,
            cfg.failed_exit_retry_backoff_secs,
            Some(&self.trade_event_recorder),
            Some(&self.signal_tx),
            Some(self.circuit_breaker.as_ref()),
        )
        .await?;

        if result.attempted > 0 {
            debug!(
                wallet_address = %wallet_address,
                attempted = result.attempted,
                succeeded = result.succeeded,
                "Processed orphan inventory recovery candidates"
            );
        }

        Ok(())
    }

    /// Process ExitFailed positions that have cooled down enough for requeue.
    async fn process_failed_exits(&self, cfg: &ExitHandlerConfig) -> anyhow::Result<()> {
        let positions = self.position_repo.get_failed_exits().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Processing failed exits for retry");

        for mut position in positions {
            self.touch_heartbeat();
            let age_secs = position.age_secs();
            if age_secs
                < failed_exit_retry_delay_secs(
                    cfg.failed_exit_retry_backoff_secs,
                    position.retry_count,
                )
            {
                continue;
            }

            let execution_mode = self.current_execution_mode().await;
            let quant_ctx = self
                .load_quant_exit_context(position.id)
                .await
                .ok()
                .flatten();
            let source = if quant_ctx.is_some() {
                SOURCE_RECOMMENDATION
            } else {
                SOURCE_ARBITRAGE
            };
            let ctx = Self::event_context(&execution_mode, source, quant_ctx.as_ref());
            let _ = self
                .position_service
                .attempt_exit_recovery(&mut position, &ctx)
                .await;

            debug!(
                position_id = %position.id,
                retry_count = position.retry_count,
                age_secs,
                "Position exit recovery attempted"
            );
        }

        Ok(())
    }

    /// Publish an alert signal to WebSocket clients.
    fn publish_alert(&self, market_id: &str, action: &str, reason: &str) {
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Alert,
            market_id: market_id.to_string(),
            outcome_id: "both".to_string(),
            action: action.to_string(),
            confidence: 0.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "reason": reason,
            }),
        };
        let _ = self.signal_tx.send(signal);
    }
}

/// Delay before retrying a failed exit: the configured backoff, doubling with
/// each consecutive failure, capped at an hour (or the backoff if longer).
/// Held shares are never abandoned; they keep being retried at this pace and
/// are closed by the resolution sweep once the market resolves.
fn failed_exit_retry_delay_secs(base_secs: u64, consecutive_failures: u32) -> u64 {
    let doublings = consecutive_failures.saturating_sub(1).min(16);
    base_secs
        .saturating_mul(1u64 << doublings)
        .min(base_secs.max(3600))
}

fn parse_quant_kind(kind: &str) -> Option<QuantSignalKind> {
    match kind {
        "flow" => Some(QuantSignalKind::Flow),
        "mean_reversion" => Some(QuantSignalKind::MeanReversion),
        "cross_market" => Some(QuantSignalKind::CrossMarket),
        "resolution_proximity" => Some(QuantSignalKind::ResolutionProximity),
        _ => None,
    }
}

fn is_not_found_error(error: &anyhow::Error) -> bool {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<PolymarketError>()
            .is_some_and(|source| {
                matches!(
                    source,
                    PolymarketError::Api {
                        status: Some(404),
                        ..
                    }
                )
            })
    }) {
        return true;
    }

    let lower = error.to_string().to_lowercase();
    lower.contains("404") || lower.contains("not found")
}

fn parse_signal_direction(direction: &str) -> Option<SignalDirection> {
    match direction {
        "buy_yes" => Some(SignalDirection::BuyYes),
        "buy_no" => Some(SignalDirection::BuyNo),
        _ => None,
    }
}

fn infer_yes_price(yes_bid: Decimal, no_bid: Decimal) -> Decimal {
    if yes_bid > Decimal::ZERO {
        yes_bid
    } else if no_bid > Decimal::ZERO {
        (Decimal::ONE - no_bid).max(Decimal::ZERO)
    } else {
        Decimal::ZERO
    }
}

fn infer_no_price(yes_bid: Decimal, no_bid: Decimal) -> Decimal {
    if no_bid > Decimal::ZERO {
        no_bid
    } else if yes_bid > Decimal::ZERO {
        (Decimal::ONE - yes_bid).max(Decimal::ZERO)
    } else {
        Decimal::ZERO
    }
}

fn direction_sign(direction: SignalDirection) -> i32 {
    match direction {
        SignalDirection::BuyYes => 1,
        SignalDirection::BuyNo => -1,
    }
}

fn json_decimal_abs(metadata: &serde_json::Value, key: &str) -> Option<Decimal> {
    let value = metadata.get(key)?;
    if let Some(raw) = value.as_str() {
        return raw.parse::<Decimal>().ok().map(|v| v.abs());
    }
    if let Some(raw) = value.as_f64() {
        return Decimal::try_from(raw).ok().map(|v| v.abs());
    }
    None
}

fn json_decimal(metadata: &serde_json::Value, key: &str) -> Option<Decimal> {
    let value = metadata.get(key)?;
    if let Some(raw) = value.as_str() {
        return raw.parse::<Decimal>().ok();
    }
    if let Some(raw) = value.as_f64() {
        return Decimal::try_from(raw).ok();
    }
    None
}

fn mean_reversion_target_hit(
    quant_ctx: &QuantExitContext,
    current_yes: Decimal,
    current_no: Decimal,
) -> bool {
    let Some(current_price) = json_decimal(&quant_ctx.metadata, "current_price") else {
        return false;
    };
    let Some(previous_price) = json_decimal(&quant_ctx.metadata, "previous_price") else {
        return false;
    };
    let target_yes = (current_price + previous_price) / Decimal::new(2, 0);
    match quant_ctx.direction {
        SignalDirection::BuyYes => current_yes >= target_yes,
        SignalDirection::BuyNo => current_no >= (Decimal::ONE - target_yes).max(Decimal::ZERO),
    }
}

fn cross_market_target_hit(quant_ctx: &QuantExitContext, current_yes: Decimal) -> bool {
    let Some(lag_current_price) = json_decimal(&quant_ctx.metadata, "lag_current_price") else {
        return false;
    };
    let lead_change = quant_ctx
        .metadata
        .get("lead_change")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0)
        .abs();
    let lag_change = quant_ctx
        .metadata
        .get("lag_change")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0)
        .abs();
    let divergence_gap = ((lead_change - lag_change) / 2.0).max(0.0);
    let Ok(divergence_gap) = Decimal::try_from(divergence_gap) else {
        return false;
    };

    match quant_ctx.direction {
        SignalDirection::BuyYes => current_yes >= lag_current_price + divergence_gap,
        SignalDirection::BuyNo => {
            current_yes <= (lag_current_price - divergence_gap).max(Decimal::ZERO)
        }
    }
}

fn resolution_lean_decay(quant_ctx: &QuantExitContext, current_yes: Decimal) -> bool {
    let Some(entry_deviation) = json_decimal_abs(&quant_ctx.metadata, "deviation") else {
        return false;
    };
    let current_deviation = (current_yes - Decimal::new(50, 2)).abs();
    current_deviation <= entry_deviation / Decimal::new(2, 0)
}

/// Feed a closed position's realized P&L to the circuit breaker, which
/// guards real money: only a live position's result counts, live trading
/// switched off or not. A paper result (or one of unknown provenance) never
/// does, so a paper win can never end a run of live losses or offset them.
/// A result the wallet did not confirm (`confirmed` false: a resolution
/// close at the record) counts as a loss, never as a win.
pub(crate) async fn record_close_with_breaker(
    pool: &PgPool,
    breaker: &CircuitBreaker,
    position: &Position,
    confirmed: bool,
) {
    let pnl = position.realized_pnl.unwrap_or_default();
    if breaker_counts_close(&position.execution_mode, pnl, confirmed) {
        // Marked by the save that stores it; a close not stored stays
        // unmarked, for the next trading leader to record.
        if let Err(error) = breaker.record_close(pnl, position.id).await {
            warn!(%error, position_id = %position.id, "Failed to record a close with the circuit breaker");
        }
    } else {
        debug!(
            position_id = %position.id,
            position_mode = %position.execution_mode,
            confirmed,
            "Close left out of the circuit breaker"
        );
        mark_breaker_recorded(pool, position.id).await;
    }
}

/// Whether a close counts with the circuit breaker: live ones only, and
/// never a win a resolution close could not confirm in the wallet.
fn breaker_counts_close(execution_mode: &str, pnl: Decimal, confirmed: bool) -> bool {
    execution_mode == "live" && (confirmed || pnl <= Decimal::ZERO)
}

/// Marks a close the circuit breaker does not count as seen.
async fn mark_breaker_recorded(pool: &PgPool, position_id: uuid::Uuid) {
    if let Err(error) = sqlx::query(
        "UPDATE positions SET breaker_recorded_at = NOW() \
         WHERE id = $1 AND breaker_recorded_at IS NULL",
    )
    .bind(position_id)
    .execute(pool)
    .await
    {
        warn!(%error, %position_id, "Could not mark a close as seen by the circuit breaker");
    }
}

/// Records with the circuit breaker the closes its stored counters miss: a
/// close is marked by the save that stores it, so an unmarked one was booked
/// by a process that stopped before saving it. Today's count, in close order
/// (the breaker's daily counters have moved past earlier days'); the rest
/// are only marked. Run by a new trading leader before it trades. During the
/// first upgrade to this version, a close the previous version makes while
/// handing over is counted twice, which errs towards tripping.
pub(crate) async fn record_missed_closes(
    pool: &PgPool,
    breaker: &CircuitBreaker,
) -> anyhow::Result<()> {
    #[derive(sqlx::FromRow)]
    struct MissedClose {
        id: uuid::Uuid,
        realized_pnl: Option<Decimal>,
        execution_mode: String,
        exit_timestamp: Option<chrono::DateTime<chrono::Utc>>,
        confirmed: bool,
    }
    let missed: Vec<MissedClose> = sqlx::query_as(
        r#"
        SELECT p.id, p.realized_pnl, p.execution_mode, p.exit_timestamp,
               NOT EXISTS (
                   SELECT 1 FROM trade_events te
                   WHERE te.position_id = p.id
                     AND te.event_type = 'closed_via_resolution'
                     AND te.metadata ? 'unmeasured'
               ) AS confirmed
        FROM positions p
        WHERE p.state = 4 AND p.breaker_recorded_at IS NULL
        ORDER BY p.exit_timestamp NULLS FIRST, p.id
        "#,
    )
    .fetch_all(pool)
    .await?;
    let today = chrono::Utc::now().date_naive();
    for close in missed {
        let pnl = close.realized_pnl.unwrap_or_default();
        let today_close = close
            .exit_timestamp
            .is_some_and(|at| at.date_naive() == today);
        if today_close && breaker_counts_close(&close.execution_mode, pnl, close.confirmed) {
            breaker.record_close(pnl, close.id).await?;
            warn!(position_id = %close.id, %pnl, "Recorded a close the circuit breaker had missed");
        } else {
            if close.execution_mode == "live" && !today_close {
                warn!(
                    position_id = %close.id,
                    %pnl,
                    "A close from an earlier day was never recorded with the circuit breaker"
                );
            }
            mark_breaker_recorded(pool, close.id).await;
        }
    }
    Ok(())
}

/// Spawn the exit handler as a background task.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_exit_handler(
    config: Arc<RwLock<ExitHandlerConfig>>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
    pool: PgPool,
    heartbeat: Arc<AtomicI64>,
    token_cache: Arc<OutcomeTokenCache>,
) {
    let handler = ExitHandler::new(
        config,
        order_executor,
        circuit_breaker,
        clob_client,
        signal_tx,
        trade_event_tx,
        pool,
        heartbeat,
        token_cache,
    );

    tokio::spawn(async move {
        if let Err(e) = handler.run().await {
            error!(error = %e, "Exit handler failed");
        }
    });

    info!("Exit handler spawned as background task");
}

#[cfg(test)]
mod strategy_exit_tests {
    use super::*;
    use serde_json::json;

    fn quant_ctx(
        kind: QuantSignalKind,
        direction: SignalDirection,
        metadata: serde_json::Value,
    ) -> QuantExitContext {
        QuantExitContext {
            signal_id: uuid::Uuid::new_v4(),
            kind,
            direction,
            metadata,
        }
    }

    #[test]
    fn mean_reversion_exit_hits_midpoint_target_for_yes() {
        let ctx = quant_ctx(
            QuantSignalKind::MeanReversion,
            SignalDirection::BuyYes,
            json!({
                "current_price": "0.40",
                "previous_price": "0.60"
            }),
        );

        assert!(mean_reversion_target_hit(
            &ctx,
            Decimal::new(50, 2),
            Decimal::new(50, 2)
        ));
        assert!(!mean_reversion_target_hit(
            &ctx,
            Decimal::new(45, 2),
            Decimal::new(55, 2)
        ));
    }

    #[test]
    fn cross_market_exit_requires_divergence_compression() {
        let ctx = quant_ctx(
            QuantSignalKind::CrossMarket,
            SignalDirection::BuyYes,
            json!({
                "lag_current_price": "0.40",
                "lead_change": 0.20,
                "lag_change": 0.04
            }),
        );

        assert!(cross_market_target_hit(&ctx, Decimal::new(48, 2)));
        assert!(!cross_market_target_hit(&ctx, Decimal::new(45, 2)));
    }

    #[test]
    fn resolution_exit_triggers_when_lean_decays_by_half() {
        let ctx = quant_ctx(
            QuantSignalKind::ResolutionProximity,
            SignalDirection::BuyYes,
            json!({
                "deviation": "0.20"
            }),
        );

        assert!(resolution_lean_decay(&ctx, Decimal::new(58, 2)));
        assert!(!resolution_lean_decay(&ctx, Decimal::new(70, 2)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test(migrations = false)]
    async fn closes_the_breaker_missed_are_recorded_once_by_the_next_leader(pool: PgPool) {
        sqlx::raw_sql(
            "CREATE TABLE positions (id UUID PRIMARY KEY, state SMALLINT, realized_pnl NUMERIC, \
                 execution_mode TEXT, exit_timestamp TIMESTAMPTZ, breaker_recorded_at TIMESTAMPTZ); \
             CREATE TABLE trade_events (position_id UUID, event_type TEXT, metadata JSONB);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let close = |pnl: i64, mode: &str, minutes_ago: i64, recorded: bool| {
            let pool = pool.clone();
            let mode = mode.to_string();
            async move {
                let id = uuid::Uuid::new_v4();
                sqlx::query(
                    "INSERT INTO positions VALUES ($1, 4, $2, $3, $4, \
                     CASE WHEN $5 THEN NOW() END)",
                )
                .bind(id)
                .bind(Decimal::from(pnl))
                .bind(mode)
                .bind(chrono::Utc::now() - chrono::Duration::minutes(minutes_ago))
                .bind(recorded)
                .execute(&pool)
                .await
                .unwrap();
                id
            }
        };
        // Missed today: a live loss counts; a live win the wallet could not
        // confirm and a paper loss do not.
        close(-30, "live", 0, false).await;
        let unconfirmed = close(50, "live", 0, false).await;
        sqlx::query(
            "INSERT INTO trade_events VALUES ($1, 'closed_via_resolution', '{\"unmeasured\": []}')",
        )
        .bind(unconfirmed)
        .execute(&pool)
        .await
        .unwrap();
        close(-40, "paper", 0, false).await;
        // Missed on an earlier day, and marked already: neither counts.
        close(-70, "live", 2 * 24 * 60, false).await;
        close(-90, "live", 0, true).await;

        sqlx::raw_sql(
            "CREATE TABLE circuit_breaker_state (id INTEGER PRIMARY KEY DEFAULT 1, \
                 tripped BOOLEAN NOT NULL DEFAULT FALSE, trip_reason VARCHAR(50), \
                 tripped_at TIMESTAMPTZ, resume_at TIMESTAMPTZ, \
                 daily_pnl DECIMAL(18, 8) NOT NULL DEFAULT 0, \
                 peak_value DECIMAL(18, 8) NOT NULL DEFAULT 0, \
                 current_value DECIMAL(18, 8) NOT NULL DEFAULT 0, \
                 consecutive_losses INTEGER NOT NULL DEFAULT 0, \
                 trips_today INTEGER NOT NULL DEFAULT 0, \
                 last_reset_date DATE NOT NULL DEFAULT CURRENT_DATE, \
                 updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()); \
             INSERT INTO circuit_breaker_state (id) VALUES (1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        for migration in [
            include_str!("../../../migrations/079_circuit_breaker_full_state.sql"),
            include_str!("../../../migrations/094_circuit_breaker_operator_epoch.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        let breaker = CircuitBreaker::with_persistence(Default::default(), pool.clone());
        breaker.lead().await.unwrap();
        record_missed_closes(&pool, &breaker).await.unwrap();
        let state = breaker.state().await;
        assert_eq!(state.daily_pnl, Decimal::from(-30));
        assert_eq!(state.consecutive_losses, 1);
        // Stored with the counters, and marked with them.
        let stored: Decimal =
            sqlx::query_scalar("SELECT daily_pnl FROM circuit_breaker_state WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, Decimal::from(-30));
        let unmarked: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM positions WHERE breaker_recorded_at IS NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unmarked, 0);

        // A second leader finds nothing left to record.
        let next = CircuitBreaker::with_persistence(Default::default(), pool.clone());
        next.lead().await.unwrap();
        record_missed_closes(&pool, &next).await.unwrap();
        assert_eq!(next.state().await.daily_pnl, Decimal::from(-30));
    }

    #[test]
    fn sales_refused_before_sending_are_told_apart() {
        // The executor's words for a sale it never sent.
        assert!(nothing_was_sent(
            "live_submission_fence_busy: another live order is still being posted; nothing was sent"
        ));
        assert!(nothing_was_sent(
            "Live order preparation exceeded its deadline; nothing was sent"
        ));
        assert!(nothing_was_sent(
            "HTTP request error: connection refused: the connection to the exchange failed, so \
             nothing was sent"
        ));
        // A rejection or an unknown outcome is a failed exit.
        assert!(!nothing_was_sent("not enough balance / allowance"));
        assert!(!nothing_was_sent("submission_requires_reconciliation"));
    }

    #[sqlx::test(migrations = false)]
    async fn live_entries_wait_while_live_exits_keep_failing(pool: PgPool) {
        sqlx::raw_sql(
            "CREATE TABLE positions (execution_mode TEXT, state SMALLINT, \
             retry_count INTEGER, last_updated TIMESTAMPTZ)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let config = Arc::new(RwLock::new(ExitHandlerConfig {
            enabled: true,
            ..ExitHandlerConfig::from_env()
        }));
        let heartbeat = Arc::new(AtomicI64::new(Utc::now().timestamp()));
        let fresh =
            || ExitReadiness::new(config.clone(), heartbeat.clone()).watching_exits(pool.clone());
        assert_eq!(fresh().refusal().await, None);

        for (mode, state, retries, minutes_ago, count, stuck) in [
            ("live", 6, 2, 0, 3, false),  // failed twice: still trying
            ("paper", 6, 9, 0, 3, false), // not live
            ("live", 2, 0, 5, 3, false),  // queued a moment ago
            ("live", 6, 3, 0, 2, false),  // two stuck markets: theirs alone
            ("live", 6, 3, 0, 3, true),   // three failing: selling is failing
            ("live", 3, 0, 45, 3, true),  // closing with no step for 45 minutes
        ] {
            sqlx::query("DELETE FROM positions")
                .execute(&pool)
                .await
                .unwrap();
            for _ in 0..count {
                sqlx::query(
                    "INSERT INTO positions VALUES ($1, $2, $3, NOW() - make_interval(mins => $4))",
                )
                .bind(mode)
                .bind(state as i16)
                .bind(retries)
                .bind(minutes_ago)
                .execute(&pool)
                .await
                .unwrap();
            }
            assert_eq!(
                fresh().refusal().await,
                stuck.then_some("live_exits_failing"),
                "{mode} {state} {retries} {minutes_ago} x{count}"
            );
        }
    }

    #[test]
    fn test_config_default() {
        let config = ExitHandlerConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.exit_poll_interval_secs, 30);
        assert_eq!(config.resolution_check_secs, 300);
        assert_eq!(config.failed_exit_retry_backoff_secs, 300);
    }

    #[test]
    fn test_config_from_env() {
        // With no env vars, should use defaults
        let config = ExitHandlerConfig::from_env();
        assert!(!config.enabled);
        assert_eq!(config.exit_poll_interval_secs, 30);
        assert_eq!(config.resolution_check_secs, 300);
        assert_eq!(config.failed_exit_retry_backoff_secs, 300);
    }

    #[test]
    fn test_held_outcomes_for_single_leg_positions() {
        let mut yes_only = Position::new(
            "m1".to_string(),
            Decimal::new(55, 2),
            Decimal::ZERO,
            Decimal::ONE,
            polymarket_core::types::ExitStrategy::ExitOnCorrection,
        );
        yes_only.apply_yes_entry_fill(Decimal::ONE);
        assert_eq!(held_outcomes(&yes_only), (true, false));

        let mut no_only = Position::new(
            "m1".to_string(),
            Decimal::ZERO,
            Decimal::new(45, 2),
            Decimal::ONE,
            polymarket_core::types::ExitStrategy::ExitOnCorrection,
        );
        no_only.apply_no_entry_fill(Decimal::ONE);
        assert_eq!(held_outcomes(&no_only), (false, true));
    }

    #[test]
    fn test_held_outcomes_ignore_legs_already_recorded_as_exited() {
        let mut both_legs = Position::new(
            "m1".to_string(),
            Decimal::new(55, 2),
            Decimal::new(45, 2),
            Decimal::ONE,
            polymarket_core::types::ExitStrategy::ExitOnCorrection,
        );
        both_legs.apply_yes_entry_fill(Decimal::ONE);
        both_legs.apply_no_entry_fill(Decimal::ONE);
        both_legs.mark_open().unwrap();
        both_legs.mark_exit_ready().unwrap();
        both_legs.mark_closing().unwrap();
        both_legs.record_yes_exit_fill(Decimal::new(54, 2)).unwrap();

        both_legs.apply_yes_exit_fill(Decimal::ONE).unwrap();
        assert_eq!(held_outcomes(&both_legs), (false, true));
    }

    #[test]
    fn test_resolved_yes_winner_from_market_outcomes() {
        let market = Market {
            id: "m1".to_string(),
            question: "Question".to_string(),
            description: None,
            outcomes: vec![
                polymarket_core::types::Outcome {
                    id: "yes".to_string(),
                    name: "Yes".to_string(),
                    token_id: "yes".to_string(),
                    price: None,
                    winner: Some(true),
                },
                polymarket_core::types::Outcome {
                    id: "no".to_string(),
                    name: "No".to_string(),
                    token_id: "no".to_string(),
                    price: None,
                    winner: Some(false),
                },
            ],
            volume: Decimal::ZERO,
            liquidity: Decimal::ZERO,
            end_date: None,
            resolved: true,
            resolution: None,
            category: None,
            tags: Vec::new(),
            fees_enabled: false,
            fee_type: None,
            fee_schedule: None,
        };

        assert_eq!(resolved_yes_winner(&market), Some(true));
    }

    #[test]
    fn a_named_outcome_market_is_resolved_by_its_tokens() {
        let outcome =
            |token: &str, name: &str, winner: Option<bool>| polymarket_core::types::Outcome {
                id: token.to_string(),
                name: name.to_string(),
                token_id: token.to_string(),
                price: None,
                winner,
            };
        let market = |up: Option<bool>, down: Option<bool>| Market {
            id: "m1".to_string(),
            question: "Up or down?".to_string(),
            description: None,
            outcomes: vec![outcome("up", "Up", up), outcome("down", "Down", down)],
            volume: Decimal::ZERO,
            liquidity: Decimal::ZERO,
            end_date: None,
            resolved: true,
            resolution: None,
            category: None,
            tags: Vec::new(),
            fees_enabled: false,
            fee_type: None,
            fee_schedule: None,
        };
        let tokens = ("up".to_string(), "down".to_string());
        assert_eq!(
            resolved_yes_winner_for_tokens(&market(Some(false), Some(true)), Some(&tokens)),
            Some(false)
        );
        assert_eq!(
            resolved_yes_winner_for_tokens(&market(Some(true), Some(false)), Some(&tokens)),
            Some(true)
        );
        // Both flagged alike, or the names not Yes/No: no winner is guessed.
        assert_eq!(
            resolved_yes_winner_for_tokens(&market(Some(false), Some(false)), Some(&tokens)),
            None
        );
        assert_eq!(
            resolved_yes_winner_for_tokens(&market(Some(true), Some(false)), None),
            None
        );
    }

    #[test]
    fn only_one_flagged_winner_names_the_winner() {
        for (yes, no, winner) in [
            (Some(true), Some(false), Some(true)),
            (Some(true), None, Some(true)),
            (Some(false), Some(true), Some(false)),
            (None, Some(true), Some(false)),
            // A split payout, or flags not yet set, names no winner.
            (Some(false), Some(false), None),
            (Some(true), Some(true), None),
            (Some(false), None, None),
            (None, Some(false), None),
            (None, None, None),
        ] {
            assert_eq!(single_winner(yes, no), winner, "{yes:?} {no:?}");
        }
    }
}

fn held_outcomes(position: &Position) -> (bool, bool) {
    position.held_outcomes()
}

/// The most a buy's fee kept of the shares bought on the V1 exchange, which
/// took buy fees in shares (the V2 exchange charges them in collateral): the
/// fee is `rate × p(1 − p)` USDC per share, paid in shares worth `p`, so
/// `rate × (1 − p)` of them, and no published rate exceeds 0.07.
const MAX_FEE_SHARE_FRACTION: Decimal = Decimal::from_parts(10, 0, 0, false, 2);

/// Shares bought on a token within this window may still be settling on chain.
const FEE_WRITE_OFF_SETTLE_MINUTES: i32 = 30;

/// How long a position's sales may go unsent in a row before the exit counts
/// as failed (and is alerted), and the pause that starts a new run.
const MAX_EXIT_DEFERRAL: std::time::Duration = std::time::Duration::from_secs(600);
const DEFERRAL_GAP: std::time::Duration = std::time::Duration::from_secs(300);

/// Whether an order failure says the order certainly never left: another
/// live order held the wallet's submission fence, or preparing it ran out of
/// time. Such a sale is tried again next pass, not counted as a failed exit
/// (whose retries slow down).
fn nothing_was_sent(message: &str) -> bool {
    message.contains("nothing was sent")
}

/// For a position, market, wallet, leg ('yes'/'no'), token and settling
/// window: what the wallet's other live positions and manual orders hold of
/// the leg, and whether an order of the wallet on the token has an unknown
/// outcome, or a position of the wallet in the market has an order not yet
/// applied (a sale another position has made but not recorded makes its
/// holding look larger than it is), or a buy of the wallet not refused may
/// still be settling. Sells are left out of the window: sold shares leave the wallet
/// after the position records them, so they never make the balance look
/// short, and a refused exit sell must not keep the write-off waiting.
const OTHER_HOLDERS_SQL: &str = "SELECT \
    COALESCE((SELECT SUM(CASE WHEN $4 = 'yes' THEN held_yes_qty ELSE held_no_qty END) FROM positions \
        WHERE id <> $1 AND market_id = $2 AND execution_mode = 'live' AND LOWER(wallet_address) = LOWER($3)), 0) \
    + COALESCE((SELECT SUM(quantity) FROM manual_inventory \
        WHERE execution_mode = 'live' AND wallet_address = LOWER($3) AND market_id = $2 AND outcome = $4), 0), \
    EXISTS(SELECT 1 FROM execution_intents WHERE signed_order->>'tokenId' = $5 \
        AND LOWER(signed_order->>'maker') = LOWER($3) \
        AND (status IN ('submitting', 'unknown') \
            OR (signed_order->>'side' = 'BUY' AND status <> 'rejected' \
                AND created_at > NOW() - make_interval(mins => $6)))) \
    OR EXISTS(SELECT 1 FROM position_order_links l JOIN positions p ON p.id = l.position_id \
        WHERE NOT l.settled AND p.market_id = $2 AND LOWER(p.wallet_address) = LOWER($3))";

/// Why a measurement waits when the wallet's balance could not be read.
const BALANCE_UNREADABLE: &str = "the wallet's balance could not be read";
/// Why it waits when a live executor's signer did not load (a restart may).
const SIGNER_NOT_LOADED: &str = "no trading wallet is loaded";

/// Whether a measurement that waits for `why` gives way, after the grace,
/// to closing at the record: the balance stays out of reach, not in flux.
fn waits_out_its_grace(why: &str) -> bool {
    why == BALANCE_UNREADABLE || why == SIGNER_NOT_LOADED
}
/// How long a resolved position waits for a balance that cannot be read
/// before it closes at its recorded holding.
const UNREADABLE_BALANCE_GRACE: std::time::Duration = std::time::Duration::from_secs(3600);
/// The trade event marking the first look that found no winner.
const WAITING_FOR_WINNER: &str = "resolution_waiting_for_winner";

/// What measuring a position's leg against the wallet found.
enum Measured {
    /// The record is what the wallet holds (any fee shares written off), or
    /// is not measured at all (a paper position, one with no wallet
    /// recorded, or nothing held).
    Matches,
    /// It could not be measured, for this reason; the record stands.
    Unmeasured(&'static str),
    /// It cannot be measured yet, for this reason.
    Waiting(&'static str),
}

/// How much of `held` to write off when the wallet holds only `balance` for
/// it: the whole shortfall when it is no more than V1 buy fees kept in shares
/// (or dust), otherwise none.
fn fee_shortfall(held: Decimal, balance: Decimal) -> Option<Decimal> {
    let shortfall = held - balance.max(Decimal::ZERO);
    (shortfall > Decimal::ZERO && shortfall <= held * MAX_FEE_SHARE_FRACTION).then_some(shortfall)
}

/// Winner of a resolved market from the YES leg's point of view. The YES leg
/// is the token the token cache maps as YES, so markets whose outcomes are
/// named (not "Yes"/"No") resolve too; outcome names are only a fallback.
fn resolved_yes_winner_for_tokens(
    market: &Market,
    tokens: Option<&(String, String)>,
) -> Option<bool> {
    if let Some((yes_token, no_token)) = tokens {
        let flag = |token: &str| {
            market
                .outcomes
                .iter()
                .find(|outcome| outcome.token_id == token)
                .and_then(|outcome| outcome.winner)
        };
        if let Some(yes_wins) = single_winner(flag(yes_token), flag(no_token)) {
            return Some(yes_wins);
        }
    }
    resolved_yes_winner(market)
}

fn resolved_yes_winner(market: &Market) -> Option<bool> {
    let flag = |name: &str| {
        market
            .outcomes
            .iter()
            .find(|outcome| outcome.name.eq_ignore_ascii_case(name))
            .and_then(|outcome| outcome.winner)
    };
    single_winner(flag("yes"), flag("no"))
}

/// Whether YES won, from the two outcomes' winner flags: only one outcome
/// flagged the winner decides it. No flag, or both alike (a split payout,
/// or not yet settled), names no winner.
fn single_winner(yes: Option<bool>, no: Option<bool>) -> Option<bool> {
    match (yes, no) {
        (Some(true), Some(false) | None) => Some(true),
        (Some(false) | None, Some(true)) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
#[path = "exit_handler/correctness_tests.rs"]
pub(crate) mod correctness_tests;
