//! Core arbitrage monitoring logic.

use crate::position_tracker::PositionTracker;
use crate::signals::{channels, RuntimeMarketInsight, RuntimeStats, SignalPublisher};
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use polymarket_core::api::clob::websocket_runtime_stats_snapshot;
use polymarket_core::api::clob::OrderBookUpdate;
use polymarket_core::api::{ClobClient, GammaClient};
use polymarket_core::config::Config;
use polymarket_core::db;
use polymarket_core::types::{ArbOpportunity, BinaryMarketBook, OrderBook};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::time::Duration as StdDuration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Default minimum depth (in USD) at best ask for both sides.
const DEFAULT_MIN_BOOK_DEPTH_USD: Decimal = Decimal::from_parts(100, 0, 0, false, 0); // $100

/// Cooldown period between signals for the same market (seconds).
const SIGNAL_COOLDOWN_SECS: i64 = 60;

/// How often to check for stale positions (every N order book updates).
const STALE_CHECK_INTERVAL: u64 = 500;
/// Minimum number of market replacements before a periodic rerank triggers a resubscribe.
const DEFAULT_SELECTION_RESUBSCRIBE_MIN_MARKET_DELTA: usize = 8;
/// Minimum relative market-set delta before a periodic rerank triggers a resubscribe.
const DEFAULT_SELECTION_RESUBSCRIBE_MIN_DELTA_RATIO: f64 = 0.08;
/// Maximum age of a held selection before a periodic rerank is allowed to refresh it anyway.
const DEFAULT_SELECTION_FORCE_REFRESH_SECS: i64 = 10 * 60;
/// Minimum time to keep an exploration market before allowing routine replacement.
const DEFAULT_EXPLORATION_HOLD_SECS: i64 = 10 * 60;
/// Minimum challenger score edge required to evict a held exploration incumbent.
const DEFAULT_EXPLORATION_SWAP_MIN_SCORE_DELTA: f64 = 0.75;

const KEY_ARB_MIN_PROFIT_THRESHOLD: &str = "ARB_MIN_PROFIT_THRESHOLD";
const KEY_ARB_MONITOR_MAX_MARKETS: &str = "ARB_MONITOR_MAX_MARKETS";
const KEY_ARB_MONITOR_EXPLORATION_SLOTS: &str = "ARB_MONITOR_EXPLORATION_SLOTS";
const KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL: &str = "ARB_MONITOR_AGGRESSIVENESS_LEVEL";
const OPPORTUNITY_EWMA_ALPHA: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, Default)]
struct ArbTelemetryCounters {
    evaluated_books: u64,
    profitable_books: u64,
    eligible_profitable_books: u64,
    filtered_by_selection: u64,
    filtered_by_profit: u64,
    filtered_by_depth: u64,
    filtered_by_cooldown: u64,
    entry_signals: u64,
    near_miss_under_1bp: u64,
    near_miss_under_5bps: u64,
    near_miss_under_25bps: u64,
    near_miss_under_50bps: u64,
    gross_positive_but_net_negative: u64,
    best_gross_profit_bps: f64,
    best_net_profit_bps: f64,
    best_eligible_gross_profit_bps: f64,
    best_eligible_net_profit_bps: f64,
    best_fee_drag_bps: f64,
    closest_threshold_gap_bps: Option<f64>,
}

