//! Arb auto-executor — subscribes to arb entry signals and executes trades.
//!
//! Bridges the gap between arb detection (arb-monitor → Redis → RedisForwarder)
//! and actual order execution. Receives `ArbOpportunity` signals via a broadcast
//! channel, resolves outcome token IDs, checks the circuit breaker, and places
//! sequential YES + NO market orders.

use chrono::Utc;
use polymarket_core::api::{ClobClient, GammaClient};
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::Market;
use polymarket_core::types::{
    ArbOpportunity, ExitStrategy, FailureReason, MarketOrder, OrderSide, Position,
};
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;
use uuid::Uuid;

use crate::learning::{ArbShadowPredictionInput, ShadowPredictionRecorder};
use crate::learning_rollouts::LearningRolloutController;
use crate::trade_events::{NewTradeEvent, TradeEventRecorder};
use crate::websocket::{SignalType, SignalUpdate};

/// Configuration for the arb auto-executor (env-var driven).
#[derive(Debug, Clone)]
pub struct ArbExecutorConfig {
    /// Whether auto-execution is enabled.
    pub enabled: bool,
    /// Base dollar amount per position (used when dynamic sizing is off).
    pub position_size: Decimal,
    /// Minimum net profit to consider a signal worth executing.
    pub min_net_profit: Decimal,
    /// Maximum age of a signal in seconds before it's considered stale.
    pub max_signal_age_secs: i64,
    /// How often to refresh the outcome token cache (seconds).
    pub cache_refresh_secs: u64,
    /// Enable dynamic position sizing based on spread width.
    pub dynamic_sizing: bool,
    /// Minimum position size for dynamic sizing.
    pub min_position_size: Decimal,
    /// Maximum position size for dynamic sizing.
    pub max_position_size: Decimal,
    /// Minimum orderbook depth (in $) on each side to enter.
    pub min_book_depth: Decimal,
    /// Fee rate for tracking fee drag (2% on Polymarket).
    pub fee_rate: Decimal,
}

impl Default for ArbExecutorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            position_size: Decimal::new(50, 0), // $50 base
            min_net_profit: Decimal::new(1, 3), // 0.001
            max_signal_age_secs: 30,
            cache_refresh_secs: 300,
            dynamic_sizing: true,
            min_position_size: Decimal::new(25, 0),  // $25 min
            max_position_size: Decimal::new(200, 0), // $200 max
            min_book_depth: Decimal::new(100, 0),    // $100 minimum depth
            fee_rate: Decimal::new(2, 2),            // 2%
        }
    }
}

