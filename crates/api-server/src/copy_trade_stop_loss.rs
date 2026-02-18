//! Automatic stop-loss and mirror-exit monitor for copy trade positions.
//!
//! Runs as a background task that:
//! 1. Periodically checks open copy-trade positions against stop-loss thresholds.
//! 2. Detects when a copied wallet sells (mirror exit) and closes our position.
//! 3. Executes sell orders to close losing or mirror-exited positions.
//! 4. Records realized P&L with the circuit breaker.

use chrono::Utc;
use polymarket_core::api::ClobClient;
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::copy_trader::CopyTrader;
use trading_engine::OrderExecutor;

use polymarket_core::types::{MarketOrder, OrderSide};
use wallet_tracker::trade_monitor::TradeMonitor;

use crate::websocket::{SignalType, SignalUpdate};

/// Configuration for the copy-trade stop-loss monitor.
#[derive(Debug, Clone)]
pub struct CopyStopLossConfig {
    /// Whether the monitor is enabled.
    pub enabled: bool,
    /// How often to check positions (seconds).
    pub check_interval_secs: u64,
    /// Stop-loss percentage (e.g. 0.15 = close if down 15% from entry).
    pub stop_loss_pct: Decimal,
    /// Take-profit percentage (e.g. 0.25 = close if up 25%).
    pub take_profit_pct: Decimal,
    /// Maximum hold time in hours before force-closing.
    pub max_hold_hours: i64,
    /// Whether to mirror exits from copied wallets.
    pub mirror_exits: bool,
}

impl Default for CopyStopLossConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_secs: 30,
            stop_loss_pct: Decimal::new(15, 2),   // 15%
            take_profit_pct: Decimal::new(25, 2), // 25%
            max_hold_hours: 72,
            mirror_exits: true,
        }
    }
}

