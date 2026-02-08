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
    /// Dollar amount per position (split across YES + NO).
    pub position_size: Decimal,
    /// Minimum net profit to consider a signal worth executing.
    pub min_net_profit: Decimal,
    /// Maximum age of a signal in seconds before it's considered stale.
    pub max_signal_age_secs: i64,
    /// How often to refresh the outcome token cache (seconds).
    pub cache_refresh_secs: u64,
}

impl Default for ArbExecutorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            position_size: Decimal::new(50, 0), // $50
            min_net_profit: Decimal::new(1, 3), // 0.001
            max_signal_age_secs: 30,
            cache_refresh_secs: 300,
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
        }
    }
}

/// Cached mapping of market_id → (yes_token_id, no_token_id).
struct OutcomeTokenCache {
    clob_client: Arc<ClobClient>,
    tokens: RwLock<HashMap<String, (String, String)>>,
}

impl OutcomeTokenCache {
    fn new(clob_client: Arc<ClobClient>) -> Self {
        Self {
            clob_client,
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Refresh the cache by fetching all markets from the CLOB API.
    async fn refresh(&self) -> anyhow::Result<usize> {
        let markets = self.clob_client.get_markets().await?;
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

    /// Look up token IDs for a given market.
    async fn get(&self, market_id: &str) -> Option<(String, String)> {
        self.tokens.read().await.get(market_id).cloned()
    }
}

/// Arb auto-executor service.
pub struct ArbAutoExecutor {
    config: ArbExecutorConfig,
    arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    position_repo: PositionRepository,
    token_cache: OutcomeTokenCache,
    /// Set of market IDs with active positions (for deduplication).
    /// Shared with ExitHandler so closed positions unblock their markets.
    active_markets: Arc<RwLock<HashSet<String>>>,
}

impl ArbAutoExecutor {
    /// Create a new arb auto-executor.
    pub fn new(
        config: ArbExecutorConfig,
        arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        pool: PgPool,
        active_markets: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            config,
            arb_entry_rx,
            signal_tx,
            order_executor,
            circuit_breaker,
            position_repo: PositionRepository::new(pool),
            token_cache: OutcomeTokenCache::new(clob_client),
            active_markets,
        }
    }

    /// Main run loop.
    pub async fn run(mut self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Arb auto-executor is disabled (set ARB_AUTO_EXECUTE=true to enable)");
            return Ok(());
        }

        info!(
            position_size = %self.config.position_size,
            min_net_profit = %self.config.min_net_profit,
            max_signal_age_secs = self.config.max_signal_age_secs,
            "Starting arb auto-executor"
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

        let cache_interval = tokio::time::Duration::from_secs(self.config.cache_refresh_secs);
        let mut cache_ticker = tokio::time::interval(cache_interval);
        // Skip the first immediate tick (we already loaded above)
        cache_ticker.tick().await;

        loop {
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
            }
        }

        Ok(())
    }

    /// Process a single arb signal through validation → execution → tracking.
    async fn process_arb_signal(&self, arb: ArbOpportunity) -> anyhow::Result<()> {
        let market_id = &arb.market_id;

        // 1. Validate signal freshness
        let age_secs = Utc::now()
            .signed_duration_since(arb.timestamp)
            .num_seconds();
        if age_secs > self.config.max_signal_age_secs {
            debug!(
                market_id = %market_id,
                age_secs = age_secs,
                "Stale arb signal, skipping"
            );
            return Ok(());
        }

        // 2. Check minimum profit threshold
        if arb.net_profit < self.config.min_net_profit {
            debug!(
                market_id = %market_id,
                net_profit = %arb.net_profit,
                min = %self.config.min_net_profit,
                "Below min profit threshold, skipping"
            );
            return Ok(());
        }

        // 3. Dedup — skip if we already have an active position in this market
        {
            let active = self.active_markets.read().await;
            if active.contains(market_id) {
                debug!(market_id = %market_id, "Active position exists, skipping");
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

        // 6. Calculate quantity: position_size / total_cost
        let quantity = self.config.position_size / arb.total_cost;

        info!(
            market_id = %market_id,
            net_profit = %arb.net_profit,
            total_cost = %arb.total_cost,
            quantity = %quantity,
            "Executing arb trade"
        );

        // 7. Create position in PENDING state
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

        // 8. Execute YES market order
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

        // 9. If YES order rejected/failed → mark EntryFailed
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

        // 10. Execute NO market order
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

        // 11. If NO order failed → one-legged position
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

        // 12. Both filled → mark position OPEN
        if let Err(e) = position.mark_open() {
            error!(error = %e, "Failed to transition position to OPEN");
        }
        if let Err(e) = self.position_repo.update(&position).await {
            error!(error = %e, "Failed to update position to OPEN");
        }

        // Add to dedup set
        self.active_markets.write().await.insert(market_id.clone());

        // 13. Record trade with circuit breaker (estimated profit)
        let estimated_pnl = arb.net_profit * quantity;
        if let Err(e) = self.circuit_breaker.record_trade(estimated_pnl, true).await {
            warn!(error = %e, "Failed to record trade with circuit breaker");
        }

        // 14. Publish success signal
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
pub fn spawn_arb_auto_executor(
    config: ArbExecutorConfig,
    arb_entry_rx: broadcast::Receiver<ArbOpportunity>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    pool: PgPool,
    active_markets: Arc<RwLock<HashSet<String>>>,
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
