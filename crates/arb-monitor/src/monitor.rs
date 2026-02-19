//! Core arbitrage monitoring logic.

use crate::position_tracker::PositionTracker;
use crate::signals::{channels, RuntimeMarketInsight, RuntimeStats, SignalPublisher};
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
use std::cmp::Ordering;
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

const KEY_ARB_MIN_PROFIT_THRESHOLD: &str = "ARB_MIN_PROFIT_THRESHOLD";
const KEY_ARB_MONITOR_MAX_MARKETS: &str = "ARB_MONITOR_MAX_MARKETS";
const KEY_ARB_MONITOR_EXPLORATION_SLOTS: &str = "ARB_MONITOR_EXPLORATION_SLOTS";
const KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL: &str = "ARB_MONITOR_AGGRESSIVENESS_LEVEL";
const OPPORTUNITY_EWMA_ALPHA: f64 = 0.25;

#[derive(Debug, Clone, Copy)]
enum AggressivenessProfile {
    Stable,
    Balanced,
    Discovery,
}

impl AggressivenessProfile {
    fn from_env() -> Self {
        match std::env::var("ARB_MONITOR_AGGRESSIVENESS")
            .unwrap_or_else(|_| "balanced".to_string())
            .to_lowercase()
            .as_str()
        {
            "stable" | "conservative" => Self::Stable,
            "discovery" | "aggressive" => Self::Discovery,
            _ => Self::Balanced,
        }
    }

    fn default_exploration_slots(self) -> usize {
        match self {
            Self::Stable => 2,
            Self::Balanced => 5,
            Self::Discovery => 8,
        }
    }

