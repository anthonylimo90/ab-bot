//! Copy trading monitor - bridges wallet trade detection to copy trader execution.
//!
//! This module monitors tracked wallets for trades and forwards them to the copy trader
//! for execution, while publishing signals to WebSocket clients.

use chrono::Utc;
use risk_manager::circuit_breaker::CircuitBreaker;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

use polymarket_core::types::OrderSide;
use trading_engine::copy_trader::{
    CopyTradeProcessOutcome, CopyTradeRejection, CopyTrader, DetectedTrade,
};
use wallet_tracker::trade_monitor::{TradeDirection, TradeMonitor, WalletTrade};

use crate::websocket::{SignalType, SignalUpdate};

/// Configuration for the copy trading monitor.
#[derive(Debug, Clone)]
pub struct CopyTradingConfig {
    /// Minimum trade value to trigger copy (raised from $0.05 to $10).
    pub min_trade_value: Decimal,
    /// Maximum latency in seconds before skipping a trade.
    pub max_latency_secs: i64,
    /// Whether copy trading is enabled.
    pub enabled: bool,
}

impl Default for CopyTradingConfig {
    fn default() -> Self {
        Self {
            min_trade_value: Decimal::new(5, 0), // $5 minimum for cold-start coverage
            max_latency_secs: 120,               // 2 min: binary markets resolve fast
            enabled: true,
        }
    }
}