impl ArbExecutorConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("ARB_AUTO_EXECUTE")
                .map(|v| v == "true")
                .unwrap_or(false),
            position_size: std::env::var("ARB_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(50, 0)),
            min_net_profit: std::env::var("ARB_MIN_NET_PROFIT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(1, 3)),
            max_signal_age_secs: std::env::var("ARB_MAX_SIGNAL_AGE_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            cache_refresh_secs: std::env::var("ARB_CACHE_REFRESH_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            dynamic_sizing: std::env::var("ARB_DYNAMIC_SIZING")
                .map(|v| v != "false")
                .unwrap_or(true),
            min_position_size: std::env::var("ARB_MIN_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(5, 0)), // $5 floor (small wallet)
            max_position_size: std::env::var("ARB_MAX_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(25, 0)), // $25 ceiling (small wallet)
            min_book_depth: std::env::var("ARB_MIN_BOOK_DEPTH")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(25, 0)), // $25 min depth (small wallet)
            fee_rate: Decimal::new(2, 2), // Always 2% on Polymarket
        }
    }
}

/// Cached mapping of market_id → (yes_token_id, no_token_id).
pub(crate) struct OutcomeTokenCache {
    clob_client: Arc<ClobClient>,
    gamma_client: GammaClient,
    tokens: RwLock<HashMap<String, (String, String)>>,
    /// Shared set of active (non-resolved) market IDs, populated on each refresh.
    /// Used to skip resolved markets before hitting CLOB.
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    /// Database pool for persisting token→condition_id mappings.
    pool: Option<PgPool>,
    /// Serialize cache mutations to avoid overlapping upserts deadlocking.
    refresh_lock: Mutex<()>,
}

impl OutcomeTokenCache {
    pub(crate) fn new(
        clob_client: Arc<ClobClient>,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            clob_client,
            gamma_client: GammaClient::new(None),
            tokens: RwLock::new(HashMap::new()),
            active_clob_markets,
            pool: None,
            refresh_lock: Mutex::new(()),
        }
    }

    pub(crate) fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Refresh the cache using Gamma as the source of tradable markets.
    pub(crate) async fn refresh(&self) -> anyhow::Result<(usize, usize)> {
        let _guard = self.refresh_lock.lock().await;
        let gamma_page_size = std::env::var("GAMMA_ARB_MARKET_PAGE_SIZE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(200);
        let markets = self
            .gamma_client
            .get_all_tradable_markets(gamma_page_size)
            .await?;
        let mut map = HashMap::new();
        let mut active_set = HashSet::new();
        // Collect token_id → condition_id mappings for DB cache
        let mut token_condition_pairs: Vec<(String, String)> = Vec::new();

        for market in &markets {
            if !market.resolved {
                active_set.insert(market.id.clone());
                // Also insert outcome token IDs so trades whose market_id
                // fell back to asset_id (when condition_id is absent) still
                // match the active set.
                for outcome in &market.outcomes {
                    active_set.insert(outcome.token_id.clone());
                    // Map token_id → condition_id (market.id) for the DB cache
                    token_condition_pairs.push((outcome.token_id.clone(), market.id.clone()));
                }
            }
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
        let active_set_size = active_set.len();
        *self.tokens.write().await = map;
        *self.active_clob_markets.write().await = active_set;

        // Deduplicate by token_id (same token can appear if cursor pages overlap)
        let deduped: std::collections::HashMap<&str, &str> = token_condition_pairs
            .iter()
            .map(|(t, c)| (t.as_str(), c.as_str()))
            .collect();
        let mut token_condition_pairs: Vec<(String, String)> = deduped
            .into_iter()
            .map(|(t, c)| (t.to_owned(), c.to_owned()))
            .collect();
        token_condition_pairs.sort_by(|a, b| a.0.cmp(&b.0));

        // Batch UPSERT token→condition_id mappings to DB for resolution checks
        if let Some(ref pool) = self.pool {
            if let Err(e) = Self::upsert_token_condition_cache(pool, &token_condition_pairs).await {
                warn!(error = %e, "Failed to update token_condition_cache");
            } else {
                debug!(
                    pairs = token_condition_pairs.len(),
                    "Updated token_condition_cache"
                );
            }
        }

        Ok((count, active_set_size))
    }

    /// Hydrate a single market into the cache without forcing a full-market refresh.
    pub(crate) async fn hydrate_market(
        &self,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        if let Some(ids) = self.get(market_id).await {
            return Ok(Some(ids));
        }

        let market = match self.clob_client.get_market_by_id(market_id).await {
            Ok(market) => market,
            Err(e) => {
                warn!(market_id = %market_id, error = %e, "Failed to fetch single market for token cache hydration");
                return Ok(None);
            }
        };

        let _guard = self.refresh_lock.lock().await;
        Ok(self.apply_market_update(&market).await?)
    }

    async fn apply_market_update(
        &self,
        market: &Market,
    ) -> anyhow::Result<Option<(String, String)>> {
        let mut tokens = self.tokens.write().await;
        let mut active_markets = self.active_clob_markets.write().await;

        if market.resolved {
            active_markets.remove(&market.id);
            for outcome in &market.outcomes {
                active_markets.remove(&outcome.token_id);
            }
        } else {
            active_markets.insert(market.id.clone());
            for outcome in &market.outcomes {
                active_markets.insert(outcome.token_id.clone());
            }
        }

        let token_pair = if market.outcomes.len() == 2 {
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
            tokens.insert(market.id.clone(), (yes_id.clone(), no_id.clone()));
            Some((yes_id, no_id))
        } else {
            tokens.remove(&market.id);
            None
        };
        drop(tokens);
        drop(active_markets);

        if let Some(ref pool) = self.pool {
            let mut token_condition_pairs: Vec<(String, String)> = market
                .outcomes
                .iter()
                .map(|outcome| (outcome.token_id.clone(), market.id.clone()))
                .collect();
            token_condition_pairs.sort_by(|a, b| a.0.cmp(&b.0));
            if let Err(e) = Self::upsert_token_condition_cache(pool, &token_condition_pairs).await {
                warn!(market_id = %market.id, error = %e, "Failed to update token_condition_cache during single-market hydration");
            } else {
                debug!(market_id = %market.id, pairs = token_condition_pairs.len(), "Hydrated token_condition_cache for single market");
            }
        }

        Ok(token_pair)
    }

    /// Batch UPSERT token→condition_id mappings into the database cache.
    async fn upsert_token_condition_cache(
        pool: &PgPool,
        pairs: &[(String, String)],
    ) -> anyhow::Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }

        // Batch in smaller deterministic chunks to reduce row-lock contention.
        const BATCH_SIZE: usize = 250;
        for chunk in pairs.chunks(BATCH_SIZE) {
            let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
                "INSERT INTO token_condition_cache (token_id, condition_id, updated_at) ",
            );
            qb.push_values(chunk, |mut b, (token_id, condition_id)| {
                b.push_bind(token_id)
                    .push_bind(condition_id)
                    .push_bind(chrono::Utc::now());
            });
            qb.push(
                " ON CONFLICT (token_id) DO UPDATE SET condition_id = EXCLUDED.condition_id, updated_at = EXCLUDED.updated_at WHERE token_condition_cache.condition_id IS DISTINCT FROM EXCLUDED.condition_id",
            );
            qb.build().execute(pool).await?;
        }

        Ok(())
    }

    /// Look up token IDs for a given market.
    pub(crate) async fn get(&self, market_id: &str) -> Option<(String, String)> {
        self.tokens.read().await.get(market_id).cloned()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ArbExecutorRuntimeStatus {
    pub enabled: bool,
    pub live_ready: bool,
    pub signals_seen: u64,
    pub lagged_signals: u64,
    pub disabled_skips: u64,
    pub stale_skips: u64,
    pub min_profit_skips: u64,
    pub active_position_skips: u64,
    pub circuit_breaker_skips: u64,
    pub token_lookup_skips: u64,
    pub depth_skips: u64,
    pub zero_cost_skips: u64,
    pub execution_failures: u64,
    pub executed: u64,
    pub cache_refresh_failures: u64,
    pub last_signal_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_decision_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_market_id: Option<String>,
    pub last_decision: Option<String>,
}

impl ArbExecutorRuntimeStatus {
    fn record_signal(&mut self, market_id: &str) {
        self.signals_seen = self.signals_seen.saturating_add(1);
        self.last_signal_at = Some(Utc::now());
        self.last_market_id = Some(market_id.to_string());
    }

    fn record_decision(&mut self, market_id: &str, decision: impl Into<String>) {
        self.last_decision_at = Some(Utc::now());
        self.last_market_id = Some(market_id.to_string());
        self.last_decision = Some(decision.into());
    }
}

#[derive(Debug, Clone, Default)]
struct ArbExecutionTelemetry {
    attempt_id: Uuid,
    signal_age_ms: i64,
    token_lookup_ms: Option<i64>,
    depth_check_ms: Option<i64>,
    preflight_ms: Option<i64>,
    yes_order_ms: Option<i64>,
    no_order_ms: Option<i64>,
    inter_leg_gap_ms: Option<i64>,
    request_to_fill_ms: Option<i64>,
    request_to_open_ms: Option<i64>,
    total_time_ms: Option<i64>,
    token_source: Option<String>,
    failure_stage: Option<String>,
    one_legged: bool,
}

impl ArbExecutionTelemetry {
    fn new(attempt_id: Uuid, signal_age_ms: i64) -> Self {
        Self {
            attempt_id,
            signal_age_ms,
            ..Self::default()
        }
    }

    fn finish_total(&mut self, started_at: &std::time::Instant) {
        self.total_time_ms = Some(started_at.elapsed().as_millis() as i64);
    }

    fn to_metadata(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("telemetry_version".to_string(), serde_json::json!(2));
        map.insert("attempt_id".to_string(), serde_json::json!(self.attempt_id));
        map.insert(
            "signal_age_ms".to_string(),
            serde_json::json!(self.signal_age_ms),
        );

        if let Some(value) = self.token_lookup_ms {
            map.insert("token_lookup_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.depth_check_ms {
            map.insert("depth_check_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.preflight_ms {
            map.insert("preflight_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.yes_order_ms {
            map.insert("yes_order_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.no_order_ms {
            map.insert("no_order_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.inter_leg_gap_ms {
            map.insert("inter_leg_gap_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.request_to_fill_ms {
            map.insert("request_to_fill_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.request_to_open_ms {
            map.insert("request_to_open_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = self.total_time_ms {
            map.insert("total_time_ms".to_string(), serde_json::json!(value));
        }
        if let Some(value) = &self.token_source {
            map.insert("token_source".to_string(), serde_json::json!(value));
        }
        if let Some(value) = &self.failure_stage {
            map.insert("failure_stage".to_string(), serde_json::json!(value));
        }
        if self.one_legged {
            map.insert("one_legged".to_string(), serde_json::json!(true));
        }

        serde_json::Value::Object(map)
    }
}

fn merge_metadata(
    metadata: serde_json::Value,
    telemetry: &ArbExecutionTelemetry,
) -> serde_json::Value {
    let mut base = match metadata {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };

    if let serde_json::Value::Object(telemetry_map) = telemetry.to_metadata() {
        for (key, value) in telemetry_map {
            base.insert(key, value);
        }
    }

    serde_json::Value::Object(base)
}

/// Arb auto-executor service.
pub struct ArbAutoExecutor {
    config: Arc<RwLock<ArbExecutorConfig>>,
    arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    pool: PgPool,
    position_repo: PositionRepository,
    token_cache: Arc<OutcomeTokenCache>,
    trade_event_recorder: TradeEventRecorder,
    shadow_prediction_recorder: ShadowPredictionRecorder,
    rollout_controller: LearningRolloutController,
    /// Set of market IDs with active positions (for deduplication).
    /// Shared with ExitHandler so closed positions unblock their markets.
    active_markets: Arc<RwLock<HashSet<String>>>,
    /// Heartbeat timestamp (epoch secs) — updated every loop iteration to prove liveness.
    heartbeat: Arc<AtomicI64>,
    /// Shared executor runtime status for dashboard/API inspection.
    runtime_status: Arc<RwLock<ArbExecutorRuntimeStatus>>,
}

impl ArbAutoExecutor {
    /// Create a new arb auto-executor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<RwLock<ArbExecutorConfig>>,
        arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        pool: PgPool,
        active_markets: Arc<RwLock<HashSet<String>>>,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
        heartbeat: Arc<AtomicI64>,
        runtime_status: Arc<RwLock<ArbExecutorRuntimeStatus>>,
    ) -> Self {
        Self {
            config,
            arb_entry_rx,
            signal_tx,
            order_executor,
            circuit_breaker,
            pool: pool.clone(),
            position_repo: PositionRepository::new(pool.clone()),
            token_cache: Arc::new(
                OutcomeTokenCache::new(clob_client, active_clob_markets).with_pool(pool.clone()),
            ),
            trade_event_recorder: TradeEventRecorder::new(pool.clone(), trade_event_tx),
            shadow_prediction_recorder: ShadowPredictionRecorder::new(pool.clone()),
            rollout_controller: LearningRolloutController::new(pool.clone()),
            active_markets,
            heartbeat,
            runtime_status,
        }
    }

    /// Snapshot the current config for use during a single tick/signal.
    async fn snapshot_config(&self) -> ArbExecutorConfig {
        self.config.read().await.clone()
    }

    /// Main run loop.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let startup_cfg = self.snapshot_config().await;

        let live_ready = self.order_executor.is_live_ready().await;
        {
            let mut runtime = self.runtime_status.write().await;
            runtime.enabled = startup_cfg.enabled;
            runtime.live_ready = live_ready;
            runtime.record_decision(
                "startup",
                format!(
                    "startup enabled={} live_ready={} min_profit={} min_depth={} max_signal_age_secs={}",
                    startup_cfg.enabled,
                    live_ready,
                    startup_cfg.min_net_profit,
                    startup_cfg.min_book_depth,
                    startup_cfg.max_signal_age_secs
                ),
            );
        }
        info!(
            enabled = startup_cfg.enabled,
            live_ready,
            position_size = %startup_cfg.position_size,
            min_position_size = %startup_cfg.min_position_size,
            max_position_size = %startup_cfg.max_position_size,
            dynamic_sizing = startup_cfg.dynamic_sizing,
            min_net_profit = %startup_cfg.min_net_profit,
            min_book_depth = %startup_cfg.min_book_depth,
            max_signal_age_secs = startup_cfg.max_signal_age_secs,
            "Arb executor startup readiness check"
        );
        if !live_ready {
            warn!(
                "OrderExecutor NOT in live mode — all orders will be paper-traded. \
                   Check WALLET_PRIVATE_KEY and wallet initialization logs."
            );
        }
        if !startup_cfg.enabled {
            warn!("ARB_AUTO_EXECUTE is disabled — signals will be logged but not executed.");
        }

        // Mark liveness immediately so the dashboard doesn't flag a stale heartbeat
        // while the slow initial cache load runs.
        self.heartbeat
            .store(Utc::now().timestamp(), Ordering::Relaxed);

        // Clean up stale positions: any position stuck in Pending (0) for >24h
        // is almost certainly a failed paper trade that will block the dedup set forever.
        let stale_cleanup_reason = serde_json::to_string(&FailureReason::Unknown {
            message: "stale_cleanup: stuck in Pending >24h".to_string(),
        })
        .unwrap_or_else(|_| {
            r#"{"unknown":{"message":"stale_cleanup: stuck in Pending >24h"}}"#.to_string()
        });

        match sqlx::query(
            r#"
            UPDATE positions
            SET state = 5, failure_reason = $1
            WHERE state = 0 AND entry_timestamp < NOW() - INTERVAL '24 hours'
            "#,
        )
        .bind(&stale_cleanup_reason)
        .execute(&self.pool)
        .await
        {
            Ok(result) if result.rows_affected() > 0 => {
                info!(
                    cleaned = result.rows_affected(),
                    "Closed stale Pending positions (>24h old)"
                );
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "Failed to clean up stale positions"),
        }

        // Load active positions for dedup (fast DB query, do this first)
        if let Ok(active) = self.position_repo.get_active().await {
            let mut set = self.active_markets.write().await;
            for pos in &active {
                set.insert(pos.market_id.clone());
            }
            info!(active_positions = %active.len(), "Loaded active positions for dedup");
        }

        // Initial token cache load — can take minutes with 190k+ markets and rate limiting.
        // Keep heartbeat alive during the slow load so the dashboard doesn't flag us as stale.
        {
            let hb = self.heartbeat.clone();
            let keeper = tokio::spawn(async move {
                let mut tick = tokio::time::interval(tokio::time::Duration::from_secs(15));
                loop {
                    tick.tick().await;
                    hb.store(Utc::now().timestamp(), Ordering::Relaxed);
                }
            });

            match self.token_cache.refresh().await {
                Ok((count, active_set_size)) => info!(
                    markets = count,
                    active_set_size, "Outcome token cache loaded"
                ),
                Err(e) => warn!(error = %e, "Failed to load token cache, will retry"),
            }

            keeper.abort();
        }

        let cache_interval = tokio::time::Duration::from_secs(startup_cfg.cache_refresh_secs);
        let mut cache_ticker = tokio::time::interval(cache_interval);
        // Skip the first immediate tick (we already loaded above)
        cache_ticker.tick().await;
        let cache_refresh_inflight = Arc::new(AtomicBool::new(false));

        // Heartbeat ticker keeps the liveness stamp fresh even when no signals arrive.
        // Must be well under the 120s staleness threshold checked by the status endpoint.
        let mut heartbeat_ticker = tokio::time::interval(tokio::time::Duration::from_secs(30));
        heartbeat_ticker.tick().await;

        loop {
            // Update heartbeat to prove liveness
            self.heartbeat
                .store(Utc::now().timestamp(), Ordering::Relaxed);

            tokio::select! {
                result = self.arb_entry_rx.recv() => {
                    match result {
                        Ok(arb) => {
                            if let Err(e) = self.process_arb_signal(arb).await {
                                error!(error = %e, "Failed to process arb signal");
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            {
                                let mut runtime = self.runtime_status.write().await;
                                runtime.lagged_signals = runtime.lagged_signals.saturating_add(n as u64);
                                runtime.record_decision("channel", format!("lagged {} arb signals", n));
                            }
                            warn!(skipped = n, "Arb executor lagged, skipped signals");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Arb entry channel closed, stopping executor");
                            break;
                        }
                    }
                }
                _ = cache_ticker.tick() => {
                    if cache_refresh_inflight
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        let token_cache = Arc::clone(&self.token_cache);
                        let runtime_status = Arc::clone(&self.runtime_status);
                        let cache_refresh_inflight = Arc::clone(&cache_refresh_inflight);
                        tokio::spawn(async move {
                            match token_cache.refresh().await {
                                Ok((count, active_set_size)) => {
                                    debug!(
                                        markets = count,
                                        active_set_size,
                                        "Token cache refreshed"
                                    );
                                }
                                Err(e) => {
                                    let mut runtime = runtime_status.write().await;
                                    runtime.cache_refresh_failures =
                                        runtime.cache_refresh_failures.saturating_add(1);
                                    runtime.record_decision(
                                        "cache",
                                        format!("refresh failed: {e}"),
                                    );
                                    warn!(error = %e, "Token cache refresh failed");
                                }
                            }
                            cache_refresh_inflight.store(false, Ordering::SeqCst);
                        });
                    } else {
                        debug!("Token cache refresh still in progress, skipping scheduled tick");
                    }
                }
                _ = heartbeat_ticker.tick() => {
                    // No-op: the heartbeat stamp at the top of the loop is sufficient.
                    // This arm just ensures the loop wakes up during quiet periods.
                }
            }
        }

        Ok(())
    }

    /// Process a single arb signal through validation → execution → tracking.
    async fn process_arb_signal(&self, arb: ArbOpportunity) -> anyhow::Result<()> {
        let process_started_at = std::time::Instant::now();
        let cfg = self.snapshot_config().await;
        let signal_age_ms = Utc::now()
            .signed_duration_since(arb.timestamp)
            .num_milliseconds()
            .max(0);
        let age_secs = signal_age_ms / 1000;
        let attempt_id = Uuid::new_v4();
        let mut telemetry = ArbExecutionTelemetry::new(attempt_id, signal_age_ms);

        {
            let mut runtime = self.runtime_status.write().await;
            runtime.enabled = cfg.enabled;
            runtime.record_signal(&arb.market_id);
        }

        // Fast-fail stale signals before any DB writes or shadow bookkeeping.
        if age_secs > cfg.max_signal_age_secs {
            let mut runtime = self.runtime_status.write().await;
            runtime.stale_skips = runtime.stale_skips.saturating_add(1);
            runtime.record_decision(
                &arb.market_id,
                format!(
                    "skipped: stale signal age={} max={}",
                    age_secs, cfg.max_signal_age_secs
                ),
            );
            info!(
                market_id = %arb.market_id,
                age_secs = age_secs,
                max = cfg.max_signal_age_secs,
                "Arb signal too stale, skipping"
            );
            telemetry.finish_total(&process_started_at);
            self.record_skip_event(&arb, "unknown", "too_stale", &telemetry)
                .await;
            return Ok(());
        }

        let live_ready = self.order_executor.is_live_ready().await;
        let execution_mode = if live_ready { "live" } else { "paper" };
        {
            let mut runtime = self.runtime_status.write().await;
            runtime.live_ready = live_ready;
        }

        // Persist every arb signal to arb_opportunities for the DynamicTuner's
        // depth_proxy and events_per_min calculations. Fire-and-forget to avoid
        // blocking the execution path.
        let pool_ref = self.pool.clone();
        let arb_clone = arb.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                r#"
                INSERT INTO arb_opportunities (market_id, timestamp, yes_ask, no_ask, total_cost, gross_profit, net_profit)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                "#,
            )
            .bind(&arb_clone.market_id)
            .bind(arb_clone.timestamp)
            .bind(arb_clone.yes_ask)
            .bind(arb_clone.no_ask)
            .bind(arb_clone.total_cost)
            .bind(arb_clone.gross_profit)
            .bind(arb_clone.net_profit)
            .execute(&pool_ref)
            .await;
        });

        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    "arb",
                    execution_mode,
                    "arb",
                    arb.market_id.clone(),
                    "signal_generated",
                )
                .with_expected_edge(arb.net_profit)
                .with_observed_edge(arb.gross_profit)
                .with_metadata(merge_metadata(
                    serde_json::json!({
                        "yes_ask": arb.yes_ask.to_string(),
                        "no_ask": arb.no_ask.to_string(),
                        "total_cost": arb.total_cost.to_string(),
                        "gross_profit": arb.gross_profit.to_string(),
                        "net_profit": arb.net_profit.to_string(),
                    }),
                    &telemetry,
                )),
            )
            .await;

        let shadow_prediction_recorder = self.shadow_prediction_recorder.clone();
        let shadow_input = ArbShadowPredictionInput {
            attempt_id,
            market_id: arb.market_id.clone(),
            execution_mode: execution_mode.to_string(),
            signal_age_ms,
            yes_ask: arb.yes_ask,
            no_ask: arb.no_ask,
            total_cost: arb.total_cost,
            gross_profit: arb.gross_profit,
            net_profit: arb.net_profit,
            live_ready,
        };
        let shadow_record_input = shadow_input.clone();
        tokio::spawn(async move {
            shadow_prediction_recorder
                .record_arb_attempt_baselines(shadow_record_input)
                .await;
        });

        // Per-signal guard: skip processing when disabled at runtime
        if !cfg.enabled {
            let mut runtime = self.runtime_status.write().await;
            runtime.disabled_skips = runtime.disabled_skips.saturating_add(1);
            runtime.record_decision(&arb.market_id, "skipped: executor disabled");
            telemetry.finish_total(&process_started_at);
            self.record_skip_event(&arb, execution_mode, "executor_disabled", &telemetry)
                .await;
            return Ok(());
        }

        let market_id = &arb.market_id;

        // 1. Validate signal freshness
        // 2. Check minimum profit threshold
        if arb.net_profit < cfg.min_net_profit {
            let mut runtime = self.runtime_status.write().await;
            runtime.min_profit_skips = runtime.min_profit_skips.saturating_add(1);
            runtime.record_decision(
                market_id,
                format!(
                    "skipped: net profit {} below {}",
                    arb.net_profit, cfg.min_net_profit
                ),
            );
            info!(
                market_id = %market_id,
                net_profit = %arb.net_profit,
                min = %cfg.min_net_profit,
                "Arb signal below min profit threshold, skipping"
            );
            telemetry.finish_total(&process_started_at);
            self.record_skip_event(&arb, execution_mode, "below_min_profit", &telemetry)
                .await;
            return Ok(());
        }

        // 3. Dedup — skip if we already have an active position in this market
        {
            let active = self.active_markets.read().await;
            if active.contains(market_id) {
                let mut runtime = self.runtime_status.write().await;
                runtime.active_position_skips = runtime.active_position_skips.saturating_add(1);
                runtime.record_decision(market_id, "skipped: active position exists");
                info!(market_id = %market_id, "Active position exists, skipping");
                telemetry.finish_total(&process_started_at);
                self.record_skip_event(&arb, execution_mode, "active_position_exists", &telemetry)
                    .await;
                return Ok(());
            }
        }

        // 4. Circuit breaker check
        if !self.circuit_breaker.can_trade().await {
            let mut runtime = self.runtime_status.write().await;
            runtime.circuit_breaker_skips = runtime.circuit_breaker_skips.saturating_add(1);
            runtime.record_decision(market_id, "skipped: circuit breaker tripped");
            warn!(market_id = %market_id, "Circuit breaker tripped, skipping arb trade");
            telemetry.finish_total(&process_started_at);
            self.record_skip_event(&arb, execution_mode, "circuit_breaker", &telemetry)
                .await;
            return Ok(());
        }

        // 5. Resolve outcome token IDs from cache
        let token_lookup_started_at = std::time::Instant::now();
        let (yes_token_id, no_token_id) = match self.token_cache.get(market_id).await {
            Some(ids) => {
                telemetry.token_source = Some("cache".to_string());
                ids
            }
            None => {
                warn!(
                    market_id = %market_id,
                    "No token IDs cached for market, attempting single-market hydration"
                );
                match self.token_cache.hydrate_market(market_id).await {
                    Ok(Some(ids)) => {
                        telemetry.token_source = Some("single_market_hydration".to_string());
                        ids
                    }
                    Ok(None) => {
                        telemetry.token_lookup_ms =
                            Some(token_lookup_started_at.elapsed().as_millis() as i64);
                        telemetry.finish_total(&process_started_at);
                        let mut runtime = self.runtime_status.write().await;
                        runtime.token_lookup_skips = runtime.token_lookup_skips.saturating_add(1);
                        runtime.record_decision(
                            market_id,
                            "skipped: token lookup failed after single-market hydration",
                        );
                        warn!(market_id = %market_id, "Market not found after single-market hydration, skipping");
                        self.record_skip_event(
                            &arb,
                            execution_mode,
                            "token_lookup_failed",
                            &telemetry,
                        )
                        .await;
                        return Ok(());
                    }
                    Err(e) => {
                        telemetry.token_lookup_ms =
                            Some(token_lookup_started_at.elapsed().as_millis() as i64);
                        telemetry.finish_total(&process_started_at);
                        let mut runtime = self.runtime_status.write().await;
                        runtime.token_lookup_skips = runtime.token_lookup_skips.saturating_add(1);
                        runtime.record_decision(
                            market_id,
                            format!("skipped: token cache hydration failed: {e}"),
                        );
                        warn!(market_id = %market_id, error = %e, "Token cache hydration failed");
                        self.record_skip_event(
                            &arb,
                            execution_mode,
                            "token_lookup_failed",
                            &telemetry,
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
        };
        telemetry.token_lookup_ms = Some(token_lookup_started_at.elapsed().as_millis() as i64);

        // 6. Market quality check — verify orderbook depth on both sides
        let depth_check_started_at = std::time::Instant::now();
        {
            let clob_client = self.order_executor.clob_client();
            let (yes_book, no_book) = tokio::join!(
                clob_client.get_order_book(&yes_token_id),
                clob_client.get_order_book(&no_token_id)
            );

            if let (Ok(yb), Ok(nb)) = (&yes_book, &no_book) {
                let yes_depth: Decimal = yb.asks.iter().map(|l| l.price * l.size).sum();
                let no_depth: Decimal = nb.asks.iter().map(|l| l.price * l.size).sum();

                if yes_depth < cfg.min_book_depth || no_depth < cfg.min_book_depth {
                    let mut runtime = self.runtime_status.write().await;
                    runtime.depth_skips = runtime.depth_skips.saturating_add(1);
                    runtime.record_decision(
                        market_id,
                        format!(
                            "skipped: insufficient depth yes={} no={} min={}",
                            yes_depth, no_depth, cfg.min_book_depth
                        ),
                    );
                    info!(
                        market_id = %market_id,
                        yes_depth = %yes_depth,
                        no_depth = %no_depth,
                        min_depth = %cfg.min_book_depth,
                        "Arb signal insufficient orderbook depth, skipping"
                    );
                    telemetry.depth_check_ms =
                        Some(depth_check_started_at.elapsed().as_millis() as i64);
                    telemetry.preflight_ms = Some(process_started_at.elapsed().as_millis() as i64);
                    telemetry.finish_total(&process_started_at);
                    self.record_skip_event(&arb, execution_mode, "insufficient_depth", &telemetry)
                        .await;
                    return Ok(());
                }
            }
        }
        telemetry.depth_check_ms = Some(depth_check_started_at.elapsed().as_millis() as i64);

        let rollout_decision = self
            .rollout_controller
            .evaluate_arb(&shadow_input, execution_mode)
            .await;
        if let Some(reason) = rollout_decision.skip_reason.as_deref() {
            let mut runtime = self.runtime_status.write().await;
            runtime.record_decision(market_id, format!("skipped: {reason}"));
            telemetry.finish_total(&process_started_at);
            self.record_skip_event(&arb, execution_mode, reason, &telemetry)
                .await;
            return Ok(());
        }

        // 7. Dynamic position sizing based on spread width
        let mut position_size = if cfg.dynamic_sizing {
            // Linear interpolation between min and max position size
            // based on where net_profit falls in the [min_net_profit, 0.05] range.
            // min_net_profit (e.g. 0.001) → min_position_size ($25)
            // 0.05+ (5 cent spread) → max_position_size ($200)
            let floor = cfg.min_net_profit;
            let ceiling = Decimal::new(5, 2); // 0.05
            let range = ceiling - floor;
            let sized = if range.is_zero() {
                cfg.min_position_size
            } else {
                let t = (arb.net_profit - floor) / range;
                let t_clamped = t.max(Decimal::ZERO).min(Decimal::ONE);
                let size_range = cfg.max_position_size - cfg.min_position_size;
                cfg.min_position_size + size_range * t_clamped
            };
            debug!(
                market_id = %market_id,
                net_profit = %arb.net_profit,
                dynamic_size = %sized,
                "Dynamic arb position sizing"
            );
            sized
        } else {
            cfg.position_size
        };

        if rollout_decision.size_multiplier < Decimal::ONE {
            position_size *= rollout_decision.size_multiplier;
            let mut runtime = self.runtime_status.write().await;
            runtime.record_decision(
                market_id,
                format!(
                    "rollout size adjusted by {}",
                    rollout_decision.size_multiplier
                ),
            );
        }

        if arb.total_cost.is_zero() {
            let mut runtime = self.runtime_status.write().await;
            runtime.zero_cost_skips = runtime.zero_cost_skips.saturating_add(1);
            runtime.record_decision(market_id, "skipped: zero total cost");
            warn!(market_id = %market_id, "Zero total_cost, skipping");
            telemetry.preflight_ms = Some(process_started_at.elapsed().as_millis() as i64);
            telemetry.finish_total(&process_started_at);
            self.record_skip_event(&arb, execution_mode, "zero_total_cost", &telemetry)
                .await;
            return Ok(());
        }
        let quantity = position_size / arb.total_cost;
        telemetry.preflight_ms = Some(process_started_at.elapsed().as_millis() as i64);

        // Fee drag tracking: `fee_drag` is the worst-case reduction in resolution payout per pair.
        let expected_fees = quantity * arb.fee_drag;
        let expected_gross = arb.gross_profit * quantity;
        let expected_net = arb.net_profit * quantity;

        info!(
            market_id = %market_id,
            net_profit_per_share = %arb.net_profit,
            gross_profit_per_share = %arb.gross_profit,
            fee_drag_per_share = %arb.fee_drag,
            worst_case_payout_per_share = %arb.worst_case_payout,
            total_cost = %arb.total_cost,
            position_size = %position_size,
            quantity = %quantity,
            expected_fee_drag = %expected_fees,
            expected_gross = %expected_gross,
            expected_net = %expected_net,
            "Executing arb trade"
        );

        // 8. Create position in PENDING state
        let mut position = Position::new(
            market_id.clone(),
            arb.yes_ask,
            arb.no_ask,
            quantity,
            ExitStrategy::HoldToResolution,
        );
        position.apply_arb_fee_model(&arb);

        if let Err(e) = self.position_repo.insert(&position).await {
            error!(error = %e, "Failed to persist pending position");
            return Err(anyhow::anyhow!("Failed to persist position: {e}"));
        }

        self.trade_event_recorder
            .record_warn({
                let metadata = merge_metadata(
                    serde_json::json!({
                        "quantity": quantity.to_string(),
                        "yes_price": arb.yes_ask.to_string(),
                        "no_price": arb.no_ask.to_string(),
                        "total_cost": arb.total_cost.to_string(),
                    }),
                    &telemetry,
                );
                NewTradeEvent::new(
                    "arb",
                    execution_mode,
                    "arb",
                    market_id.clone(),
                    "entry_requested",
                )
                .with_position(position.id)
                .with_state(None, Some("pending"))
                .with_expected_edge(expected_net)
                .with_requested_size(position_size)
                .with_metadata(metadata)
            })
            .await;

        // 9. Execute YES market order
        let yes_order = MarketOrder::new(market_id.clone(), yes_token_id, OrderSide::Buy, quantity);
        let yes_order_started_at = std::time::Instant::now();
        let yes_result = self.order_executor.execute_market_order(yes_order).await;
        telemetry.yes_order_ms = Some(yes_order_started_at.elapsed().as_millis() as i64);
        let yes_report = match yes_result {
            Ok(report) => report,
            Err(e) => {
                telemetry.failure_stage = Some("yes_order_error".to_string());
                telemetry.finish_total(&process_started_at);
                let mut runtime = self.runtime_status.write().await;
                runtime.execution_failures = runtime.execution_failures.saturating_add(1);
                runtime.record_decision(
                    market_id,
                    format!("execution failure: YES order error: {e}"),
                );
                error!(error = %e, market_id = %market_id, "YES order execution error");
                position.mark_entry_failed(FailureReason::ConnectivityError {
                    message: format!("YES order error: {e}"),
                });
                let _ = self.position_repo.update(&position).await;
                self.record_failure_event(
                    execution_mode,
                    market_id,
                    &position,
                    "yes_order_error",
                    &telemetry,
                )
                .await;
                self.publish_failure_signal(market_id, "YES order execution error");
                return Ok(());
            }
        };

        // 10. If YES order rejected/failed → mark EntryFailed
        if !yes_report.is_success() {
            let msg = yes_report
                .error_message
                .unwrap_or_else(|| "YES order not filled".to_string());
            telemetry.failure_stage = Some("yes_order_rejected".to_string());
            telemetry.finish_total(&process_started_at);
            let mut runtime = self.runtime_status.write().await;
            runtime.execution_failures = runtime.execution_failures.saturating_add(1);
            runtime.record_decision(
                market_id,
                format!("execution failure: YES order rejected: {msg}"),
            );
            warn!(market_id = %market_id, reason = %msg, "YES order failed");
            position.mark_entry_failed(FailureReason::OrderRejected { message: msg });
            let _ = self.position_repo.update(&position).await;
            self.record_failure_event(
                execution_mode,
                market_id,
                &position,
                "yes_order_rejected",
                &telemetry,
            )
            .await;
            self.publish_failure_signal(market_id, "YES order rejected");
            return Ok(());
        }
        let yes_order_finished_at = std::time::Instant::now();

        // 11. Execute NO market order
        let no_order = MarketOrder::new(market_id.clone(), no_token_id, OrderSide::Buy, quantity);
        let no_order_started_at = std::time::Instant::now();
        telemetry.inter_leg_gap_ms = Some(
            no_order_started_at
                .duration_since(yes_order_finished_at)
                .as_millis() as i64,
        );
        let no_result = self.order_executor.execute_market_order(no_order).await;
        telemetry.no_order_ms = Some(no_order_started_at.elapsed().as_millis() as i64);
        let no_report = match no_result {
            Ok(report) => report,
            Err(e) => {
                telemetry.failure_stage = Some("no_order_error".to_string());
                telemetry.one_legged = true;
                telemetry.finish_total(&process_started_at);
                let mut runtime = self.runtime_status.write().await;
                runtime.execution_failures = runtime.execution_failures.saturating_add(1);
                runtime
                    .record_decision(market_id, format!("execution failure: NO order error: {e}"));
                error!(error = %e, market_id = %market_id, "NO order execution error (one-legged)");
                position.mark_entry_failed(FailureReason::ConnectivityError {
                    message: format!("NO order error (one-legged, YES filled): {e}"),
                });
                let _ = self.position_repo.update(&position).await;
                self.record_failure_event(
                    execution_mode,
                    market_id,
                    &position,
                    "no_order_error",
                    &telemetry,
                )
                .await;
                self.publish_failure_signal(market_id, "NO order error (one-legged)");
                return Ok(());
            }
        };

        // 12. If NO order failed → one-legged position
        if !no_report.is_success() {
            let msg = no_report
                .error_message
                .unwrap_or_else(|| "NO order not filled".to_string());
            telemetry.failure_stage = Some("no_order_rejected".to_string());
            telemetry.one_legged = true;
            telemetry.finish_total(&process_started_at);
            let mut runtime = self.runtime_status.write().await;
            runtime.execution_failures = runtime.execution_failures.saturating_add(1);
            runtime.record_decision(
                market_id,
                format!("execution failure: NO order rejected: {msg}"),
            );
            warn!(
                market_id = %market_id,
                reason = %msg,
                "NO order failed (one-legged, YES filled — flagged for review)"
            );
            position.mark_entry_failed(FailureReason::OrderRejected {
                message: format!("One-legged: YES filled but NO failed: {msg}"),
            });
            let _ = self.position_repo.update(&position).await;
            self.record_failure_event(
                execution_mode,
                market_id,
                &position,
                "no_order_rejected",
                &telemetry,
            )
            .await;
            self.publish_failure_signal(market_id, "One-legged position (NO failed)");
            return Ok(());
        }

        let actual_fill_cost = yes_report.total_value() + no_report.total_value();
        let modeled_fill_cost = arb.total_cost * quantity;
        let actual_resolution_payout = position.resolution_payout_per_share * quantity;
        let observed_net = actual_resolution_payout - actual_fill_cost;
        let execution_slippage = actual_fill_cost - modeled_fill_cost;
        let execution_slippage_bps = if modeled_fill_cost.is_zero() {
            Decimal::ZERO
        } else {
            (execution_slippage / modeled_fill_cost) * Decimal::new(10_000, 0)
        };
        telemetry.request_to_fill_ms = Some(process_started_at.elapsed().as_millis() as i64);

        self.trade_event_recorder
            .record_warn({
                let metadata = merge_metadata(
                    serde_json::json!({
                        "yes_fill_price": yes_report.average_price.to_string(),
                        "no_fill_price": no_report.average_price.to_string(),
                        "quantity": quantity.to_string(),
                        "modeled_fill_cost": modeled_fill_cost.to_string(),
                        "actual_fill_cost": actual_fill_cost.to_string(),
                        "execution_slippage": execution_slippage.to_string(),
                        "execution_slippage_bps": execution_slippage_bps.to_string(),
                    }),
                    &telemetry,
                );
                NewTradeEvent::new(
                    "arb",
                    execution_mode,
                    "arb",
                    market_id.clone(),
                    "entry_filled",
                )
                .with_position(position.id)
                .with_state(Some("pending"), Some("pending"))
                .with_expected_edge(expected_net)
                .with_observed_edge(observed_net)
                .with_requested_size(position_size)
                .with_filled_size(actual_fill_cost)
                .with_fill_price(
                    (yes_report.average_price + no_report.average_price) / Decimal::new(2, 0),
                )
                .with_metadata(metadata)
            })
            .await;

        // 13. Both filled → mark position OPEN
        if let Err(e) = position.mark_open() {
            error!(error = %e, "Failed to transition position to OPEN");
        }
        if let Err(e) = self.position_repo.update(&position).await {
            error!(error = %e, "Failed to update position to OPEN");
        }
        telemetry.request_to_open_ms = Some(process_started_at.elapsed().as_millis() as i64);
        telemetry.finish_total(&process_started_at);

        self.trade_event_recorder
            .record_warn({
                let metadata = merge_metadata(
                    serde_json::json!({
                        "quantity": quantity.to_string(),
                        "total_cost": arb.total_cost.to_string(),
                        "modeled_fill_cost": modeled_fill_cost.to_string(),
                        "actual_fill_cost": actual_fill_cost.to_string(),
                        "execution_slippage": execution_slippage.to_string(),
                        "execution_slippage_bps": execution_slippage_bps.to_string(),
                    }),
                    &telemetry,
                );
                NewTradeEvent::new(
                    "arb",
                    execution_mode,
                    "arb",
                    market_id.clone(),
                    "position_open",
                )
                .with_position(position.id)
                .with_state(Some("pending"), Some("open"))
                .with_expected_edge(expected_net)
                .with_observed_edge(observed_net)
                .with_requested_size(position_size)
                .with_filled_size(actual_fill_cost)
                .with_metadata(metadata)
            })
            .await;

        // Add to dedup set
        self.active_markets.write().await.insert(market_id.clone());

        // 14. Publish success signal
        let estimated_pnl = arb.net_profit * quantity;
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: market_id.clone(),
            outcome_id: "both".to_string(),
            action: "executed".to_string(),
            confidence: 1.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "position_id": position.id.to_string(),
                "quantity": quantity.to_string(),
                "total_cost": arb.total_cost.to_string(),
                "net_profit": arb.net_profit.to_string(),
                "estimated_pnl": estimated_pnl.to_string(),
                "yes_price": arb.yes_ask.to_string(),
                "no_price": arb.no_ask.to_string(),
                "exit_strategy": "hold_to_resolution",
            }),
        };
        let _ = self.signal_tx.send(signal);

        {
            let mut runtime = self.runtime_status.write().await;
            runtime.executed = runtime.executed.saturating_add(1);
            runtime.record_decision(
                market_id,
                format!(
                    "executed: position={} estimated_pnl={}",
                    position.id, estimated_pnl
                ),
            );
        }
        info!(
            market_id = %market_id,
            position_id = %position.id,
            quantity = %quantity,
            estimated_pnl = %estimated_pnl,
            "Arb position OPENED successfully"
        );

        Ok(())
    }

    /// Publish a failure signal to WebSocket clients.
    fn publish_failure_signal(&self, market_id: &str, reason: &str) {
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::Arbitrage,
            market_id: market_id.to_string(),
            outcome_id: "both".to_string(),
            action: "execution_failed".to_string(),
            confidence: 0.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "reason": reason,
            }),
        };
        let _ = self.signal_tx.send(signal);
    }

    async fn record_skip_event(
        &self,
        arb: &ArbOpportunity,
        execution_mode: &str,
        reason: &str,
        telemetry: &ArbExecutionTelemetry,
    ) {
        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    "arb",
                    execution_mode.to_string(),
                    "arb",
                    arb.market_id.clone(),
                    "signal_skipped",
                )
                .with_reason(Some(reason))
                .with_expected_edge(arb.net_profit)
                .with_observed_edge(arb.gross_profit)
                .with_metadata(merge_metadata(
                    serde_json::json!({
                        "yes_ask": arb.yes_ask.to_string(),
                        "no_ask": arb.no_ask.to_string(),
                        "total_cost": arb.total_cost.to_string(),
                    }),
                    telemetry,
                )),
            )
            .await;
    }

    async fn record_failure_event(
        &self,
        execution_mode: &str,
        market_id: &str,
        position: &Position,
        reason: &str,
        telemetry: &ArbExecutionTelemetry,
    ) {
        self.trade_event_recorder
            .record_warn(
                NewTradeEvent::new(
                    "arb",
                    execution_mode.to_string(),
                    "arb",
                    market_id.to_string(),
                    "position_failed",
                )
                .with_position(position.id)
                .with_state(Some("pending"), Some("entry_failed"))
                .with_reason(Some(reason))
                .with_unrealized_pnl(position.unrealized_pnl)
                .with_metadata(merge_metadata(serde_json::json!({}), telemetry)),
            )
            .await;
    }
}

