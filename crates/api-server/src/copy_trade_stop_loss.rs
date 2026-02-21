//! Automatic stop-loss and mirror-exit monitor for copy trade positions.
//!
//! Runs as a background task that:
//! 1. Periodically checks open copy-trade positions against stop-loss thresholds.
//! 2. Detects when a copied wallet sells (mirror exit) and closes our position.
//! 3. Executes sell orders to close losing or mirror-exited positions.
//! 4. Records realized P&L with the circuit breaker.

use chrono::Utc;
use polymarket_core::api::ClobClient;
use polymarket_core::Error as PolyError;
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info, warn};
use trading_engine::copy_trader::CopyTrader;
use trading_engine::OrderExecutor;

use crate::auto_optimizer::AutomationEvent;

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

/// Distinguishes recoverable vs terminal price-fetch failures.
enum PriceFetchError {
    /// The CLOB returned 404 — market is resolved, delisted, or the token ID is invalid.
    MarketNotFound,
    /// Order book exists but has no bids (illiquid market).
    NoBids,
    /// Transient / unexpected error.
    Other(anyhow::Error),
}

impl std::fmt::Display for PriceFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MarketNotFound => write!(f, "market not found (404)"),
            Self::NoBids => write!(f, "no bids available"),
            Self::Other(e) => write!(f, "{}", e),
        }
    }
}

/// How many consecutive 404s before we auto-close a position.
const MAX_NOT_FOUND_STRIKES: u32 = 5;

/// A copy trade position loaded from the database for monitoring.
#[derive(Debug, Clone)]
struct CopyPosition {
    id: uuid::Uuid,
    market_id: String,
    outcome: String,
    source_token_id: Option<String>,
    source_wallet: Option<String>,
    quantity: Decimal,
    entry_price: Decimal,
    opened_at: chrono::DateTime<Utc>,
}

impl CopyPosition {
    fn orderbook_token_id(&self) -> &str {
        self.source_token_id.as_deref().unwrap_or(&self.outcome)
    }
}

/// Background service that monitors copy trade positions.
pub struct CopyStopLossMonitor {
    config: Arc<RwLock<CopyStopLossConfig>>,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    trade_monitor: Option<Arc<TradeMonitor>>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    /// Sender for pushing position-close events to the auto-optimizer.
    event_tx: Option<mpsc::Sender<AutomationEvent>>,
    /// Tracks consecutive 404s per (market_id, token_id) so we can auto-close
    /// positions whose markets have been resolved or delisted.
    not_found_strikes: HashMap<(String, String), u32>,
}

