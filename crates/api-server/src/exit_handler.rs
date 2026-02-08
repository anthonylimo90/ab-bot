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
use polymarket_core::api::ClobClient;
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::{FailureReason, MarketOrder, OrderSide, Position};
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;

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
}

impl Default for ExitHandlerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            exit_poll_interval_secs: 30,
            resolution_check_secs: 300,
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

    async fn get(&self, market_id: &str) -> Option<(String, String)> {
        self.tokens.read().await.get(market_id).cloned()
    }
}

/// Exit handler service — closes positions via sell orders or resolution detection.
pub struct ExitHandler {
    config: ExitHandlerConfig,
    position_repo: PositionRepository,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    token_cache: OutcomeTokenCache,
    /// Shared dedup set with ArbAutoExecutor.
    arb_dedup: Arc<RwLock<HashSet<String>>>,
}

impl ExitHandler {
    pub fn new(
        config: ExitHandlerConfig,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        pool: PgPool,
        arb_dedup: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            config,
            position_repo: PositionRepository::new(pool),
            order_executor,
            circuit_breaker,
            clob_client: clob_client.clone(),
            signal_tx,
            token_cache: OutcomeTokenCache::new(clob_client),
            arb_dedup,
        }
    }

    /// Main run loop with two tickers.
    pub async fn run(self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Exit handler is disabled (set EXIT_HANDLER_ENABLED=true to enable)");
            return Ok(());
        }

        info!(
            exit_poll_secs = self.config.exit_poll_interval_secs,
            resolution_check_secs = self.config.resolution_check_secs,
            "Starting exit handler"
        );

        // Initial token cache load
        match self.token_cache.refresh().await {
            Ok(count) => info!(markets = count, "Exit handler: token cache loaded"),
            Err(e) => warn!(error = %e, "Exit handler: failed to load token cache, will retry"),
        }

        let mut exit_ticker = tokio::time::interval(tokio::time::Duration::from_secs(
            self.config.exit_poll_interval_secs,
        ));
        let mut resolution_ticker = tokio::time::interval(tokio::time::Duration::from_secs(
            self.config.resolution_check_secs,
        ));

        // Skip the first immediate ticks
        exit_ticker.tick().await;
        resolution_ticker.tick().await;

        loop {
            tokio::select! {
                _ = exit_ticker.tick() => {
                    if let Err(e) = self.process_exit_ready().await {
                        error!(error = %e, "Failed to process exit-ready positions");
                    }
                    if let Err(e) = self.process_failed_exits().await {
                        error!(error = %e, "Failed to process failed exits");
                    }
                }
                _ = resolution_ticker.tick() => {
                    if let Err(e) = self.check_market_resolutions().await {
                        error!(error = %e, "Failed to check market resolutions");
                    }
                }
            }
        }
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

        // Mark Closing
        if let Err(e) = position.mark_closing() {
            anyhow::bail!("Cannot mark position closing: {}", e);
        }
        let _ = self.position_repo.update(position).await;

        // Resolve token IDs
        let (yes_token_id, no_token_id) = match self.token_cache.get(&market_id).await {
            Some(ids) => ids,
            None => {
                // Attempt refresh
                let _ = self.token_cache.refresh().await;
                match self.token_cache.get(&market_id).await {
                    Some(ids) => ids,
                    None => {
                        position.mark_exit_failed(FailureReason::ConnectivityError {
                            message: "No token IDs for market".to_string(),
                        });
                        let _ = self.position_repo.update(position).await;
                        self.publish_alert(&market_id, "exit_failed", "No token IDs for market");
                        return Ok(());
                    }
                }
            }
        };

        // Execute YES sell
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

        let yes_price = yes_report.average_price;

        // Execute NO sell
        let no_order = MarketOrder::new(
            market_id.clone(),
            no_token_id,
            OrderSide::Sell,
            position.quantity,
        );

        let no_report = match self.order_executor.execute_market_order(no_order).await {
            Ok(report) => report,
            Err(e) => {
                error!(error = %e, market_id = %market_id, "NO sell order error (YES already sold)");
                position.mark_exit_failed(FailureReason::ConnectivityError {
                    message: format!("NO sell error (YES sold at {yes_price}): {e}"),
                });
                let _ = self.position_repo.update(position).await;
                self.publish_alert(
                    &market_id,
                    "exit_failed",
                    "NO sell order error (one-legged)",
                );
                return Ok(());
            }
        };

        if !no_report.is_success() {
            let msg = no_report
                .error_message
                .unwrap_or_else(|| "NO sell not filled".to_string());
            position.mark_exit_failed(FailureReason::OrderRejected {
                message: format!("NO sell failed (YES sold at {yes_price}): {msg}"),
            });
            let _ = self.position_repo.update(position).await;
            self.publish_alert(
                &market_id,
                "exit_failed",
                "NO sell order rejected (one-legged)",
            );
            return Ok(());
        }

        let no_price = no_report.average_price;
        let fee = Decimal::new(2, 2); // 2%

        // Close position
        position.close_via_exit(yes_price, no_price, fee);
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

    /// Check for HoldToResolution positions whose markets have resolved.
    async fn check_market_resolutions(&self) -> anyhow::Result<()> {
        let positions = self.position_repo.get_hold_to_resolution().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Checking market resolutions");

        // Collect unique market IDs
        let market_ids: HashSet<String> = positions.iter().map(|p| p.market_id.clone()).collect();

        // Fetch markets from CLOB
        let markets = match self.clob_client.get_markets().await {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "Failed to fetch markets for resolution check");
                return Ok(());
            }
        };

        // Find resolved markets that match our positions
        let resolved_market_ids: HashSet<String> = markets
            .iter()
            .filter(|m| m.resolved && market_ids.contains(&m.id))
            .map(|m| m.id.clone())
            .collect();

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

            position.close_via_resolution(fee);
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

/// Spawn the exit handler as a background task.
pub fn spawn_exit_handler(
    config: ExitHandlerConfig,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    pool: PgPool,
    arb_dedup: Arc<RwLock<HashSet<String>>>,
) {
    let handler = ExitHandler::new(
        config,
        order_executor,
        circuit_breaker,
        clob_client,
        signal_tx,
        pool,
        arb_dedup,
    );

    tokio::spawn(async move {
        if let Err(e) = handler.run().await {
            error!(error = %e, "Exit handler failed");
        }
    });

    info!("Exit handler spawned as background task");
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
}