/// Spawn the arb auto-executor as a background task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_arb_auto_executor(
    config: Arc<RwLock<ArbExecutorConfig>>,
    arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    trade_event_tx: broadcast::Sender<crate::trade_events::TradeEventUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    active_markets: Arc<RwLock<HashSet<String>>>,
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    heartbeat: Arc<AtomicI64>,
    runtime_status: Arc<RwLock<ArbExecutorRuntimeStatus>>,
) {
    let executor = ArbAutoExecutor::new(
        config,
        arb_entry_rx,
        signal_tx,
        trade_event_tx,
        order_executor,
        circuit_breaker,
        clob_client,
        pool,
        active_markets,
        active_clob_markets,
        heartbeat,
        runtime_status,
    );

    tokio::spawn(async move {
        if let Err(e) = executor.run().await {
            error!(error = %e, "Arb auto-executor failed");
        }
    });

    info!("Arb auto-executor spawned as background task");
}

trait ArbTradeEventExt {
    fn with_position(self, position_id: uuid::Uuid) -> Self;
    fn with_state(self, from: Option<&str>, to: Option<&str>) -> Self;
    fn with_reason(self, reason: Option<&str>) -> Self;
    fn with_expected_edge(self, edge: Decimal) -> Self;
    fn with_observed_edge(self, edge: Decimal) -> Self;
    fn with_requested_size(self, size: Decimal) -> Self;
    fn with_filled_size(self, size: Decimal) -> Self;
    fn with_fill_price(self, price: Decimal) -> Self;
    fn with_unrealized_pnl(self, pnl: Decimal) -> Self;
    fn with_metadata(self, metadata: serde_json::Value) -> Self;
}

