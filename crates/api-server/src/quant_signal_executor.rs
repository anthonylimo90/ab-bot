//! Quant signal executor.
//!
//! Receives `QuantSignal` from the broadcast channel, evaluates each against
//! risk limits, dedup, and confidence thresholds, then executes single-leg
//! market orders. Structural copy of `arb_executor.rs` adapted for quant signals.

use chrono::Utc;
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::signal::{QuantSignal, QuantSignalKind, SignalDirection};
use polymarket_core::types::{ExitStrategy, MarketOrder, OrderSide, Position};
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;

use crate::arb_executor::OutcomeTokenCache;
use crate::trade_events::{NewTradeEvent, TradeEventRecorder};
use crate::websocket::{SignalType, SignalUpdate};

/// Configuration for the quant signal executor.
#[derive(Debug, Clone)]
pub struct QuantSignalExecutorConfig {
    /// Whether execution is enabled (false = paper mode, signals still logged).
    pub enabled: bool,
    /// Base position size in USD (before confidence weighting).
    pub base_position_size_usd: Decimal,
    /// Minimum confidence to execute (0.0–1.0).
    pub min_confidence: f64,
    /// Maximum signal age in seconds before discarding.
    pub max_signal_age_secs: i64,
    /// Maximum simultaneous quant positions.
    pub max_quant_positions: usize,
    /// Strategy allocation weights (should sum to ~1.0).
    pub flow_allocation_pct: f64,
    pub cross_market_allocation_pct: f64,
    pub mean_reversion_allocation_pct: f64,
    pub resolution_allocation_pct: f64,
    /// Minimum orderbook depth in USD on the target side.
    pub min_book_depth: Decimal,
    /// Per-strategy maximum daily loss before halting that strategy.
    pub strategy_max_daily_loss_usd: Decimal,
    /// Per-strategy maximum consecutive losses before halting.
    pub strategy_max_consecutive_losses: u32,
}

