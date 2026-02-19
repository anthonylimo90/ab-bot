//! Core arbitrage monitoring logic.

use crate::position_tracker::PositionTracker;
use crate::signals::{channels, RuntimeStats, SignalPublisher};
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use polymarket_core::api::clob::OrderBookUpdate;
use polymarket_core::api::ClobClient;
use polymarket_core::config::Config;
use polymarket_core::db;
use polymarket_core::types::{ArbOpportunity, BinaryMarketBook, OrderBook};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::time::Duration as StdDuration;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Minimum depth (in USD) at best ask for both sides.
const MIN_DEPTH_USD: Decimal = Decimal::from_parts(100, 0, 0, false, 0); // $100

/// Cooldown period between signals for the same market (seconds).
const SIGNAL_COOLDOWN_SECS: i64 = 60;

/// How often to check for stale positions (every N order book updates).
const STALE_CHECK_INTERVAL: u64 = 500;

#[derive(Debug, Clone, serde::Deserialize)]
struct DynamicConfigUpdate {
    key: String,
    value: Decimal,
}

/// Main arbitrage monitor service.
pub struct ArbMonitor {
    clob_client: ClobClient,
    position_tracker: PositionTracker,
    signal_publisher: SignalPublisher,
    /// Current order books by (market_id, outcome_id).
    order_books: HashMap<(String, String), OrderBook>,
    /// Market outcome pairings (market_id -> (yes_outcome_id, no_outcome_id)).
    market_outcomes: HashMap<String, (String, String)>,
    /// Minimum net profit threshold for entry signals.
    min_profit_threshold: Decimal,
    /// Last signal timestamp per market (for dedup/cooldown).
    last_signal_time: HashMap<String, DateTime<Utc>>,
    /// Optional cap for actively scanned markets.
    max_markets_cap: Option<usize>,
    /// Market ids sorted by liquidity (highest first).
    all_market_ids: Vec<String>,
    /// Active market subset based on cap.
    eligible_markets: HashSet<String>,
    /// Dynamic config update stream from Redis.
    dynamic_config_rx: mpsc::UnboundedReceiver<DynamicConfigUpdate>,
}

impl ArbMonitor {
    /// Create a new arbitrage monitor.
    pub async fn new(config: Config) -> Result<Self> {
        // Initialize database pool
        let pool = db::create_pool(&config.database).await?;

        // Initialize Redis connection
        let redis_client = redis::Client::open(config.redis.url.as_str())?;

        // Create CLOB client
        let clob_client = ClobClient::new(config.polymarket.clob_url, config.polymarket.ws_url);

        // Create position tracker
        let position_tracker = PositionTracker::new(pool.clone());

        // Create signal publisher
        let signal_publisher = SignalPublisher::new(redis_client, config.alerts).await?;

        let dynamic_config_rx = spawn_dynamic_config_listener(config.redis.url.clone());

        Ok(Self {
            clob_client,
            position_tracker,
            signal_publisher,
            order_books: HashMap::new(),
            market_outcomes: HashMap::new(),
            // Raised from 0.1% to 0.5% — must clear fees + slippage to be worth trading
            min_profit_threshold: std::env::var("ARB_MIN_PROFIT_THRESHOLD")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| Decimal::new(5, 3)), // default 0.005 = 0.5%
            last_signal_time: HashMap::new(),
            max_markets_cap: std::env::var("ARB_MONITOR_MAX_MARKETS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok()),
            all_market_ids: Vec::new(),
            eligible_markets: HashSet::new(),
            dynamic_config_rx,
        })
    }

    /// Run the monitoring loop.
    pub async fn run(&mut self) -> Result<()> {
        info!("Fetching active markets...");

        // Fetch markets and identify binary markets
        let markets = self.clob_client.get_markets().await?;
        let mut binary_markets: Vec<_> = markets
            .iter()
            .filter(|m| m.outcomes.len() == 2 && !m.resolved)
            .collect();

        // Keep sorted list so dynamic cap can widen/tighten without restart.
        binary_markets.sort_by(|a, b| b.liquidity.cmp(&a.liquidity));

        self.all_market_ids = binary_markets.iter().map(|m| m.id.clone()).collect();
        self.rebuild_eligible_markets();

        info!(
            total_markets = self.all_market_ids.len(),
            active_markets = self.eligible_markets.len(),
            max_cap = ?self.max_markets_cap,
            "Initialized arb market universe"
        );

        // Build market outcome mappings for all markets (not just currently active subset)
        for market in &binary_markets {
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
                self.market_outcomes
                    .insert(market.id.clone(), (yes_id, no_id));
            }
        }