impl ArbTradeEventExt for NewTradeEvent {
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

    fn with_expected_edge(mut self, edge: Decimal) -> Self {
        self.expected_edge = Some(edge);
        self
    }

    fn with_observed_edge(mut self, edge: Decimal) -> Self {
        self.observed_edge = Some(edge);
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
    fn test_config_default() {
        let config = ArbExecutorConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.position_size, Decimal::new(50, 0));
        assert_eq!(config.min_net_profit, Decimal::new(1, 3));
        assert_eq!(config.max_signal_age_secs, 30);
        assert_eq!(config.cache_refresh_secs, 300);
        assert!(config.dynamic_sizing);
        assert_eq!(config.min_position_size, Decimal::new(25, 0));
        assert_eq!(config.max_position_size, Decimal::new(200, 0));
        assert_eq!(config.min_book_depth, Decimal::new(100, 0));
        assert_eq!(config.fee_rate, Decimal::new(2, 2));
    }

    #[test]
    fn test_quantity_calculation() {
        // position_size=50, total_cost=0.94 → ~53.19 shares
        let position_size = Decimal::new(50, 0);
        let total_cost = Decimal::new(94, 2);
        let quantity = position_size / total_cost;
        // 50 / 0.94 ≈ 53.191...
        assert!(quantity > Decimal::new(53, 0));
        assert!(quantity < Decimal::new(54, 0));
    }
}
