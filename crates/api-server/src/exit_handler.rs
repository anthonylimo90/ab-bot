//! Exit handler — closes positions via market exit or resolution.
//!
//! Companion to `arb_executor.rs`: the arb executor opens positions but nothing
//! closes them. This module handles two exit paths:
//! - **ExitOnCorrection**: sell YES + NO when arb-monitor marks position `ExitReady`
//! - **HoldToResolution**: wait for market to resolve ($1 payout)
//!
//! Shares the `active_markets` dedup set with `ArbAutoExecutor` via `Arc<RwLock<>>`
//! so closed positions unblock their markets for future trades.

use chrono::Utc;
use polymarket_core::api::{ClobClient, GammaClient};
use polymarket_core::db::positions::{PositionRepository, SOURCE_ARBITRAGE, SOURCE_RECOMMENDATION};
use polymarket_core::error::Error as PolymarketError;
use polymarket_core::types::signal::{QuantSignalKind, SignalDirection};
use polymarket_core::types::{FailureReason, Market, MarketOrder, OrderSide, Position};
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
use crate::trade_events::TradeEventRecorder;
use crate::wallet_inventory::recover_wallet_orphan_inventory;
use crate::websocket::{SignalType, SignalUpdate};

enum ExitBidStatus {
    Available(Decimal, Decimal),
    Resolved,
    Unavailable,
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
    pub quant_take_profit_pct: Decimal,
    /// Unrealized loss threshold for generic quant exits.
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
            enabled: std::env::var("EXIT_HANDLER_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(false),
            exit_poll_interval_secs: std::env::var("EXIT_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            resolution_check_secs: std::env::var("EXIT_RESOLUTION_CHECK_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            quant_take_profit_pct: std::env::var("QUANT_TAKE_PROFIT_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(15, 2)),
            quant_stop_loss_pct: std::env::var("QUANT_STOP_LOSS_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(10, 2)),
            quant_max_hold_hours: std::env::var("QUANT_MAX_HOLD_HOURS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(24),
            failed_exit_retry_backoff_secs: std::env::var("EXIT_FAILED_RETRY_BACKOFF_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
        }
    }
}

/// Cached mapping of market_id → (yes_token_id, no_token_id).
struct OutcomeTokenCache {
    gamma_client: GammaClient,
    tokens: RwLock<HashMap<String, (String, String)>>,
}

impl OutcomeTokenCache {
    fn new() -> Self {
        Self {
            gamma_client: GammaClient::new(None),
            tokens: RwLock::new(HashMap::new()),
        }
    }

    async fn refresh(&self) -> anyhow::Result<usize> {
        let gamma_page_size = std::env::var("GAMMA_ARB_MARKET_PAGE_SIZE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(200);
        let markets = self
            .gamma_client
            .get_all_tradable_markets(gamma_page_size)
            .await?;
        let mut map = HashMap::new();

        for market in &markets {
            if market.outcomes.len() == 2 {
                let (yes_id, no_id) = if market.outcomes[0].name.to_lowercase().contains("yes") {
                    (
                        market.outcomes[0].token_id.clone(),
                        market.outcomes[1].token_id.clone(),
                    )
                } else {
                    (
                        market.outcomes[1].token_id.clone(),
                        market.outcomes[0].token_id.clone(),
                    )
                };
                map.insert(market.id.clone(), (yes_id, no_id));
            }
        }

        let count = map.len();
        *self.tokens.write().await = map;
        Ok(count)
    }

    async fn get(&self, market_id: &str) -> Option<(String, String)> {
        self.tokens.read().await.get(market_id).cloned()
    }

    async fn hydrate_market(&self, market_id: &str) -> anyhow::Result<Option<(String, String)>> {
        if let Some(ids) = self.get(market_id).await {
            return Ok(Some(ids));
        }

        let market = match self.gamma_client.get_market(market_id).await {
            Ok(market) => market,
            Err(error) => {
                warn!(
                    market_id = %market_id,
                    error = %error,
                    "Failed to fetch market for exit token cache hydration"
                );
                return Ok(None);
            }
        };

        let outcome_names = market
            .outcomes
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok());
        let token_ids = market
            .clob_token_ids
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok());

        let Some(outcome_names) = outcome_names else {
            return Ok(None);
        };
        let Some(token_ids) = token_ids else {
            return Ok(None);
        };

        if outcome_names.len() != 2 || token_ids.len() != 2 {
            return Ok(None);
        }

        let (yes_id, no_id) = if outcome_names[0].to_lowercase().contains("yes") {
            (token_ids[0].clone(), token_ids[1].clone())
        } else {
            (token_ids[1].clone(), token_ids[0].clone())
        };

        self.tokens
            .write()
            .await
            .insert(market_id.to_string(), (yes_id.clone(), no_id.clone()));
        Ok(Some((yes_id, no_id)))
    }

    async fn hydrate_clob_market(
        &self,
        clob_client: &ClobClient,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        if let Some(ids) = self.get(market_id).await {
            return Ok(Some(ids));
        }

        let market = match clob_client.get_market_by_id(market_id).await {
            Ok(market) => market,
            Err(error) => {
                warn!(
                    market_id = %market_id,
                    error = %error,
                    "Failed to fetch CLOB market for exit token cache hydration"
                );
                return Ok(None);
            }
        };

        if market.outcomes.len() != 2 {
            return Ok(None);
        }

        let (yes_id, no_id) = if market.outcomes[0].name.to_lowercase().contains("yes") {
            (
                market.outcomes[0].token_id.clone(),
                market.outcomes[1].token_id.clone(),
            )
        } else {
            (
                market.outcomes[1].token_id.clone(),
                market.outcomes[0].token_id.clone(),
            )
        };

        let mut tokens = self.tokens.write().await;
        tokens.insert(market.id.clone(), (yes_id.clone(), no_id.clone()));
        if market.id != market_id {
            tokens.insert(market_id.to_string(), (yes_id.clone(), no_id.clone()));
        }
        Ok(Some((yes_id, no_id)))
    }

    async fn alias(&self, alias: &str, ids: &(String, String)) {
        self.tokens
            .write()
            .await
            .insert(alias.to_string(), ids.clone());
    }
}

/// Exit handler service — closes positions via sell orders or resolution detection.
pub struct ExitHandler {
    config: Arc<RwLock<ExitHandlerConfig>>,
    position_repo: PositionRepository,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    token_cache: OutcomeTokenCache,
    /// Shared dedup set with ArbAutoExecutor.
    arb_dedup: Arc<RwLock<HashSet<String>>>,
    trade_event_recorder: TradeEventRecorder,
    /// Centralized service for position state mutations.
    position_service: crate::position_service::PositionService,
    /// Heartbeat timestamp (epoch secs) — updated every tick to prove liveness.
    heartbeat: Arc<AtomicI64>,
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
    pub fn new(
        config: Arc<RwLock<ExitHandlerConfig>>,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
        pool: PgPool,
        arb_dedup: Arc<RwLock<HashSet<String>>>,
        heartbeat: Arc<AtomicI64>,
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
            token_cache: OutcomeTokenCache::new(),
            arb_dedup,
            trade_event_recorder: TradeEventRecorder::new(pool.clone(), trade_event_tx),
            position_service,
            heartbeat,
        }
    }

    /// Snapshot the current config for use during a single tick.
    async fn snapshot_config(&self) -> ExitHandlerConfig {
        self.config.read().await.clone()
    }

    fn touch_heartbeat(&self) {
        self.heartbeat
            .store(Utc::now().timestamp(), Ordering::Relaxed);
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

        // Initial token cache load
        match self.token_cache.refresh().await {
            Ok(count) => info!(markets = count, "Exit handler: token cache loaded"),
            Err(e) => warn!(error = %e, "Exit handler: failed to load token cache, will retry"),
        }

        let mut exit_ticker = tokio::time::interval(tokio::time::Duration::from_secs(
            startup_cfg.exit_poll_interval_secs,
        ));
        let mut resolution_ticker = tokio::time::interval(tokio::time::Duration::from_secs(
            startup_cfg.resolution_check_secs,
        ));

        // Skip the first immediate ticks
        exit_ticker.tick().await;
        resolution_ticker.tick().await;

        loop {
            // Update heartbeat to prove liveness
            self.touch_heartbeat();

            tokio::select! {
                _ = exit_ticker.tick() => {
                    // Per-tick guard: skip when disabled at runtime
                    let cfg = self.snapshot_config().await;
                    if !cfg.enabled {
                        continue;
                    }
                    if let Err(e) = self.evaluate_open_positions(&cfg).await {
                        error!(error = %e, "Failed to evaluate open positions for exit");
                    }
                    if let Err(e) = self.process_failed_exits(&cfg).await {
                        error!(error = %e, "Failed to process failed exits");
                    }
                    if let Err(e) = self.process_exit_ready().await {
                        error!(error = %e, "Failed to process exit-ready positions");
                    }
                    if let Err(e) = self.process_one_legged_recovery().await {
                        error!(error = %e, "Failed to process one-legged recovery");
                    }
                    if let Err(e) = self.process_orphan_inventory_recovery(&cfg).await {
                        error!(error = %e, "Failed to process orphan inventory recovery");
                    }
                }
                _ = resolution_ticker.tick() => {
                    // Per-tick guard: skip when disabled at runtime
                    if !self.snapshot_config().await.enabled {
                        continue;
                    }
                    if let Err(e) = self.check_market_resolutions().await {
                        error!(error = %e, "Failed to check market resolutions");
                    }
                }
            }
        }
    }

    /// Evaluate open ExitOnCorrection positions and promote exit-eligible ones.
    async fn evaluate_open_positions(&self, cfg: &ExitHandlerConfig) -> anyhow::Result<()> {
        let mut candidates = self.position_repo.get_open_exit_candidates().await?;
        if candidates.is_empty() {
            return Ok(());
        }

        let quant_contexts = self.load_quant_exit_contexts(&candidates).await?;

        debug!(count = candidates.len(), "Evaluating open exit candidates");

        let fee = Decimal::new(2, 2);
        for candidate in &mut candidates {
            self.touch_heartbeat();
            let quant_ctx = quant_contexts.get(&candidate.position.id);
            let (yes_bid, no_bid) = match self.current_exit_bids(&candidate.position).await? {
                ExitBidStatus::Available(yes_bid, no_bid) => (yes_bid, no_bid),
                ExitBidStatus::Resolved => {
                    let _ = self
                        .close_open_position_via_resolution(
                            &mut candidate.position,
                            candidate.source,
                            quant_ctx,
                        )
                        .await?;
                    continue;
                }
                ExitBidStatus::Unavailable => continue,
            };

            candidate.position.update_pnl(yes_bid, no_bid, fee);
            self.position_repo.update(&candidate.position).await?;

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
                    continue;
                }
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

    /// Execute the sell orders for a single ExitReady position.
    async fn execute_exit(&self, position: &mut Position) -> anyhow::Result<()> {
        let market_id = position.market_id.clone();
        self.touch_heartbeat();
        let execution_mode = if self.order_executor.is_live_ready().await {
            "live"
        } else {
            "paper"
        };
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
        let ctx = Self::event_context(execution_mode, source, quant_ctx.as_ref());
        if let Err(e) = self.position_service.mark_closing(position, &ctx).await {
            warn!(error = %e, "mark_closing failed");
            return Ok(());
        }

        // Resolve token IDs
        let (yes_token_id, no_token_id) = match self.resolve_market_tokens(&market_id).await? {
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

        let (has_yes, has_no) = held_outcomes(position);
        if has_yes {
            let yes_report = match self
                .execute_sell_order_with_refresh(
                    &market_id,
                    "YES",
                    &yes_token_id,
                    OrderSide::Sell,
                    position.quantity,
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

            if !yes_report.is_success() {
                let msg = yes_report
                    .error_message
                    .unwrap_or_else(|| "YES sell not filled".to_string());
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

        if has_no {
            let no_report = match self
                .execute_sell_order_with_refresh(
                    &market_id,
                    "NO",
                    &no_token_id,
                    OrderSide::Sell,
                    position.quantity,
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

            if !no_report.is_success() {
                let msg = no_report
                    .error_message
                    .unwrap_or_else(|| "NO sell not filled".to_string());
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

        // Record with circuit breaker
        let realized_pnl = position.realized_pnl.unwrap_or_default();
        let is_win = realized_pnl > Decimal::ZERO;
        if let Err(e) = self
            .circuit_breaker
            .record_trade(realized_pnl, is_win)
            .await
        {
            warn!(error = %e, "Failed to record exit trade with circuit breaker");
        }

        // Remove from dedup
        self.arb_dedup.write().await.remove(&market_id);

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

    async fn current_exit_bids(&self, position: &Position) -> anyhow::Result<ExitBidStatus> {
        let market_id = position.market_id.as_str();
        let Some((yes_token_id, no_token_id)) = self.resolve_market_tokens(market_id).await? else {
            warn!(market_id = %market_id, "No token IDs for exit price evaluation");
            return Ok(ExitBidStatus::Unavailable);
        };

        let (has_yes, has_no) = held_outcomes(position);
        let yes_bid = if has_yes {
            match self
                .load_exit_best_bid(market_id, &yes_token_id, "YES")
                .await?
            {
                Some(bid) => bid,
                None => return self.exit_bid_fallback_status(market_id).await,
            }
        } else {
            Decimal::ZERO
        };

        let no_bid = if has_no {
            match self
                .load_exit_best_bid(market_id, &no_token_id, "NO")
                .await?
            {
                Some(bid) => bid,
                None => return self.exit_bid_fallback_status(market_id).await,
            }
        } else {
            Decimal::ZERO
        };

        Ok(ExitBidStatus::Available(yes_bid, no_bid))
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
            Ok(book) => Ok(Some(book.best_bid().unwrap_or(Decimal::ZERO))),
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

    async fn finalize_resolution_close(
        &self,
        position: &mut Position,
        source: i16,
        quant_ctx: Option<&QuantExitContext>,
        resolved_yes_winner: Option<bool>,
    ) -> anyhow::Result<bool> {
        let market_id = position.market_id.clone();
        let fee = Decimal::new(2, 2);

        let method = if let Some(yes_wins) = resolved_yes_winner {
            CloseMethod::ResolutionWithWinner { yes_wins, fee }
        } else if position.has_full_pair_exposure() {
            CloseMethod::ResolutionConservative { fee }
        } else {
            warn!(
                position_id = %position.id,
                market_id = %market_id,
                "Resolved market is missing winner metadata; leaving non-paired exposure open to avoid mispricing"
            );
            return Ok(false);
        };

        let execution_mode = self.current_execution_mode().await;
        let ctx = Self::event_context(&execution_mode, source, quant_ctx);
        if let Err(e) = self
            .position_service
            .close_position(position, method, &ctx)
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

        let realized_pnl = position.realized_pnl.unwrap_or_default();
        let is_win = realized_pnl > Decimal::ZERO;
        if let Err(error) = self
            .circuit_breaker
            .record_trade(realized_pnl, is_win)
            .await
        {
            warn!(
                error = %error,
                "Failed to record resolution fallback trade with circuit breaker"
            );
        }

        self.arb_dedup.write().await.remove(&market_id);

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

        self.finalize_resolution_close(position, source, quant_ctx, resolved_yes_winner(&market))
            .await
    }

    async fn resolve_market_tokens(
        &self,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        if let Some(ids) = self.token_cache.get(market_id).await {
            return Ok(Some(ids));
        }

        self.touch_heartbeat();
        if let Err(error) = self.token_cache.refresh().await {
            warn!(
                market_id = %market_id,
                error = %error,
                "Failed to refresh exit token cache before retry"
            );
        }

        if let Some(ids) = self.token_cache.get(market_id).await {
            return Ok(Some(ids));
        }

        if let Some(ids) = self
            .token_cache
            .hydrate_clob_market(self.clob_client.as_ref(), market_id)
            .await?
        {
            return Ok(Some(ids));
        }

        if let Some(condition_id) = self.lookup_condition_id_for_token(market_id).await? {
            if let Some(ids) = self.token_cache.get(&condition_id).await {
                self.token_cache.alias(market_id, &ids).await;
                return Ok(Some(ids));
            }

            if let Some(ids) = self
                .token_cache
                .hydrate_clob_market(self.clob_client.as_ref(), &condition_id)
                .await?
            {
                self.token_cache.alias(market_id, &ids).await;
                return Ok(Some(ids));
            }

            if let Some(ids) = self.token_cache.hydrate_market(&condition_id).await? {
                self.token_cache.alias(market_id, &ids).await;
                return Ok(Some(ids));
            }
        }

        self.token_cache.hydrate_market(market_id).await
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
            (position.unrealized_pnl / entry_cost) * Decimal::new(100, 0)
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

    /// Check for HoldToResolution positions whose markets have resolved.
    async fn check_market_resolutions(&self) -> anyhow::Result<()> {
        let positions = self.position_repo.get_hold_to_resolution().await?;
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
                    resolved_markets.insert(market_id.clone(), resolved_yes_winner(&market));
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

            let closed = self
                .finalize_resolution_close(&mut position, SOURCE_ARBITRAGE, None, yes_winner)
                .await?;
            if !closed {
                continue;
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
    ) -> anyhow::Result<polymarket_core::types::ExecutionReport> {
        let order = MarketOrder::new(market_id.to_string(), token_id.to_string(), side, quantity);

        match self.order_executor.execute_market_order(order).await {
            Ok(report) => Ok(report),
            Err(error) if is_not_found_error(&error) => {
                warn!(
                    market_id = %market_id,
                    token_id = %token_id,
                    error = %error,
                    "Sell leg hit 404; refreshing token cache and retrying once"
                );

                self.touch_heartbeat();
                if let Err(refresh_error) = self.token_cache.refresh().await {
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
                self.order_executor.execute_market_order(retry_order).await
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
        self.finalize_resolution_close(position, source, quant_ctx, resolved_yes_winner(&market))
            .await
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
            if position.yes_entry_price <= Decimal::ZERO {
                warn!(
                    market_id = %market_id,
                    position_id = %position.id,
                    "Cannot flatten one-legged position without a recorded YES entry price"
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
                    "yes",
                    "one-legged recovery: flattening held YES leg",
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

            self.arb_dedup.write().await.insert(market_id.clone());

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
            if age_secs < cfg.failed_exit_retry_backoff_secs {
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

/// Spawn the exit handler as a background task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_exit_handler(
    config: Arc<RwLock<ExitHandlerConfig>>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
    pool: PgPool,
    arb_dedup: Arc<RwLock<HashSet<String>>>,
    heartbeat: Arc<AtomicI64>,
) {
    let handler = ExitHandler::new(
        config,
        order_executor,
        circuit_breaker,
        clob_client,
        signal_tx,
        trade_event_tx,
        pool,
        arb_dedup,
        heartbeat,
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
        let yes_only = Position::new(
            "m1".to_string(),
            Decimal::new(55, 2),
            Decimal::ZERO,
            Decimal::ONE,
            polymarket_core::types::ExitStrategy::ExitOnCorrection,
        );
        assert_eq!(held_outcomes(&yes_only), (true, false));

        let no_only = Position::new(
            "m1".to_string(),
            Decimal::ZERO,
            Decimal::new(45, 2),
            Decimal::ONE,
            polymarket_core::types::ExitStrategy::ExitOnCorrection,
        );
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
        both_legs.mark_open().unwrap();
        both_legs.mark_exit_ready().unwrap();
        both_legs.mark_closing().unwrap();
        both_legs.record_yes_exit_fill(Decimal::new(54, 2)).unwrap();

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
        };

        assert_eq!(resolved_yes_winner(&market), Some(true));
    }
}

fn held_outcomes(position: &Position) -> (bool, bool) {
    position.held_outcomes()
}

fn resolved_yes_winner(market: &Market) -> Option<bool> {
    market.outcomes.iter().find_map(|outcome| {
        let winner = outcome.winner?;
        if outcome.name.eq_ignore_ascii_case("yes") {
            Some(winner)
        } else if outcome.name.eq_ignore_ascii_case("no") {
            Some(!winner)
        } else {
            None
        }
    })
}