    fn from_level(level: f64) -> Self {
        if level <= 0.5 {
            Self::Stable
        } else if level >= 1.5 {
            Self::Discovery
        } else {
            Self::Balanced
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Balanced => "balanced",
            Self::Discovery => "discovery",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ScoringWeights {
    selection_baseline_weight: f64,
    selection_quality_weight: f64,
    selection_hit_rate_weight: f64,
    selection_freshness_weight: f64,
    selection_sticky_bonus: f64,
    exploration_baseline_weight: f64,
    exploration_novelty_weight: f64,
    exploration_rotation_weight: f64,
    exploration_upside_weight: f64,
    exploration_unseen_bonus: f64,
}

impl ScoringWeights {
    fn for_profile(profile: AggressivenessProfile) -> Self {
        match profile {
            AggressivenessProfile::Stable => Self {
                selection_baseline_weight: 1.20,
                selection_quality_weight: 0.70,
                selection_hit_rate_weight: 1.00,
                selection_freshness_weight: 0.50,
                selection_sticky_bonus: 0.35,
                exploration_baseline_weight: 0.70,
                exploration_novelty_weight: 0.55,
                exploration_rotation_weight: 0.60,
                exploration_upside_weight: 0.30,
                exploration_unseen_bonus: 0.75,
            },
            AggressivenessProfile::Balanced => Self {
                selection_baseline_weight: 1.00,
                selection_quality_weight: 0.95,
                selection_hit_rate_weight: 0.80,
                selection_freshness_weight: 1.00,
                selection_sticky_bonus: 0.20,
                exploration_baseline_weight: 0.55,
                exploration_novelty_weight: 1.10,
                exploration_rotation_weight: 0.90,
                exploration_upside_weight: 0.45,
                exploration_unseen_bonus: 1.40,
            },
            AggressivenessProfile::Discovery => Self {
                selection_baseline_weight: 0.80,
                selection_quality_weight: 1.25,
                selection_hit_rate_weight: 0.60,
                selection_freshness_weight: 1.20,
                selection_sticky_bonus: 0.10,
                exploration_baseline_weight: 0.40,
                exploration_novelty_weight: 1.45,
                exploration_rotation_weight: 1.20,
                exploration_upside_weight: 0.80,
                exploration_unseen_bonus: 1.90,
            },
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DynamicConfigUpdate {
    key: String,
    value: Decimal,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Clone, Copy, Default)]
struct MarketProfile {
    liquidity: f64,
    volume: f64,
}

#[derive(Debug, Clone, Default)]
struct MarketOpportunityStats {
    evaluated_books: u64,
    profitable_books: u64,
    signaled_books: u64,
    ewma_net_profit: f64,
    last_seen_at: Option<DateTime<Utc>>,
    last_signal_at: Option<DateTime<Utc>>,
    last_selected_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct RankedMarket {
    market_id: String,
    total_score: f64,
    baseline_score: f64,
    opportunity_score: f64,
    hit_rate_score: f64,
    freshness_score: f64,
    sticky_score: f64,
    novelty_score: Option<f64>,
    rotation_score: Option<f64>,
    upside_score: Option<f64>,
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
    /// Aggressiveness profile used for dynamic scoring.
    aggressiveness_profile: AggressivenessProfile,
    /// Scoring weights derived from aggressiveness profile.
    scoring_weights: ScoringWeights,
    /// Optional cap for actively scanned markets.
    max_markets_cap: Option<usize>,
    /// Market ids sorted by baseline quality (liquidity + volume).
    all_market_ids: Vec<String>,
    /// Static market metadata used in ranking.
    market_profiles: HashMap<String, MarketProfile>,
    /// Rolling market-level opportunity stats used for dynamic scoring.
    market_stats: HashMap<String, MarketOpportunityStats>,
    /// Active market subset based on cap.
    eligible_markets: HashSet<String>,
    /// Snapshot of top monitored markets with score breakdown.
    selection_snapshot: Vec<RuntimeMarketInsight>,
    /// Number of core (non-exploration) markets in current selection.
    core_market_count: usize,
    /// Number of exploration markets in current selection.
    exploration_market_count: usize,
    /// Last time market selection was re-ranked.
    last_rerank_at: Option<DateTime<Utc>>,
    /// Last time the websocket subscription was rebuilt.
    last_resubscribe_at: Option<DateTime<Utc>>,
    /// Number of slots dedicated to exploration.
    exploration_slots: usize,
    /// Dynamic config update stream from Redis.
    dynamic_config_rx: mpsc::UnboundedReceiver<DynamicConfigUpdate>,
    /// Bounds for dynamic keys loaded from DB (fallbacks if unavailable).
    dynamic_bounds: HashMap<String, (Decimal, Decimal)>,
    /// Allow-list for dynamic update publisher source field.
    allowed_dynamic_sources: HashSet<String>,
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

        let dynamic_bounds = load_dynamic_bounds(&pool).await;
        let dynamic_values = load_dynamic_values(&pool).await;
        let min_profit_env = std::env::var(KEY_ARB_MIN_PROFIT_THRESHOLD)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| Decimal::new(5, 3));
        let max_markets_env = std::env::var(KEY_ARB_MONITOR_MAX_MARKETS)
            .ok()
            .and_then(|s| s.parse::<usize>().ok());
        let min_profit_threshold = dynamic_values
            .get(KEY_ARB_MIN_PROFIT_THRESHOLD)
            .copied()
            .or(Some(min_profit_env))
            .and_then(|v| clamp_dynamic_value(KEY_ARB_MIN_PROFIT_THRESHOLD, v, &dynamic_bounds))
            .unwrap_or(min_profit_env);
        let max_markets_cap = dynamic_values
            .get(KEY_ARB_MONITOR_MAX_MARKETS)
            .copied()
            .and_then(|v| clamp_dynamic_value(KEY_ARB_MONITOR_MAX_MARKETS, v, &dynamic_bounds))
            .and_then(decimal_to_cap)
            .or(max_markets_env);
        let aggressiveness_profile = dynamic_values
            .get(KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL)
            .copied()
            .and_then(|v| {
                clamp_dynamic_value(KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL, v, &dynamic_bounds)
            })
            .and_then(|v| v.to_f64())
            .map(AggressivenessProfile::from_level)
            .unwrap_or_else(AggressivenessProfile::from_env);
        let scoring_weights = ScoringWeights::for_profile(aggressiveness_profile);
        let exploration_slots_env = std::env::var(KEY_ARB_MONITOR_EXPLORATION_SLOTS)
            .ok()
            .and_then(|s| s.parse::<usize>().ok());
        let exploration_slots = dynamic_values
            .get(KEY_ARB_MONITOR_EXPLORATION_SLOTS)
            .copied()
            .and_then(|v| {
                clamp_dynamic_value(KEY_ARB_MONITOR_EXPLORATION_SLOTS, v, &dynamic_bounds)
            })
            .and_then(decimal_to_cap)
            .or(exploration_slots_env)
            .unwrap_or(aggressiveness_profile.default_exploration_slots());

        let dynamic_redis_url =
            std::env::var("DYNAMIC_CONFIG_REDIS_URL").unwrap_or_else(|_| config.redis.url.clone());
        let dynamic_config_rx = spawn_dynamic_config_listener(dynamic_redis_url);

        Ok(Self {
            clob_client,
            position_tracker,
            signal_publisher,
            order_books: HashMap::new(),
            market_outcomes: HashMap::new(),
            min_profit_threshold,
            last_signal_time: HashMap::new(),
            aggressiveness_profile,
            scoring_weights,
            max_markets_cap,
            all_market_ids: Vec::new(),
            market_profiles: HashMap::new(),
            market_stats: HashMap::new(),
            eligible_markets: HashSet::new(),
            selection_snapshot: Vec::new(),
            core_market_count: 0,
            exploration_market_count: 0,
            last_rerank_at: None,
            last_resubscribe_at: None,
            exploration_slots,
            dynamic_config_rx,
            dynamic_bounds,
            allowed_dynamic_sources: load_allowed_dynamic_sources(),
        })
    }

    /// Run the monitoring loop.
    pub async fn run(&mut self) -> Result<()> {
        info!("Fetching active markets...");

        // Fetch markets and identify binary markets
        let markets = self.clob_client.get_markets().await?;
        let binary_markets: Vec<_> = markets
            .iter()
            .filter(|m| m.outcomes.len() == 2 && !m.resolved)
            .collect();

        for market in &binary_markets {
            self.market_profiles.insert(
                market.id.clone(),
                MarketProfile {
                    liquidity: market.liquidity.to_f64().unwrap_or(0.0),
                    volume: market.volume.to_f64().unwrap_or(0.0),
                },
            );
        }

        self.all_market_ids = binary_markets.iter().map(|m| m.id.clone()).collect();
        self.all_market_ids.sort_by(|a, b| {
            let score_a = baseline_profile_score(self.market_profiles.get(a));
            let score_b = baseline_profile_score(self.market_profiles.get(b));
            compare_f64_desc(score_a, score_b)
        });
        self.position_tracker.load_active_positions().await?;
        let _ = self.rebuild_eligible_markets();

        info!(
            total_markets = self.all_market_ids.len(),
            active_markets = self.eligible_markets.len(),
            max_cap = ?self.max_markets_cap,
            aggressiveness = ?self.aggressiveness_profile,
            exploration_slots = self.exploration_slots,
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

        // Subscribe to the current active market subset via token IDs.
        let mut updates = self
            .clob_client
            .subscribe_orderbook(self.active_subscription_asset_ids())
            .await?;
        self.last_resubscribe_at = Some(Utc::now());
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
                let target_assets = self.active_subscription_asset_ids();
                info!(
                    asset_count = target_assets.len(),
                    "Resubscribing orderbook stream after dynamic market selection update"
                );
                loop {
                    match self
                        .clob_client
                        .subscribe_orderbook(target_assets.clone())
                        .await
                    {
                        Ok(new_updates) => {
                            updates = new_updates;
                            resets_since_tick += 1;
                            resubscribe_requested = false;
                            self.last_resubscribe_at = Some(Utc::now());
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
                        core_markets: self.core_market_count as f64,
                        exploration_markets: self.exploration_market_count as f64,
                        last_rerank_at: self.last_rerank_at,
                        last_resubscribe_at: self.last_resubscribe_at,
                        selected_markets: self.selection_snapshot.clone(),
                    };
                    if let Err(e) = self.signal_publisher.publish_runtime_stats(&stats).await {
                        warn!(error = %e, "Failed to publish arb runtime stats");
                    }
                    updates_since_tick = 0;
                    stalls_since_tick = 0;
                    resets_since_tick = 0;
                    if self.rebuild_eligible_markets() {
                        info!(
                            active_markets = self.eligible_markets.len(),
                            "Refreshed monitored markets using dynamic opportunity scoring"
                        );
                        resubscribe_requested = true;
                    }
                }
                maybe_update = tokio::time::timeout(StdDuration::from_secs(update_timeout_secs), updates.recv()) => {
                    let Some(update) = (match maybe_update {
                        Ok(update) => update,
                        Err(_) => {
                            stalls_since_tick += 1;
                            warn!(timeout_secs = update_timeout_secs, "No orderbook updates received before timeout; reconnecting websocket subscription");
                            loop {
                                match self.clob_client.subscribe_orderbook(self.active_subscription_asset_ids()).await {
                                    Ok(new_updates) => {
                                        updates = new_updates;
                                        resets_since_tick += 1;
                                        self.last_resubscribe_at = Some(Utc::now());
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
                            match self.clob_client.subscribe_orderbook(self.active_subscription_asset_ids()).await {
                                Ok(new_updates) => {
                                    updates = new_updates;
                                    resets_since_tick += 1;
                                    self.last_resubscribe_at = Some(Utc::now());
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

                    if health_tick.is_multiple_of(100) {
                        crate::touch_health_file();
                    }
                    if health_tick.is_multiple_of(5000) {
                        info!(updates = health_tick, "Arb monitor processed orderbook updates");
                    }
                    // Periodically check for stale positions and publish exit signals
                    if health_tick.is_multiple_of(STALE_CHECK_INTERVAL) {
                        if let Err(e) = self.position_tracker.check_stale_positions().await {
                            warn!(error = %e, "Failed to check stale positions");
                        }
                    }
                }
            }
        }
    }

    fn apply_dynamic_update(&mut self, update: DynamicConfigUpdate) -> bool {
        if !self
            .allowed_dynamic_sources
            .contains(update.source.as_str())
        {
            warn!(
                source = %update.source,
                key = %update.key,
                "Ignoring dynamic update from unauthorized source"
            );
            return false;
        }

        let Some(value) = clamp_dynamic_value(&update.key, update.value, &self.dynamic_bounds)
        else {
            warn!(key = %update.key, "Ignoring unsupported dynamic config key");
            return false;
        };
        if value != update.value {
            warn!(
                key = %update.key,
                source = %update.source,
                old = %update.value,
                new = %value,
                "Clamped dynamic update to configured bounds"
            );
        }

        match update.key.as_str() {
            KEY_ARB_MIN_PROFIT_THRESHOLD => {
                self.min_profit_threshold = value;
                info!(
                    threshold = %self.min_profit_threshold,
                    "Applied dynamic ARB_MIN_PROFIT_THRESHOLD"
                );
                false
            }
            KEY_ARB_MONITOR_MAX_MARKETS => {
                let cap = decimal_to_cap(value);
                self.max_markets_cap = cap;
                let changed = self.rebuild_eligible_markets();
                info!(
                    cap = ?self.max_markets_cap,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_MAX_MARKETS"
                );
                changed
            }
            KEY_ARB_MONITOR_EXPLORATION_SLOTS => {
                let slots = decimal_to_cap(value).unwrap_or(self.exploration_slots.max(1));
                self.exploration_slots = slots;
                let changed = self.rebuild_eligible_markets();
                info!(
                    exploration_slots = self.exploration_slots,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_EXPLORATION_SLOTS"
                );
                changed
            }
            KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL => {
                let level = value.to_f64().unwrap_or(1.0);
                self.aggressiveness_profile = AggressivenessProfile::from_level(level);
                self.scoring_weights = ScoringWeights::for_profile(self.aggressiveness_profile);
                let changed = self.rebuild_eligible_markets();
                info!(
                    aggressiveness = self.aggressiveness_profile.as_str(),
                    level,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_AGGRESSIVENESS_LEVEL"
                );
                changed
            }
            _ => false,
        }
    }

    fn rebuild_eligible_markets(&mut self) -> bool {
        let previous = self.eligible_markets.clone();
        let active_count = self
            .max_markets_cap
            .map(|cap| cap.min(self.all_market_ids.len()))
            .unwrap_or(self.all_market_ids.len());

        let now = Utc::now();
        let selected = select_market_ids(
            &self.all_market_ids,
            &self.market_profiles,
            &self.market_stats,
            &self.eligible_markets,
            self.scoring_weights,
            self.min_profit_threshold,
            active_count,
            self.exploration_slots,
            now,
        );

        let mut next = HashSet::new();
        for market_id in selected.selected_ids {
            if let Some(stats) = self.market_stats.get_mut(&market_id) {
                stats.last_selected_at = Some(now);
            }
            next.insert(market_id);
        }

        // Keep markets with open positions subscribed so exit tracking keeps working.
        for position in self.position_tracker.get_active_positions() {
            next.insert(position.market_id.clone());
        }

        let changed = next != previous;
        self.eligible_markets = next;
        self.selection_snapshot = selected.insights;
        self.core_market_count = selected.core_count;
        self.exploration_market_count = selected.exploration_count;
        self.last_rerank_at = Some(now);
        changed
    }

    fn track_market_evaluation(&mut self, market_id: &str, at: DateTime<Utc>) {
        let stats = self.market_stats.entry(market_id.to_string()).or_default();
        stats.evaluated_books = stats.evaluated_books.saturating_add(1);
        stats.last_seen_at = Some(at);
    }

    fn track_market_opportunity(
        &mut self,
        market_id: &str,
        net_profit: Decimal,
        at: DateTime<Utc>,
    ) {
        let stats = self.market_stats.entry(market_id.to_string()).or_default();
        stats.profitable_books = stats.profitable_books.saturating_add(1);
        stats.last_seen_at = Some(at);

        let profit = net_profit.to_f64().unwrap_or(0.0).max(0.0);
        stats.ewma_net_profit = if stats.profitable_books <= 1 {
            profit
        } else {
            stats.ewma_net_profit * (1.0 - OPPORTUNITY_EWMA_ALPHA) + profit * OPPORTUNITY_EWMA_ALPHA
        };
    }

    fn track_market_signal(&mut self, market_id: &str, at: DateTime<Utc>) {
        let stats = self.market_stats.entry(market_id.to_string()).or_default();
        stats.signaled_books = stats.signaled_books.saturating_add(1);
        stats.last_signal_at = Some(at);
    }

    fn active_subscription_asset_ids(&self) -> Vec<String> {
        let mut assets = HashSet::new();
        for market_id in self
            .all_market_ids
            .iter()
            .filter(|id| self.eligible_markets.contains(*id))
        {
            if let Some((yes_id, no_id)) = self.market_outcomes.get(market_id) {
                assets.insert(yes_id.clone());
                assets.insert(no_id.clone());
            }
        }
        assets.into_iter().collect()
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
        if let Some((yes_id, no_id)) = self.market_outcomes.get(&update.market_id).cloned() {
            let yes_key = (update.market_id.clone(), yes_id);
            let no_key = (update.market_id.clone(), no_id);

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
                let observed_at = Utc::now();
                self.track_market_evaluation(&update.market_id, observed_at);

                // Calculate arbitrage opportunity
                if let Some(arb) =
                    ArbOpportunity::calculate(&binary_book, ArbOpportunity::DEFAULT_FEE)
                {
                    if arb.is_profitable() && has_depth {
                        self.track_market_opportunity(&arb.market_id, arb.net_profit, observed_at);
                    }

                    if eligible_for_entries
                        && arb.is_profitable()
                        && arb.net_profit >= self.min_profit_threshold
                        && has_depth
                    {
                        // Dedup/cooldown: skip if we signaled this market recently
                        let should_signal = match self.last_signal_time.get(&arb.market_id) {
                            Some(last) => {
                                (observed_at - *last).num_seconds() >= SIGNAL_COOLDOWN_SECS
                            }
                            None => true,
                        };

                        if should_signal {
                            self.last_signal_time
                                .insert(arb.market_id.clone(), observed_at);
                            self.track_market_signal(&arb.market_id, observed_at);
                            self.handle_arb_opportunity(&arb, &binary_book).await?;
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

#[derive(Debug, sqlx::FromRow)]
struct DynamicBoundsRow {
    key: String,
    min_value: Decimal,
    max_value: Decimal,
}

#[derive(Debug, sqlx::FromRow)]
struct DynamicValueRow {
    key: String,
    current_value: Decimal,
}

async fn load_dynamic_bounds(pool: &sqlx::PgPool) -> HashMap<String, (Decimal, Decimal)> {
    let rows: Vec<DynamicBoundsRow> = match sqlx::query_as(
        r#"
        SELECT key, min_value, max_value
        FROM dynamic_config
        WHERE enabled = TRUE
        "#,
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "Failed loading dynamic bounds; using fallback bounds");
            return fallback_dynamic_bounds();
        }
    };

    if rows.is_empty() {
        fallback_dynamic_bounds()
    } else {
        rows.into_iter()
            .map(|row| (row.key, (row.min_value, row.max_value)))
            .collect()
    }
}

async fn load_dynamic_values(pool: &sqlx::PgPool) -> HashMap<String, Decimal> {
    let rows: Vec<DynamicValueRow> = match sqlx::query_as(
        r#"
        SELECT key, current_value
        FROM dynamic_config
        WHERE enabled = TRUE
        "#,
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "Failed loading dynamic values; using env defaults");
            return HashMap::new();
        }
    };

    rows.into_iter()
        .map(|row| (row.key, row.current_value))
        .collect()
}

fn load_allowed_dynamic_sources() -> HashSet<String> {
    std::env::var("DYNAMIC_CONFIG_ALLOWED_SOURCES")
        .unwrap_or_else(|_| {
            "dynamic_tuner,dynamic_tuner_rollback,dynamic_tuner_sync,workspace_manual".to_string()
        })
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn clamp_dynamic_value(
    key: &str,
    value: Decimal,
    bounds: &HashMap<String, (Decimal, Decimal)>,
) -> Option<Decimal> {
    let (min, max) = bounds
        .get(key)
        .cloned()
        .or_else(|| fallback_bounds_for_key(key))?;
    Some(value.max(min).min(max))
}

fn decimal_to_cap(value: Decimal) -> Option<usize> {
    value
        .to_u64()
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
}

fn compare_f64_desc(left: f64, right: f64) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

fn baseline_profile_score(profile: Option<&MarketProfile>) -> f64 {
    let Some(profile) = profile else {
        return 0.0;
    };
    let liquidity = profile.liquidity.max(0.0);
    let volume = profile.volume.max(0.0);
    let liquidity_score = (liquidity + 1.0).ln();
    let volume_score = (volume + 1.0).ln();
    liquidity_score * 0.65 + volume_score * 0.35
}

fn recency_bonus(last_seen_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f64 {
    let Some(last_seen_at) = last_seen_at else {
        return 0.0;
    };
    let age_secs = (now - last_seen_at).num_seconds().max(0);
    if age_secs <= 60 {
        0.35
    } else if age_secs <= 5 * 60 {
        0.18
    } else if age_secs <= 15 * 60 {
        0.08
    } else {
        0.0
    }
}

fn market_selection_ranked(
    market_id: &str,
    profiles: &HashMap<String, MarketProfile>,
    stats: &HashMap<String, MarketOpportunityStats>,
    currently_eligible: &HashSet<String>,
    weights: ScoringWeights,
    min_profit_threshold: Decimal,
    now: DateTime<Utc>,
) -> RankedMarket {
    let baseline = baseline_profile_score(profiles.get(market_id));
    let threshold = min_profit_threshold.to_f64().unwrap_or(0.005).max(0.0001);
    let sticky_bonus = if currently_eligible.contains(market_id) {
        weights.selection_sticky_bonus
    } else {
        0.0
    };

    let Some(market_stats) = stats.get(market_id) else {
        let baseline_component = baseline * weights.selection_baseline_weight;
        return RankedMarket {
            market_id: market_id.to_string(),
            total_score: baseline_component + sticky_bonus,
            baseline_score: baseline_component,
            opportunity_score: 0.0,
            hit_rate_score: 0.0,
            freshness_score: 0.0,
            sticky_score: sticky_bonus,
            novelty_score: None,
            rotation_score: None,
            upside_score: None,
        };
    };

    let opportunity_quality = (market_stats.ewma_net_profit / threshold).clamp(0.0, 4.0);
    let confidence = (market_stats.evaluated_books as f64 / 220.0).min(1.0);
    let hit_rate = if market_stats.profitable_books > 0 {
        market_stats.signaled_books as f64 / market_stats.profitable_books as f64
    } else {
        0.0
    };
    let freshness = recency_bonus(market_stats.last_seen_at, now);

    let baseline_component = baseline * weights.selection_baseline_weight;
    let opportunity_component = opportunity_quality * weights.selection_quality_weight;
    let hit_component = (hit_rate * confidence) * weights.selection_hit_rate_weight;
    let freshness_component = freshness * weights.selection_freshness_weight;

    RankedMarket {
        market_id: market_id.to_string(),
        total_score: baseline_component
            + opportunity_component
            + hit_component
            + freshness_component
            + sticky_bonus,
        baseline_score: baseline_component,
        opportunity_score: opportunity_component,
        hit_rate_score: hit_component,
        freshness_score: freshness_component,
        sticky_score: sticky_bonus,
        novelty_score: None,
        rotation_score: None,
        upside_score: None,
    }
}

fn market_exploration_ranked(
    market_id: &str,
    profiles: &HashMap<String, MarketProfile>,
    stats: &HashMap<String, MarketOpportunityStats>,
    currently_eligible: &HashSet<String>,
    weights: ScoringWeights,
    min_profit_threshold: Decimal,
    now: DateTime<Utc>,
) -> RankedMarket {
    let base = market_selection_ranked(
        market_id,
        profiles,
        stats,
        currently_eligible,
        weights,
        min_profit_threshold,
        now,
    );
    let threshold = min_profit_threshold.to_f64().unwrap_or(0.005).max(0.0001);

    let Some(market_stats) = stats.get(market_id) else {
        // Favor unseen markets for discovery.
        return RankedMarket {
            market_id: market_id.to_string(),
            total_score: base.total_score * weights.exploration_baseline_weight
                + weights.exploration_unseen_bonus,
            baseline_score: base.baseline_score * weights.exploration_baseline_weight,
            opportunity_score: base.opportunity_score * weights.exploration_baseline_weight,
            hit_rate_score: base.hit_rate_score * weights.exploration_baseline_weight,
            freshness_score: base.freshness_score * weights.exploration_baseline_weight,
            sticky_score: base.sticky_score * weights.exploration_baseline_weight,
            novelty_score: Some(weights.exploration_unseen_bonus),
            rotation_score: Some(0.0),
            upside_score: Some(0.0),
        };
    };

    let novelty = 1.0 / (1.0 + market_stats.evaluated_books as f64 / 70.0);
    let upside = (market_stats.ewma_net_profit / threshold).clamp(0.0, 3.0) * 0.45;
    let rotation = match market_stats.last_selected_at {
        Some(last) => ((now - last).num_seconds().max(0) as f64 / (30.0 * 60.0)).min(1.0),
        None => 1.0,
    };

    let novelty_component = novelty * weights.exploration_novelty_weight;
    let rotation_component = rotation * weights.exploration_rotation_weight;
    let upside_component = upside * weights.exploration_upside_weight;
    let scaled_base = base.total_score * weights.exploration_baseline_weight;

    RankedMarket {
        market_id: market_id.to_string(),
        total_score: scaled_base + novelty_component + rotation_component + upside_component,
        baseline_score: base.baseline_score * weights.exploration_baseline_weight,
        opportunity_score: base.opportunity_score * weights.exploration_baseline_weight,
        hit_rate_score: base.hit_rate_score * weights.exploration_baseline_weight,
        freshness_score: base.freshness_score * weights.exploration_baseline_weight,
        sticky_score: base.sticky_score * weights.exploration_baseline_weight,
        novelty_score: Some(novelty_component),
        rotation_score: Some(rotation_component),
        upside_score: Some(upside_component),
    }
}

struct MarketSelectionResult {
    selected_ids: Vec<String>,
    insights: Vec<RuntimeMarketInsight>,
    core_count: usize,
    exploration_count: usize,
}

#[allow(clippy::too_many_arguments)]
fn select_market_ids(
    ordered_market_ids: &[String],
    profiles: &HashMap<String, MarketProfile>,
    stats: &HashMap<String, MarketOpportunityStats>,
    currently_eligible: &HashSet<String>,
    weights: ScoringWeights,
    min_profit_threshold: Decimal,
    active_count: usize,
    requested_exploration_slots: usize,
    now: DateTime<Utc>,
) -> MarketSelectionResult {
    if active_count == 0 || ordered_market_ids.is_empty() {
        return MarketSelectionResult {
            selected_ids: Vec::new(),
            insights: Vec::new(),
            core_count: 0,
            exploration_count: 0,
        };
    }

    let mut ranked: Vec<RankedMarket> = ordered_market_ids
        .iter()
        .map(|market_id| {
            market_selection_ranked(
                market_id,
                profiles,
                stats,
                currently_eligible,
                weights,
                min_profit_threshold,
                now,
            )
        })
        .collect();
    ranked.sort_by(|a, b| compare_f64_desc(a.total_score, b.total_score));

    let exploration_slots = requested_exploration_slots.min(active_count.saturating_sub(1));
    let core_slots = active_count.saturating_sub(exploration_slots);

    let mut selected: Vec<RankedMarket> = ranked.iter().take(core_slots).cloned().collect();
    let mut selected_ids: Vec<String> = selected.iter().map(|r| r.market_id.clone()).collect();
    let core_count = selected_ids.len();

    if exploration_slots > 0 {
        let selected_set: HashSet<&str> = selected_ids.iter().map(String::as_str).collect();
        let mut exploration_pool: Vec<RankedMarket> = ranked
            .iter()
            .filter(|ranked_market| !selected_set.contains(ranked_market.market_id.as_str()))
            .map(|ranked_market| {
                market_exploration_ranked(
                    &ranked_market.market_id,
                    profiles,
                    stats,
                    currently_eligible,
                    weights,
                    min_profit_threshold,
                    now,
                )
            })
            .collect();
        exploration_pool.sort_by(|a, b| compare_f64_desc(a.total_score, b.total_score));

        let picked: Vec<RankedMarket> = exploration_pool
            .into_iter()
            .take(exploration_slots)
            .collect();
        selected_ids.extend(picked.iter().map(|p| p.market_id.clone()));
        selected.extend(picked);
    }

    if selected_ids.len() < active_count {
        let selected_set: HashSet<&str> = selected_ids.iter().map(String::as_str).collect();
        let fill_candidates: Vec<RankedMarket> = ranked
            .iter()
            .filter(|market_id| !selected_set.contains(market_id.market_id.as_str()))
            .take(active_count - selected_ids.len())
            .cloned()
            .collect();
        selected_ids.extend(fill_candidates.iter().map(|m| m.market_id.clone()));
        selected.extend(fill_candidates);
    }

    // Keep only unique ids preserving order.
    let mut seen = HashSet::new();
    selected_ids.retain(|id| seen.insert(id.clone()));

    let selected_map: HashMap<String, RankedMarket> = selected
        .into_iter()
        .map(|ranked_market| (ranked_market.market_id.clone(), ranked_market))
        .collect();

    let insights: Vec<RuntimeMarketInsight> = selected_ids
        .iter()
        .take(12)
        .enumerate()
        .filter_map(|(idx, market_id)| {
            selected_map
                .get(market_id)
                .map(|ranked_market| RuntimeMarketInsight {
                    market_id: market_id.clone(),
                    tier: if idx < core_count {
                        "core".to_string()
                    } else {
                        "exploration".to_string()
                    },
                    total_score: ranked_market.total_score,
                    baseline_score: ranked_market.baseline_score,
                    opportunity_score: ranked_market.opportunity_score,
                    hit_rate_score: ranked_market.hit_rate_score,
                    freshness_score: ranked_market.freshness_score,
                    sticky_score: ranked_market.sticky_score,
                    novelty_score: ranked_market.novelty_score,
                    rotation_score: ranked_market.rotation_score,
                    upside_score: ranked_market.upside_score,
                })
        })
        .collect();

    MarketSelectionResult {
        selected_ids,
        insights,
        core_count,
        exploration_count: exploration_slots.min(active_count.saturating_sub(core_count)),
    }
}

fn fallback_dynamic_bounds() -> HashMap<String, (Decimal, Decimal)> {
    let mut map = HashMap::new();
    for key in [
        KEY_ARB_MIN_PROFIT_THRESHOLD,
        KEY_ARB_MONITOR_MAX_MARKETS,
        KEY_ARB_MONITOR_EXPLORATION_SLOTS,
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL,
    ] {
        if let Some(bounds) = fallback_bounds_for_key(key) {
            map.insert(key.to_string(), bounds);
        }
    }
    map
}

fn fallback_bounds_for_key(key: &str) -> Option<(Decimal, Decimal)> {
    match key {
        KEY_ARB_MIN_PROFIT_THRESHOLD => Some((Decimal::new(2, 3), Decimal::new(5, 2))),
        KEY_ARB_MONITOR_MAX_MARKETS => Some((Decimal::new(25, 0), Decimal::new(1500, 0))),
        KEY_ARB_MONITOR_EXPLORATION_SLOTS => Some((Decimal::new(1, 0), Decimal::new(500, 0))),
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL => Some((Decimal::ZERO, Decimal::new(2, 0))),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn profile(liquidity: f64, volume: f64) -> MarketProfile {
        MarketProfile { liquidity, volume }
    }

    #[test]
    fn selection_score_prefers_stronger_opportunity_quality() {
        let now = Utc::now();
        let ordered = vec!["m1".to_string(), "m2".to_string(), "m3".to_string()];

        let profiles = HashMap::from([
            ("m1".to_string(), profile(1_000.0, 2_000.0)),
            ("m2".to_string(), profile(1_000.0, 2_000.0)),
            ("m3".to_string(), profile(900.0, 1_900.0)),
        ]);

        let stats = HashMap::from([
            (
                "m1".to_string(),
                MarketOpportunityStats {
                    evaluated_books: 120,
                    profitable_books: 20,
                    signaled_books: 6,
                    ewma_net_profit: 0.007,
                    last_seen_at: Some(now - Duration::minutes(1)),
                    ..Default::default()
                },
            ),
            (
                "m2".to_string(),
                MarketOpportunityStats {
                    evaluated_books: 120,
                    profitable_books: 20,
                    signaled_books: 9,
                    ewma_net_profit: 0.016,
                    last_seen_at: Some(now - Duration::seconds(30)),
                    ..Default::default()
                },
            ),
        ]);

        let selected = select_market_ids(
            &ordered,
            &profiles,
            &stats,
            &HashSet::new(),
            ScoringWeights::for_profile(AggressivenessProfile::Balanced),
            Decimal::new(5, 3),
            2,
            0,
            now,
        );

        assert_eq!(selected.selected_ids.len(), 2);
        assert_eq!(selected.selected_ids[0], "m2");
    }

    #[test]
    fn exploration_slot_surfaces_unseen_markets() {
        let now = Utc::now();
        let ordered = vec![
            "core-a".to_string(),
            "core-b".to_string(),
            "explore-c".to_string(),
            "tail-d".to_string(),
        ];

        let profiles = HashMap::from([
            ("core-a".to_string(), profile(2_000.0, 5_000.0)),
            ("core-b".to_string(), profile(1_900.0, 4_900.0)),
            ("explore-c".to_string(), profile(1_800.0, 4_200.0)),
            ("tail-d".to_string(), profile(200.0, 300.0)),
        ]);

        let stats = HashMap::from([
            (
                "core-a".to_string(),
                MarketOpportunityStats {
                    evaluated_books: 400,
                    profitable_books: 45,
                    signaled_books: 20,
                    ewma_net_profit: 0.011,
                    last_seen_at: Some(now - Duration::minutes(1)),
                    last_selected_at: Some(now - Duration::seconds(40)),
                    ..Default::default()
                },
            ),
            (
                "core-b".to_string(),
                MarketOpportunityStats {
                    evaluated_books: 350,
                    profitable_books: 38,
                    signaled_books: 16,
                    ewma_net_profit: 0.010,
                    last_seen_at: Some(now - Duration::minutes(2)),
                    last_selected_at: Some(now - Duration::seconds(40)),
                    ..Default::default()
                },
            ),
        ]);

        let selected = select_market_ids(
            &ordered,
            &profiles,
            &stats,
            &HashSet::from(["core-a".to_string(), "core-b".to_string()]),
            ScoringWeights::for_profile(AggressivenessProfile::Balanced),
            Decimal::new(5, 3),
            3,
            1,
            now,
        );

        assert_eq!(selected.selected_ids.len(), 3);
        assert!(selected.selected_ids.contains(&"core-a".to_string()));
        assert!(selected.selected_ids.contains(&"core-b".to_string()));
        assert!(selected.selected_ids.contains(&"explore-c".to_string()));
    }

    #[test]
    fn aggressiveness_profiles_adjust_discovery_bias() {
        let stable = ScoringWeights::for_profile(AggressivenessProfile::Stable);
        let discovery = ScoringWeights::for_profile(AggressivenessProfile::Discovery);

        assert_eq!(AggressivenessProfile::Stable.default_exploration_slots(), 2);
        assert_eq!(
            AggressivenessProfile::Balanced.default_exploration_slots(),
            5
        );
        assert_eq!(
            AggressivenessProfile::Discovery.default_exploration_slots(),
            8
        );
        assert!(discovery.selection_quality_weight > stable.selection_quality_weight);
        assert!(discovery.exploration_novelty_weight > stable.exploration_novelty_weight);
        assert!(discovery.exploration_unseen_bonus > stable.exploration_unseen_bonus);
    }
}