        // Subscribe to the current active market subset.
        let mut updates = self
            .clob_client
            .subscribe_orderbook(self.active_subscription_market_ids())
            .await?;
        let update_timeout_secs = std::env::var("ARB_UPDATE_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(120);

        info!("Subscribed to order book updates, monitoring for arbitrage...");

        // Process updates
        let mut health_tick = 0u64;
        let mut stats_tick = tokio::time::interval(tokio::time::Duration::from_secs(60));
        stats_tick.tick().await;

        let mut updates_since_tick = 0u64;
        let mut stalls_since_tick = 0u64;
        let mut resets_since_tick = 0u64;
        let mut resubscribe_requested = false;

        loop {
            if resubscribe_requested {
                let target_markets = self.active_subscription_market_ids();
                info!(
                    market_count = target_markets.len(),
                    "Resubscribing orderbook stream after dynamic market-cap update"
                );
                loop {
                    match self
                        .clob_client
                        .subscribe_orderbook(target_markets.clone())
                        .await
                    {
                        Ok(new_updates) => {
                            updates = new_updates;
                            resets_since_tick += 1;
                            resubscribe_requested = false;
                            break;
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                "Failed resubscribing orderbook stream, retrying in 3s"
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                        }
                    }
                }
            }

            tokio::select! {
                maybe_cfg = self.dynamic_config_rx.recv() => {
                    if let Some(update) = maybe_cfg {
                        if self.apply_dynamic_update(update) {
                            resubscribe_requested = true;
                        }
                    }
                }
                _ = stats_tick.tick() => {
                    let stats = RuntimeStats {
                        updates_per_minute: updates_since_tick as f64,
                        stalls_last_minute: stalls_since_tick as f64,
                        resets_last_minute: resets_since_tick as f64,
                        monitored_markets: self.eligible_markets.len() as f64,
                    };
                    if let Err(e) = self.signal_publisher.publish_runtime_stats(&stats).await {
                        warn!(error = %e, "Failed to publish arb runtime stats");
                    }
                    updates_since_tick = 0;
                    stalls_since_tick = 0;
                    resets_since_tick = 0;
                }
                maybe_update = tokio::time::timeout(StdDuration::from_secs(update_timeout_secs), updates.recv()) => {
                    let Some(update) = (match maybe_update {
                        Ok(update) => update,
                        Err(_) => {
                            stalls_since_tick += 1;
                            warn!(timeout_secs = update_timeout_secs, "No orderbook updates received before timeout; reconnecting websocket subscription");
                            loop {
                                match self.clob_client.subscribe_orderbook(self.active_subscription_market_ids()).await {
                                    Ok(new_updates) => {
                                        updates = new_updates;
                                        resets_since_tick += 1;
                                        break;
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Failed reconnecting orderbook stream, retrying in 3s");
                                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    }
                                }
                            }
                            continue;
                        }
                    }) else {
                        warn!("Orderbook update channel closed; reconnecting websocket subscription");
                        loop {
                            match self.clob_client.subscribe_orderbook(self.active_subscription_market_ids()).await {
                                Ok(new_updates) => {
                                    updates = new_updates;
                                    resets_since_tick += 1;
                                    break;
                                }
                                Err(e) => {
                                    warn!(error = %e, "Failed reconnecting after channel close, retrying in 3s");
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                }
                            }
                        }
                        continue;
                    };

                    updates_since_tick += 1;
                    self.process_update(update).await?;
                    health_tick += 1;

                    if health_tick % 100 == 0 {
                        crate::touch_health_file();
                    }
                    if health_tick % 5000 == 0 {
                        info!(updates = health_tick, "Arb monitor processed orderbook updates");
                    }
                    // Periodically check for stale positions and publish exit signals
                    if health_tick % STALE_CHECK_INTERVAL == 0 {
                        if let Err(e) = self.position_tracker.check_stale_positions().await {
                            warn!(error = %e, "Failed to check stale positions");
                        }
                    }
                }
            }
        }
    }

    fn apply_dynamic_update(&mut self, update: DynamicConfigUpdate) -> bool {
        match update.key.as_str() {
            "ARB_MIN_PROFIT_THRESHOLD" => {
                self.min_profit_threshold = update.value;
                info!(
                    threshold = %self.min_profit_threshold,
                    "Applied dynamic ARB_MIN_PROFIT_THRESHOLD"
                );
                false
            }
            "ARB_MONITOR_MAX_MARKETS" => {
                let previous_count = self.eligible_markets.len();
                let cap = update
                    .value
                    .to_u64()
                    .and_then(|v| usize::try_from(v).ok())
                    .filter(|v| *v > 0);
                self.max_markets_cap = cap;
                self.rebuild_eligible_markets();
                info!(
                    cap = ?self.max_markets_cap,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_MAX_MARKETS"
                );
                self.eligible_markets.len() != previous_count
            }
            _ => false,
        }
    }