impl QuantSignalExecutorConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("QUANT_EXECUTOR_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(false),
            base_position_size_usd: std::env::var("QUANT_BASE_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(30, 0)),
            min_confidence: std::env::var("QUANT_MIN_CONFIDENCE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.65),
            max_signal_age_secs: std::env::var("QUANT_MAX_SIGNAL_AGE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(120),
            max_quant_positions: std::env::var("QUANT_MAX_POSITIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(20),
            flow_allocation_pct: std::env::var("QUANT_FLOW_ALLOCATION_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.40),
            cross_market_allocation_pct: std::env::var("QUANT_CROSS_MARKET_ALLOCATION_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.30),
            mean_reversion_allocation_pct: std::env::var("QUANT_MEAN_REVERSION_ALLOCATION_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.20),
            resolution_allocation_pct: std::env::var("QUANT_RESOLUTION_ALLOCATION_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.10),
            min_book_depth: std::env::var("QUANT_MIN_BOOK_DEPTH")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(50, 0)),
            strategy_max_daily_loss_usd: std::env::var("QUANT_STRATEGY_MAX_DAILY_LOSS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(200, 0)),
            strategy_max_consecutive_losses: std::env::var("QUANT_STRATEGY_MAX_CONSECUTIVE_LOSSES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
        }
    }

    /// Get the allocation weight for a signal kind.
    fn allocation_for(&self, kind: QuantSignalKind) -> f64 {
        match kind {
            QuantSignalKind::Flow => self.flow_allocation_pct,
            QuantSignalKind::CrossMarket => self.cross_market_allocation_pct,
            QuantSignalKind::MeanReversion => self.mean_reversion_allocation_pct,
            QuantSignalKind::ResolutionProximity => self.resolution_allocation_pct,
        }
    }
}

/// Per-strategy risk state tracking.
///
/// Each `QuantSignalKind` gets its own `StrategyState` that tracks daily P&L
/// and consecutive losses independently. When a strategy breaches its limits,
/// it is halted without tripping the global circuit breaker.
#[derive(Debug, Clone)]
struct StrategyState {
    /// Accumulated daily P&L for this strategy (resets at midnight UTC).
    daily_pnl: Decimal,
    /// The UTC date this daily_pnl applies to.
    daily_pnl_date: chrono::NaiveDate,
    /// Consecutive losses (reset on win).
    consecutive_losses: u32,
    /// Whether this strategy is currently halted.
    halted: bool,
    /// Reason for halting.
    halt_reason: Option<String>,
}

impl StrategyState {
    fn new() -> Self {
        Self {
            daily_pnl: Decimal::ZERO,
            daily_pnl_date: Utc::now().date_naive(),
            consecutive_losses: 0,
            halted: false,
            halt_reason: None,
        }
    }

    /// Record a trade outcome. Returns true if the strategy should be halted.
    fn record_outcome(
        &mut self,
        pnl: Decimal,
        max_daily_loss: Decimal,
        max_consecutive_losses: u32,
    ) -> bool {
        let today = Utc::now().date_naive();

        // Reset daily P&L at midnight UTC
        if today != self.daily_pnl_date {
            self.daily_pnl = Decimal::ZERO;
            self.daily_pnl_date = today;
            // Also un-halt if the halt was daily-loss-based
            if self.halted {
                info!("Strategy daily loss reset, un-halting");
                self.halted = false;
                self.halt_reason = None;
            }
        }

        self.daily_pnl += pnl;

        if pnl > Decimal::ZERO {
            self.consecutive_losses = 0;
        } else {
            self.consecutive_losses += 1;
        }

        // Check halt conditions
        if self.daily_pnl < -max_daily_loss {
            self.halted = true;
            self.halt_reason = Some(format!(
                "daily_loss_exceeded: {} < -{}",
                self.daily_pnl, max_daily_loss
            ));
            return true;
        }

        if self.consecutive_losses >= max_consecutive_losses {
            self.halted = true;
            self.halt_reason = Some(format!(
                "consecutive_losses: {} >= {}",
                self.consecutive_losses, max_consecutive_losses
            ));
            return true;
        }

        false
    }
}

/// The quant signal executor background task.
struct QuantSignalExecutor {
    config: Arc<RwLock<QuantSignalExecutorConfig>>,
    signal_rx: broadcast::Receiver<QuantSignal>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    position_repo: PositionRepository,
    pool: PgPool,
    token_cache: OutcomeTokenCache,
    trade_event_recorder: TradeEventRecorder,
    /// Per-strategy risk state (daily P&L, consecutive losses).
    strategy_states: HashMap<QuantSignalKind, StrategyState>,
    /// Closed quant positions that have already been folded into strategy state.
    processed_strategy_outcomes: HashSet<uuid::Uuid>,
    heartbeat: Arc<AtomicI64>,
}

impl QuantSignalExecutor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: Arc<RwLock<QuantSignalExecutorConfig>>,
        signal_rx: broadcast::Receiver<QuantSignal>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<polymarket_core::api::ClobClient>,
        pool: PgPool,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
        heartbeat: Arc<AtomicI64>,
    ) -> Self {
        let position_repo = PositionRepository::new(pool.clone());
        let token_cache =
            OutcomeTokenCache::new(clob_client, active_clob_markets).with_pool(pool.clone());
        let trade_event_recorder = TradeEventRecorder::new(pool.clone(), trade_event_tx);

        let mut strategy_states = HashMap::new();
        strategy_states.insert(QuantSignalKind::Flow, StrategyState::new());
        strategy_states.insert(QuantSignalKind::CrossMarket, StrategyState::new());
        strategy_states.insert(QuantSignalKind::MeanReversion, StrategyState::new());
        strategy_states.insert(QuantSignalKind::ResolutionProximity, StrategyState::new());

        Self {
            config,
            signal_rx,
            signal_tx,
            order_executor,
            circuit_breaker,
            position_repo,
            pool,
            token_cache,
            trade_event_recorder,
            strategy_states,
            processed_strategy_outcomes: HashSet::new(),
            heartbeat,
        }
    }

    /// Snapshot config without holding lock across awaits.
    async fn snapshot_config(&self) -> QuantSignalExecutorConfig {
        self.config.read().await.clone()
    }

    /// Main executor loop.
    async fn run(mut self) {
        let cfg = self.snapshot_config().await;
        info!(
            enabled = cfg.enabled,
            base_size = %cfg.base_position_size_usd,
            min_confidence = cfg.min_confidence,
            max_positions = cfg.max_quant_positions,
            "Quant signal executor started"
        );

        // Initial cache load
        if let Err(e) = self.token_cache.refresh().await {
            warn!(error = %e, "Initial token cache refresh failed");
        }

        // Rebuild today's per-strategy realized outcomes on startup.
        self.sync_strategy_outcomes().await;

        let mut cache_ticker = tokio::time::interval(std::time::Duration::from_secs(300));
        cache_ticker.tick().await; // skip first tick (just loaded)

        let mut heartbeat_ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        let mut outcome_ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        outcome_ticker.tick().await;

        loop {
            self.heartbeat
                .store(Utc::now().timestamp(), Ordering::Relaxed);

            tokio::select! {
                result = self.signal_rx.recv() => {
                    match result {
                        Ok(signal) => {
                            if let Err(e) = self.process_signal(signal).await {
                                error!(error = %e, "Quant signal processing failed");
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "Quant executor lagged, skipped signals");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Quant signal channel closed, shutting down executor");
                            break;
                        }
                    }
                }
                _ = cache_ticker.tick() => {
                    if let Err(e) = self.token_cache.refresh().await {
                        warn!(error = %e, "Token cache refresh failed");
                    }
                }
                _ = outcome_ticker.tick() => {
                    self.sync_strategy_outcomes().await;
                }
                _ = heartbeat_ticker.tick() => { /* keeps heartbeat advancing */ }
            }
        }
    }

    /// Fold today's closed quant outcomes back into the in-memory strategy breakers.
    async fn sync_strategy_outcomes(&mut self) {
        #[derive(sqlx::FromRow)]
        struct QuantOutcomeRow {
            position_id: uuid::Uuid,
            kind: String,
            realized_pnl: Decimal,
        }

        let today = Utc::now().date_naive();
        let today_start = chrono::DateTime::<Utc>::from_naive_utc_and_offset(
            today.and_hms_opt(0, 0, 0).expect("valid midnight"),
            Utc,
        );
        let rows: Vec<QuantOutcomeRow> = match sqlx::query_as(
            r#"
            SELECT
                p.id AS position_id,
                qs.kind,
                p.realized_pnl
            FROM positions p
            JOIN quant_signals qs ON qs.position_id = p.id
            WHERE p.source = 3
              AND p.state = 4
              AND p.realized_pnl IS NOT NULL
              AND p.exit_timestamp >= $1
            ORDER BY p.exit_timestamp ASC
            "#,
        )
        .bind(today_start)
        .fetch_all(&self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                warn!(error = %e, "Failed syncing quant strategy outcomes");
                return;
            }
        };

        let cfg = self.snapshot_config().await;
        for row in rows {
            if !self.processed_strategy_outcomes.insert(row.position_id) {
                continue;
            }

            if let Some(kind) = parse_quant_signal_kind(&row.kind) {
                self.record_strategy_outcome(
                    kind,
                    row.realized_pnl,
                    row.realized_pnl > Decimal::ZERO,
                    &cfg,
                );
            } else {
                warn!(kind = %row.kind, position_id = %row.position_id, "Unknown quant signal kind while syncing outcomes");
            }
        }
    }

    /// Process a single quant signal through the 15-step pipeline.
    async fn process_signal(&mut self, signal: QuantSignal) -> anyhow::Result<()> {
        let cfg = self.snapshot_config().await;
        let execution_mode = self.execution_mode(&cfg).await.to_string();

        // ── Fire-and-forget: persist signal to quant_signals ──
        let pool = self.pool.clone();
        let signal_id = signal.id;
        let kind_str = signal.kind.as_str().to_string();
        let dir_str = signal.direction.as_str().to_string();
        let cid = signal.condition_id.clone();
        let confidence = signal.confidence;
        let size = signal.suggested_size_usd;
        let meta = signal.metadata.clone();
        let gen_at = signal.generated_at;

        tokio::spawn(async move {
            if let Err(error) = sqlx::query(
                r#"
                INSERT INTO quant_signals (id, kind, condition_id, direction, confidence, size_usd, metadata, generated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (id, generated_at) DO NOTHING
                "#,
            )
            .bind(signal_id)
            .bind(&kind_str)
            .bind(&cid)
            .bind(&dir_str)
            .bind(confidence)
            .bind(size)
            .bind(&meta)
            .bind(gen_at)
            .execute(&pool)
            .await
            {
                warn!(
                    error = %error,
                    signal_id = %signal_id,
                    kind = %kind_str,
                    condition_id = %cid,
                    "Failed to persist quant signal"
                );
            }
        });

        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    signal.kind.as_str(),
                    execution_mode.clone(),
                    "quant",
                    signal.condition_id.clone(),
                    "signal_generated",
                )
                .with_signal(&signal)
                .with_metadata(signal.metadata.clone())
                .with_requested_size(signal.suggested_size_usd),
            )
            .await;

        // Step 1: Check enabled
        if !cfg.enabled {
            debug!(
                signal_id = %signal.id,
                kind = signal.kind.as_str(),
                "Quant executor disabled, signal recorded but not executed"
            );
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("executor_disabled"),
            )
            .await;
            return Ok(());
        }

        // Step 2: Freshness check
        let age_secs = Utc::now()
            .signed_duration_since(signal.generated_at)
            .num_seconds();
        if age_secs > cfg.max_signal_age_secs {
            debug!(
                signal_id = %signal.id,
                age_secs = age_secs,
                "Signal too stale, skipping"
            );
            self.update_signal_status(signal.id, "skipped", Some("too_stale"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("too_stale"),
            )
            .await;
            return Ok(());
        }

        // Step 3: Expiry check
        if signal.is_expired() {
            debug!(signal_id = %signal.id, "Signal expired, skipping");
            self.update_signal_status(signal.id, "skipped", Some("expired"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_expired",
                Some("expired"),
            )
            .await;
            return Ok(());
        }

        // Step 4: Confidence threshold
        if !signal.meets_confidence(cfg.min_confidence) {
            debug!(
                signal_id = %signal.id,
                confidence = signal.confidence,
                min = cfg.min_confidence,
                "Below confidence threshold, skipping"
            );
            self.update_signal_status(signal.id, "skipped", Some("below_confidence"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("below_confidence"),
            )
            .await;
            return Ok(());
        }

        // Step 5: Dedup — skip if open position in same condition_id
        match self
            .position_repo
            .active_quant_executor_position_exists_for_market(&signal.condition_id)
            .await
        {
            Ok(true) => {
                debug!(
                    signal_id = %signal.id,
                    condition_id = &signal.condition_id,
                    "Duplicate market, skipping"
                );
                self.update_signal_status(signal.id, "skipped", Some("duplicate"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("duplicate"),
                )
                .await;
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                warn!(
                    signal_id = %signal.id,
                    error = %e,
                    "Failed active quant market dedup check"
                );
                self.update_signal_status(signal.id, "skipped", Some("dedup_check_failed"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("dedup_check_failed"),
                )
                .await;
                return Ok(());
            }
        }

        // Step 6: Circuit breaker
        if !self.circuit_breaker.can_trade().await {
            debug!(signal_id = %signal.id, "Circuit breaker tripped, skipping");
            self.update_signal_status(signal.id, "skipped", Some("circuit_breaker"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("circuit_breaker"),
            )
            .await;
            return Ok(());
        }

        // Step 6b: Per-strategy circuit breaker
        if let Some(state) = self.strategy_states.get(&signal.kind) {
            if state.halted {
                debug!(
                    signal_id = %signal.id,
                    kind = signal.kind.as_str(),
                    reason = state.halt_reason.as_deref().unwrap_or("unknown"),
                    "Strategy halted by per-strategy circuit breaker, skipping"
                );
                self.update_signal_status(signal.id, "skipped", Some("strategy_halted"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("strategy_halted"),
                )
                .await;
                return Ok(());
            }
        }

        // Step 7: Resolve token IDs from OutcomeTokenCache
        let (yes_token_id, no_token_id) = match self.token_cache.get(&signal.condition_id).await {
            Some(tokens) => tokens,
            None => {
                // Try one refresh and retry
                let _ = self.token_cache.refresh().await;
                match self.token_cache.get(&signal.condition_id).await {
                    Some(tokens) => tokens,
                    None => {
                        debug!(
                            signal_id = %signal.id,
                            condition_id = &signal.condition_id,
                            "Market not in token cache, skipping"
                        );
                        self.update_signal_status(signal.id, "skipped", Some("market_cache_empty"))
                            .await;
                        self.record_signal_outcome_event(
                            &signal,
                            &execution_mode,
                            "signal_skipped",
                            Some("market_cache_empty"),
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
        };

        // Step 8: Determine which token to buy based on direction
        let (target_token_id, outcome_name) = match signal.direction {
            SignalDirection::BuyYes => (yes_token_id, "Yes"),
            SignalDirection::BuyNo => (no_token_id, "No"),
        };

        // Step 9: Orderbook depth check on target side
        let book = match self
            .order_executor
            .clob_client()
            .get_order_book(&target_token_id)
            .await
        {
            Ok(book) => book,
            Err(e) => {
                warn!(
                    signal_id = %signal.id,
                    error = %e,
                    "Failed to fetch orderbook"
                );
                self.update_signal_status(signal.id, "skipped", Some("orderbook_error"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("orderbook_error"),
                )
                .await;
                return Ok(());
            }
        };

        let best_ask = match book.best_ask() {
            Some(price) => price,
            None => {
                debug!(signal_id = %signal.id, "No asks in orderbook, skipping");
                self.update_signal_status(signal.id, "skipped", Some("no_liquidity"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("no_liquidity"),
                )
                .await;
                return Ok(());
            }
        };

        let total_depth: Decimal = book.asks.iter().map(|l| l.price * l.size).sum();
        if total_depth < cfg.min_book_depth {
            debug!(
                signal_id = %signal.id,
                depth = %total_depth,
                min = %cfg.min_book_depth,
                "Insufficient orderbook depth, skipping"
            );
            self.update_signal_status(signal.id, "skipped", Some("insufficient_depth"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("insufficient_depth"),
            )
            .await;
            return Ok(());
        }

        // Step 10: Confidence-weighted sizing
        let allocation_weight = cfg.allocation_for(signal.kind);
        let confidence_decimal =
            Decimal::try_from(signal.confidence).unwrap_or(Decimal::new(65, 2));
        let allocation_decimal =
            Decimal::try_from(allocation_weight).unwrap_or(Decimal::new(40, 2));
        let position_size_usd =
            cfg.base_position_size_usd * confidence_decimal * allocation_decimal;

        if position_size_usd < Decimal::new(1, 0) {
            debug!(
                signal_id = %signal.id,
                size = %position_size_usd,
                "Position size too small after weighting, skipping"
            );
            self.update_signal_status(signal.id, "skipped", Some("size_too_small"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("size_too_small"),
            )
            .await;
            return Ok(());
        }

        // Quantity = size_usd / best_ask_price
        let quantity = if best_ask > Decimal::ZERO {
            position_size_usd / best_ask
        } else {
            self.update_signal_status(signal.id, "skipped", Some("zero_price"))
                .await;
            self.record_signal_outcome_event(
                &signal,
                &execution_mode,
                "signal_skipped",
                Some("zero_price"),
            )
            .await;
            return Ok(());
        };

        // Step 11: Portfolio check — max open quant positions
        match self
            .position_repo
            .count_active_quant_executor_positions()
            .await
        {
            Ok(open_count) if open_count as usize >= cfg.max_quant_positions => {
                debug!(
                    signal_id = %signal.id,
                    open = open_count,
                    max = cfg.max_quant_positions,
                    "Max quant positions reached, skipping"
                );
                self.update_signal_status(signal.id, "skipped", Some("max_positions_reached"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("max_positions_reached"),
                )
                .await;
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                warn!(
                    signal_id = %signal.id,
                    error = %e,
                    "Failed quant position count check"
                );
                self.update_signal_status(signal.id, "skipped", Some("position_count_failed"))
                    .await;
                self.record_signal_outcome_event(
                    &signal,
                    &execution_mode,
                    "signal_skipped",
                    Some("position_count_failed"),
                )
                .await;
                return Ok(());
            }
        }

        // Step 12: Create PENDING position and persist to DB
        // For single-leg quant trades, the "other side" price is set to zero.
        let (yes_price, no_price) = match signal.direction {
            SignalDirection::BuyYes => (best_ask, Decimal::ZERO),
            SignalDirection::BuyNo => (Decimal::ZERO, best_ask),
        };
        let mut position = Position::new(
            signal.condition_id.clone(),
            yes_price,
            no_price,
            quantity,
            ExitStrategy::ExitOnCorrection,
        );
        self.position_repo.insert(&position).await?;

        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    signal.kind.as_str(),
                    execution_mode.clone(),
                    "quant",
                    signal.condition_id.clone(),
                    "entry_requested",
                )
                .with_signal(&signal)
                .with_position(position.id)
                .with_state(None, Some("pending"))
                .with_requested_size(position_size_usd)
                .with_fill_price(best_ask)
                .with_metadata(signal.metadata.clone()),
            )
            .await;

        // Tag this position as quant-originated (source=3=recommendation)
        // so P&L attribution and dynamic tuner queries can distinguish it.
        sqlx::query("UPDATE positions SET source = 3, source_signal_id = $1 WHERE id = $2")
            .bind(signal.id)
            .bind(position.id)
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to tag position source: {e}"))?;

        info!(
            signal_id = %signal.id,
            position_id = %position.id,
            kind = signal.kind.as_str(),
            direction = signal.direction.as_str(),
            condition_id = &signal.condition_id,
            confidence = signal.confidence,
            size_usd = %position_size_usd,
            quantity = %quantity,
            price = %best_ask,
            "Executing quant signal"
        );

        // Step 13: Execute single-leg FOK market order
        let order = MarketOrder::new(
            signal.condition_id.clone(),
            target_token_id.clone(),
            OrderSide::Buy,
            quantity,
        );

        let report = match self.order_executor.execute_market_order(order).await {
            Ok(report) => report,
            Err(e) => {
                warn!(
                    signal_id = %signal.id,
                    position_id = %position.id,
                    error = %e,
                    "Order execution failed"
                );
                position.mark_entry_failed(
                    polymarket_core::types::FailureReason::ConnectivityError {
                        message: format!("Quant order execution failed: {}", e),
                    },
                );
                let _ = self.position_repo.update(&position).await;
                self.update_signal_status(signal.id, "failed", Some("execution_error"))
                    .await;
                self.record_position_failure_event(
                    &signal,
                    &execution_mode,
                    &position,
                    "execution_error",
                )
                .await;
                self.publish_failure_signal(&signal.condition_id, &format!("Order failed: {e}"));
                return Ok(());
            }
        };

        if !report.is_success() {
            warn!(
                signal_id = %signal.id,
                position_id = %position.id,
                "Order rejected"
            );
            position.mark_entry_failed(polymarket_core::types::FailureReason::OrderRejected {
                message: "Quant order rejected by exchange".to_string(),
            });
            let _ = self.position_repo.update(&position).await;
            self.update_signal_status(signal.id, "failed", Some("order_rejected"))
                .await;
            self.record_position_failure_event(
                &signal,
                &execution_mode,
                &position,
                "order_rejected",
            )
            .await;
            self.publish_failure_signal(&signal.condition_id, "Order rejected by exchange");
            return Ok(());
        }

        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    signal.kind.as_str(),
                    execution_mode.clone(),
                    "quant",
                    signal.condition_id.clone(),
                    "entry_filled",
                )
                .with_signal(&signal)
                .with_position(position.id)
                .with_state(Some("pending"), Some("pending"))
                .with_requested_size(position_size_usd)
                .with_filled_size(report.total_value())
                .with_fill_price(report.average_price)
                .with_metadata(signal.metadata.clone()),
            )
            .await;

        // Step 14: Mark position OPEN
        if let Err(e) = position.mark_open() {
            error!(error = %e, "Failed to transition position to OPEN");
        }
        let _ = self.position_repo.update(&position).await;

        // Link signal to position
        self.link_signal_to_position(signal.id, position.id).await;

        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    signal.kind.as_str(),
                    execution_mode.clone(),
                    "quant",
                    signal.condition_id.clone(),
                    "position_open",
                )
                .with_signal(&signal)
                .with_position(position.id)
                .with_state(Some("pending"), Some("open"))
                .with_requested_size(position_size_usd)
                .with_filled_size(report.total_value())
                .with_fill_price(report.average_price)
                .with_metadata(signal.metadata.clone()),
            )
            .await;

        // Step 15: Publish WebSocket SignalUpdate
        let ws_signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage, // reuse existing type for now
            market_id: signal.condition_id.clone(),
            outcome_id: outcome_name.to_string(),
            action: "quant_executed".to_string(),
            confidence: signal.confidence,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "position_id": position.id.to_string(),
                "signal_kind": signal.kind.as_str(),
                "direction": signal.direction.as_str(),
                "quantity": quantity.to_string(),
                "price": best_ask.to_string(),
                "size_usd": position_size_usd.to_string(),
                "confidence": signal.confidence,
            }),
        };
        let _ = self.signal_tx.send(ws_signal);

        info!(
            signal_id = %signal.id,
            position_id = %position.id,
            kind = signal.kind.as_str(),
            direction = signal.direction.as_str(),
            "Quant signal executed successfully"
        );

        Ok(())
    }

    /// Record an outcome with the per-strategy circuit breaker.
    fn record_strategy_outcome(
        &mut self,
        kind: QuantSignalKind,
        pnl: Decimal,
        success: bool,
        cfg: &QuantSignalExecutorConfig,
    ) {
        // Entry submissions do not realize P&L. Treat zero-P&L events as
        // operational noise rather than strategy losses so client-side rejects
        // and startup retries do not trip the per-strategy breaker.
        if pnl.is_zero() {
            debug!(
                kind = kind.as_str(),
                success, "Skipping zero-PnL quant strategy outcome"
            );
            return;
        }

        if let Some(state) = self.strategy_states.get_mut(&kind) {
            let halted = state.record_outcome(
                pnl,
                cfg.strategy_max_daily_loss_usd,
                cfg.strategy_max_consecutive_losses,
            );
            if halted {
                warn!(
                    kind = kind.as_str(),
                    reason = state.halt_reason.as_deref().unwrap_or("unknown"),
                    daily_pnl = %state.daily_pnl,
                    consecutive_losses = state.consecutive_losses,
                    "Per-strategy circuit breaker tripped — halting strategy"
                );
            }
        }
    }

    /// Update the execution status of a signal in the DB.
    async fn update_signal_status(
        &self,
        signal_id: uuid::Uuid,
        status: &str,
        skip_reason: Option<&str>,
    ) {
        if let Err(error) = sqlx::query(
            "UPDATE quant_signals SET execution_status = $1, skip_reason = $2 WHERE id = $3",
        )
        .bind(status)
        .bind(skip_reason)
        .bind(signal_id)
        .execute(&self.pool)
        .await
        {
            warn!(
                error = %error,
                signal_id = %signal_id,
                status = %status,
                skip_reason = skip_reason,
                "Failed to update quant signal status"
            );
        }
    }

    /// Link an executed signal to its position.
    async fn link_signal_to_position(&self, signal_id: uuid::Uuid, position_id: uuid::Uuid) {
        if let Err(error) = sqlx::query(
            "UPDATE quant_signals SET execution_status = 'executed', position_id = $1 WHERE id = $2",
        )
        .bind(position_id)
        .bind(signal_id)
        .execute(&self.pool)
        .await
        {
            warn!(
                error = %error,
                signal_id = %signal_id,
                position_id = %position_id,
                "Failed to link quant signal to position"
            );
        }
    }

    /// Publish a failure signal to WebSocket.
    fn publish_failure_signal(&self, market_id: &str, reason: &str) {
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: market_id.to_string(),
            outcome_id: String::new(),
            action: "quant_execution_failed".to_string(),
            confidence: 0.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({ "reason": reason }),
        };
        let _ = self.signal_tx.send(signal);
    }

    async fn execution_mode(&self, cfg: &QuantSignalExecutorConfig) -> &'static str {
        if cfg.enabled && self.order_executor.is_live_ready().await {
            "live"
        } else {
            "paper"
        }
    }

    async fn record_signal_outcome_event(
        &self,
        signal: &QuantSignal,
        execution_mode: &str,
        event_type: &str,
        reason: Option<&str>,
    ) {
        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    signal.kind.as_str(),
                    execution_mode.to_string(),
                    "quant",
                    signal.condition_id.clone(),
                    event_type,
                )
                .with_signal(signal)
                .with_reason(reason)
                .with_requested_size(signal.suggested_size_usd)
                .with_metadata(signal.metadata.clone()),
            )
            .await;
    }

    async fn record_position_failure_event(
        &self,
        signal: &QuantSignal,
        execution_mode: &str,
        position: &Position,
        reason: &str,
    ) {
        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    signal.kind.as_str(),
                    execution_mode.to_string(),
                    "quant",
                    signal.condition_id.clone(),
                    "position_failed",
                )
                .with_signal(signal)
                .with_position(position.id)
                .with_state(Some("pending"), Some("entry_failed"))
                .with_reason(Some(reason))
                .with_unrealized_pnl(position.unrealized_pnl)
                .with_metadata(signal.metadata.clone()),
            )
            .await;
    }
}

fn parse_quant_signal_kind(kind: &str) -> Option<QuantSignalKind> {
    match kind {
        "flow" => Some(QuantSignalKind::Flow),
        "cross_market" => Some(QuantSignalKind::CrossMarket),
        "mean_reversion" => Some(QuantSignalKind::MeanReversion),
        "resolution_proximity" => Some(QuantSignalKind::ResolutionProximity),
        _ => None,
    }
}

/// Spawn the quant signal executor background task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_quant_signal_executor(
    config: Arc<RwLock<QuantSignalExecutorConfig>>,
    signal_rx: broadcast::Receiver<QuantSignal>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<polymarket_core::api::ClobClient>,
    pool: PgPool,
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    heartbeat: Arc<AtomicI64>,
) {
    let executor = QuantSignalExecutor::new(
        config,
        signal_rx,
        signal_tx,
        trade_event_tx,
        order_executor,
        circuit_breaker,
        clob_client,
        pool,
        active_clob_markets,
        heartbeat,
    );

    tokio::spawn(async move {
        executor.run().await;
        warn!("Quant signal executor loop ended unexpectedly");
    });

    info!("Quant signal executor spawned");
}

trait TradeEventBuilderExt {
    fn with_signal(self, signal: &QuantSignal) -> Self;
    fn with_position(self, position_id: uuid::Uuid) -> Self;
    fn with_state(self, from: Option<&str>, to: Option<&str>) -> Self;
    fn with_reason(self, reason: Option<&str>) -> Self;
    fn with_requested_size(self, size: Decimal) -> Self;
    fn with_filled_size(self, size: Decimal) -> Self;
    fn with_fill_price(self, price: Decimal) -> Self;
    fn with_unrealized_pnl(self, pnl: Decimal) -> Self;
    fn with_metadata(self, metadata: serde_json::Value) -> Self;
}

impl TradeEventBuilderExt for NewTradeEvent {
    fn with_signal(mut self, signal: &QuantSignal) -> Self {
        self.signal_id = Some(signal.id);
        self.direction = Some(signal.direction.as_str().to_string());
        self.confidence = Some(signal.confidence);
        self
    }

    fn with_position(mut self, position_id: uuid::Uuid) -> Self {
        self.position_id = Some(position_id);
        self
    }

    fn with_state(mut self, from: Option<&str>, to: Option<&str>) -> Self {
        self.state_from = from.map(str::to_string);
        self.state_to = to.map(str::to_string);
        self
    }

    fn with_reason(mut self, reason: Option<&str>) -> Self {
        self.reason = reason.map(str::to_string);
        self
    }

    fn with_requested_size(mut self, size: Decimal) -> Self {
        self.requested_size_usd = Some(size);
        self
    }

    fn with_filled_size(mut self, size: Decimal) -> Self {
        self.filled_size_usd = Some(size);
        self
    }

    fn with_fill_price(mut self, price: Decimal) -> Self {
        self.fill_price = Some(price);
        self
    }

    fn with_unrealized_pnl(mut self, pnl: Decimal) -> Self {
        self.unrealized_pnl = Some(pnl);
        self
    }

    fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = QuantSignalExecutorConfig::from_env();
        assert!(!config.enabled); // default disabled (paper first)
        assert_eq!(config.base_position_size_usd, Decimal::new(30, 0));
        assert_eq!(config.min_confidence, 0.65);
        assert_eq!(config.max_quant_positions, 20);
    }

    #[test]
    fn test_strategy_state_daily_pnl_halt() {
        let mut state = StrategyState::new();
        let max_loss = Decimal::new(200, 0);
        let max_consec = 5;

        // Record losses that exceed daily limit
        state.record_outcome(Decimal::new(-100, 0), max_loss, max_consec);
        assert!(!state.halted);
        state.record_outcome(Decimal::new(-80, 0), max_loss, max_consec);
        assert!(!state.halted);
        let halted = state.record_outcome(Decimal::new(-50, 0), max_loss, max_consec);
        assert!(halted);
        assert!(state.halted);
        assert!(state.halt_reason.as_ref().unwrap().contains("daily_loss"));
    }

    #[test]
    fn test_strategy_state_consecutive_losses_halt() {
        let mut state = StrategyState::new();
        let max_loss = Decimal::new(200, 0);
        let max_consec = 3;

        // 3 consecutive losses should halt
        state.record_outcome(Decimal::new(-1, 0), max_loss, max_consec);
        state.record_outcome(Decimal::new(-1, 0), max_loss, max_consec);
        let halted = state.record_outcome(Decimal::new(-1, 0), max_loss, max_consec);
        assert!(halted);
        assert!(state.consecutive_losses >= max_consec);
    }

    #[test]
    fn test_strategy_state_win_resets_consecutive() {
        let mut state = StrategyState::new();
        let max_loss = Decimal::new(200, 0);
        let max_consec = 5;

        state.record_outcome(Decimal::new(-1, 0), max_loss, max_consec);
        state.record_outcome(Decimal::new(-1, 0), max_loss, max_consec);
        assert_eq!(state.consecutive_losses, 2);

        // Win resets consecutive losses
        state.record_outcome(Decimal::new(10, 0), max_loss, max_consec);
        assert_eq!(state.consecutive_losses, 0);
    }

    #[test]
    fn test_allocation_weights() {
        let config = QuantSignalExecutorConfig::from_env();
        assert_eq!(config.allocation_for(QuantSignalKind::Flow), 0.40);
        assert_eq!(config.allocation_for(QuantSignalKind::CrossMarket), 0.30);
        assert_eq!(config.allocation_for(QuantSignalKind::MeanReversion), 0.20);
        assert_eq!(
            config.allocation_for(QuantSignalKind::ResolutionProximity),
            0.10
        );

        // Weights should approximately sum to 1.0
        let total = config.flow_allocation_pct
            + config.cross_market_allocation_pct
            + config.mean_reversion_allocation_pct
            + config.resolution_allocation_pct;
        assert!((total - 1.0).abs() < 0.01);
    }
}
