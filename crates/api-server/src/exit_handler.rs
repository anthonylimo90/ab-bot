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
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::signal::{QuantSignalKind, SignalDirection};
use polymarket_core::types::{FailureReason, MarketOrder, OrderSide, Position};
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;

use crate::trade_events::{NewTradeEvent, TradeEventRecorder};
use crate::websocket::{SignalType, SignalUpdate};

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
    /// Heartbeat timestamp (epoch secs) — updated every tick to prove liveness.
    heartbeat: Arc<AtomicI64>,
}

#[derive(Debug, Clone)]
struct QuantExitContext {
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
            heartbeat,
        }
    }

    /// Snapshot the current config for use during a single tick.
    async fn snapshot_config(&self) -> ExitHandlerConfig {
        self.config.read().await.clone()
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
            "Starting exit handler (always-on, per-tick guard)"
        );

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
            self.heartbeat
                .store(Utc::now().timestamp(), Ordering::Relaxed);

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
                    if let Err(e) = self.process_exit_ready().await {
                        error!(error = %e, "Failed to process exit-ready positions");
                    }
                    if let Err(e) = self.process_failed_exits().await {
                        error!(error = %e, "Failed to process failed exits");
                    }
                    if let Err(e) = self.process_one_legged_recovery().await {
                        error!(error = %e, "Failed to process one-legged recovery");
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
            let Some((yes_bid, no_bid)) = self.current_exit_bids(&candidate.position).await? else {
                continue;
            };

            candidate.position.update_pnl(yes_bid, no_bid, fee);
            self.position_repo.update(&candidate.position).await?;

            let quant_ctx = quant_contexts.get(&candidate.position.id);
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
                if let Err(e) = candidate.position.mark_exit_ready() {
                    warn!(
                        position_id = %candidate.position.id,
                        error = %e,
                        "Cannot mark exit candidate ready"
                    );
                    continue;
                }
                self.position_repo.update(&candidate.position).await?;
                self.record_exit_ready_event(&candidate.position, candidate.source, quant_ctx)
                    .await;
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
        let execution_mode = if self.order_executor.is_live_ready().await {
            "live"
        } else {
            "paper"
        };
        let quant_ctx = if position.yes_entry_price.is_zero() || position.no_entry_price.is_zero() {
            self.load_quant_exit_context(position.id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        // Mark Closing
        if let Err(e) = position.mark_closing() {
            anyhow::bail!("Cannot mark position closing: {}", e);
        }
        let _ = self.position_repo.update(position).await;
        self.record_exit_requested_event(position, execution_mode, quant_ctx.as_ref())
            .await;

        // Resolve token IDs
        let (yes_token_id, no_token_id) = match self.resolve_market_tokens(&market_id).await? {
            Some(ids) => ids,
            None => {
                position.mark_exit_failed(FailureReason::ConnectivityError {
                    message: "No token IDs for market".to_string(),
                });
                let _ = self.position_repo.update(position).await;
                self.publish_alert(&market_id, "exit_failed", "No token IDs for market");
                return Ok(());
            }
        };

        let (has_yes, has_no) = held_outcomes(position);
        let yes_price = if has_yes {
            let yes_order = MarketOrder::new(
                market_id.clone(),
                yes_token_id,
                OrderSide::Sell,
                position.quantity,
            );

            let yes_report = match self.order_executor.execute_market_order(yes_order).await {
                Ok(report) => report,
                Err(e) => {
                    error!(error = %e, market_id = %market_id, "YES sell order error");
                    position.mark_exit_failed(FailureReason::ConnectivityError {
                        message: format!("YES sell error: {e}"),
                    });
                    let _ = self.position_repo.update(position).await;
                    self.publish_alert(&market_id, "exit_failed", "YES sell order error");
                    return Ok(());
                }
            };

            if !yes_report.is_success() {
                let msg = yes_report
                    .error_message
                    .unwrap_or_else(|| "YES sell not filled".to_string());
                position.mark_exit_failed(FailureReason::OrderRejected {
                    message: format!("YES sell failed: {msg}"),
                });
                let _ = self.position_repo.update(position).await;
                self.publish_alert(&market_id, "exit_failed", "YES sell order rejected");
                return Ok(());
            }

            yes_report.average_price
        } else {
            Decimal::ZERO
        };

        let no_price = if has_no {
            let no_order = MarketOrder::new(
                market_id.clone(),
                no_token_id,
                OrderSide::Sell,
                position.quantity,
            );

            let no_report = match self.order_executor.execute_market_order(no_order).await {
                Ok(report) => report,
                Err(e) => {
                    error!(error = %e, market_id = %market_id, "NO sell order error");
                    position.mark_exit_failed(FailureReason::ConnectivityError {
                        message: format!("NO sell error: {e}"),
                    });
                    let _ = self.position_repo.update(position).await;
                    self.publish_alert(&market_id, "exit_failed", "NO sell order error");
                    return Ok(());
                }
            };

            if !no_report.is_success() {
                let msg = no_report
                    .error_message
                    .unwrap_or_else(|| "NO sell not filled".to_string());
                position.mark_exit_failed(FailureReason::OrderRejected {
                    message: format!("NO sell failed: {msg}"),
                });
                let _ = self.position_repo.update(position).await;
                self.publish_alert(&market_id, "exit_failed", "NO sell order rejected");
                return Ok(());
            }

            no_report.average_price
        } else {
            Decimal::ZERO
        };

        let fee = Decimal::new(2, 2); // 2%

        // Close position
        if let Err(e) = position.close_via_exit(yes_price, no_price, fee) {
            warn!(position_id = %position.id, error = %e, "Cannot close position via exit");
            return Ok(());
        }
        let _ = self.position_repo.update(position).await;

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

        self.record_position_closed_event(
            position,
            execution_mode,
            quant_ctx.as_ref(),
            "closed_via_exit",
        )
        .await;

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

    async fn current_exit_bids(
        &self,
        position: &Position,
    ) -> anyhow::Result<Option<(Decimal, Decimal)>> {
        let market_id = position.market_id.as_str();
        let Some((yes_token_id, no_token_id)) = self.resolve_market_tokens(market_id).await? else {
            warn!(market_id = %market_id, "No token IDs for exit price evaluation");
            return Ok(None);
        };

        let (has_yes, has_no) = held_outcomes(position);
        let yes_bid = if has_yes {
            match self
                .order_executor
                .clob_client()
                .get_order_book(&yes_token_id)
                .await
            {
                Ok(book) => book.best_bid().unwrap_or(Decimal::ZERO),
                Err(e) => {
                    warn!(market_id = %market_id, error = %e, "Failed loading YES orderbook for exit evaluation");
                    return Ok(None);
                }
            }
        } else {
            Decimal::ZERO
        };

        let no_bid = if has_no {
            match self
                .order_executor
                .clob_client()
                .get_order_book(&no_token_id)
                .await
            {
                Ok(book) => book.best_bid().unwrap_or(Decimal::ZERO),
                Err(e) => {
                    warn!(market_id = %market_id, error = %e, "Failed loading NO orderbook for exit evaluation");
                    return Ok(None);
                }
            }
        } else {
            Decimal::ZERO
        };

        Ok(Some((yes_bid, no_bid)))
    }

    async fn resolve_market_tokens(
        &self,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        if let Some(ids) = self.token_cache.get(market_id).await {
            return Ok(Some(ids));
        }

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
            .filter(|candidate| candidate.source == 3)
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

    async fn record_exit_ready_event(
        &self,
        position: &Position,
        source: i16,
        quant_ctx: Option<&QuantExitContext>,
    ) {
        let execution_mode = self.current_execution_mode().await;
        self.trade_event_recorder
            .record_warn(
                trade_event_for_position(
                    position,
                    source,
                    quant_ctx,
                    &execution_mode,
                    "exit_marked_ready",
                    Some("open"),
                    Some("exit_ready"),
                )
                .with_unrealized_pnl(position.unrealized_pnl),
            )
            .await;
    }

    async fn record_exit_requested_event(
        &self,
        position: &Position,
        execution_mode: &str,
        quant_ctx: Option<&QuantExitContext>,
    ) {
        self.trade_event_recorder
            .record_warn(
                trade_event_for_position(
                    position,
                    if quant_ctx.is_some() { 3 } else { 1 },
                    quant_ctx,
                    execution_mode,
                    "exit_requested",
                    Some("exit_ready"),
                    Some("closing"),
                )
                .with_unrealized_pnl(position.unrealized_pnl),
            )
            .await;
    }

    async fn record_position_closed_event(
        &self,
        position: &Position,
        execution_mode: &str,
        quant_ctx: Option<&QuantExitContext>,
        event_type: &str,
    ) {
        self.trade_event_recorder
            .record_warn(
                trade_event_for_position(
                    position,
                    if quant_ctx.is_some() { 3 } else { 1 },
                    quant_ctx,
                    execution_mode,
                    event_type,
                    Some("closing"),
                    Some("closed"),
                )
                .with_realized_pnl(position.realized_pnl.unwrap_or_default())
                .with_unrealized_pnl(position.unrealized_pnl),
            )
            .await;
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
        if source != 3 {
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
        let mut resolved_market_ids = HashSet::new();
        for market_id in &market_ids {
            match self.clob_client.get_market_by_id(market_id).await {
                Ok(market) if market.resolved => {
                    resolved_market_ids.insert(market_id.clone());
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(market_id = %market_id, error = %e, "Failed to fetch market for resolution check");
                }
            }
        }

        if resolved_market_ids.is_empty() {
            debug!("No resolved markets found for held positions");
            return Ok(());
        }

        info!(
            resolved = resolved_market_ids.len(),
            "Found resolved markets with open positions"
        );

        let fee = Decimal::new(2, 2); // 2%

        for mut position in positions {
            if !resolved_market_ids.contains(&position.market_id) {
                continue;
            }

            let market_id = position.market_id.clone();

            if let Err(e) = position.close_via_resolution(fee) {
                warn!(position_id = %position.id, market_id = %market_id, error = %e, "Cannot close position via resolution");
                continue;
            }
            let _ = self.position_repo.update(&position).await;

            // Record with circuit breaker
            let realized_pnl = position.realized_pnl.unwrap_or_default();
            let is_win = realized_pnl > Decimal::ZERO;
            if let Err(e) = self
                .circuit_breaker
                .record_trade(realized_pnl, is_win)
                .await
            {
                warn!(error = %e, "Failed to record resolution trade with circuit breaker");
            }

            // Remove from dedup
            self.arb_dedup.write().await.remove(&market_id);

            let execution_mode = self.current_execution_mode().await;
            self.record_position_closed_event(
                &position,
                &execution_mode,
                None,
                "closed_via_resolution",
            )
            .await;

            // Publish signal
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
                }),
            };
            let _ = self.signal_tx.send(signal);

            info!(
                market_id = %market_id,
                position_id = %position.id,
                realized_pnl = %realized_pnl,
                "Position closed via resolution"
            );
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

    /// Process one-legged entry failures: attempt to buy the missing NO leg.
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
            let market_id = position.market_id.clone();

            // Resolve NO token ID
            let (_yes_token_id, no_token_id) = match self.resolve_market_tokens(&market_id).await? {
                Some(ids) => ids,
                None => {
                    warn!(
                        market_id = %market_id,
                        position_id = %position.id,
                        "Cannot recover one-legged: no token IDs for market"
                    );
                    continue;
                }
            };

            // Attempt to buy the NO leg
            let no_order = MarketOrder::new(
                market_id.clone(),
                no_token_id,
                OrderSide::Buy,
                position.quantity,
            );

            let no_report = match self.order_executor.execute_market_order(no_order).await {
                Ok(report) => report,
                Err(e) => {
                    warn!(
                        market_id = %market_id,
                        position_id = %position.id,
                        error = %e,
                        "One-legged recovery: NO order execution error"
                    );
                    // Increment retry count to avoid infinite loops
                    position.retry_count += 1;
                    position.last_updated = Utc::now();
                    let _ = self.position_repo.update(&position).await;
                    continue;
                }
            };

            if !no_report.is_success() {
                let msg = no_report
                    .error_message
                    .unwrap_or_else(|| "NO order not filled".to_string());
                warn!(
                    market_id = %market_id,
                    position_id = %position.id,
                    reason = %msg,
                    "One-legged recovery: NO order failed"
                );
                position.retry_count += 1;
                position.last_updated = Utc::now();
                let _ = self.position_repo.update(&position).await;
                continue;
            }

            // NO leg filled — transition to Open
            match position.recover_one_legged_to_open() {
                Ok(()) => {
                    let _ = self.position_repo.update(&position).await;
                    // Add to dedup set since position is now active
                    self.arb_dedup.write().await.insert(market_id.clone());

                    self.publish_alert(
                        &market_id,
                        "one_legged_recovered",
                        "NO leg placed, position now Open",
                    );
                    info!(
                        market_id = %market_id,
                        position_id = %position.id,
                        "One-legged position recovered to Open"
                    );
                }
                Err(e) => {
                    warn!(
                        market_id = %market_id,
                        position_id = %position.id,
                        error = %e,
                        "One-legged recovery: state transition failed"
                    );
                }
            }
        }

        Ok(())
    }

    /// Process ExitFailed positions eligible for retry.
    async fn process_failed_exits(&self) -> anyhow::Result<()> {
        let positions = self.position_repo.get_failed_exits().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Processing failed exits for retry");

        for mut position in positions {
            if position.attempt_exit_recovery() {
                if let Err(e) = self.position_repo.update(&position).await {
                    warn!(
                        position_id = %position.id,
                        error = %e,
                        "Failed to persist exit recovery"
                    );
                } else {
                    debug!(
                        position_id = %position.id,
                        retry_count = position.retry_count,
                        "Position moved back to ExitReady for retry"
                    );
                }
            }
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

fn trade_event_for_position(
    position: &Position,
    source: i16,
    quant_ctx: Option<&QuantExitContext>,
    execution_mode: &str,
    event_type: &str,
    state_from: Option<&str>,
    state_to: Option<&str>,
) -> NewTradeEvent {
    let (strategy, source_label, signal_id, direction) = if let Some(ctx) = quant_ctx {
        (
            ctx.kind.as_str().to_string(),
            "quant".to_string(),
            Some(ctx.signal_id),
            Some(ctx.direction.as_str().to_string()),
        )
    } else if source == 3 {
        ("quant".to_string(), "quant".to_string(), None, None)
    } else {
        ("arb".to_string(), "arb".to_string(), None, None)
    };

    let mut event = NewTradeEvent::new(
        strategy,
        execution_mode,
        source_label,
        position.market_id.clone(),
        event_type,
    );
    event.position_id = Some(position.id);
    event.signal_id = signal_id;
    event.direction = direction;
    event.state_from = state_from.map(str::to_string);
    event.state_to = state_to.map(str::to_string);
    event.metadata = serde_json::json!({});
    event
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

trait ExitTradeEventExt {
    fn with_realized_pnl(self, pnl: Decimal) -> Self;
    fn with_unrealized_pnl(self, pnl: Decimal) -> Self;
}

impl ExitTradeEventExt for NewTradeEvent {
    fn with_realized_pnl(mut self, pnl: Decimal) -> Self {
        self.realized_pnl = Some(pnl);
        self
    }

    fn with_unrealized_pnl(mut self, pnl: Decimal) -> Self {
        self.unrealized_pnl = Some(pnl);
        self
    }
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
    }

    #[test]
    fn test_config_from_env() {
        // With no env vars, should use defaults
        let config = ExitHandlerConfig::from_env();
        assert!(!config.enabled);
        assert_eq!(config.exit_poll_interval_secs, 30);
        assert_eq!(config.resolution_check_secs, 300);
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
}

fn held_outcomes(position: &Position) -> (bool, bool) {
    let has_yes = position.yes_entry_price > Decimal::ZERO;
    let has_no = position.no_entry_price > Decimal::ZERO;
    (has_yes, has_no)
}
