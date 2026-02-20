//! Arb auto-executor — subscribes to arb entry signals and executes trades.
//!
//! Bridges the gap between arb detection (arb-monitor → Redis → RedisForwarder)
//! and actual order execution. Receives `ArbOpportunity` signals via a broadcast
//! channel, resolves outcome token IDs, checks the circuit breaker, and places
//! sequential YES + NO market orders.

use chrono::Utc;
use polymarket_core::api::ClobClient;
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::{
    ArbOpportunity, ExitStrategy, FailureReason, MarketOrder, OrderSide, Position,
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
                .unwrap_or(Decimal::new(25, 0)),
            max_position_size: std::env::var("ARB_MAX_POSITION_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(200, 0)),
            min_book_depth: std::env::var("ARB_MIN_BOOK_DEPTH")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(100, 0)),
            fee_rate: Decimal::new(2, 2), // Always 2% on Polymarket
        }
    }
}

/// Cached mapping of market_id → (yes_token_id, no_token_id).
struct OutcomeTokenCache {
    clob_client: Arc<ClobClient>,
    tokens: RwLock<HashMap<String, (String, String)>>,
    /// Shared set of active (non-resolved) market IDs, populated on each refresh.
    /// Copy trading monitor reads this to skip resolved markets before hitting CLOB.
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
}