impl CopyStopLossConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("COPY_STOP_LOSS_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            check_interval_secs: std::env::var("COPY_STOP_LOSS_CHECK_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
            stop_loss_pct: std::env::var("COPY_STOP_LOSS_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(15, 2)),
            take_profit_pct: std::env::var("COPY_TAKE_PROFIT_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(25, 2)),
            max_hold_hours: std::env::var("COPY_MAX_HOLD_HOURS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(72),
            mirror_exits: std::env::var("COPY_MIRROR_EXITS")
                .map(|v| v != "false")
                .unwrap_or(true),
        }
    }
}

/// A copy trade position loaded from the database for monitoring.
#[derive(Debug, Clone)]
struct CopyPosition {
    id: uuid::Uuid,
    market_id: String,
    outcome: String,
    quantity: Decimal,
    entry_price: Decimal,
    source_wallet: Option<String>,
    opened_at: chrono::DateTime<Utc>,
}

/// Background service that monitors copy trade positions.
pub struct CopyStopLossMonitor {
    config: CopyStopLossConfig,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    trade_monitor: Option<Arc<TradeMonitor>>,
    signal_tx: broadcast::Sender<SignalUpdate>,
}

impl CopyStopLossMonitor {
    pub fn new(
        config: CopyStopLossConfig,
        pool: PgPool,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        copy_trader: Arc<RwLock<CopyTrader>>,
        trade_monitor: Option<Arc<TradeMonitor>>,
        signal_tx: broadcast::Sender<SignalUpdate>,
    ) -> Self {
        Self {
            config,
            pool,
            order_executor,
            circuit_breaker,
            clob_client,
            copy_trader,
            trade_monitor,
            signal_tx,
        }
    }

    /// Main run loop.
    pub async fn run(self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Copy trade stop-loss monitor is disabled");
            return Ok(());
        }

        info!(
            stop_loss_pct = %self.config.stop_loss_pct,
            take_profit_pct = %self.config.take_profit_pct,
            max_hold_hours = self.config.max_hold_hours,
            mirror_exits = self.config.mirror_exits,
            "Starting copy trade stop-loss monitor"
        );

        // Subscribe to wallet trade feed for mirror exits
        let mut mirror_rx = self.trade_monitor.as_ref().map(|tm| tm.subscribe());

        let check_interval = tokio::time::Duration::from_secs(self.config.check_interval_secs);
        let mut check_ticker = tokio::time::interval(check_interval);

        loop {
            tokio::select! {
                _ = check_ticker.tick() => {
                    if let Err(e) = self.check_positions().await {
                        error!(error = %e, "Failed to check copy trade stop-losses");
                    }
                }
                // Mirror exit: if a copied wallet sells, close our position
                result = async {
                    match mirror_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match result {
                        Ok(wallet_trade) => {
                            if self.config.mirror_exits
                                && wallet_trade.direction == wallet_tracker::trade_monitor::TradeDirection::Sell
                            {
                                if let Err(e) = self.handle_mirror_exit(&wallet_trade).await {
                                    error!(error = %e, "Failed to handle mirror exit");
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "Stop-loss monitor lagged on mirror feed");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Mirror exit feed closed");
                            mirror_rx = None;
                        }
                    }
                }
            }
        }
    }

    /// Check all open copy-trade positions for stop-loss, take-profit, and time exits.
    async fn check_positions(&self) -> anyhow::Result<()> {
        let positions = self.load_open_copy_positions().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Checking copy trade positions");

        // Fetch current prices keyed by (market_id, outcome) to handle
        // multiple outcomes in the same market correctly
        let mut outcome_prices: HashMap<(String, String), Decimal> = HashMap::new();
        for pos in &positions {
            let key = (pos.market_id.clone(), pos.outcome.clone());
            if outcome_prices.contains_key(&key) {
                continue;
            }
            match self.fetch_current_price(&pos.market_id, &pos.outcome).await {
                Ok(price) => {
                    outcome_prices.insert(key, price);
                }
                Err(e) => {
                    warn!(
                        market_id = %pos.market_id,
                        outcome = %pos.outcome,
                        error = %e,
                        "Failed to fetch price for stop-loss check"
                    );
                }
            }
        }

        let now = Utc::now();

        for pos in &positions {
            let current_price =
                match outcome_prices.get(&(pos.market_id.clone(), pos.outcome.clone())) {
                    Some(p) => *p,
                    None => continue,
                };

            let pnl_pct = if pos.entry_price > Decimal::ZERO {
                (current_price - pos.entry_price) / pos.entry_price
            } else {
                Decimal::ZERO
            };

            let hold_hours = now.signed_duration_since(pos.opened_at).num_hours();

            // Stop-loss: close if loss exceeds threshold
            if pnl_pct < Decimal::ZERO && pnl_pct.abs() >= self.config.stop_loss_pct {
                info!(
                    position_id = %pos.id,
                    market = %pos.market_id,
                    pnl_pct = %pnl_pct,
                    "Stop-loss triggered for copy trade"
                );
                self.close_position(pos, "stop_loss").await;
                continue;
            }

            // Take-profit: close if profit exceeds threshold
            if pnl_pct > Decimal::ZERO && pnl_pct >= self.config.take_profit_pct {
                info!(
                    position_id = %pos.id,
                    market = %pos.market_id,
                    pnl_pct = %pnl_pct,
                    "Take-profit triggered for copy trade"
                );
                self.close_position(pos, "take_profit").await;
                continue;
            }

            // Time-based exit: close if held too long
            if hold_hours >= self.config.max_hold_hours {
                warn!(
                    position_id = %pos.id,
                    market = %pos.market_id,
                    hold_hours = hold_hours,
                    "Max hold time exceeded for copy trade"
                );
                self.close_position(pos, "time_exit").await;
            }
        }

        Ok(())
    }

    /// Handle a mirror exit: if the source wallet sells a token we hold, close our position.
    async fn handle_mirror_exit(
        &self,
        wallet_trade: &wallet_tracker::trade_monitor::WalletTrade,
    ) -> anyhow::Result<()> {
        // Find open copy positions from this wallet in this market
        let rows =
            sqlx::query_as::<_, (uuid::Uuid, String, Decimal, Decimal, chrono::DateTime<Utc>)>(
                r#"
            SELECT id, outcome, quantity, entry_price, opened_at
            FROM positions
            WHERE is_copy_trade = true
              AND source_wallet = $1
              AND market_id = $2
              AND is_open = true
            "#,
            )
            .bind(&wallet_trade.wallet_address)
            .bind(&wallet_trade.market_id)
            .fetch_all(&self.pool)
            .await?;

        for (id, outcome, quantity, entry_price, opened_at) in rows {
            info!(
                position_id = %id,
                source_wallet = %wallet_trade.wallet_address,
                market = %wallet_trade.market_id,
                "Mirror exit: source wallet sold, closing our copy position"
            );

            let pos = CopyPosition {
                id,
                market_id: wallet_trade.market_id.clone(),
                outcome,
                quantity,
                entry_price,
                source_wallet: Some(wallet_trade.wallet_address.clone()),
                opened_at,
            };

            self.close_position(&pos, "mirror_exit").await;
        }

        Ok(())
    }

    /// Close a copy trade position by placing a sell order.
    ///
    /// Uses an atomic claim pattern: sets `state = 3` (Closing) with a
    /// `WHERE is_open = true` guard so only one concurrent caller wins.
    async fn close_position(&self, pos: &CopyPosition, reason: &str) {
        // Atomically claim the position — transition to Closing.
        // If another task already claimed it, this returns 0 rows and we skip.
        let claimed = match sqlx::query_scalar::<_, uuid::Uuid>(
            "UPDATE positions SET state = 3, is_open = false WHERE id = $1 AND is_open = true RETURNING id",
        )
        .bind(pos.id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(Some(_)) => true,
            Ok(None) => {
                debug!(position_id = %pos.id, "Position already claimed by another task, skipping");
                return;
            }
            Err(e) => {
                error!(error = %e, position_id = %pos.id, "Failed to claim position for closing");
                return;
            }
        };

        debug_assert!(claimed);

        // Place sell order
        let order = MarketOrder::new(
            pos.market_id.clone(),
            pos.outcome.clone(),
            OrderSide::Sell,
            pos.quantity,
        );

        match self.order_executor.execute_market_order(order).await {
            Ok(report) if report.is_success() => {
                let actual_pnl = (report.average_price - pos.entry_price) * report.filled_quantity
                    - report.fees_paid;

                // Finalize: mark as Closed (state = 4)
                if let Err(e) = sqlx::query(
                    "UPDATE positions SET realized_pnl = $1, exit_timestamp = NOW(), state = 4 WHERE id = $2",
                )
                .bind(actual_pnl)
                .bind(pos.id)
                .execute(&self.pool)
                .await
                {
                    error!(error = %e, position_id = %pos.id, "Failed to finalize closed position");
                }

                // Record with circuit breaker
                let is_win = actual_pnl >= Decimal::ZERO;
                if let Err(e) = self.circuit_breaker.record_trade(actual_pnl, is_win).await {
                    warn!(error = %e, "Failed to record exit with circuit breaker");
                }

                // Decrement open position count
                {
                    let mut ct = self.copy_trader.write().await;
                    ct.record_position_closed();
                }

                info!(
                    position_id = %pos.id,
                    reason = reason,
                    realized_pnl = %actual_pnl,
                    "Copy trade position closed"
                );

                // Publish signal
                let signal = SignalUpdate {
                    signal_id: uuid::Uuid::new_v4(),
                    signal_type: SignalType::CopyTrade,
                    market_id: pos.market_id.clone(),
                    outcome_id: pos.outcome.clone(),
                    action: "closed".to_string(),
                    confidence: 1.0,
                    timestamp: Utc::now(),
                    metadata: serde_json::json!({
                        "position_id": pos.id.to_string(),
                        "reason": reason,
                        "realized_pnl": actual_pnl.to_string(),
                        "entry_price": pos.entry_price.to_string(),
                        "exit_price": report.average_price.to_string(),
                    }),
                };
                let _ = self.signal_tx.send(signal);
            }
            Ok(report) => {
                // Sell failed — revert the claim so the position can be retried
                warn!(
                    position_id = %pos.id,
                    status = ?report.status,
                    error = ?report.error_message,
                    "Exit order failed for copy position, reverting claim"
                );
                let _ = sqlx::query("UPDATE positions SET state = 1, is_open = true WHERE id = $1")
                    .bind(pos.id)
                    .execute(&self.pool)
                    .await;
            }
            Err(e) => {
                // Execution error — revert the claim so the position can be retried
                error!(
                    position_id = %pos.id,
                    error = %e,
                    "Failed to execute exit order for copy position, reverting claim"
                );
                let _ = sqlx::query("UPDATE positions SET state = 1, is_open = true WHERE id = $1")
                    .bind(pos.id)
                    .execute(&self.pool)
                    .await;
            }
        }
    }

    /// Load open copy trade positions from the database.
    async fn load_open_copy_positions(&self) -> anyhow::Result<Vec<CopyPosition>> {
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                String,
                String,
                Decimal,
                Decimal,
                Option<String>,
                Option<chrono::DateTime<Utc>>,
                chrono::DateTime<Utc>,
            ),
        >(
            r#"
            SELECT id, market_id, outcome, quantity, entry_price, source_wallet, opened_at, entry_timestamp
            FROM positions
            WHERE is_copy_trade = true AND is_open = true
            ORDER BY COALESCE(opened_at, entry_timestamp) ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    market_id,
                    outcome,
                    quantity,
                    entry_price,
                    source_wallet,
                    opened_at,
                    entry_timestamp,
                )| {
                    CopyPosition {
                        id,
                        market_id,
                        outcome,
                        quantity,
                        entry_price,
                        source_wallet,
                        opened_at: opened_at.unwrap_or(entry_timestamp),
                    }
                },
            )
            .collect())
    }

    /// Fetch the current best bid price for an outcome.
    async fn fetch_current_price(
        &self,
        _market_id: &str,
        outcome_id: &str,
    ) -> anyhow::Result<Decimal> {
        let book = self.clob_client.get_order_book(outcome_id).await?;
        book.bids
            .first()
            .map(|l| l.price)
            .ok_or_else(|| anyhow::anyhow!("No bids available"))
    }
}

/// Spawn the copy-trade stop-loss monitor as a background task.
pub fn spawn_copy_stop_loss_monitor(
    config: CopyStopLossConfig,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    trade_monitor: Option<Arc<TradeMonitor>>,
    signal_tx: broadcast::Sender<SignalUpdate>,
) {
    let monitor = CopyStopLossMonitor::new(
        config,
        pool,
        order_executor,
        circuit_breaker,
        clob_client,
        copy_trader,
        trade_monitor,
        signal_tx,
    );

    tokio::spawn(async move {
        if let Err(e) = monitor.run().await {
            error!(error = %e, "Copy trade stop-loss monitor failed");
        }
    });

    info!("Copy trade stop-loss monitor spawned as background task");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = CopyStopLossConfig::default();
        assert!(config.enabled);
        assert_eq!(config.stop_loss_pct, Decimal::new(15, 2));
        assert_eq!(config.take_profit_pct, Decimal::new(25, 2));
        assert_eq!(config.max_hold_hours, 72);
        assert!(config.mirror_exits);
    }
}