impl CopyStopLossMonitor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<RwLock<CopyStopLossConfig>>,
        pool: PgPool,
        order_executor: Arc<OrderExecutor>,
        circuit_breaker: Arc<CircuitBreaker>,
        clob_client: Arc<ClobClient>,
        copy_trader: Arc<RwLock<CopyTrader>>,
        trade_monitor: Option<Arc<TradeMonitor>>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        event_tx: Option<mpsc::Sender<AutomationEvent>>,
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
            event_tx,
            not_found_strikes: HashMap::new(),
        }
    }

    /// Snapshot the current config from the shared Arc.
    async fn snapshot_config(&self) -> CopyStopLossConfig {
        self.config.read().await.clone()
    }

    /// Main run loop.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let initial_config = self.snapshot_config().await;
        if !initial_config.enabled {
            info!("Copy trade stop-loss monitor is disabled");
            return Ok(());
        }

        info!(
            stop_loss_pct = %initial_config.stop_loss_pct,
            take_profit_pct = %initial_config.take_profit_pct,
            max_hold_hours = initial_config.max_hold_hours,
            mirror_exits = initial_config.mirror_exits,
            "Starting copy trade stop-loss monitor"
        );

        // Subscribe to wallet trade feed for mirror exits
        let mut mirror_rx = self.trade_monitor.as_ref().map(|tm| tm.subscribe());

        let check_interval = tokio::time::Duration::from_secs(initial_config.check_interval_secs);
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
                            let cfg = self.snapshot_config().await;
                            if cfg.mirror_exits
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
    async fn check_positions(&mut self) -> anyhow::Result<()> {
        let cfg = self.snapshot_config().await;
        let positions = self.load_open_copy_positions().await?;
        if positions.is_empty() {
            return Ok(());
        }

        debug!(count = positions.len(), "Checking copy trade positions");

        /// Outcome of a price lookup for a (market, token) pair.
        enum PriceOutcome {
            /// We got a live price.
            Price(Decimal),
            /// The CLOB says this market/token no longer exists (404).
            NotFound,
            /// Transient error — skip this cycle but don't count a strike.
            Unavailable,
        }

        // Fetch current prices keyed by source token ID where available.
        let mut outcome_prices: HashMap<(String, String), PriceOutcome> = HashMap::new();
        for pos in &positions {
            let token_id = pos.orderbook_token_id().to_string();
            let key = (pos.market_id.clone(), token_id.clone());
            if outcome_prices.contains_key(&key) {
                continue;
            }
            match self.fetch_current_price(&pos.market_id, &token_id).await {
                Ok(price) => {
                    // Got a live price — clear any previous 404 strikes
                    self.not_found_strikes.remove(&key);
                    outcome_prices.insert(key, PriceOutcome::Price(price));
                }
                Err(PriceFetchError::MarketNotFound) => {
                    debug!(
                        market_id = %pos.market_id,
                        token_id = %token_id,
                        "Market/token not found on CLOB (404) — may be resolved or delisted"
                    );
                    outcome_prices.insert(key, PriceOutcome::NotFound);
                }
                Err(PriceFetchError::NoBids) => {
                    debug!(
                        market_id = %pos.market_id,
                        token_id = %token_id,
                        "No bids on order book — illiquid market"
                    );
                    outcome_prices.insert(key, PriceOutcome::Unavailable);
                }
                Err(e) => {
                    warn!(
                        market_id = %pos.market_id,
                        token_id = %token_id,
                        error = %e,
                        "Failed to fetch price for stop-loss check"
                    );
                    outcome_prices.insert(key, PriceOutcome::Unavailable);
                }
            }
        }

        let now = Utc::now();

        for pos in &positions {
            let hold_hours = now.signed_duration_since(pos.opened_at).num_hours();

            // Time-based exit does not require a live price lookup.
            if hold_hours >= cfg.max_hold_hours {
                info!(
                    position_id = %pos.id,
                    market = %pos.market_id,
                    hold_hours = hold_hours,
                    "Max hold time exceeded for copy trade"
                );
                self.close_position(pos, "time_exit").await;
                continue;
            }

            let key = (pos.market_id.clone(), pos.orderbook_token_id().to_string());

            match outcome_prices.get(&key) {
                Some(PriceOutcome::Price(current_price)) => {
                    let current_price = *current_price;
                    let pnl_pct = if pos.entry_price > Decimal::ZERO {
                        (current_price - pos.entry_price) / pos.entry_price
                    } else {
                        Decimal::ZERO
                    };

                    // Stop-loss: close if loss exceeds threshold
                    if pnl_pct < Decimal::ZERO && pnl_pct.abs() >= cfg.stop_loss_pct {
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
                    if pnl_pct > Decimal::ZERO && pnl_pct >= cfg.take_profit_pct {
                        info!(
                            position_id = %pos.id,
                            market = %pos.market_id,
                            pnl_pct = %pnl_pct,
                            "Take-profit triggered for copy trade"
                        );
                        self.close_position(pos, "take_profit").await;
                        continue;
                    }
                }
                Some(PriceOutcome::NotFound) => {
                    // Market is gone from the CLOB — count strikes and auto-close
                    // after a threshold to avoid acting on a transient CLOB glitch.
                    let strikes = self.not_found_strikes.entry(key).or_insert(0);
                    *strikes += 1;

                    if *strikes >= MAX_NOT_FOUND_STRIKES {
                        info!(
                            position_id = %pos.id,
                            market = %pos.market_id,
                            strikes = *strikes,
                            "Market not found for {} consecutive checks — closing position as market_resolved",
                            MAX_NOT_FOUND_STRIKES
                        );
                        self.close_position(pos, "market_resolved").await;
                    } else {
                        debug!(
                            position_id = %pos.id,
                            market = %pos.market_id,
                            strikes = *strikes,
                            max = MAX_NOT_FOUND_STRIKES,
                            "Market not found, strike {}/{}",
                            strikes,
                            MAX_NOT_FOUND_STRIKES
                        );
                    }
                }
                Some(PriceOutcome::Unavailable) | None => {
                    // Transient error or no bids — skip this cycle
                }
            }
        }

        Ok(())
    }

    /// Handle a mirror exit: if the source wallet sells a token we hold, close our position.
    async fn handle_mirror_exit(
        &self,
        wallet_trade: &wallet_tracker::trade_monitor::WalletTrade,
    ) -> anyhow::Result<()> {
        // Find open copy positions from this wallet in this market/token.
        let rows = sqlx::query_as::<
            _,
            (
                uuid::Uuid,
                String,
                Option<String>,
                Decimal,
                Decimal,
                Option<chrono::DateTime<Utc>>,
                chrono::DateTime<Utc>,
            ),
        >(
            r#"
            SELECT
              p.id,
              p.outcome,
              COALESCE(
                p.source_token_id,
                (
                  SELECT cth.source_token_id
                  FROM copy_trade_history cth
                  WHERE LOWER(cth.source_wallet) = LOWER(p.source_wallet)
                    AND cth.source_market_id = p.market_id
                  ORDER BY ABS(EXTRACT(EPOCH FROM (cth.source_timestamp - COALESCE(p.opened_at, p.entry_timestamp)))) ASC
                  LIMIT 1
                )
              ) AS resolved_token_id,
              p.quantity,
              p.entry_price,
              p.opened_at,
              p.entry_timestamp
            FROM positions p
            WHERE p.is_copy_trade = true
              AND p.source_wallet = $1
              AND p.market_id = $2
              AND p.is_open = true
              AND COALESCE(
                    p.source_token_id,
                    (
                      SELECT cth.source_token_id
                      FROM copy_trade_history cth
                      WHERE LOWER(cth.source_wallet) = LOWER(p.source_wallet)
                        AND cth.source_market_id = p.market_id
                      ORDER BY ABS(EXTRACT(EPOCH FROM (cth.source_timestamp - COALESCE(p.opened_at, p.entry_timestamp)))) ASC
                      LIMIT 1
                    )
                  ) = $3
            "#,
        )
        .bind(&wallet_trade.wallet_address)
        .bind(&wallet_trade.market_id)
        .bind(&wallet_trade.token_id)
        .fetch_all(&self.pool)
        .await?;

        for (id, outcome, source_token_id, quantity, entry_price, opened_at, entry_timestamp) in
            rows
        {
            info!(
                position_id = %id,
                source_wallet = %wallet_trade.wallet_address,
                market = %wallet_trade.market_id,
                token_id = %wallet_trade.token_id,
                "Mirror exit: source wallet sold, closing our copy position"
            );

            let pos = CopyPosition {
                id,
                market_id: wallet_trade.market_id.clone(),
                outcome,
                source_token_id,
                source_wallet: Some(wallet_trade.wallet_address.clone()),
                quantity,
                entry_price,
                opened_at: opened_at.unwrap_or(entry_timestamp),
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

        // Terminal 404 path: market/token no longer exists on CLOB.
        // Also guard against zero-sized positions, which cannot be sold.
        if reason == "market_resolved" || pos.quantity <= Decimal::ZERO {
            if pos.quantity <= Decimal::ZERO {
                info!(
                    position_id = %pos.id,
                    quantity = %pos.quantity,
                    reason = reason,
                    "Closing copy position without exit order due to non-positive quantity"
                );
            }

            self.finalize_position_without_order(pos, reason, Decimal::ZERO)
                .await;
            return;
        }

        // Place sell order
        let order = MarketOrder::new(
            pos.market_id.clone(),
            pos.orderbook_token_id().to_string(),
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

                // Notify auto-optimizer of position close
                self.emit_position_closed(pos, actual_pnl).await;

                // Publish signal
                let signal = SignalUpdate {
                    signal_id: uuid::Uuid::new_v4(),
                    signal_type: SignalType::CopyTrade,
                    market_id: pos.market_id.clone(),
                    outcome_id: pos.orderbook_token_id().to_string(),
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

    /// Finalize a position closure when an on-chain exit order cannot be placed.
    async fn finalize_position_without_order(
        &self,
        pos: &CopyPosition,
        reason: &str,
        realized_pnl: Decimal,
    ) {
        // Mark as closed in DB.
        if let Err(e) = sqlx::query(
            "UPDATE positions SET realized_pnl = $1, exit_timestamp = NOW(), state = 4 WHERE id = $2",
        )
        .bind(realized_pnl)
        .bind(pos.id)
        .execute(&self.pool)
        .await
        {
            error!(
                error = %e,
                position_id = %pos.id,
                "Failed to finalize position closure without order"
            );
            return;
        }

        // Keep CopyTrader open-position accounting accurate.
        {
            let mut ct = self.copy_trader.write().await;
            ct.record_position_closed();
        }

        info!(
            position_id = %pos.id,
            reason = reason,
            realized_pnl = %realized_pnl,
            "Copy trade position closed without exit order"
        );

        // Notify auto-optimizer of position close
        self.emit_position_closed(pos, realized_pnl).await;

        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::CopyTrade,
            market_id: pos.market_id.clone(),
            outcome_id: pos.orderbook_token_id().to_string(),
            action: "closed".to_string(),
            confidence: 1.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "position_id": pos.id.to_string(),
                "reason": reason,
                "realized_pnl": realized_pnl.to_string(),
                "entry_price": pos.entry_price.to_string(),
                "exit_price": null,
            }),
        };
        let _ = self.signal_tx.send(signal);
    }

    /// Emit a `PositionClosed` event to the auto-optimizer so it can track
    /// consecutive losses and trigger wallet demotions in real time.
    async fn emit_position_closed(&self, pos: &CopyPosition, pnl: Decimal) {
        let Some(ref tx) = self.event_tx else { return };
        let Some(ref wallet) = pos.source_wallet else {
            return;
        };

        // Look up workspace_id from active wallet allocation
        let ws_id = sqlx::query_as::<_, (uuid::Uuid,)>(
            "SELECT workspace_id FROM workspace_wallet_allocations WHERE LOWER(wallet_address) = LOWER($1) AND tier = 'active' LIMIT 1",
        )
        .bind(wallet)
        .fetch_optional(&self.pool)
        .await;

        match ws_id {
            Ok(Some((workspace_id,))) => {
                if let Err(e) = tx.try_send(AutomationEvent::PositionClosed {
                    workspace_id,
                    wallet_address: wallet.clone(),
                    pnl,
                    is_win: pnl >= Decimal::ZERO,
                }) {
                    warn!(
                        error = %e,
                        wallet = %wallet,
                        "Failed to send position-close event to optimizer"
                    );
                }
            }
            Ok(None) => {
                debug!(
                    wallet = %wallet,
                    position_id = %pos.id,
                    "No active-tier allocation found for wallet; skipping optimizer event"
                );
            }
            Err(e) => {
                warn!(
                    error = %e,
                    wallet = %wallet,
                    "DB error looking up workspace for optimizer event"
                );
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
                Option<String>,
                Option<String>,
                Decimal,
                Decimal,
                Option<chrono::DateTime<Utc>>,
                chrono::DateTime<Utc>,
            ),
        >(
            r#"
            SELECT
              p.id,
              p.market_id,
              p.outcome,
              COALESCE(
                p.source_token_id,
                (
                  SELECT cth.source_token_id
                  FROM copy_trade_history cth
                  WHERE LOWER(cth.source_wallet) = LOWER(p.source_wallet)
                    AND cth.source_market_id = p.market_id
                  ORDER BY ABS(EXTRACT(EPOCH FROM (cth.source_timestamp - COALESCE(p.opened_at, p.entry_timestamp)))) ASC
                  LIMIT 1
                )
              ) AS resolved_token_id,
              p.source_wallet,
              p.quantity,
              p.entry_price,
              p.opened_at,
              p.entry_timestamp
            FROM positions p
            WHERE p.is_copy_trade = true AND p.is_open = true
            ORDER BY COALESCE(p.opened_at, p.entry_timestamp) ASC
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
                    source_token_id,
                    source_wallet,
                    quantity,
                    entry_price,
                    opened_at,
                    entry_timestamp,
                )| {
                    CopyPosition {
                        id,
                        market_id,
                        outcome,
                        source_token_id,
                        source_wallet,
                        quantity,
                        entry_price,
                        opened_at: opened_at.unwrap_or(entry_timestamp),
                    }
                },
            )
            .collect())
    }

    /// Result of a price fetch, distinguishing "market gone" from transient errors.
    async fn fetch_current_price(
        &self,
        _market_id: &str,
        outcome_id: &str,
    ) -> std::result::Result<Decimal, PriceFetchError> {
        match self.clob_client.get_order_book(outcome_id).await {
            Ok(book) => book
                .bids
                .first()
                .map(|l| l.price)
                .ok_or(PriceFetchError::NoBids),
            Err(PolyError::Api {
                status: Some(404), ..
            }) => Err(PriceFetchError::MarketNotFound),
            Err(e) => Err(PriceFetchError::Other(e.into())),
        }
    }
}

/// Spawn the copy-trade stop-loss monitor as a background task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_copy_stop_loss_monitor(
    config: Arc<RwLock<CopyStopLossConfig>>,
    pool: PgPool,
    order_executor: Arc<OrderExecutor>,
    circuit_breaker: Arc<CircuitBreaker>,
    clob_client: Arc<ClobClient>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    trade_monitor: Option<Arc<TradeMonitor>>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    event_tx: Option<mpsc::Sender<AutomationEvent>>,
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
        event_tx,
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