impl OutcomeTokenCache {
    fn new(
        clob_client: Arc<ClobClient>,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            clob_client,
            tokens: RwLock::new(HashMap::new()),
            active_clob_markets,
        }
    }

    /// Refresh the cache by fetching all markets from the CLOB API.
    async fn refresh(&self) -> anyhow::Result<usize> {
        let markets = self.clob_client.get_markets().await?;
        let mut map = HashMap::new();
        let mut active_set = HashSet::new();

        for market in &markets {
            if !market.resolved {
                active_set.insert(market.id.clone());
                // Also insert outcome token IDs so trades whose market_id
                // fell back to asset_id (when condition_id is absent) still
                // match the active set.
                for outcome in &market.outcomes {
                    active_set.insert(outcome.token_id.clone());
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
        *self.tokens.write().await = map;
        *self.active_clob_markets.write().await = active_set;
        Ok(count)
    }

    /// Look up token IDs for a given market.
    async fn get(&self, market_id: &str) -> Option<(String, String)> {
        self.tokens.read().await.get(market_id).cloned()
    }
}

/// Arb auto-executor service.
pub struct ArbAutoExecutor {
    config: Arc<RwLock<ArbExecutorConfig>>,
    arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    position_repo: PositionRepository,
    token_cache: OutcomeTokenCache,
    /// Set of market IDs with active positions (for deduplication).
    /// Shared with ExitHandler so closed positions unblock their markets.
    active_markets: Arc<RwLock<HashSet<String>>>,
    /// Heartbeat timestamp (epoch secs) — updated every loop iteration to prove liveness.
    heartbeat: Arc<AtomicI64>,
}

impl ArbAutoExecutor {
    /// Create a new arb auto-executor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<RwLock<ArbExecutorConfig>>,
        arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        pool: PgPool,
        active_markets: Arc<RwLock<HashSet<String>>>,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
        heartbeat: Arc<AtomicI64>,
    ) -> Self {
        Self {
            config,
            arb_entry_rx,
            signal_tx,
            order_executor,
            circuit_breaker,
            position_repo: PositionRepository::new(pool),
            token_cache: OutcomeTokenCache::new(clob_client, active_clob_markets),
            active_markets,
            heartbeat,
        }
    }

    /// Snapshot the current config for use during a single tick/signal.
    async fn snapshot_config(&self) -> ArbExecutorConfig {
        self.config.read().await.clone()
    }

    /// Main run loop.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let startup_cfg = self.snapshot_config().await;

        info!(
            enabled = startup_cfg.enabled,
            position_size = %startup_cfg.position_size,
            min_net_profit = %startup_cfg.min_net_profit,
            max_signal_age_secs = startup_cfg.max_signal_age_secs,
            "Starting arb auto-executor (always-on, per-signal guard)"
        );

        // Initial token cache load
        match self.token_cache.refresh().await {
            Ok(count) => info!(markets = count, "Outcome token cache loaded"),
            Err(e) => warn!(error = %e, "Failed to load token cache, will retry"),
        }

        // Load active positions for dedup
        if let Ok(active) = self.position_repo.get_active().await {
            let mut set = self.active_markets.write().await;
            for pos in &active {
                set.insert(pos.market_id.clone());
            }
            info!(active_positions = %active.len(), "Loaded active positions for dedup");
        }

        let cache_interval = tokio::time::Duration::from_secs(startup_cfg.cache_refresh_secs);
        let mut cache_ticker = tokio::time::interval(cache_interval);
        // Skip the first immediate tick (we already loaded above)
        cache_ticker.tick().await;

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
                            warn!(skipped = n, "Arb executor lagged, skipped signals");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Arb entry channel closed, stopping executor");
                            break;
                        }
                    }
                }
                _ = cache_ticker.tick() => {
                    match self.token_cache.refresh().await {
                        Ok(count) => debug!(markets = count, "Token cache refreshed"),
                        Err(e) => warn!(error = %e, "Token cache refresh failed"),
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
        let cfg = self.snapshot_config().await;

        // Per-signal guard: skip processing when disabled at runtime
        if !cfg.enabled {
            return Ok(());
        }

        let market_id = &arb.market_id;

        // 1. Validate signal freshness
        let age_secs = Utc::now()
            .signed_duration_since(arb.timestamp)
            .num_seconds();
        if age_secs > cfg.max_signal_age_secs {
            info!(
                market_id = %market_id,
                age_secs = age_secs,
                max = cfg.max_signal_age_secs,
                "Arb signal too stale, skipping"
            );
            return Ok(());
        }

        // 2. Check minimum profit threshold
        if arb.net_profit < cfg.min_net_profit {
            info!(
                market_id = %market_id,
                net_profit = %arb.net_profit,
                min = %cfg.min_net_profit,
                "Arb signal below min profit threshold, skipping"
            );
            return Ok(());
        }

        // 3. Dedup — skip if we already have an active position in this market
        {
            let active = self.active_markets.read().await;
            if active.contains(market_id) {
                info!(market_id = %market_id, "Active position exists, skipping");
                return Ok(());
            }
        }

        // 4. Circuit breaker check
        if !self.circuit_breaker.can_trade().await {
            warn!(market_id = %market_id, "Circuit breaker tripped, skipping arb trade");
            return Ok(());
        }

        // 5. Resolve outcome token IDs from cache
        let (yes_token_id, no_token_id) = match self.token_cache.get(market_id).await {
            Some(ids) => ids,
            None => {
                warn!(
                    market_id = %market_id,
                    "No token IDs cached for market, attempting refresh"
                );
                // Try a single refresh and retry
                if let Err(e) = self.token_cache.refresh().await {
                    error!(error = %e, "Token cache refresh failed");
                    return Ok(());
                }
                match self.token_cache.get(market_id).await {
                    Some(ids) => ids,
                    None => {
                        warn!(market_id = %market_id, "Market not found after refresh, skipping");
                        return Ok(());
                    }
                }
            }
        };

        // 6. Market quality check — verify orderbook depth on both sides
        {
            let yes_book = self
                .order_executor
                .clob_client()
                .get_order_book(&yes_token_id)
                .await;
            let no_book = self
                .order_executor
                .clob_client()
                .get_order_book(&no_token_id)
                .await;

            if let (Ok(yb), Ok(nb)) = (&yes_book, &no_book) {
                let yes_depth: Decimal = yb.asks.iter().map(|l| l.price * l.size).sum();
                let no_depth: Decimal = nb.asks.iter().map(|l| l.price * l.size).sum();

                if yes_depth < cfg.min_book_depth || no_depth < cfg.min_book_depth {
                    info!(
                        market_id = %market_id,
                        yes_depth = %yes_depth,
                        no_depth = %no_depth,
                        min_depth = %cfg.min_book_depth,
                        "Arb signal insufficient orderbook depth, skipping"
                    );
                    return Ok(());
                }
            }
        }

        // 7. Dynamic position sizing based on spread width
        let position_size = if cfg.dynamic_sizing {
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

        if arb.total_cost.is_zero() {
            warn!(market_id = %market_id, "Zero total_cost, skipping");
            return Ok(());
        }
        let quantity = position_size / arb.total_cost;

        // Fee drag tracking: log the expected fees vs expected profit
        let expected_fees = quantity * arb.total_cost * cfg.fee_rate;
        let expected_gross = arb.gross_profit * quantity;
        let expected_net = arb.net_profit * quantity;

        info!(
            market_id = %market_id,
            net_profit_per_share = %arb.net_profit,
            total_cost = %arb.total_cost,
            position_size = %position_size,
            quantity = %quantity,
            expected_fees = %expected_fees,
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

        if let Err(e) = self.position_repo.insert(&position).await {
            error!(error = %e, "Failed to persist pending position");
            return Err(anyhow::anyhow!("Failed to persist position: {e}"));
        }

        // 9. Execute YES market order
        let yes_order = MarketOrder::new(market_id.clone(), yes_token_id, OrderSide::Buy, quantity);

        let yes_report = match self.order_executor.execute_market_order(yes_order).await {
            Ok(report) => report,
            Err(e) => {
                error!(error = %e, market_id = %market_id, "YES order execution error");
                position.mark_entry_failed(FailureReason::ConnectivityError {
                    message: format!("YES order error: {e}"),
                });
                let _ = self.position_repo.update(&position).await;
                self.publish_failure_signal(market_id, "YES order execution error");
                return Ok(());
            }
        };

        // 10. If YES order rejected/failed → mark EntryFailed
        if !yes_report.is_success() {
            let msg = yes_report
                .error_message
                .unwrap_or_else(|| "YES order not filled".to_string());
            warn!(market_id = %market_id, reason = %msg, "YES order failed");
            position.mark_entry_failed(FailureReason::OrderRejected { message: msg });
            let _ = self.position_repo.update(&position).await;
            self.publish_failure_signal(market_id, "YES order rejected");
            return Ok(());
        }

        // 11. Execute NO market order
        let no_order = MarketOrder::new(market_id.clone(), no_token_id, OrderSide::Buy, quantity);

        let no_report = match self.order_executor.execute_market_order(no_order).await {
            Ok(report) => report,
            Err(e) => {
                error!(error = %e, market_id = %market_id, "NO order execution error (one-legged)");
                position.mark_entry_failed(FailureReason::ConnectivityError {
                    message: format!("NO order error (one-legged, YES filled): {e}"),
                });
                let _ = self.position_repo.update(&position).await;
                self.publish_failure_signal(market_id, "NO order error (one-legged)");
                return Ok(());
            }
        };

        // 12. If NO order failed → one-legged position
        if !no_report.is_success() {
            let msg = no_report
                .error_message
                .unwrap_or_else(|| "NO order not filled".to_string());
            warn!(
                market_id = %market_id,
                reason = %msg,
                "NO order failed (one-legged, YES filled — flagged for review)"
            );
            position.mark_entry_failed(FailureReason::OrderRejected {
                message: format!("One-legged: YES filled but NO failed: {msg}"),
            });
            let _ = self.position_repo.update(&position).await;
            self.publish_failure_signal(market_id, "One-legged position (NO failed)");
            return Ok(());
        }

        // 13. Both filled → mark position OPEN
        if let Err(e) = position.mark_open() {
            error!(error = %e, "Failed to transition position to OPEN");
        }
        if let Err(e) = self.position_repo.update(&position).await {
            error!(error = %e, "Failed to update position to OPEN");
        }

        // Add to dedup set
        self.active_markets.write().await.insert(market_id.clone());

        // 14. Record trade with circuit breaker (estimated profit)
        let estimated_pnl = arb.net_profit * quantity;
        if let Err(e) = self.circuit_breaker.record_trade(estimated_pnl, true).await {
            warn!(error = %e, "Failed to record trade with circuit breaker");
        }

        // 15. Publish success signal
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
}

/// Spawn the arb auto-executor as a background task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_arb_auto_executor(
    config: Arc<RwLock<ArbExecutorConfig>>,
    arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    active_markets: Arc<RwLock<HashSet<String>>>,
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    heartbeat: Arc<AtomicI64>,
) {
    let executor = ArbAutoExecutor::new(
        config,
        arb_entry_rx,
        signal_tx,
        order_executor,
        circuit_breaker,
        clob_client,
        pool,
        active_markets,
        active_clob_markets,
        heartbeat,
    );

    tokio::spawn(async move {
        if let Err(e) = executor.run().await {
            error!(error = %e, "Arb auto-executor failed");
        }
    });

    info!("Arb auto-executor spawned as background task");
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