    fn rebuild_eligible_markets(&mut self) {
        let active_count = self
            .max_markets_cap
            .map(|cap| cap.min(self.all_market_ids.len()))
            .unwrap_or(self.all_market_ids.len());

        self.eligible_markets.clear();
        for market_id in self.all_market_ids.iter().take(active_count) {
            self.eligible_markets.insert(market_id.clone());
        }

        // Keep markets with open positions subscribed so exit tracking keeps working.
        for position in self.position_tracker.get_active_positions() {
            self.eligible_markets.insert(position.market_id.clone());
        }
    }

    fn active_subscription_market_ids(&self) -> Vec<String> {
        self.all_market_ids
            .iter()
            .filter(|id| self.eligible_markets.contains(*id))
            .cloned()
            .collect()
    }

    /// Process an order book update.
    async fn process_update(&mut self, update: OrderBookUpdate) -> Result<()> {
        let eligible_for_entries = self.eligible_markets.contains(&update.market_id);

        // Store the updated order book
        let book = OrderBook {
            market_id: update.market_id.clone(),
            outcome_id: update.asset_id.clone(),
            timestamp: update.timestamp,
            bids: update.bids,
            asks: update.asks,
        };
        self.order_books
            .insert((update.market_id.clone(), update.asset_id.clone()), book);

        // Check if we have both sides for this market
        if let Some((yes_id, no_id)) = self.market_outcomes.get(&update.market_id) {
            let yes_key = (update.market_id.clone(), yes_id.clone());
            let no_key = (update.market_id.clone(), no_id.clone());

            if let (Some(yes_book), Some(no_book)) = (
                self.order_books.get(&yes_key),
                self.order_books.get(&no_key),
            ) {
                let binary_book = BinaryMarketBook {
                    market_id: update.market_id.clone(),
                    timestamp: update.timestamp,
                    yes_book: yes_book.clone(),
                    no_book: no_book.clone(),
                };

                // Check liquidity depth on both sides before considering arb
                let has_depth = binary_book.entry_cost_with_depth(MIN_DEPTH_USD).is_some();

                // Calculate arbitrage opportunity
                if eligible_for_entries {
                    if let Some(arb) =
                        ArbOpportunity::calculate(&binary_book, ArbOpportunity::DEFAULT_FEE)
                    {
                        if arb.is_profitable()
                            && arb.net_profit >= self.min_profit_threshold
                            && has_depth
                        {
                            // Dedup/cooldown: skip if we signaled this market recently
                            let now = Utc::now();
                            let should_signal = match self.last_signal_time.get(&arb.market_id) {
                                Some(last) => (now - *last).num_seconds() >= SIGNAL_COOLDOWN_SECS,
                                None => true,
                            };

                            if should_signal {
                                self.last_signal_time.insert(arb.market_id.clone(), now);
                                self.handle_arb_opportunity(&arb, &binary_book).await?;
                            }
                        }
                    }
                }

                // Always update P&L for open positions, even if market is outside active-entry cap.
                self.position_tracker
                    .update_market_positions(&update.market_id, &binary_book)
                    .await?;
            }
        }

        Ok(())
    }

    /// Handle a detected arbitrage opportunity.
    async fn handle_arb_opportunity(
        &mut self,
        arb: &ArbOpportunity,
        book: &BinaryMarketBook,
    ) -> Result<()> {
        info!(
            "ARB DETECTED: market={} cost={:.4} profit={:.4}",
            arb.market_id, arb.total_cost, arb.net_profit
        );

        // Publish entry signal
        self.signal_publisher.publish_entry_signal(arb).await?;

        // Check for exit opportunities on open positions
        self.position_tracker
            .check_exit_opportunities(&arb.market_id, book)
            .await?;

        Ok(())
    }
}

fn spawn_dynamic_config_listener(
    redis_url: String,
) -> mpsc::UnboundedReceiver<DynamicConfigUpdate> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            if let Err(e) = run_dynamic_config_listener_once(&redis_url, &tx).await {
                warn!(error = %e, "Dynamic config listener failed; reconnecting");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    });

    rx
}

async fn run_dynamic_config_listener_once(
    redis_url: &str,
    tx: &mpsc::UnboundedSender<DynamicConfigUpdate>,
) -> Result<()> {
    let client = redis::Client::open(redis_url)?;
    let conn = client.get_async_connection().await?;
    let mut pubsub = conn.into_pubsub();

    pubsub.subscribe(channels::DYNAMIC_CONFIG_UPDATES).await?;
    info!(
        channel = channels::DYNAMIC_CONFIG_UPDATES,
        "Subscribed to dynamic config updates"
    );

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Invalid dynamic config payload");
                continue;
            }
        };

        match serde_json::from_str::<DynamicConfigUpdate>(&payload) {
            Ok(update) => {
                let _ = tx.send(update);
            }
            Err(e) => {
                warn!(error = %e, payload = %payload, "Failed to parse dynamic config update");
            }
        }
    }

    Ok(())
}