impl ArbTelemetryCounters {
    fn as_runtime_stats(self) -> ArbTelemetryRuntimeStats {
        ArbTelemetryRuntimeStats {
            evaluated_books_per_minute: self.evaluated_books as f64,
            profitable_books_per_minute: self.profitable_books as f64,
            eligible_profitable_books_per_minute: self.eligible_profitable_books as f64,
            filtered_by_selection_per_minute: self.filtered_by_selection as f64,
            filtered_by_profit_per_minute: self.filtered_by_profit as f64,
            filtered_by_depth_per_minute: self.filtered_by_depth as f64,
            filtered_by_cooldown_per_minute: self.filtered_by_cooldown as f64,
            entry_signals_per_minute: self.entry_signals as f64,
            near_miss_under_1bp_per_minute: self.near_miss_under_1bp as f64,
            near_miss_under_5bps_per_minute: self.near_miss_under_5bps as f64,
            near_miss_under_25bps_per_minute: self.near_miss_under_25bps as f64,
            near_miss_under_50bps_per_minute: self.near_miss_under_50bps as f64,
            gross_positive_but_net_negative_per_minute: self.gross_positive_but_net_negative as f64,
            best_gross_profit_bps_per_minute: self.best_gross_profit_bps,
            best_net_profit_bps_per_minute: self.best_net_profit_bps,
            best_eligible_gross_profit_bps_per_minute: self.best_eligible_gross_profit_bps,
            best_eligible_net_profit_bps_per_minute: self.best_eligible_net_profit_bps,
            best_fee_drag_bps_per_minute: self.best_fee_drag_bps,
            closest_threshold_gap_bps_per_minute: self.closest_threshold_gap_bps,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ArbTelemetryRuntimeStats {
    evaluated_books_per_minute: f64,
    profitable_books_per_minute: f64,
    eligible_profitable_books_per_minute: f64,
    filtered_by_selection_per_minute: f64,
    filtered_by_profit_per_minute: f64,
    filtered_by_depth_per_minute: f64,
    filtered_by_cooldown_per_minute: f64,
    entry_signals_per_minute: f64,
    near_miss_under_1bp_per_minute: f64,
    near_miss_under_5bps_per_minute: f64,
    near_miss_under_25bps_per_minute: f64,
    near_miss_under_50bps_per_minute: f64,
    gross_positive_but_net_negative_per_minute: f64,
    best_gross_profit_bps_per_minute: f64,
    best_net_profit_bps_per_minute: f64,
    best_eligible_gross_profit_bps_per_minute: f64,
    best_eligible_net_profit_bps_per_minute: f64,
    best_fee_drag_bps_per_minute: f64,
    closest_threshold_gap_bps_per_minute: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
enum SelectionRebuildReason {
    Startup,
    DynamicConfig,
    PeriodicRerank,
}

#[derive(Debug, Clone, Copy, Default)]
struct SelectionRebuildOutcome {
    applied: bool,
    changed: bool,
    suppressed: bool,
    market_delta: usize,
    asset_delta: usize,
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
    gamma_client: GammaClient,
    position_tracker: PositionTracker,
    signal_publisher: SignalPublisher,
    /// Current order books by (market_id, outcome_id).
    order_books: HashMap<(String, String), OrderBook>,
    /// Market outcome pairings (market_id -> (yes_outcome_id, no_outcome_id)).
    market_outcomes: HashMap<String, (String, String)>,
    /// Minimum net profit threshold for entry signals.
    min_profit_threshold: Decimal,
    /// Minimum order book depth required on both sides before signaling.
    min_book_depth: Decimal,
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
    /// Whether a market currently uses Polymarket's fee-enabled pricing model.
    market_fees_enabled: HashMap<String, bool>,
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
    /// Markets currently occupying exploration slots in the live selection.
    current_exploration_markets: HashSet<String>,
    /// Last time market selection was re-ranked.
    last_rerank_at: Option<DateTime<Utc>>,
    /// Last time a reranked selection was actually applied to the live subscription.
    last_selection_apply_at: Option<DateTime<Utc>>,
    /// Last time the websocket subscription was rebuilt.
    last_resubscribe_at: Option<DateTime<Utc>>,
    /// Number of slots dedicated to exploration.
    exploration_slots: usize,
    /// Absolute market-set delta needed before a periodic rerank swaps subscriptions.
    selection_resubscribe_min_market_delta: usize,
    /// Relative market-set delta needed before a periodic rerank swaps subscriptions.
    selection_resubscribe_min_delta_ratio: f64,
    /// Maximum time to hold a stable selection before allowing a periodic refresh.
    selection_force_refresh_secs: i64,
    /// Minimum time to keep an exploration incumbent before allowing routine rotation.
    exploration_hold_secs: i64,
    /// Required score advantage before replacing a held exploration incumbent.
    exploration_swap_min_score_delta: f64,
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
        let gamma_client = GammaClient::new(None);

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
        let min_book_depth = std::env::var("ARB_MONITOR_MIN_BOOK_DEPTH")
            .or_else(|_| std::env::var("ARB_MIN_BOOK_DEPTH"))
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MIN_BOOK_DEPTH_USD);
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
        let selection_resubscribe_min_market_delta =
            std::env::var("ARB_SELECTION_RESUBSCRIBE_MIN_MARKET_DELTA")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(DEFAULT_SELECTION_RESUBSCRIBE_MIN_MARKET_DELTA);
        let selection_resubscribe_min_delta_ratio =
            std::env::var("ARB_SELECTION_RESUBSCRIBE_MIN_DELTA_RATIO")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(DEFAULT_SELECTION_RESUBSCRIBE_MIN_DELTA_RATIO);
        let selection_force_refresh_secs = std::env::var("ARB_SELECTION_FORCE_REFRESH_SECS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SELECTION_FORCE_REFRESH_SECS);
        let exploration_hold_secs = std::env::var("ARB_EXPLORATION_HOLD_SECS")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_EXPLORATION_HOLD_SECS);
        let exploration_swap_min_score_delta =
            std::env::var("ARB_EXPLORATION_SWAP_MIN_SCORE_DELTA")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(DEFAULT_EXPLORATION_SWAP_MIN_SCORE_DELTA);

        let dynamic_redis_url =
            std::env::var("DYNAMIC_CONFIG_REDIS_URL").unwrap_or_else(|_| config.redis.url.clone());
        let dynamic_config_rx = spawn_dynamic_config_listener(dynamic_redis_url);

        Ok(Self {
            clob_client,
            gamma_client,
            position_tracker,
            signal_publisher,
            order_books: HashMap::new(),
            market_outcomes: HashMap::new(),
            min_profit_threshold,
            min_book_depth,
            last_signal_time: HashMap::new(),
            aggressiveness_profile,
            scoring_weights,
            max_markets_cap,
            all_market_ids: Vec::new(),
            market_profiles: HashMap::new(),
            market_fees_enabled: HashMap::new(),
            market_stats: HashMap::new(),
            eligible_markets: HashSet::new(),
            selection_snapshot: Vec::new(),
            core_market_count: 0,
            exploration_market_count: 0,
            current_exploration_markets: HashSet::new(),
            last_rerank_at: None,
            last_selection_apply_at: None,
            last_resubscribe_at: None,
            exploration_slots,
            selection_resubscribe_min_market_delta,
            selection_resubscribe_min_delta_ratio,
            selection_force_refresh_secs,
            exploration_hold_secs,
            exploration_swap_min_score_delta,
            dynamic_config_rx,
            dynamic_bounds,
            allowed_dynamic_sources: load_allowed_dynamic_sources(),
        })
    }

    /// Run the monitoring loop.
    pub async fn run(&mut self) -> Result<()> {
        info!("Fetching active markets...");

        // Fetch tradable markets from Gamma, which is authoritative for active
        // discovery. CLOB `/markets?active=true` currently includes many closed
        // historical markets and starves the websocket selection.
        let gamma_page_size = std::env::var("GAMMA_ARB_MARKET_PAGE_SIZE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(200);
        let markets = self
            .gamma_client
            .get_all_tradable_markets(gamma_page_size)
            .await?;
        let binary_markets: Vec<_> = markets.iter().filter(|m| m.outcomes.len() == 2).collect();

        for market in &binary_markets {
            self.market_profiles.insert(
                market.id.clone(),
                MarketProfile {
                    liquidity: market.liquidity.to_f64().unwrap_or(0.0),
                    volume: market.volume.to_f64().unwrap_or(0.0),
                },
            );
            self.market_fees_enabled
                .insert(market.id.clone(), market.fees_enabled);
        }

        self.all_market_ids = binary_markets.iter().map(|m| m.id.clone()).collect();
        self.all_market_ids.sort_by(|a, b| {
            let score_a = baseline_profile_score(self.market_profiles.get(a));
            let score_b = baseline_profile_score(self.market_profiles.get(b));
            compare_f64_desc(score_a, score_b)
        });
        self.position_tracker.load_active_positions().await?;
        let _ = self.rebuild_eligible_markets(SelectionRebuildReason::Startup);

        info!(
            total_markets = self.all_market_ids.len(),
            fee_enabled_markets = self
                .market_fees_enabled
                .values()
                .filter(|enabled| **enabled)
                .count(),
            active_markets = self.eligible_markets.len(),
            active_assets = self.active_subscription_asset_count(),
            max_cap = ?self.max_markets_cap,
            aggressiveness = ?self.aggressiveness_profile,
            exploration_slots = self.exploration_slots,
            selection_min_market_delta = self.selection_resubscribe_min_market_delta,
            selection_min_delta_ratio = self.selection_resubscribe_min_delta_ratio,
            selection_force_refresh_secs = self.selection_force_refresh_secs,
            min_profit_threshold = %self.min_profit_threshold,
            min_book_depth = %self.min_book_depth,
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
        // Heartbeat ticker keeps the health file fresh even when no orderbook
        // updates are flowing (e.g. WebSocket hung). Without this, liveness
        // depends entirely on update volume.
        let mut heartbeat_tick = tokio::time::interval(tokio::time::Duration::from_secs(60));
        heartbeat_tick.tick().await;

        let mut updates_since_tick = 0u64;
        let mut stalls_since_tick = 0u64;
        let mut resets_since_tick = 0u64;
        let mut arb_telemetry = ArbTelemetryCounters::default();
        let mut selection_applied_since_tick = 0u64;
        let mut selection_suppressed_since_tick = 0u64;
        let mut last_selection_market_delta = 0usize;
        let mut last_selection_asset_delta = 0usize;
        let mut resubscribe_requested = false;
        let mut ws_runtime_prev = websocket_runtime_stats_snapshot();

        const MAX_RESUBSCRIBE_RETRIES: u32 = 10;
        const RESUBSCRIBE_BASE_DELAY_SECS: u64 = 3;
        const RESUBSCRIBE_MAX_DELAY_SECS: u64 = 60;

        loop {
            if resubscribe_requested {
                let target_assets = self.active_subscription_asset_ids();
                info!(
                    asset_count = target_assets.len(),
                    "Resubscribing orderbook stream after dynamic market selection update"
                );
                let mut attempt = 0u32;
                loop {
                    attempt += 1;
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
                            let delay = RESUBSCRIBE_BASE_DELAY_SECS
                                .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)))
                                .min(RESUBSCRIBE_MAX_DELAY_SECS);
                            if attempt >= MAX_RESUBSCRIBE_RETRIES {
                                warn!(
                                    error = %e,
                                    attempts = attempt,
                                    "Resubscribe failed after max retries, deferring to next select! cycle"
                                );
                                // Leave resubscribe_requested=true so the next
                                // select! iteration will retry, keeping the
                                // dynamic_config_rx channel responsive.
                                break;
                            }
                            warn!(
                                error = %e,
                                attempt,
                                max_retries = MAX_RESUBSCRIBE_RETRIES,
                                retry_delay_secs = delay,
                                "Failed resubscribing orderbook stream, retrying"
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                        }
                    }
                }
            }

            tokio::select! {
                maybe_cfg = self.dynamic_config_rx.recv() => {
                    if let Some(update) = maybe_cfg {
                        let outcome = self.apply_dynamic_update(update);
                        if outcome.applied {
                            selection_applied_since_tick = selection_applied_since_tick.saturating_add(1);
                        }
                        if outcome.suppressed {
                            selection_suppressed_since_tick = selection_suppressed_since_tick.saturating_add(1);
                        }
                        if outcome.changed {
                            last_selection_market_delta = outcome.market_delta;
                            last_selection_asset_delta = outcome.asset_delta;
                        }
                        if outcome.applied {
                            resubscribe_requested = true;
                        }
                    }
                }
                _ = heartbeat_tick.tick() => {
                    crate::touch_health_file();
                }
                _ = stats_tick.tick() => {
                    let arb_runtime = arb_telemetry.as_runtime_stats();
                    let ws_runtime = websocket_runtime_stats_snapshot();
                    let monitored_assets = self.active_subscription_asset_count() as f64;
                    let stats = RuntimeStats {
                        updates_per_minute: updates_since_tick as f64,
                        stalls_last_minute: stalls_since_tick as f64,
                        resets_last_minute: resets_since_tick as f64,
                        ws_text_messages_per_minute: ws_runtime
                            .text_messages_received_total
                            .saturating_sub(ws_runtime_prev.text_messages_received_total)
                            as f64,
                        ws_orderbook_updates_per_minute: ws_runtime
                            .orderbook_updates_emitted_total
                            .saturating_sub(ws_runtime_prev.orderbook_updates_emitted_total)
                            as f64,
                        ws_parse_misses_per_minute: ws_runtime
                            .parse_misses_total
                            .saturating_sub(ws_runtime_prev.parse_misses_total)
                            as f64,
                        ws_snapshot_messages_per_minute: ws_runtime
                            .snapshot_messages_total
                            .saturating_sub(ws_runtime_prev.snapshot_messages_total)
                            as f64,
                        ws_price_change_messages_per_minute: ws_runtime
                            .price_change_messages_total
                            .saturating_sub(ws_runtime_prev.price_change_messages_total)
                            as f64,
                        monitored_markets: self.eligible_markets.len() as f64,
                        monitored_assets,
                        evaluated_books_per_minute: arb_runtime.evaluated_books_per_minute,
                        profitable_books_per_minute: arb_runtime.profitable_books_per_minute,
                        eligible_profitable_books_per_minute: arb_runtime.eligible_profitable_books_per_minute,
                        filtered_by_selection_per_minute: arb_runtime.filtered_by_selection_per_minute,
                        filtered_by_profit_per_minute: arb_runtime.filtered_by_profit_per_minute,
                        filtered_by_depth_per_minute: arb_runtime.filtered_by_depth_per_minute,
                        filtered_by_cooldown_per_minute: arb_runtime.filtered_by_cooldown_per_minute,
                        entry_signals_per_minute: arb_runtime.entry_signals_per_minute,
                        near_miss_under_1bp_per_minute: arb_runtime.near_miss_under_1bp_per_minute,
                        near_miss_under_5bps_per_minute: arb_runtime.near_miss_under_5bps_per_minute,
                        near_miss_under_25bps_per_minute: arb_runtime.near_miss_under_25bps_per_minute,
                        near_miss_under_50bps_per_minute: arb_runtime.near_miss_under_50bps_per_minute,
                        gross_positive_but_net_negative_per_minute: arb_runtime
                            .gross_positive_but_net_negative_per_minute,
                        best_gross_profit_bps_per_minute: arb_runtime
                            .best_gross_profit_bps_per_minute,
                        best_net_profit_bps_per_minute: arb_runtime.best_net_profit_bps_per_minute,
                        best_eligible_gross_profit_bps_per_minute: arb_runtime
                            .best_eligible_gross_profit_bps_per_minute,
                        best_eligible_net_profit_bps_per_minute: arb_runtime.best_eligible_net_profit_bps_per_minute,
                        best_fee_drag_bps_per_minute: arb_runtime.best_fee_drag_bps_per_minute,
                        closest_threshold_gap_bps_per_minute: arb_runtime.closest_threshold_gap_bps_per_minute,
                        selection_refreshes_applied_per_minute: selection_applied_since_tick as f64,
                        selection_refreshes_suppressed_per_minute: selection_suppressed_since_tick as f64,
                        last_selection_market_delta: last_selection_market_delta as f64,
                        last_selection_asset_delta: last_selection_asset_delta as f64,
                        core_markets: self.core_market_count as f64,
                        exploration_markets: self.exploration_market_count as f64,
                        last_rerank_at: self.last_rerank_at,
                        last_resubscribe_at: self.last_resubscribe_at,
                        ws_last_message_at: ws_runtime.last_message_at,
                        ws_last_orderbook_update_at: ws_runtime.last_orderbook_update_at,
                        ws_last_parse_miss_at: ws_runtime.last_parse_miss_at,
                        ws_last_parse_miss_kind: ws_runtime.last_parse_miss_kind.clone(),
                        ws_last_message_kind: ws_runtime.last_message_kind.clone(),
                        selected_markets: self.selection_snapshot.clone(),
                    };
                    if let Err(e) = self.signal_publisher.publish_runtime_stats(&stats).await {
                        warn!(error = %e, "Failed to publish arb runtime stats");
                    }
                    info!(
                        monitored_markets = self.eligible_markets.len(),
                        monitored_assets = monitored_assets as u64,
                        min_profit_threshold = %self.min_profit_threshold,
                        min_book_depth = %self.min_book_depth,
                        updates = updates_since_tick,
                        evaluated_books = arb_telemetry.evaluated_books,
                        profitable_books = arb_telemetry.profitable_books,
                        eligible_profitable_books = arb_telemetry.eligible_profitable_books,
                        filtered_by_selection = arb_telemetry.filtered_by_selection,
                        filtered_by_profit = arb_telemetry.filtered_by_profit,
                        filtered_by_depth = arb_telemetry.filtered_by_depth,
                        filtered_by_cooldown = arb_telemetry.filtered_by_cooldown,
                        entry_signals = arb_telemetry.entry_signals,
                        near_miss_under_1bp = arb_telemetry.near_miss_under_1bp,
                        near_miss_under_5bps = arb_telemetry.near_miss_under_5bps,
                        near_miss_under_25bps = arb_telemetry.near_miss_under_25bps,
                        near_miss_under_50bps = arb_telemetry.near_miss_under_50bps,
                        gross_positive_but_net_negative = arb_telemetry.gross_positive_but_net_negative,
                        best_gross_profit_bps = stats.best_gross_profit_bps_per_minute,
                        best_net_profit_bps = stats.best_net_profit_bps_per_minute,
                        best_eligible_gross_profit_bps = stats.best_eligible_gross_profit_bps_per_minute,
                        best_eligible_net_profit_bps = stats.best_eligible_net_profit_bps_per_minute,
                        best_fee_drag_bps = stats.best_fee_drag_bps_per_minute,
                        closest_threshold_gap_bps = ?stats.closest_threshold_gap_bps_per_minute,
                        selection_refreshes_applied = selection_applied_since_tick,
                        selection_refreshes_suppressed = selection_suppressed_since_tick,
                        last_selection_market_delta,
                        last_selection_asset_delta,
                        ws_text_messages = stats.ws_text_messages_per_minute as u64,
                        ws_orderbook_updates = stats.ws_orderbook_updates_per_minute as u64,
                        ws_parse_misses = stats.ws_parse_misses_per_minute as u64,
                        ws_snapshot_messages = stats.ws_snapshot_messages_per_minute as u64,
                        ws_price_change_messages = stats.ws_price_change_messages_per_minute as u64,
                        ws_last_message_at = ?stats.ws_last_message_at,
                        ws_last_orderbook_update_at = ?stats.ws_last_orderbook_update_at,
                        ws_last_parse_miss_at = ?stats.ws_last_parse_miss_at,
                        ws_last_parse_miss_kind = ?stats.ws_last_parse_miss_kind,
                        ws_last_message_kind = ?stats.ws_last_message_kind,
                        stalls = stalls_since_tick,
                        resets = resets_since_tick,
                        "Arb monitor minute telemetry"
                    );
                    ws_runtime_prev = ws_runtime;
                    updates_since_tick = 0;
                    stalls_since_tick = 0;
                    resets_since_tick = 0;
                    arb_telemetry = ArbTelemetryCounters::default();
                    selection_applied_since_tick = 0;
                    selection_suppressed_since_tick = 0;
                    last_selection_market_delta = 0;
                    last_selection_asset_delta = 0;
                    let outcome = self.rebuild_eligible_markets(SelectionRebuildReason::PeriodicRerank);
                    if outcome.applied {
                        selection_applied_since_tick = selection_applied_since_tick.saturating_add(1);
                        last_selection_market_delta = outcome.market_delta;
                        last_selection_asset_delta = outcome.asset_delta;
                        info!(
                            active_markets = self.eligible_markets.len(),
                            "Refreshed monitored markets using dynamic opportunity scoring"
                        );
                        resubscribe_requested = true;
                    } else if outcome.suppressed {
                        selection_suppressed_since_tick = selection_suppressed_since_tick.saturating_add(1);
                        last_selection_market_delta = outcome.market_delta;
                        last_selection_asset_delta = outcome.asset_delta;
                        info!(
                            market_delta = outcome.market_delta,
                            asset_delta = outcome.asset_delta,
                            threshold_markets = self.selection_resubscribe_min_market_delta,
                            threshold_ratio = self.selection_resubscribe_min_delta_ratio,
                            hold_secs = self.selection_force_refresh_secs,
                            "Suppressed periodic selection refresh due to immaterial delta"
                        );
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
                    self.process_update(update, &mut arb_telemetry).await?;
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

    fn apply_dynamic_update(&mut self, update: DynamicConfigUpdate) -> SelectionRebuildOutcome {
        if !self
            .allowed_dynamic_sources
            .contains(update.source.as_str())
        {
            warn!(
                source = %update.source,
                key = %update.key,
                "Ignoring dynamic update from unauthorized source"
            );
            return SelectionRebuildOutcome::default();
        }

        let Some(value) = clamp_dynamic_value(&update.key, update.value, &self.dynamic_bounds)
        else {
            warn!(key = %update.key, "Ignoring unsupported dynamic config key");
            return SelectionRebuildOutcome::default();
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
                if self.min_profit_threshold == value {
                    debug!(
                        threshold = %self.min_profit_threshold,
                        "Ignoring no-op dynamic ARB_MIN_PROFIT_THRESHOLD update"
                    );
                    return SelectionRebuildOutcome::default();
                }
                self.min_profit_threshold = value;
                info!(
                    threshold = %self.min_profit_threshold,
                    "Applied dynamic ARB_MIN_PROFIT_THRESHOLD"
                );
                SelectionRebuildOutcome::default()
            }
            KEY_ARB_MONITOR_MAX_MARKETS => {
                let cap = decimal_to_cap(value);
                if self.max_markets_cap == cap {
                    debug!(
                        cap = ?self.max_markets_cap,
                        "Ignoring no-op dynamic ARB_MONITOR_MAX_MARKETS update"
                    );
                    return SelectionRebuildOutcome::default();
                }
                self.max_markets_cap = cap;
                let outcome = self.rebuild_eligible_markets(SelectionRebuildReason::DynamicConfig);
                info!(
                    cap = ?self.max_markets_cap,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_MAX_MARKETS"
                );
                outcome
            }
            KEY_ARB_MONITOR_EXPLORATION_SLOTS => {
                let slots = decimal_to_cap(value).unwrap_or(self.exploration_slots.max(1));
                if self.exploration_slots == slots {
                    debug!(
                        exploration_slots = self.exploration_slots,
                        "Ignoring no-op dynamic ARB_MONITOR_EXPLORATION_SLOTS update"
                    );
                    return SelectionRebuildOutcome::default();
                }
                self.exploration_slots = slots;
                let outcome = self.rebuild_eligible_markets(SelectionRebuildReason::DynamicConfig);
                info!(
                    exploration_slots = self.exploration_slots,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_EXPLORATION_SLOTS"
                );
                outcome
            }
            KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL => {
                let level = value.to_f64().unwrap_or(1.0);
                let profile = AggressivenessProfile::from_level(level);
                if self.aggressiveness_profile == profile {
                    debug!(
                        aggressiveness = self.aggressiveness_profile.as_str(),
                        level, "Ignoring no-op dynamic ARB_MONITOR_AGGRESSIVENESS_LEVEL update"
                    );
                    return SelectionRebuildOutcome::default();
                }
                self.aggressiveness_profile = profile;
                self.scoring_weights = ScoringWeights::for_profile(self.aggressiveness_profile);
                let outcome = self.rebuild_eligible_markets(SelectionRebuildReason::DynamicConfig);
                info!(
                    aggressiveness = self.aggressiveness_profile.as_str(),
                    level,
                    active_markets = self.eligible_markets.len(),
                    "Applied dynamic ARB_MONITOR_AGGRESSIVENESS_LEVEL"
                );
                outcome
            }
            _ => SelectionRebuildOutcome::default(),
        }
    }

    fn rebuild_eligible_markets(
        &mut self,
        reason: SelectionRebuildReason,
    ) -> SelectionRebuildOutcome {
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
            &self.current_exploration_markets,
            self.scoring_weights,
            self.min_profit_threshold,
            active_count,
            self.exploration_slots,
            self.exploration_hold_secs,
            self.exploration_swap_min_score_delta,
            now,
        );

        let mut next = HashSet::new();
        for market_id in selected.selected_ids {
            next.insert(market_id);
        }

        // Keep markets with open positions subscribed so exit tracking keeps working.
        for position in self.position_tracker.get_active_positions() {
            next.insert(position.market_id.clone());
        }

        self.last_rerank_at = Some(now);
        let changed = next != previous;
        if !changed {
            self.mark_markets_selected(previous.iter(), now);
            return SelectionRebuildOutcome::default();
        }

        let market_delta = previous.symmetric_difference(&next).count();
        let asset_delta = self.selection_asset_delta(&previous, &next);
        let forced = matches!(
            reason,
            SelectionRebuildReason::Startup | SelectionRebuildReason::DynamicConfig
        );
        let ratio_threshold =
            (previous.len() as f64 * self.selection_resubscribe_min_delta_ratio).ceil() as usize;
        let material_market_delta = market_delta
            >= self
                .selection_resubscribe_min_market_delta
                .max(ratio_threshold.max(1));
        let hold_expired = self
            .last_selection_apply_at
            .map(|last| (now - last).num_seconds() >= self.selection_force_refresh_secs)
            .unwrap_or(true);

        if forced || material_market_delta || hold_expired {
            self.eligible_markets = next;
            self.selection_snapshot = selected.insights;
            self.core_market_count = selected.core_count;
            self.exploration_market_count = selected.exploration_count;
            self.current_exploration_markets = selected.exploration_ids.iter().cloned().collect();
            self.last_selection_apply_at = Some(now);
            let applied_markets: Vec<String> = self.eligible_markets.iter().cloned().collect();
            self.mark_markets_selected(applied_markets.iter(), now);
            SelectionRebuildOutcome {
                applied: true,
                changed: true,
                suppressed: false,
                market_delta,
                asset_delta,
            }
        } else {
            self.mark_markets_selected(previous.iter(), now);
            SelectionRebuildOutcome {
                applied: false,
                changed: true,
                suppressed: true,
                market_delta,
                asset_delta,
            }
        }
    }

    fn selection_asset_delta(
        &self,
        previous_markets: &HashSet<String>,
        next_markets: &HashSet<String>,
    ) -> usize {
        let mut previous_assets = HashSet::new();
        for market_id in previous_markets {
            if let Some((yes_id, no_id)) = self.market_outcomes.get(market_id) {
                previous_assets.insert(yes_id.as_str());
                previous_assets.insert(no_id.as_str());
            }
        }

        let mut next_assets = HashSet::new();
        for market_id in next_markets {
            if let Some((yes_id, no_id)) = self.market_outcomes.get(market_id) {
                next_assets.insert(yes_id.as_str());
                next_assets.insert(no_id.as_str());
            }
        }

        previous_assets.symmetric_difference(&next_assets).count()
    }

    fn mark_markets_selected<'a, I>(&mut self, market_ids: I, at: DateTime<Utc>)
    where
        I: IntoIterator<Item = &'a String>,
    {
        for market_id in market_ids {
            if let Some(stats) = self.market_stats.get_mut(market_id) {
                stats.last_selected_at = Some(at);
            }
        }
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

    fn active_subscription_asset_count(&self) -> usize {
        self.active_subscription_asset_ids().len()
    }

    /// Process an order book update.
    async fn process_update(
        &mut self,
        update: OrderBookUpdate,
        arb_telemetry: &mut ArbTelemetryCounters,
    ) -> Result<()> {
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
                let has_depth = binary_book
                    .entry_cost_with_depth(self.min_book_depth)
                    .is_some();
                let fees_enabled = self
                    .market_fees_enabled
                    .get(&update.market_id)
                    .copied()
                    .unwrap_or(false);
                let observed_at = Utc::now();
                self.track_market_evaluation(&update.market_id, observed_at);
                arb_telemetry.evaluated_books = arb_telemetry.evaluated_books.saturating_add(1);

                // Calculate arbitrage opportunity
                if let Some(arb) =
                    ArbOpportunity::calculate_with_fees_enabled(&binary_book, fees_enabled)
                {
                    let gross_profit_bps = arb.gross_profit.to_f64().unwrap_or(0.0) * 10_000.0;
                    let net_profit_bps = arb.net_profit.to_f64().unwrap_or(0.0) * 10_000.0;
                    let fee_drag_bps = arb.fee_drag.to_f64().unwrap_or(0.0) * 10_000.0;
                    arb_telemetry.best_gross_profit_bps =
                        arb_telemetry.best_gross_profit_bps.max(gross_profit_bps);
                    arb_telemetry.best_net_profit_bps =
                        arb_telemetry.best_net_profit_bps.max(net_profit_bps);
                    arb_telemetry.best_fee_drag_bps =
                        arb_telemetry.best_fee_drag_bps.max(fee_drag_bps);
                    if eligible_for_entries && has_depth {
                        arb_telemetry.best_eligible_gross_profit_bps = arb_telemetry
                            .best_eligible_gross_profit_bps
                            .max(gross_profit_bps);
                        arb_telemetry.best_eligible_net_profit_bps = arb_telemetry
                            .best_eligible_net_profit_bps
                            .max(net_profit_bps);
                        let threshold_gap_bps = ((self.min_profit_threshold - arb.net_profit)
                            .max(Decimal::ZERO))
                        .to_f64()
                        .unwrap_or(0.0)
                            * 10_000.0;
                        if threshold_gap_bps > 0.0 {
                            arb_telemetry.closest_threshold_gap_bps = Some(
                                arb_telemetry
                                    .closest_threshold_gap_bps
                                    .map(|prev| prev.min(threshold_gap_bps))
                                    .unwrap_or(threshold_gap_bps),
                            );
                            if threshold_gap_bps <= 1.0 {
                                arb_telemetry.near_miss_under_1bp =
                                    arb_telemetry.near_miss_under_1bp.saturating_add(1);
                            }
                            if threshold_gap_bps <= 5.0 {
                                arb_telemetry.near_miss_under_5bps =
                                    arb_telemetry.near_miss_under_5bps.saturating_add(1);
                            }
                            if threshold_gap_bps <= 25.0 {
                                arb_telemetry.near_miss_under_25bps =
                                    arb_telemetry.near_miss_under_25bps.saturating_add(1);
                            }
                            if threshold_gap_bps <= 50.0 {
                                arb_telemetry.near_miss_under_50bps =
                                    arb_telemetry.near_miss_under_50bps.saturating_add(1);
                            }
                        }
                    }

                    if arb.is_profitable() {
                        arb_telemetry.profitable_books =
                            arb_telemetry.profitable_books.saturating_add(1);
                        if has_depth {
                            self.track_market_opportunity(
                                &arb.market_id,
                                arb.net_profit,
                                observed_at,
                            );
                        }
                    } else if arb.gross_profit > Decimal::ZERO {
                        arb_telemetry.gross_positive_but_net_negative = arb_telemetry
                            .gross_positive_but_net_negative
                            .saturating_add(1);
                    }

                    if arb.is_profitable() {
                        if !eligible_for_entries {
                            arb_telemetry.filtered_by_selection =
                                arb_telemetry.filtered_by_selection.saturating_add(1);
                        } else {
                            arb_telemetry.eligible_profitable_books =
                                arb_telemetry.eligible_profitable_books.saturating_add(1);
                            if arb.net_profit < self.min_profit_threshold {
                                arb_telemetry.filtered_by_profit =
                                    arb_telemetry.filtered_by_profit.saturating_add(1);
                            } else if !has_depth {
                                arb_telemetry.filtered_by_depth =
                                    arb_telemetry.filtered_by_depth.saturating_add(1);
                            } else {
                                // Dedup/cooldown: skip if we signaled this market recently
                                let should_signal = match self.last_signal_time.get(&arb.market_id)
                                {
                                    Some(last) => {
                                        (observed_at - *last).num_seconds() >= SIGNAL_COOLDOWN_SECS
                                    }
                                    None => true,
                                };

                                if should_signal {
                                    self.last_signal_time
                                        .insert(arb.market_id.clone(), observed_at);
                                    self.track_market_signal(&arb.market_id, observed_at);
                                    arb_telemetry.entry_signals =
                                        arb_telemetry.entry_signals.saturating_add(1);
                                    self.handle_arb_opportunity(&arb, &binary_book).await?;
                                } else {
                                    arb_telemetry.filtered_by_cooldown =
                                        arb_telemetry.filtered_by_cooldown.saturating_add(1);
                                }
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
    exploration_ids: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn select_market_ids(
    ordered_market_ids: &[String],
    profiles: &HashMap<String, MarketProfile>,
    stats: &HashMap<String, MarketOpportunityStats>,
    currently_eligible: &HashSet<String>,
    current_exploration_markets: &HashSet<String>,
    weights: ScoringWeights,
    min_profit_threshold: Decimal,
    active_count: usize,
    requested_exploration_slots: usize,
    exploration_hold_secs: i64,
    exploration_swap_min_score_delta: f64,
    now: DateTime<Utc>,
) -> MarketSelectionResult {
    if active_count == 0 || ordered_market_ids.is_empty() {
        return MarketSelectionResult {
            selected_ids: Vec::new(),
            insights: Vec::new(),
            core_count: 0,
            exploration_count: 0,
            exploration_ids: Vec::new(),
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
        let exploration_map: HashMap<String, RankedMarket> = exploration_pool
            .iter()
            .cloned()
            .map(|ranked_market| (ranked_market.market_id.clone(), ranked_market))
            .collect();
        let mut incumbents: Vec<RankedMarket> = current_exploration_markets
            .iter()
            .filter_map(|market_id| exploration_map.get(market_id).cloned())
            .collect();
        incumbents.sort_by(|a, b| compare_f64_desc(a.total_score, b.total_score));
        let challengers: Vec<RankedMarket> = exploration_pool
            .into_iter()
            .filter(|ranked_market| !current_exploration_markets.contains(&ranked_market.market_id))
            .collect();

        let mut picked: Vec<RankedMarket> = Vec::new();
        let mut used_market_ids: HashSet<String> = selected_ids.iter().cloned().collect();
        let mut incumbent_idx = 0usize;
        let mut challenger_idx = 0usize;

        for _ in 0..exploration_slots {
            let incumbent =
                next_available_ranked(&incumbents, &mut incumbent_idx, &used_market_ids);
            let challenger =
                next_available_ranked(&challengers, &mut challenger_idx, &used_market_ids);
            let chosen = match (incumbent, challenger) {
                (Some(incumbent), Some(challenger))
                    if incumbent_is_held(
                        stats,
                        &incumbent.market_id,
                        now,
                        exploration_hold_secs,
                    ) && challenger.total_score
                        < incumbent.total_score + exploration_swap_min_score_delta =>
                {
                    incumbent
                }
                (Some(incumbent), Some(challenger)) => {
                    if challenger.total_score > incumbent.total_score {
                        challenger
                    } else {
                        incumbent
                    }
                }
                (Some(incumbent), None) => incumbent,
                (None, Some(challenger)) => challenger,
                (None, None) => break,
            };
            used_market_ids.insert(chosen.market_id.clone());
            picked.push(chosen);
        }
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
    let exploration_count = exploration_slots.min(active_count.saturating_sub(core_count));
    let exploration_ids: Vec<String> = selected_ids
        .iter()
        .skip(core_count)
        .take(exploration_count)
        .cloned()
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
        exploration_count,
        exploration_ids,
    }
}

fn next_available_ranked(
    ranked: &[RankedMarket],
    cursor: &mut usize,
    used_market_ids: &HashSet<String>,
) -> Option<RankedMarket> {
    while let Some(candidate) = ranked.get(*cursor) {
        *cursor += 1;
        if !used_market_ids.contains(&candidate.market_id) {
            return Some(candidate.clone());
        }
    }
    None
}

fn incumbent_is_held(
    stats: &HashMap<String, MarketOpportunityStats>,
    market_id: &str,
    now: DateTime<Utc>,
    hold_secs: i64,
) -> bool {
    stats
        .get(market_id)
        .and_then(|market_stats| market_stats.last_selected_at)
        .map(|last_selected_at| (now - last_selected_at).num_seconds() < hold_secs)
        .unwrap_or(false)
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
        // Hard-cap at 0.005 (0.5%) — higher values cause a death spiral.
        KEY_ARB_MIN_PROFIT_THRESHOLD => Some((Decimal::new(2, 3), Decimal::new(5, 3))),
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
            &HashSet::new(),
            ScoringWeights::for_profile(AggressivenessProfile::Balanced),
            Decimal::new(5, 3),
            2,
            0,
            DEFAULT_EXPLORATION_HOLD_SECS,
            DEFAULT_EXPLORATION_SWAP_MIN_SCORE_DELTA,
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
            &HashSet::new(),
            ScoringWeights::for_profile(AggressivenessProfile::Balanced),
            Decimal::new(5, 3),
            3,
            1,
            DEFAULT_EXPLORATION_HOLD_SECS,
            DEFAULT_EXPLORATION_SWAP_MIN_SCORE_DELTA,
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

    #[test]
    fn held_exploration_market_is_retained_without_material_challenger_edge() {
        let now = Utc::now();
        let ordered = vec![
            "core-a".to_string(),
            "core-b".to_string(),
            "incumbent-x".to_string(),
            "challenger-y".to_string(),
        ];

        let profiles = HashMap::from([
            ("core-a".to_string(), profile(2_000.0, 5_000.0)),
            ("core-b".to_string(), profile(1_900.0, 4_900.0)),
            ("incumbent-x".to_string(), profile(1_750.0, 4_000.0)),
            ("challenger-y".to_string(), profile(1_760.0, 4_050.0)),
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
                    last_selected_at: Some(now - Duration::seconds(30)),
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
                    last_selected_at: Some(now - Duration::seconds(30)),
                    ..Default::default()
                },
            ),
            (
                "incumbent-x".to_string(),
                MarketOpportunityStats {
                    evaluated_books: 120,
                    profitable_books: 10,
                    signaled_books: 4,
                    ewma_net_profit: 0.006,
                    last_seen_at: Some(now - Duration::seconds(20)),
                    last_selected_at: Some(now - Duration::seconds(45)),
                    ..Default::default()
                },
            ),
        ]);

        let current_eligible = HashSet::from([
            "core-a".to_string(),
            "core-b".to_string(),
            "incumbent-x".to_string(),
        ]);
        let current_exploration = HashSet::from(["incumbent-x".to_string()]);

        let selected = select_market_ids(
            &ordered,
            &profiles,
            &stats,
            &current_eligible,
            &current_exploration,
            ScoringWeights::for_profile(AggressivenessProfile::Balanced),
            Decimal::new(5, 3),
            3,
            1,
            15 * 60,
            10.0,
            now,
        );

        assert!(selected.selected_ids.contains(&"incumbent-x".to_string()));
        assert_eq!(selected.exploration_ids, vec!["incumbent-x".to_string()]);
    }
}