impl CopyTradingConfig {
    /// Create config from environment variables.
    pub fn from_env() -> Self {
        Self {
            min_trade_value: std::env::var("COPY_MIN_TRADE_VALUE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::new(5, 0)),
            max_latency_secs: std::env::var("COPY_MAX_LATENCY_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(120),
            enabled: std::env::var("COPY_TRADING_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
        }
    }
}

/// Copy trading monitor that bridges TradeMonitor to CopyTrader.
pub struct CopyTradingMonitor {
    config: CopyTradingConfig,
    trade_monitor: Arc<TradeMonitor>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    circuit_breaker: Arc<CircuitBreaker>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    pool: PgPool,
    /// Runtime-tunable max latency threshold (seconds).  Written by the
    /// dynamic config subscriber, read per-trade with `Relaxed` ordering.
    max_latency_secs: Arc<AtomicI64>,
    /// Set of active (non-resolved) CLOB market IDs, populated by OutcomeTokenCache.
    /// When non-empty, trades for markets not in this set are skipped.
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    /// Total copy-trading capital in cents. Read per-trade, written by dynamic config subscriber.
    copy_total_capital: Arc<AtomicI64>,
}

impl CopyTradingMonitor {
    /// Create a new copy trading monitor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CopyTradingConfig,
        trade_monitor: Arc<TradeMonitor>,
        copy_trader: Arc<RwLock<CopyTrader>>,
        circuit_breaker: Arc<CircuitBreaker>,
        signal_tx: broadcast::Sender<SignalUpdate>,
        pool: PgPool,
        max_latency_secs: Arc<AtomicI64>,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
        copy_total_capital: Arc<AtomicI64>,
    ) -> Self {
        Self {
            config,
            trade_monitor,
            copy_trader,
            circuit_breaker,
            signal_tx,
            pool,
            max_latency_secs,
            active_clob_markets,
            copy_total_capital,
        }
    }

    /// Start the monitoring loop - runs until cancelled.
    pub async fn run(&self) -> anyhow::Result<()> {
        if !self.config.enabled {
            info!("Copy trading monitor is disabled");
            return Ok(());
        }

        info!("Starting copy trading monitor");

        let mut trade_rx = self.trade_monitor.subscribe();

        loop {
            match trade_rx.recv().await {
                Ok(wallet_trade) => {
                    if let Err(e) = self.process_trade(wallet_trade).await {
                        error!(error = %e, "Failed to process detected trade");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "Copy trading monitor lagged, skipped messages");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("Trade monitor channel closed, stopping copy trading monitor");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Publish a skip signal for trades that aren't copied.
    fn publish_skip_signal(&self, trade: &WalletTrade, skip_type: &str, reason: &str) {
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::CopyTrade,
            market_id: trade.market_id.clone(),
            outcome_id: trade.token_id.clone(),
            action: "skipped".to_string(),
            confidence: 0.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "wallet_address": trade.wallet_address,
                "skip_type": skip_type,
                "reason": reason,
                "value": trade.value.to_string(),
            }),
        };
        let _ = self.signal_tx.send(signal);
    }

    /// Publish a failure signal for trades that errored during copy.
    fn publish_failure_signal(&self, trade: &WalletTrade, error: &str) {
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::CopyTrade,
            market_id: trade.market_id.clone(),
            outcome_id: trade.token_id.clone(),
            action: "failed".to_string(),
            confidence: 0.0,
            timestamp: Utc::now(),
            metadata: serde_json::json!({
                "wallet_address": trade.wallet_address,
                "error": error,
                "value": trade.value.to_string(),
            }),
        };
        let _ = self.signal_tx.send(signal);
    }

    /// Persist skipped/failed outcomes for observability and tuning metrics.
    async fn record_trade_outcome(
        &self,
        trade: &WalletTrade,
        status: i16,
        skip_reason: Option<&str>,
        error_message: Option<&str>,
    ) {
        let direction_i16: i16 = match trade.direction {
            TradeDirection::Buy => 0,
            TradeDirection::Sell => 1,
        };

        let allocation_pct = {
            let ct = self.copy_trader.read().await;
            ct.get_tracked_wallet(&trade.wallet_address)
                .map(|w| w.allocation_pct)
                .unwrap_or(Decimal::ZERO)
        };

        if let Err(e) = sqlx::query(
            r#"
            INSERT INTO copy_trade_history (
                source_wallet, source_tx_hash,
                source_market_id, source_token_id, source_direction,
                source_price, source_quantity, source_timestamp,
                allocation_pct, status, skip_reason, error_message
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                $9, $10, $11, $12
            )
            "#,
        )
        .bind(&trade.wallet_address)
        .bind(&trade.tx_hash)
        .bind(&trade.market_id)
        .bind(&trade.token_id)
        .bind(direction_i16)
        .bind(trade.price)
        .bind(trade.quantity)
        .bind(trade.timestamp)
        .bind(allocation_pct)
        .bind(status)
        .bind(skip_reason)
        .bind(error_message)
        .execute(&self.pool)
        .await
        {
            warn!(error = %e, "Failed to persist copy trade outcome");
        }
    }

    async fn process_trade(&self, trade: WalletTrade) -> anyhow::Result<()> {
        // Read runtime-tunable total capital (stored as cents) and push to copy trader.
        let capital_cents = self.copy_total_capital.load(Ordering::Relaxed);
        if capital_cents > 0 {
            let capital = Decimal::new(capital_cents, 2); // cents → dollars
            let mut ct = self.copy_trader.write().await;
            ct.update_capital(capital);
        }

        let policy_min_trade_value = {
            let copy_trader = self.copy_trader.read().await;
            copy_trader.policy().min_trade_value
        };

        // Check minimum trade value
        if trade.value < policy_min_trade_value {
            info!(
                wallet = %trade.wallet_address,
                value = %trade.value,
                min = %policy_min_trade_value,
                "Trade below minimum value, skipping"
            );
            self.publish_skip_signal(
                &trade,
                "below_minimum",
                &format!(
                    "Trade value ${} below minimum ${}",
                    trade.value, policy_min_trade_value
                ),
            );
            self.record_trade_outcome(
                &trade,
                3,
                Some("below_minimum"),
                Some("Trade value below minimum"),
            )
            .await;
            return Ok(());
        }

        // Check latency (runtime-tunable via dynamic config)
        let now = Utc::now();
        let latency = now.signed_duration_since(trade.timestamp).num_seconds();
        let max_latency = self.max_latency_secs.load(Ordering::Relaxed);
        if latency > max_latency {
            info!(
                wallet = %trade.wallet_address,
                latency = latency,
                max = max_latency,
                "Trade too old, skipping"
            );
            self.publish_skip_signal(
                &trade,
                "too_stale",
                &format!("Trade is {}s old (max {}s)", latency, max_latency),
            );
            self.record_trade_outcome(
                &trade,
                3,
                Some("too_stale"),
                Some("Trade exceeded max latency"),
            )
            .await;
            return Ok(());
        }

        // Resolved-market filter: skip trades for markets no longer active on CLOB.
        // When the set is empty (token cache not yet populated), also block trades
        // rather than letting resolved-market trades slip through. The slippage
        // pre-check is a fallback but wastes API calls on markets we already know
        // are dead once the cache is populated.
        {
            let active_markets = self.active_clob_markets.read().await;
            if active_markets.is_empty() {
                warn!(
                    wallet = %trade.wallet_address,
                    market_id = %trade.market_id,
                    "Active market set not yet populated, skipping trade until cache is ready"
                );
                self.record_trade_outcome(
                    &trade,
                    3,
                    Some("market_cache_empty"),
                    Some("Active market cache not yet populated"),
                )
                .await;
                return Ok(());
            }
            if !active_markets.contains(&trade.market_id) {
                // Distinguish missing condition_id (token_id used as fallback) from
                // genuinely resolved markets. This helps quantify the token_id issue.
                let is_condition_id_fallback = trade.market_id == trade.token_id;
                if is_condition_id_fallback {
                    warn!(
                        wallet = %trade.wallet_address,
                        market_id = %trade.market_id,
                        token_id = %trade.token_id,
                        "condition_id was None — market_id is token_id fallback, attempting DB cache lookup"
                    );

                    // Try to resolve the real condition_id from token_condition_cache
                    if let Ok(Some((resolved_condition_id,))) = sqlx::query_as::<_, (String,)>(
                        "SELECT condition_id FROM token_condition_cache WHERE token_id = $1",
                    )
                    .bind(&trade.token_id)
                    .fetch_optional(&self.pool)
                    .await
                    {
                        if active_markets.contains(&resolved_condition_id) {
                            info!(
                                wallet = %trade.wallet_address,
                                token_id = %trade.token_id,
                                resolved_condition_id = %resolved_condition_id,
                                "Resolved token_id to active condition_id via DB cache"
                            );
                            // Drop the read lock and re-process with corrected market_id.
                            // The corrected trade will pass the active market check on re-entry.
                            drop(active_markets);
                            let mut corrected_trade = trade.clone();
                            corrected_trade.market_id = resolved_condition_id;
                            return Box::pin(self.process_trade(corrected_trade)).await;
                        }
                    }
                }
                info!(
                    wallet = %trade.wallet_address,
                    market_id = %trade.market_id,
                    active_set_size = active_markets.len(),
                    condition_id_fallback = is_condition_id_fallback,
                    "Market not in active CLOB set, skipping (resolved or delisted)"
                );
                self.publish_skip_signal(
                    &trade,
                    "market_not_active",
                    &format!(
                        "Market {} not in active CLOB set (resolved or delisted)",
                        trade.market_id
                    ),
                );
                self.record_trade_outcome(
                    &trade,
                    3,
                    Some("market_not_active"),
                    Some("Market not in active CLOB set"),
                )
                .await;
                return Ok(());
            }
        }

        // Circuit breaker check — copy trades must respect it
        if !self.circuit_breaker.can_trade().await {
            warn!(
                wallet = %trade.wallet_address,
                "Circuit breaker tripped, skipping copy trade"
            );
            self.publish_skip_signal(
                &trade,
                "circuit_breaker",
                "Circuit breaker is tripped — all trading halted",
            );
            self.record_trade_outcome(
                &trade,
                3,
                Some("circuit_breaker"),
                Some("Circuit breaker is tripped"),
            )
            .await;
            return Ok(());
        }

        // Policy checks (daily limit, position count, min value)
        {
            let mut copy_trader = self.copy_trader.write().await;
            if let Err(rejection) = copy_trader.check_policy(trade.value) {
                let (skip_type, reason) = match &rejection {
                    CopyTradeRejection::BelowMinTradeValue { value, min } => (
                        "below_minimum",
                        format!("Trade value ${value} below policy minimum ${min}"),
                    ),
                    CopyTradeRejection::DailyCapitalLimitReached { deployed, limit } => (
                        "daily_limit",
                        format!("Daily capital limit reached (${deployed}/${limit})"),
                    ),
                    CopyTradeRejection::TooManyOpenPositions { current, limit } => (
                        "position_limit",
                        format!("Too many open copy positions ({current}/{limit})"),
                    ),
                    CopyTradeRejection::CircuitBreakerTripped => {
                        ("circuit_breaker", "Circuit breaker tripped".to_string())
                    }
                    CopyTradeRejection::SlippageTooHigh { slippage_pct, max } => (
                        "slippage",
                        format!("Slippage {slippage_pct} exceeds max {max}"),
                    ),
                    CopyTradeRejection::MarketNearResolution { market_price } => (
                        "near_resolution",
                        format!(
                            "Market price {market_price} indicates resolved/near-resolution market"
                        ),
                    ),
                    CopyTradeRejection::ZeroCalculatedQuantity {
                        total_capital,
                        allocation_pct,
                    } => (
                        "zero_capital",
                        format!(
                            "Zero copy quantity (capital={total_capital}, alloc={allocation_pct}%)"
                        ),
                    ),
                    CopyTradeRejection::MarketNotFound { outcome_id } => (
                        "market_not_found",
                        format!("Outcome {outcome_id} not found on CLOB (resolved or delisted)"),
                    ),
                };
                warn!(
                    wallet = %trade.wallet_address,
                    rejection = ?rejection,
                    "Copy trade rejected by policy"
                );
                self.publish_skip_signal(&trade, skip_type, &reason);
                self.record_trade_outcome(&trade, 3, Some(skip_type), Some(&reason))
                    .await;
                return Ok(());
            }
        }

        // Convert WalletTrade to DetectedTrade
        let detected = DetectedTrade {
            wallet_address: trade.wallet_address.clone(),
            market_id: trade.market_id.clone(),
            outcome_id: trade.token_id.clone(),
            side: match trade.direction {
                TradeDirection::Buy => OrderSide::Buy,
                TradeDirection::Sell => OrderSide::Sell,
            },
            price: trade.price,
            quantity: trade.quantity,
            timestamp: trade.timestamp,
            tx_hash: trade.tx_hash.clone(),
        };

        // Publish signal before attempting copy (for UI notification)
        let signal = SignalUpdate {
            signal_id: uuid::Uuid::new_v4(),
            signal_type: SignalType::CopyTrade,
            market_id: trade.market_id.clone(),
            outcome_id: trade.token_id.clone(),
            action: match trade.direction {
                TradeDirection::Buy => "buy".to_string(),
                TradeDirection::Sell => "sell".to_string(),
            },
            confidence: 1.0,
            timestamp: now,
            metadata: serde_json::json!({
                "wallet_address": trade.wallet_address,
                "price": trade.price.to_string(),
                "quantity": trade.quantity.to_string(),
                "value": trade.value.to_string(),
                "tx_hash": trade.tx_hash,
                "latency_secs": latency,
            }),
        };

        // Send signal to WebSocket clients
        let _ = self.signal_tx.send(signal);

        // Process the trade through CopyTrader
        // Use a scoped read lock — drop it before any write lock to avoid deadlock
        let (result, allocation_pct) = {
            let copy_trader = self.copy_trader.read().await;
            let result = copy_trader
                .process_detected_trade_with_reason(&detected)
                .await;
            let alloc = copy_trader
                .get_tracked_wallet(&trade.wallet_address)
                .map(|w| w.allocation_pct)
                .unwrap_or(Decimal::ZERO);
            (result, alloc)
        }; // read lock dropped here

        match result {
            Ok(CopyTradeProcessOutcome::Executed(report)) => {
                if !report.is_success() {
                    let err_msg = report
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "order rejected".to_string());
                    warn!(
                        wallet = %trade.wallet_address,
                        market = %trade.market_id,
                        order_id = %report.order_id,
                        status = ?report.status,
                        error = %err_msg,
                        "Copy trade execution was rejected"
                    );
                    self.publish_failure_signal(
                        &trade,
                        &format!("Copy order rejected: {}", err_msg),
                    );
                    self.record_trade_outcome(&trade, 4, Some("order_rejected"), Some(&err_msg))
                        .await;
                    return Ok(());
                }

                let trade_value = report.filled_quantity * report.average_price;
                let has_open_fill = report.filled_quantity > Decimal::ZERO;

                info!(
                    wallet = %trade.wallet_address,
                    market = %trade.market_id,
                    direction = ?trade.direction,
                    copied_quantity = %report.filled_quantity,
                    trade_value = %trade_value,
                    "Successfully copied trade"
                );

                if has_open_fill {
                    // Record position opening with the copy trader for daily/position tracking.
                    let mut ct = self.copy_trader.write().await;
                    ct.record_position_opened(trade_value);
                } else {
                    warn!(
                        wallet = %trade.wallet_address,
                        market = %trade.market_id,
                        order_id = %report.order_id,
                        filled_quantity = %report.filled_quantity,
                        "Copy trade execution reported success with non-positive fill; treating as failed copy"
                    );
                    self.publish_failure_signal(
                        &trade,
                        "Copy order had non-positive fill quantity",
                    );
                    self.record_trade_outcome(
                        &trade,
                        4,
                        Some("invalid_fill"),
                        Some("Copy order had non-positive fill quantity"),
                    )
                    .await;
                    return Ok(());
                }

                // Record in copy_trade_history
                let slippage = if trade.price > Decimal::ZERO {
                    report.average_price - trade.price
                } else {
                    Decimal::ZERO
                };
                let direction_i16: i16 = match trade.direction {
                    TradeDirection::Buy => 0,
                    TradeDirection::Sell => 1,
                };

                if let Err(e) = sqlx::query(
                    r#"
                    INSERT INTO copy_trade_history (
                        source_wallet, source_tx_hash,
                        source_market_id, source_token_id, source_direction,
                        source_price, source_quantity, source_timestamp,
                        copy_order_id, copy_price, copy_quantity, copy_timestamp,
                        allocation_pct, slippage,
                        status
                    ) VALUES (
                        $1, $2, $3, $4, $5, $6, $7, $8,
                        $9, $10, $11, $12,
                        $13, $14, $15
                    )
                    "#,
                )
                .bind(&trade.wallet_address)
                .bind(&trade.tx_hash)
                .bind(&trade.market_id)
                .bind(&trade.token_id)
                .bind(direction_i16)
                .bind(trade.price)
                .bind(trade.quantity)
                .bind(trade.timestamp)
                .bind(report.order_id)
                .bind(report.average_price)
                .bind(report.filled_quantity)
                .bind(report.executed_at)
                .bind(allocation_pct)
                .bind(slippage)
                .bind(1_i16) // status = 1 (executed)
                .execute(&self.pool)
                .await
                {
                    warn!(error = %e, "Failed to record copy trade history");
                }

                // Insert position for dashboard visibility only when we actually hold size.
                if has_open_fill {
                    let position_id = uuid::Uuid::new_v4();
                    let side_str = match trade.direction {
                        TradeDirection::Buy => "long",
                        TradeDirection::Sell => "short",
                    };
                    let outcome_str = match trade.direction {
                        TradeDirection::Buy => "yes",
                        TradeDirection::Sell => "no",
                    };

                    if let Err(e) = sqlx::query(
                        r#"
                        INSERT INTO positions (
                            id, market_id, outcome, side, quantity,
                            entry_price, current_price, unrealized_pnl,
                            is_copy_trade, source_wallet, is_open, opened_at,
                            source_token_id,
                            yes_entry_price, no_entry_price, entry_timestamp,
                            exit_strategy, state, source
                        ) VALUES (
                            $1, $2, $3, $4, $5,
                            $6, $6, 0,
                            true, $7, true, NOW(),
                            $8,
                            $9, $10, NOW(),
                            1, 1, 2
                        )
                        "#,
                    )
                    .bind(position_id)
                    .bind(&trade.market_id)
                    .bind(outcome_str)
                    .bind(side_str)
                    .bind(report.filled_quantity)
                    .bind(report.average_price)
                    .bind(&trade.wallet_address)
                    .bind(&trade.token_id)
                    .bind(if side_str == "long" {
                        report.average_price
                    } else {
                        Decimal::ZERO
                    })
                    .bind(if side_str == "short" {
                        report.average_price
                    } else {
                        Decimal::ZERO
                    })
                    .execute(&self.pool)
                    .await
                    {
                        warn!(error = %e, "Failed to insert copy trade position");
                    }
                }

                // Insert execution report
                let exec_side: i16 = match trade.direction {
                    TradeDirection::Buy => 0,
                    TradeDirection::Sell => 1,
                };

                if let Err(e) = sqlx::query(
                    r#"
                    INSERT INTO execution_reports (
                        order_id, market_id, outcome_id, side, status,
                        requested_quantity, filled_quantity, average_price,
                        fees_paid, executed_at, source
                    ) VALUES ($1, $2, $3, $4, 3, $5, $6, $7, $8, $9, 2)
                    "#,
                )
                .bind(report.order_id)
                .bind(&trade.market_id)
                .bind(&trade.token_id)
                .bind(exec_side)
                .bind(report.filled_quantity)
                .bind(report.filled_quantity)
                .bind(report.average_price)
                .bind(report.fees_paid)
                .bind(report.executed_at)
                .execute(&self.pool)
                .await
                {
                    warn!(error = %e, "Failed to insert execution report");
                }

                // Publish success signal
                let success_signal = SignalUpdate {
                    signal_id: uuid::Uuid::new_v4(),
                    signal_type: SignalType::CopyTrade,
                    market_id: trade.market_id,
                    outcome_id: trade.token_id,
                    action: "copied".to_string(),
                    confidence: 1.0,
                    timestamp: Utc::now(),
                    metadata: serde_json::json!({
                        "wallet_address": trade.wallet_address,
                        "copied_quantity": report.filled_quantity.to_string(),
                        "execution_price": report.average_price.to_string(),
                        "order_id": report.order_id.to_string(),
                    }),
                };
                let _ = self.signal_tx.send(success_signal);
            }
            Ok(CopyTradeProcessOutcome::Rejected(rejection)) => {
                let (skip_type, reason) = match &rejection {
                    CopyTradeRejection::BelowMinTradeValue { value, min } => (
                        "below_minimum",
                        format!("Trade value ${value} below policy minimum ${min}"),
                    ),
                    CopyTradeRejection::DailyCapitalLimitReached { deployed, limit } => (
                        "daily_limit",
                        format!("Daily capital limit reached (${deployed}/${limit})"),
                    ),
                    CopyTradeRejection::TooManyOpenPositions { current, limit } => (
                        "position_limit",
                        format!("Too many open copy positions ({current}/{limit})"),
                    ),
                    CopyTradeRejection::CircuitBreakerTripped => {
                        ("circuit_breaker", "Circuit breaker is tripped".to_string())
                    }
                    CopyTradeRejection::SlippageTooHigh { slippage_pct, max } => (
                        "slippage",
                        format!("Slippage {slippage_pct} exceeds max {max}"),
                    ),
                    CopyTradeRejection::MarketNearResolution { market_price } => (
                        "near_resolution",
                        format!(
                            "Market price {market_price} indicates resolved/near-resolution market"
                        ),
                    ),
                    CopyTradeRejection::ZeroCalculatedQuantity {
                        total_capital,
                        allocation_pct,
                    } => (
                        "zero_capital",
                        format!(
                            "Zero copy quantity (capital={total_capital}, alloc={allocation_pct}%)"
                        ),
                    ),
                    CopyTradeRejection::MarketNotFound { outcome_id } => (
                        "market_not_found",
                        format!("Outcome {outcome_id} not found on CLOB (resolved or delisted)"),
                    ),
                };
                info!(
                    wallet = %trade.wallet_address,
                    rejection = ?rejection,
                    "Trade rejected by copy-trader runtime policy"
                );
                self.publish_skip_signal(&trade, skip_type, &reason);
                self.record_trade_outcome(&trade, 3, Some(skip_type), Some(&reason))
                    .await;
            }
            Ok(CopyTradeProcessOutcome::Skipped) => {
                info!(
                    wallet = %trade.wallet_address,
                    "Trade not copied (wallet not tracked, disabled, or trade filtered)"
                );
                self.publish_skip_signal(
                    &trade,
                    "not_copied",
                    "Wallet not tracked, disabled, or trade filtered",
                );
                self.record_trade_outcome(
                    &trade,
                    3,
                    Some("not_copied"),
                    Some("Wallet not tracked, disabled, or trade filtered"),
                )
                .await;
            }
            Err(e) => {
                error!(
                    wallet = %trade.wallet_address,
                    error = %e,
                    "Failed to copy trade"
                );
                self.publish_failure_signal(&trade, &e.to_string());
                self.record_trade_outcome(&trade, 4, Some("execution_error"), Some(&e.to_string()))
                    .await;
            }
        }

        Ok(())
    }
}

/// Spawn the copy trading monitor as a background task.
#[allow(clippy::too_many_arguments)]
pub fn spawn_copy_trading_monitor(
    config: CopyTradingConfig,
    trade_monitor: Arc<TradeMonitor>,
    copy_trader: Arc<RwLock<CopyTrader>>,
    circuit_breaker: Arc<CircuitBreaker>,
    signal_tx: broadcast::Sender<SignalUpdate>,
    pool: PgPool,
    max_latency_secs: Arc<AtomicI64>,
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    copy_total_capital: Arc<AtomicI64>,
) {
    let monitor = CopyTradingMonitor::new(
        config,
        trade_monitor,
        copy_trader,
        circuit_breaker,
        signal_tx,
        pool,
        max_latency_secs,
        active_clob_markets,
        copy_total_capital,
    );

    tokio::spawn(async move {
        if let Err(e) = monitor.run().await {
            error!(error = %e, "Copy trading monitor failed");
        }
    });

    info!("Copy trading monitor spawned as background task");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = CopyTradingConfig::default();
        assert_eq!(config.min_trade_value, Decimal::new(5, 0));
        assert_eq!(config.max_latency_secs, 120);
        assert!(config.enabled);
    }

    #[test]
    fn test_trade_direction_conversion() {
        let buy: OrderSide = match TradeDirection::Buy {
            TradeDirection::Buy => OrderSide::Buy,
            TradeDirection::Sell => OrderSide::Sell,
        };
        assert!(matches!(buy, OrderSide::Buy));

        let sell: OrderSide = match TradeDirection::Sell {
            TradeDirection::Buy => OrderSide::Buy,
            TradeDirection::Sell => OrderSide::Sell,
        };
        assert!(matches!(sell, OrderSide::Sell));
    }
}
