//! Dynamic runtime tuning for arbitrage and quant signal thresholds.
//!
//! The tuner senses execution quality + market conditions every few minutes,
//! computes bounded targets, applies gradual updates, and broadcasts config
//! changes over Redis so services can adapt without restart.

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use redis::AsyncCommands;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{debug, error, info, warn};
use wallet_tracker::MarketRegime;

use risk_manager::circuit_breaker::CircuitBreaker;

const KEY_ARB_MIN_PROFIT_THRESHOLD: &str = "ARB_MIN_PROFIT_THRESHOLD";
const KEY_ARB_MONITOR_MAX_MARKETS: &str = "ARB_MONITOR_MAX_MARKETS";
const KEY_ARB_MONITOR_EXPLORATION_SLOTS: &str = "ARB_MONITOR_EXPLORATION_SLOTS";
const KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL: &str = "ARB_MONITOR_AGGRESSIVENESS_LEVEL";
const KEY_ARB_POSITION_SIZE: &str = "ARB_POSITION_SIZE";
const KEY_ARB_MIN_NET_PROFIT: &str = "ARB_MIN_NET_PROFIT";
const KEY_ARB_MIN_BOOK_DEPTH: &str = "ARB_MIN_BOOK_DEPTH";
const KEY_ARB_MAX_SIGNAL_AGE_SECS: &str = "ARB_MAX_SIGNAL_AGE_SECS";
const KEY_QUANT_BASE_POSITION_SIZE: &str = "QUANT_BASE_POSITION_SIZE";

const EPSILON: f64 = 1e-6;

/// Redis keys/channels for dynamic tuning.
pub mod channels {
    pub const CONFIG_UPDATES: &str = "dynamic:config:update";
    pub const ARB_RUNTIME_STATS_LATEST: &str = "arb:runtime:stats:latest";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicConfigUpdate {
    pub key: String,
    pub value: Decimal,
    pub reason: String,
    #[serde(default)]
    pub source: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub metrics: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ArbRuntimeStats {
    updates_per_minute: f64,
    stalls_last_minute: f64,
    resets_last_minute: f64,
    monitored_markets: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TuningMetrics {
    slippage_skip_rate: f64,
    below_min_skip_rate: f64,
    successful_fill_rate: f64,
    attempts_last_window: f64,
    fills_last_window: f64,
    /// Arbitrage signals skipped in the window for a reason the tuner's
    /// own settings gate (profit threshold, book depth). They never become
    /// positions, so the watchdog would not see them otherwise.
    #[serde(default)]
    gated_skips_last_window: f64,
    top_skip_reason: Option<String>,
    realized_slippage_p90: f64,
    depth_proxy: f64,
    volatility_proxy: f64,
    ws_stall_rate: f64,
    ws_reset_rate: f64,
    updates_per_minute: f64,
    recent_pnl: f64,
    recent_drawdown: f64,
    cb_tripped: bool,
    current_regime: String,
    /// Quant strategy net P&L over the last 7 days.
    quant_pnl_7d: f64,
    #[serde(default)]
    quant_closed_7d: f64,
    /// Whether the no-trade watchdog was active this cycle (as the tuner
    /// judged it, for the dashboard to show).
    #[serde(default)]
    no_trade_watchdog: bool,
}

#[derive(Debug, Clone)]
pub struct DynamicTunerConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub apply_changes: bool,
    pub regime_hysteresis_intervals: usize,
    pub max_drawdown_freeze: f64,
    pub evaluation_delay_minutes: i64,
    pub fill_rate_degrade_delta: f64,
    pub pnl_degrade_delta: f64,
    pub bootstrap_enabled: bool,
    pub bootstrap_max_attempts: i64,
    pub no_trade_window_minutes: i64,
    pub no_trade_min_attempts: i64,
    pub redis_url: String,
}

impl Default for DynamicTunerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            apply_changes: true,
            regime_hysteresis_intervals: 2,
            max_drawdown_freeze: 0.20,
            evaluation_delay_minutes: 10,
            fill_rate_degrade_delta: 0.08,
            pnl_degrade_delta: 75.0,
            bootstrap_enabled: true,
            bootstrap_max_attempts: 100,
            no_trade_window_minutes: 120,
            no_trade_min_attempts: 20,
            redis_url: "redis://127.0.0.1:6379".to_string(),
        }
    }
}

impl DynamicTunerConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: polymarket_core::config::env_flag("DYNAMIC_TUNER_ENABLED", true),
            interval_secs: polymarket_core::config::env_count("DYNAMIC_TUNER_INTERVAL_SECS", 300),
            apply_changes: polymarket_core::config::env_flag("DYNAMIC_TUNER_APPLY", true),
            regime_hysteresis_intervals: polymarket_core::config::env_count(
                "DYNAMIC_TUNER_REGIME_STREAK",
                2,
            ),
            max_drawdown_freeze: polymarket_core::config::env_number(
                "DYNAMIC_TUNER_FREEZE_DRAWDOWN",
                0.20,
            ),
            evaluation_delay_minutes: polymarket_core::config::env_count(
                "DYNAMIC_TUNER_EVAL_DELAY_MINUTES",
                10,
            ),
            fill_rate_degrade_delta: polymarket_core::config::env_number(
                "DYNAMIC_TUNER_FILL_DEGRADE_DELTA",
                0.08,
            ),
            pnl_degrade_delta: polymarket_core::config::env_number(
                "DYNAMIC_TUNER_PNL_DEGRADE_DELTA",
                75.0,
            ),
            bootstrap_enabled: polymarket_core::config::env_flag(
                "DYNAMIC_TUNER_BOOTSTRAP_ENABLED",
                true,
            ),
            bootstrap_max_attempts: polymarket_core::config::env_count(
                "DYNAMIC_TUNER_BOOTSTRAP_MAX_ATTEMPTS",
                100,
            ),
            no_trade_window_minutes: polymarket_core::config::env_count(
                "DYNAMIC_TUNER_NO_TRADE_WINDOW_MINUTES",
                120,
            ),
            no_trade_min_attempts: polymarket_core::config::env_count(
                "DYNAMIC_TUNER_NO_TRADE_MIN_ATTEMPTS",
                20,
            ),
            redis_url: std::env::var("DYNAMIC_TUNER_REDIS_URL")
                .or_else(|_| std::env::var("REDIS_URL"))
                .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string()),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct DynamicConfigRow {
    key: String,
    current_value: Decimal,
    default_value: Decimal,
    min_value: Decimal,
    max_value: Decimal,
    max_step_pct: Decimal,
    enabled: bool,
    last_good_value: Decimal,
    pending_eval: bool,
    pending_baseline: Option<serde_json::Value>,
    last_applied_at: Option<DateTime<Utc>>,
    updated_by: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

/// How long the tuner leaves a value an operator set alone.
const MANUAL_HOLD_HOURS: i64 = 24;
/// The tuner never relaxes the book depth floor below this share of its
/// default, however long entries keep failing.
const MIN_DEPTH_FLOOR_SHARE: f64 = 0.6;
/// The share of its default the arbitrage profit threshold may be relaxed
/// to. Slippage is not measured yet (`realized_slippage_p90` is 0), so the
/// target rests on an assumed figure and is not trusted far below the
/// configured default.
const MIN_PROFIT_FLOOR_SHARE: f64 = 0.8;
/// Closed quant trades in seven days needed before their profit grows the
/// base size, and the most the tuner grows it to (as a multiple of default).
const QUANT_SIZE_MIN_TRADES: i64 = 10;
const QUANT_SIZE_MAX_MULTIPLE: f64 = 2.0;
/// The most the tuner grows the monitored market count to, as a multiple
/// of its default. A count already past it (an operator set more) is held.
const MAX_MARKETS_MAX_MULTIPLE: f64 = 2.0;
/// Skip reasons that one of the tuner's settings decides.
const GATED_SKIP_REASONS: [&str; 2] = ["below_min_profit", "insufficient_depth"];

impl DynamicConfigRow {
    /// Set by an operator recently: the tuner does not move it.
    fn manually_held(&self) -> bool {
        self.updated_by.as_deref() == Some("workspace_manual")
            && self
                .updated_at
                .is_some_and(|at| Utc::now() - at < chrono::Duration::hours(MANUAL_HOLD_HOURS))
    }
}

pub struct DynamicTuner {
    pool: PgPool,
    config: DynamicTunerConfig,
    current_regime: Arc<RwLock<MarketRegime>>,
    circuit_breaker: Arc<CircuitBreaker>,
    /// Cached Redis connection — created once on first use and reused across cycles.
    redis_conn: Mutex<Option<redis::aio::ConnectionManager>>,
    /// The executor whose mode (live or paper) the results tuned on must
    /// share: paper results never size live trades.
    order_executor: Option<Arc<trading_engine::OrderExecutor>>,
}

impl DynamicTuner {
    pub fn new(
        pool: PgPool,
        current_regime: Arc<RwLock<MarketRegime>>,
        circuit_breaker: Arc<CircuitBreaker>,
    ) -> Self {
        Self {
            pool,
            config: DynamicTunerConfig::from_env(),
            current_regime,
            circuit_breaker,
            redis_conn: Mutex::new(None),
            order_executor: None,
        }
    }

    /// Tune on the results of the mode `executor` trades in.
    pub fn with_order_executor(mut self, executor: Arc<trading_engine::OrderExecutor>) -> Self {
        self.order_executor = Some(executor);
        self
    }

    /// The execution mode whose results are tuned on: live only when the
    /// executor can trade live, as the entry executors decide it.
    async fn traded_mode(&self) -> &'static str {
        match &self.order_executor {
            Some(executor) if executor.is_live_ready().await => "live",
            _ => "paper",
        }
    }

    pub async fn start(self: Arc<Self>) {
        if !self.config.enabled {
            info!("Dynamic tuner is disabled");
            self.persist_runtime_state("disabled", "dynamic tuner disabled", None)
                .await;
            return;
        }

        if let Err(e) = self.seed_defaults().await {
            warn!(error = %e, "Failed seeding dynamic defaults");
        }
        if let Err(e) = self.publish_current_snapshot().await {
            warn!(error = %e, "Failed publishing startup dynamic config snapshot");
        }

        let mut stable_regime = *self.current_regime.read().await;
        let mut candidate_regime: Option<MarketRegime> = None;
        let mut candidate_count: usize = 0;

        info!(
            interval_secs = self.config.interval_secs,
            apply_changes = self.config.apply_changes,
            "Dynamic tuner started"
        );
        self.persist_runtime_state("started", "dynamic tuner started", None)
            .await;

        // A zero period (from the environment) would panic the ticker.
        let mut ticker = interval(TokioDuration::from_secs(self.config.interval_secs.max(1)));
        // tokio::time::interval ticks immediately once; consume it so the next
        // tick aligns with the configured interval after the initial cycle.
        ticker.tick().await;

        if let Err(e) = self
            .run_cycle(
                &mut stable_regime,
                &mut candidate_regime,
                &mut candidate_count,
            )
            .await
        {
            warn!(error = %e, "Initial dynamic tuning cycle failed");
            self.persist_runtime_state("error", &format!("cycle failed: {e}"), None)
                .await;
        }

        loop {
            ticker.tick().await;

            if let Err(e) = self
                .run_cycle(
                    &mut stable_regime,
                    &mut candidate_regime,
                    &mut candidate_count,
                )
                .await
            {
                warn!(error = %e, "Dynamic tuning cycle failed");
                self.persist_runtime_state("error", &format!("cycle failed: {e}"), None)
                    .await;
            }
        }
    }

    async fn publish_current_snapshot(&self) -> anyhow::Result<()> {
        let mut redis_manager = self.redis_connection_manager().await;
        let rows = self.load_rows().await?;

        for row in rows.into_iter().filter(|r| r.enabled) {
            self.publish_update(
                redis_manager.as_mut(),
                &DynamicConfigUpdate {
                    key: row.key,
                    value: row.current_value,
                    reason: "startup sync".to_string(),
                    source: "dynamic_tuner_sync".to_string(),
                    timestamp: Utc::now(),
                    metrics: serde_json::json!({ "sync": "startup" }),
                },
            )
            .await?;
        }

        Ok(())
    }

    async fn run_cycle(
        &self,
        stable_regime: &mut MarketRegime,
        candidate_regime: &mut Option<MarketRegime>,
        candidate_count: &mut usize,
    ) -> anyhow::Result<()> {
        let mut redis_manager = self.redis_connection_manager().await;
        let mut rows = self.load_rows().await?;
        if rows.is_empty() {
            self.seed_defaults().await?;
            rows = self.load_rows().await?;
        }

        let mut metrics = self.collect_metrics(redis_manager.as_mut()).await?;
        // Judged before any freeze, so the stored verdict is this cycle's.
        metrics.no_trade_watchdog = self.no_trade_watchdog(&metrics);
        let current_regime = *self.current_regime.read().await;
        let resolved = self.resolve_regime(
            current_regime,
            stable_regime,
            candidate_regime,
            candidate_count,
        );
        metrics.current_regime = format!("{:?}", resolved);

        self.evaluate_pending(rows.as_slice(), &metrics).await?;
        // evaluate_pending can mutate pending_eval/current_value; refresh rows
        // so this cycle doesn't operate on stale pre-evaluation state.
        rows = self.load_rows().await?;

        if metrics.cb_tripped || metrics.recent_drawdown >= self.config.max_drawdown_freeze {
            let freeze_reason = "risk guard active: circuit breaker/drawdown";
            self.record_history(
                None,
                None,
                None,
                "frozen",
                freeze_reason,
                Some(&metrics),
                None,
            )
            .await?;
            info!(
                cb_tripped = metrics.cb_tripped,
                drawdown = metrics.recent_drawdown,
                "Dynamic tuner frozen by risk guard"
            );
            self.persist_runtime_state("frozen", freeze_reason, Some(&metrics))
                .await;
            return Ok(());
        }

        let mut by_key: HashMap<String, DynamicConfigRow> = HashMap::new();
        for row in rows {
            by_key.insert(row.key.clone(), row);
        }

        let targets = self.compute_targets(&by_key, &metrics, resolved);
        let mut applied_count = 0usize;
        let mut recommended_count = 0usize;
        let bootstrap_active = self.config.bootstrap_enabled
            && metrics.attempts_last_window < self.config.bootstrap_max_attempts as f64;
        let no_trade_watchdog = metrics.no_trade_watchdog;
        let top_skip_reason = metrics
            .top_skip_reason
            .clone()
            .unwrap_or_else(|| "none".to_string());

        if no_trade_watchdog {
            warn!(
                attempts = metrics.attempts_last_window,
                fills = metrics.fills_last_window,
                top_skip_reason = %top_skip_reason,
                "No-trade watchdog active: applying adaptive relaxation"
            );
        }

        for (key, raw_target) in targets {
            let Some(row) = by_key.get(&key) else {
                continue;
            };
            if !row.enabled {
                continue;
            }
            if row.pending_eval || row.manually_held() {
                continue;
            }

            let current = decimal_to_f64(row.current_value);
            let stepped = apply_step_limit(
                current,
                raw_target,
                decimal_to_f64(row.max_step_pct),
                decimal_to_f64(row.default_value),
            );
            let bounded = stepped
                .max(decimal_to_f64(row.min_value))
                .min(decimal_to_f64(row.max_value));

            if (bounded - current).abs() < EPSILON {
                continue;
            }

            // At the column's scale, as the database stores it: the guards
            // below compare against the stored value.
            let Some(new_value) = Decimal::from_f64_retain(bounded).map(|value| {
                value.round_dp_with_strategy(10, RoundingStrategy::MidpointAwayFromZero)
            }) else {
                continue;
            };

            let reason = format!(
                "auto tune: regime={} fill_rate={:.3} slip_p90={:.4} pnl={:.2} drawdown={:.3} attempts={:.0} fills={:.0} top_skip={} bootstrap={} watchdog={}",
                metrics.current_regime,
                metrics.successful_fill_rate,
                metrics.realized_slippage_p90,
                metrics.recent_pnl,
                metrics.recent_drawdown,
                metrics.attempts_last_window,
                metrics.fills_last_window,
                top_skip_reason,
                bootstrap_active,
                no_trade_watchdog
            );

            if self.config.apply_changes {
                if !self.apply_change(row, new_value, &reason, &metrics).await? {
                    // Changed since it was read (an operator's update): left.
                    continue;
                }
                let publish_result = self
                    .publish_update(
                        redis_manager.as_mut(),
                        &DynamicConfigUpdate {
                            key: key.clone(),
                            value: new_value,
                            reason: reason.clone(),
                            source: "dynamic_tuner".to_string(),
                            timestamp: Utc::now(),
                            metrics: serde_json::to_value(&metrics)
                                .unwrap_or(serde_json::json!({})),
                        },
                    )
                    .await;
                if let Err(e) = publish_result {
                    // Stored is applied: every process re-reads the stored
                    // config within seconds (the API every 5 s, the arb
                    // monitor every 30 s); the publish only speeds it up.
                    warn!(
                        key = %key,
                        value = %new_value,
                        error = %e,
                        "Dynamic config publish failed; processes take the stored change on their next re-read"
                    );
                }
                applied_count += 1;
            } else {
                self.record_history(
                    Some(&key),
                    Some(row.current_value),
                    Some(new_value),
                    "recommended",
                    &reason,
                    Some(&metrics),
                    None,
                )
                .await?;
                recommended_count += 1;
            }
        }

        if no_trade_watchdog && applied_count == 0 && recommended_count == 0 {
            self.record_history(
                None,
                None,
                None,
                "watchdog",
                &format!(
                    "no-trade watchdog active but no bounded changes available (top_skip={})",
                    top_skip_reason
                ),
                Some(&metrics),
                None,
            )
            .await?;
        }

        let cycle_reason = if self.config.apply_changes {
            format!("cycle complete: applied={applied_count}")
        } else {
            format!("cycle complete: shadow_recommended={recommended_count}")
        };
        self.persist_runtime_state("ok", &cycle_reason, Some(&metrics))
            .await;

        Ok(())
    }

    async fn evaluate_pending(
        &self,
        rows: &[DynamicConfigRow],
        metrics: &TuningMetrics,
    ) -> anyhow::Result<()> {
        if !self.config.apply_changes {
            return Ok(());
        }

        let now = Utc::now();
        for row in rows.iter().filter(|r| r.pending_eval) {
            let Some(applied_at) = row.last_applied_at else {
                continue;
            };
            if (now - applied_at).num_minutes() < self.config.evaluation_delay_minutes {
                continue;
            }

            let baseline = row
                .pending_baseline
                .as_ref()
                .and_then(|v| serde_json::from_value::<TuningMetrics>(v.clone()).ok())
                .unwrap_or_default();

            let fill_degraded = metrics.successful_fill_rate + self.config.fill_rate_degrade_delta
                < baseline.successful_fill_rate;
            let pnl_degraded =
                metrics.recent_pnl < baseline.recent_pnl - self.config.pnl_degrade_delta;

            if fill_degraded || pnl_degraded {
                let reason = format!(
                    "rollback: fill_degraded={} pnl_degraded={} baseline_fill={:.3} current_fill={:.3} baseline_pnl={:.2} current_pnl={:.2}",
                    fill_degraded,
                    pnl_degraded,
                    baseline.successful_fill_rate,
                    metrics.successful_fill_rate,
                    baseline.recent_pnl,
                    metrics.recent_pnl
                );

                let rolled_back = sqlx::query(
                    r#"
                    UPDATE dynamic_config
                    SET current_value = $2,
                        pending_eval = FALSE,
                        pending_baseline = NULL,
                        last_reason = $3,
                        updated_by = 'dynamic_tuner'
                    WHERE key = $1 AND pending_eval AND current_value = $4
                    "#,
                )
                .bind(&row.key)
                .bind(row.last_good_value)
                .bind(&reason)
                .bind(row.current_value)
                .execute(&self.pool)
                .await?
                .rows_affected();
                if rolled_back == 0 {
                    // Changed since it was read (an operator's update): it
                    // stands, and nothing is announced.
                    continue;
                }

                self.record_history(
                    Some(&row.key),
                    Some(row.current_value),
                    Some(row.last_good_value),
                    "rollback",
                    &reason,
                    Some(metrics),
                    Some(metrics),
                )
                .await?;

                let mut rollback_redis = self.redis_connection_manager().await;
                self.publish_update(
                    rollback_redis.as_mut(),
                    &DynamicConfigUpdate {
                        key: row.key.clone(),
                        value: row.last_good_value,
                        reason,
                        source: "dynamic_tuner_rollback".to_string(),
                        timestamp: Utc::now(),
                        metrics: serde_json::to_value(metrics).unwrap_or(serde_json::json!({})),
                    },
                )
                .await
                .unwrap_or_else(|e| {
                    warn!(
                        key = %row.key,
                        error = %e,
                        "Failed publishing rollback dynamic config update"
                    );
                });
            } else {
                let passed = sqlx::query(
                    r#"
                    UPDATE dynamic_config
                    SET pending_eval = FALSE,
                        pending_baseline = NULL,
                        last_good_value = current_value,
                        updated_by = 'dynamic_tuner'
                    WHERE key = $1 AND pending_eval AND current_value = $2
                    "#,
                )
                .bind(&row.key)
                .bind(row.current_value)
                .execute(&self.pool)
                .await?
                .rows_affected();
                if passed == 0 {
                    // An operator's update since the read ended the
                    // evaluation; it keeps its own marker (and hold).
                    continue;
                }

                self.record_history(
                    Some(&row.key),
                    Some(row.current_value),
                    Some(row.current_value),
                    "evaluation",
                    "post-change evaluation passed",
                    Some(metrics),
                    Some(metrics),
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Many entries tried or signals held back by the tuner's own gates,
    /// and nothing filled.
    fn no_trade_watchdog(&self, metrics: &TuningMetrics) -> bool {
        metrics.attempts_last_window + metrics.gated_skips_last_window
            >= self.config.no_trade_min_attempts as f64
            && metrics.fills_last_window <= EPSILON
    }

    fn compute_targets(
        &self,
        rows: &HashMap<String, DynamicConfigRow>,
        metrics: &TuningMetrics,
        regime: MarketRegime,
    ) -> HashMap<String, f64> {
        let mut targets = HashMap::new();

        let max_markets_current = rows
            .get(KEY_ARB_MONITOR_MAX_MARKETS)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(300.0);
        let max_markets_default = rows
            .get(KEY_ARB_MONITOR_MAX_MARKETS)
            .map(|r| decimal_to_f64(r.default_value))
            .unwrap_or(300.0);

        let no_trade_watchdog = self.no_trade_watchdog(metrics);
        let top_skip = metrics
            .top_skip_reason
            .as_deref()
            .unwrap_or("none")
            .to_string();

        let (_, _, regime_safety_margin) = match regime {
            MarketRegime::BullCalm => (0.95_f64, 0.0012_f64, 0.0010_f64),
            MarketRegime::BullVolatile => (1.03, 0.0020, 0.0018),
            MarketRegime::BearCalm => (1.05, 0.0018, 0.0022),
            MarketRegime::BearVolatile => (1.10, 0.0028, 0.0030),
            MarketRegime::Ranging => (1.00, 0.0016, 0.0018),
            MarketRegime::Uncertain => (1.07, 0.0022, 0.0025),
        };

        // ARB_MIN_PROFIT_THRESHOLD: expected slippage + safety margin.
        //
        // Guard: only tune when we have real execution data.  With zero
        // attempts AND zero fills the metrics are phantom values and any
        // movement is pure noise — the tuner should hold steady rather
        // than drift upward into a death spiral.
        // Signals the threshold itself holds back count too: they are why
        // the watchdog is on, and they never become attempts.
        if metrics.attempts_last_window >= 1.0
            || metrics.fills_last_window >= 1.0
            || no_trade_watchdog
        {
            let expected_arb_slippage = (metrics.realized_slippage_p90 * 2.0).max(0.0015);
            let low_depth_penalty = (0.0010 - metrics.depth_proxy).max(0.0) * 0.3;
            let vol_penalty = metrics.volatility_proxy * 0.01;
            let mut desired_arb_profit =
                expected_arb_slippage + regime_safety_margin + low_depth_penalty + vol_penalty;
            // Watchdog: if many attempts but zero fills, reduce the
            // parameter that is actually causing rejections. Only signals
            // skipped for falling short of the threshold justify lowering it;
            // skips for an existing position, a stale signal, the circuit
            // breaker, depth or slippage say nothing about the threshold.
            if no_trade_watchdog && top_skip == "below_min_profit" {
                desired_arb_profit *= 0.90;
            }
            if metrics.realized_slippage_p90 <= 0.0 {
                if let Some(row) = rows.get(KEY_ARB_MIN_PROFIT_THRESHOLD) {
                    let floor = decimal_to_f64(row.default_value) * MIN_PROFIT_FLOOR_SHARE;
                    desired_arb_profit = desired_arb_profit.max(floor);
                }
            }
            // No ratchet floor — apply_step_limit already caps movement
            // to max_step_pct (12%) per cycle, which is sufficient.
            // The old `.max(current * 0.7)` prevented the tuner from
            // correcting its own upward drift.
            targets.insert(KEY_ARB_MIN_PROFIT_THRESHOLD.to_string(), desired_arb_profit);
        }

        // ARB_MIN_BOOK_DEPTH: relax when watchdog says depth is the blocking reason.
        // This targets the actual gating parameter instead of reducing the profit
        // threshold, which would push into lower-quality opportunities. Once
        // depth no longer blocks, a relaxed floor returns toward its default
        // (apply_step_limit bounds the pace); one above the default is held.
        if let Some(row) = rows.get(KEY_ARB_MIN_BOOK_DEPTH) {
            let current_depth = decimal_to_f64(row.current_value);
            let default_depth = decimal_to_f64(row.default_value);
            if no_trade_watchdog && top_skip == "insufficient_depth" {
                // 5% a step, and never below a set share of the default:
                // failing entries are not cured by ever thinner books.
                let floor = default_depth * MIN_DEPTH_FLOOR_SHARE;
                let relaxed_depth = (current_depth * 0.95).max(floor.min(current_depth));
                targets.insert(KEY_ARB_MIN_BOOK_DEPTH.to_string(), relaxed_depth);
            } else if current_depth + EPSILON < default_depth {
                targets.insert(KEY_ARB_MIN_BOOK_DEPTH.to_string(), default_depth);
            }
        }

        // ARB_MONITOR_MAX_MARKETS: adapt to stream health + throughput.
        let health_penalty = (metrics.ws_stall_rate * 0.9 + metrics.ws_reset_rate * 0.6).min(1.0);
        let throughput_bonus = if metrics.updates_per_minute > 500.0 {
            0.10
        } else {
            0.0
        };
        // Few updates from a healthy stream only means quiet markets; shrinking
        // the subscription then lowers the rate further and never recovers.
        let healthy_stream = health_penalty < 0.10;
        let throughput_penalty = if !healthy_stream
            && metrics.updates_per_minute > EPSILON
            && metrics.updates_per_minute < 120.0
        {
            0.10
        } else {
            0.0
        };

        let desired_market_factor =
            (1.0 - health_penalty - throughput_penalty + throughput_bonus).clamp(0.70, 1.20);
        let desired_market_target =
            if healthy_stream && max_markets_current + EPSILON < max_markets_default {
                // Once ingestion is healthy again, bias back toward the configured
                // default cap. apply_step_limit still bounds the live change rate.
                max_markets_default
            } else {
                let grown = (max_markets_current * desired_market_factor).round();
                // Growth stops at a multiple of the default; shrinking is not capped.
                let cap = max_markets_default * MAX_MARKETS_MAX_MULTIPLE;
                if grown > max_markets_current && grown > cap {
                    cap.max(max_markets_current)
                } else {
                    grown
                }
            };
        targets.insert(
            KEY_ARB_MONITOR_MAX_MARKETS.to_string(),
            desired_market_target,
        );

        // QUANT_BASE_POSITION_SIZE: adjust based on quant strategy profitability.
        let quant_size_current = rows
            .get(KEY_QUANT_BASE_POSITION_SIZE)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(30.0);

        let quant_size_default = rows
            .get(KEY_QUANT_BASE_POSITION_SIZE)
            .map(|r| decimal_to_f64(r.default_value))
            .unwrap_or(30.0);
        let desired_quant_size = if metrics.quant_pnl_7d > 0.0
            && metrics.quant_closed_7d >= QUANT_SIZE_MIN_TRADES as f64
        {
            // Profitable over enough trades: a modest 5% raise, up to a
            // multiple of the default. A size already past it (an operator
            // set more) is held.
            let cap = quant_size_default * QUANT_SIZE_MAX_MULTIPLE;
            if quant_size_current >= cap {
                quant_size_current
            } else {
                (quant_size_current * 1.05).min(cap)
            }
        } else if metrics.quant_pnl_7d < -50.0 {
            // Quant strategies are bleeding — pull back by 10%
            quant_size_current * 0.90
        } else {
            // Hold steady
            quant_size_current
        };
        targets.insert(KEY_QUANT_BASE_POSITION_SIZE.to_string(), desired_quant_size);

        targets
    }

    fn resolve_regime(
        &self,
        observed: MarketRegime,
        stable_regime: &mut MarketRegime,
        candidate_regime: &mut Option<MarketRegime>,
        candidate_count: &mut usize,
    ) -> MarketRegime {
        if observed == *stable_regime {
            *candidate_regime = None;
            *candidate_count = 0;
            return *stable_regime;
        }

        match candidate_regime {
            Some(current) if *current == observed => {
                *candidate_count += 1;
            }
            _ => {
                *candidate_regime = Some(observed);
                *candidate_count = 1;
            }
        }

        if *candidate_count >= self.config.regime_hysteresis_intervals {
            info!(from = ?*stable_regime, to = ?observed, "Regime hysteresis switched stable regime");
            *stable_regime = observed;
            *candidate_regime = None;
            *candidate_count = 0;
        }

        *stable_regime
    }

    async fn collect_metrics(
        &self,
        redis_manager: Option<&mut redis::aio::ConnectionManager>,
    ) -> anyhow::Result<TuningMetrics> {
        let (attempts_last_window, fills_last_window): (f64, f64) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*)::double precision AS attempts_last_window,
                COUNT(*) FILTER (
                    WHERE state IN (1, 2, 3, 4, 6, 7)
                )::double precision AS fills_last_window
            FROM positions
            WHERE exit_strategy = 0
              AND entry_timestamp >= NOW() - ($1::bigint * INTERVAL '1 minute')
            "#,
        )
        .bind(self.config.no_trade_window_minutes)
        .fetch_one(&self.pool)
        .await
        .unwrap_or((0.0, 0.0));

        let recent_pnl: Decimal = sqlx::query_scalar(
            r#"
            SELECT COALESCE(SUM(realized_pnl), 0)
            FROM positions
            WHERE exit_timestamp >= NOW() - INTERVAL '6 hours'
              AND realized_pnl IS NOT NULL
              AND source IN (1, 2, 3)
              AND execution_mode = $1
            "#,
        )
        .bind(self.traded_mode().await)
        .fetch_one(&self.pool)
        .await?;

        // How much prices move hour to hour (a price is a probability, so
        // this is in price units): per market, the spread of its hourly
        // closes over the last day, averaged across markets, at most 0.10.
        // Wallets' annualized return volatility is far larger and would hold
        // the threshold at its ceiling.
        let volatility_proxy: f64 = sqlx::query_scalar(
            r#"
            SELECT LEAST(COALESCE(AVG(price_volatility), 0), 0.10)
            FROM (
                SELECT STDDEV_POP(close::double precision) AS price_volatility
                FROM orderbook_hourly
                WHERE bucket >= NOW() - INTERVAL '24 hours'
                GROUP BY market_id
                HAVING COUNT(*) >= 3
            ) per_market
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0.0);

        let (depth_proxy, arb_events_per_min): (f64, f64) = sqlx::query_as(
            r#"
            SELECT
                COALESCE(AVG(net_profit::double precision), 0) AS depth_proxy,
                COALESCE(COUNT(*)::double precision / 5.0, 0) AS events_per_min
            FROM arb_opportunities
            WHERE timestamp >= NOW() - INTERVAL '5 minutes'
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or((0.0, 0.0));

        let cb_state = self.circuit_breaker.state().await;
        let drawdown = if cb_state.peak_value > Decimal::ZERO {
            let dd = (cb_state.peak_value - cb_state.current_value) / cb_state.peak_value;
            dd.max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        let runtime_stats = self.fetch_runtime_stats(redis_manager).await;
        let ws_stall_rate = if runtime_stats.stalls_last_minute > 0.0 {
            runtime_stats.stalls_last_minute / (runtime_stats.stalls_last_minute + 1.0)
        } else {
            0.0
        };
        let ws_reset_rate = if runtime_stats.resets_last_minute > 0.0 {
            runtime_stats.resets_last_minute / (runtime_stats.resets_last_minute + 1.0)
        } else {
            0.0
        };

        // Quant strategy net P&L over the last 7 days, and how many trades
        let (quant_pnl_7d, quant_closed_7d): (Decimal, i64) = sqlx::query_as(
            r#"
            SELECT COALESCE(SUM(realized_pnl), 0), COUNT(*)
            FROM positions
            WHERE source = 3
              AND exit_timestamp >= NOW() - INTERVAL '7 days'
              AND realized_pnl IS NOT NULL
              AND execution_mode = $1
            "#,
        )
        .bind(self.traded_mode().await)
        .fetch_one(&self.pool)
        .await
        .unwrap_or((Decimal::ZERO, 0));

        let gated_skips_last_window: f64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)::double precision
            FROM trade_events
            WHERE source = 'arb'
              AND event_type = 'signal_skipped'
              AND reason = ANY($2)
              AND execution_mode = $3
              AND occurred_at >= NOW() - ($1::bigint * INTERVAL '1 minute')
            "#,
        )
        .bind(self.config.no_trade_window_minutes)
        .bind(&GATED_SKIP_REASONS[..])
        .bind(self.traded_mode().await)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0.0);

        // Query the most common arb skip reason from recent trade events.
        let top_skip_reason: Option<String> = sqlx::query_scalar(
            r#"
            SELECT reason
            FROM trade_events
            WHERE source = 'arb'
              AND event_type = 'signal_skipped'
              AND reason IS NOT NULL
              AND occurred_at >= NOW() - INTERVAL '120 minutes'
            GROUP BY reason
            ORDER BY COUNT(*) DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();

        Ok(TuningMetrics {
            slippage_skip_rate: 0.0,
            below_min_skip_rate: 0.0,
            successful_fill_rate: ratio(fills_last_window, attempts_last_window),
            attempts_last_window,
            fills_last_window,
            gated_skips_last_window,
            top_skip_reason,
            realized_slippage_p90: 0.0,
            depth_proxy,
            volatility_proxy,
            ws_stall_rate,
            ws_reset_rate,
            updates_per_minute: if runtime_stats.updates_per_minute > 0.0 {
                runtime_stats.updates_per_minute
            } else {
                arb_events_per_min
            },
            recent_pnl: decimal_to_f64(recent_pnl),
            recent_drawdown: decimal_to_f64(drawdown),
            cb_tripped: cb_state.tripped,
            current_regime: "Uncertain".to_string(),
            quant_pnl_7d: decimal_to_f64(quant_pnl_7d),
            quant_closed_7d: quant_closed_7d as f64,
            no_trade_watchdog: false,
        })
    }

    async fn fetch_runtime_stats(
        &self,
        redis_manager: Option<&mut redis::aio::ConnectionManager>,
    ) -> ArbRuntimeStats {
        let Some(redis) = redis_manager else {
            return ArbRuntimeStats::default();
        };

        let raw: Option<String> = redis
            .get(channels::ARB_RUNTIME_STATS_LATEST)
            .await
            .unwrap_or(None);

        raw.and_then(|v| serde_json::from_str::<ArbRuntimeStats>(&v).ok())
            .unwrap_or_default()
    }

    async fn seed_defaults(&self) -> anyhow::Result<()> {
        let seeds = vec![
            ConfigSeed {
                key: KEY_ARB_MIN_PROFIT_THRESHOLD,
                default_value: polymarket_core::config::env_number(
                    "ARB_MIN_PROFIT_THRESHOLD",
                    Decimal::new(5, 3),
                ),
                min_value: Decimal::new(2, 3),
                // Hard-cap at 0.005 (0.5%) — real arb spreads are 0.5-2%.
                // Higher values create a death spiral: no signals → tuner raises → even fewer signals.
                max_value: Decimal::new(5, 3),
                max_step_pct: Decimal::new(12, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MONITOR_MAX_MARKETS,
                default_value: Decimal::from(polymarket_core::config::env_count::<i64>(
                    "ARB_MONITOR_MAX_MARKETS",
                    300,
                )),
                min_value: Decimal::new(25, 0),
                max_value: Decimal::new(1500, 0),
                max_step_pct: Decimal::new(15, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MONITOR_EXPLORATION_SLOTS,
                default_value: Decimal::from(polymarket_core::config::env_count::<i64>(
                    "ARB_MONITOR_EXPLORATION_SLOTS",
                    5,
                )),
                min_value: Decimal::new(1, 0),
                max_value: Decimal::new(500, 0),
                max_step_pct: Decimal::new(25, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL,
                default_value: env_aggressiveness_level(),
                min_value: Decimal::ZERO,
                max_value: Decimal::new(2, 0),
                max_step_pct: Decimal::new(100, 2),
            },
            // ── Arb executor tuning knobs ──
            ConfigSeed {
                key: KEY_ARB_POSITION_SIZE,
                default_value: polymarket_core::config::env_positive_decimal(
                    "ARB_POSITION_SIZE",
                    Decimal::new(10, 0),
                ),
                min_value: Decimal::new(5, 0),  // $5 floor
                max_value: Decimal::new(25, 0), // $25 ceiling (small wallet)
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MIN_NET_PROFIT,
                default_value: polymarket_core::config::env_number(
                    "ARB_MIN_NET_PROFIT",
                    Decimal::new(1, 3),
                ),
                min_value: Decimal::new(5, 4), // 0.0005 floor
                max_value: Decimal::new(5, 2), // 0.05 ceiling
                max_step_pct: Decimal::new(15, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MIN_BOOK_DEPTH,
                default_value: polymarket_core::config::env_positive_decimal(
                    "ARB_MIN_BOOK_DEPTH",
                    Decimal::new(25, 0),
                ),
                min_value: Decimal::new(10, 0),  // $10 floor
                max_value: Decimal::new(200, 0), // $200 ceiling (small wallet)
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MAX_SIGNAL_AGE_SECS,
                default_value: Decimal::from(polymarket_core::config::env_count::<i64>(
                    "ARB_MAX_SIGNAL_AGE_SECS",
                    30,
                )),
                min_value: Decimal::new(5, 0),   // 5s floor
                max_value: Decimal::new(300, 0), // 300s ceiling
                max_step_pct: Decimal::new(25, 2),
            },
            // ── Quant signal executor tuning knob ──
            ConfigSeed {
                key: KEY_QUANT_BASE_POSITION_SIZE,
                default_value: polymarket_core::config::env_positive_decimal(
                    "QUANT_BASE_POSITION_SIZE",
                    Decimal::new(30, 0),
                ),
                min_value: Decimal::new(10, 0),    // $10 floor
                max_value: Decimal::new(200, 0),   // $200 ceiling
                max_step_pct: Decimal::new(15, 2), // 15% per cycle
            },
        ];

        for seed in seeds {
            let seeded_value = clamp_to_bounds(seed.default_value, seed.min_value, seed.max_value);
            if seeded_value != seed.default_value {
                warn!(
                    key = seed.key,
                    configured_default = %seed.default_value,
                    seeded_default = %seeded_value,
                    min = %seed.min_value,
                    max = %seed.max_value,
                    "Configured dynamic default is out of bounds; clamping to valid range"
                );
            }

            sqlx::query(
                r#"
                INSERT INTO dynamic_config (
                    key, current_value, default_value, min_value, max_value,
                    max_step_pct, enabled, last_good_value, updated_by, last_reason
                ) VALUES ($1, $2, $2, $3, $4, $5, TRUE, $2, 'bootstrap', 'initial seed')
                ON CONFLICT (key) DO UPDATE SET
                    min_value = EXCLUDED.min_value,
                    max_value = EXCLUDED.max_value,
                    max_step_pct = EXCLUDED.max_step_pct,
                    default_value = EXCLUDED.default_value,
                    current_value = CASE
                        WHEN dynamic_config.updated_by = 'bootstrap' THEN EXCLUDED.default_value
                        ELSE LEAST(GREATEST(dynamic_config.current_value, EXCLUDED.min_value), EXCLUDED.max_value)
                    END,
                    last_good_value = CASE
                        WHEN dynamic_config.updated_by = 'bootstrap' THEN EXCLUDED.default_value
                        ELSE LEAST(GREATEST(dynamic_config.last_good_value, EXCLUDED.min_value), EXCLUDED.max_value)
                    END
                -- Untouched when nothing changed: an update would renew
                -- updated_at, and with it an operator's hold.
                WHERE (dynamic_config.min_value, dynamic_config.max_value,
                       dynamic_config.max_step_pct, dynamic_config.default_value)
                      IS DISTINCT FROM
                      (EXCLUDED.min_value, EXCLUDED.max_value,
                       EXCLUDED.max_step_pct, EXCLUDED.default_value)
                "#,
            )
            .bind(seed.key)
            .bind(seeded_value)
            .bind(seed.min_value)
            .bind(seed.max_value)
            .bind(seed.max_step_pct)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    async fn load_rows(&self) -> anyhow::Result<Vec<DynamicConfigRow>> {
        let rows: Vec<DynamicConfigRow> = sqlx::query_as(
            r#"
            SELECT key, current_value, default_value, min_value, max_value,
                   max_step_pct, enabled, last_good_value, pending_eval,
                   pending_baseline, last_applied_at, updated_by, updated_at
            FROM dynamic_config
            ORDER BY key
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn apply_change(
        &self,
        row: &DynamicConfigRow,
        new_value: Decimal,
        reason: &str,
        metrics: &TuningMetrics,
    ) -> anyhow::Result<bool> {
        // Only over the row this pass read: an operator's update since then
        // stands, even one that saved the same value (it changes who set
        // it and when, which holds the tuner off).
        let changed = sqlx::query(
            r#"
            UPDATE dynamic_config
            SET current_value = $2,
                pending_eval = TRUE,
                pending_baseline = $3,
                last_applied_at = NOW(),
                last_reason = $4,
                updated_by = 'dynamic_tuner'
            WHERE key = $1 AND current_value = $5 AND NOT pending_eval
              AND updated_by IS NOT DISTINCT FROM $6
              AND updated_at IS NOT DISTINCT FROM $7
            "#,
        )
        .bind(&row.key)
        .bind(new_value)
        .bind(serde_json::to_value(metrics).unwrap_or(serde_json::json!({})))
        .bind(reason)
        .bind(row.current_value)
        .bind(&row.updated_by)
        .bind(row.updated_at)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(false);
        }

        self.record_history(
            Some(&row.key),
            Some(row.current_value),
            Some(new_value),
            "applied",
            reason,
            Some(metrics),
            None,
        )
        .await?;

        info!(
            key = %row.key,
            old = %row.current_value,
            new = %new_value,
            "Dynamic config updated"
        );

        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_history(
        &self,
        config_key: Option<&str>,
        old_value: Option<Decimal>,
        new_value: Option<Decimal>,
        action: &str,
        reason: &str,
        metrics_snapshot: Option<&TuningMetrics>,
        outcome_metrics: Option<&TuningMetrics>,
    ) -> anyhow::Result<()> {
        let metrics_json =
            metrics_snapshot.map(|m| serde_json::to_value(m).unwrap_or(serde_json::json!({})));
        let outcome_json =
            outcome_metrics.map(|m| serde_json::to_value(m).unwrap_or(serde_json::json!({})));

        sqlx::query(
            r#"
            INSERT INTO dynamic_config_history
                (config_key, old_value, new_value, action, reason, metrics_snapshot, outcome_metrics)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(config_key)
        .bind(old_value)
        .bind(new_value)
        .bind(action)
        .bind(reason)
        .bind(metrics_json)
        .bind(outcome_json)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Return the cached Redis ConnectionManager, creating it on first use.
    ///
    /// Reusing a single ConnectionManager across cycles avoids opening a new
    /// TCP connection on every `run_cycle` call (every ~5 minutes). Cloning
    /// a `ConnectionManager` shares the underlying multiplexed connection.
    async fn redis_connection_manager(&self) -> Option<redis::aio::ConnectionManager> {
        let mut guard = self.redis_conn.lock().await;
        if guard.is_none() {
            let client = redis::Client::open(self.config.redis_url.as_str()).ok()?;
            *guard = redis::aio::ConnectionManager::new(client).await.ok();
        }
        guard.clone()
    }

    async fn publish_update(
        &self,
        redis_manager: Option<&mut redis::aio::ConnectionManager>,
        update: &DynamicConfigUpdate,
    ) -> anyhow::Result<()> {
        let Some(redis) = redis_manager else {
            anyhow::bail!("No Redis connection for dynamic config publish");
        };

        let payload = match serde_json::to_string(update) {
            Ok(payload) => payload,
            Err(e) => {
                anyhow::bail!("Failed serializing dynamic config update: {e}");
            }
        };

        let result: redis::RedisResult<()> = redis.publish(channels::CONFIG_UPDATES, payload).await;
        if let Err(e) = result {
            anyhow::bail!(
                "Failed publishing dynamic config update for {}: {e}",
                update.key
            );
        }
        Ok(())
    }

    async fn persist_runtime_state(
        &self,
        status: &str,
        reason: &str,
        metrics: Option<&TuningMetrics>,
    ) {
        let metrics_json =
            metrics.map(|m| serde_json::to_value(m).unwrap_or(serde_json::json!({})));
        if let Err(e) = sqlx::query(
            r#"
            INSERT INTO dynamic_tuner_state (
                singleton, last_run_at, last_run_status, last_run_reason, last_metrics
            )
            VALUES (TRUE, NOW(), $1, $2, $3)
            ON CONFLICT (singleton) DO UPDATE
            SET last_run_at = EXCLUDED.last_run_at,
                last_run_status = EXCLUDED.last_run_status,
                last_run_reason = EXCLUDED.last_run_reason,
                last_metrics = EXCLUDED.last_metrics
            "#,
        )
        .bind(status)
        .bind(reason)
        .bind(metrics_json)
        .execute(&self.pool)
        .await
        {
            warn!(error = %e, "Failed persisting dynamic tuner runtime state");
        }
    }
}

/// Subscribes to dynamic config updates and applies them to local API runtime.
pub fn spawn_dynamic_config_subscriber(
    redis_url: String,
    pool: PgPool,
    arb_executor_config: Option<Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>>,
    quant_executor_config: Option<
        Arc<RwLock<crate::quant_signal_executor::QuantSignalExecutorConfig>>,
    >,
) {
    tokio::spawn(async move {
        loop {
            match run_dynamic_config_subscriber(
                redis_url.as_str(),
                pool.clone(),
                arb_executor_config.clone(),
                quant_executor_config.clone(),
            )
            .await
            {
                Err(e) => error!(error = %e, "Dynamic config subscriber failed, retrying"),
                Ok(()) => warn!("Dynamic config subscription ended; reconnecting"),
            }
            // Every exit waits before reconnecting, an ended stream included.
            tokio::time::sleep(TokioDuration::from_secs(5)).await;
        }
    });

    info!("Dynamic config subscriber spawned");
}

/// With the tuner switched off, remove the dynamic config rows still holding
/// their seed (an earlier run's or a migration's): left, every process would
/// apply them over its environment, clamped to the tuning bounds. Without
/// them the environment applies; an operator's values, and the tuner's own
/// from when it ran, stay. Run by every API process before its first
/// snapshot.
pub async fn clear_untouched_seeds_when_disabled(pool: &PgPool) {
    // As the tuner reads it.
    if polymarket_core::config::env_flag("DYNAMIC_TUNER_ENABLED", true) {
        return;
    }
    match sqlx::query("DELETE FROM dynamic_config WHERE updated_by = 'bootstrap'")
        .execute(pool)
        .await
    {
        Ok(done) if done.rows_affected() > 0 => info!(
            removed = done.rows_affected(),
            "Dynamic tuner disabled: untouched dynamic config seeds removed; the environment applies"
        ),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "Could not clear untouched dynamic config seeds"),
    }
}

/// Apply the stored dynamic config (manual overrides and tuned values) to
/// the executors. Called before they start, so they never trade on
/// environment defaults while Redis is unreachable, and on each subscriber
/// (re)connect.
pub async fn apply_dynamic_config_snapshot(
    pool: &PgPool,
    arb_executor_config: Option<&Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>>,
    quant_executor_config: Option<
        &Arc<RwLock<crate::quant_signal_executor::QuantSignalExecutorConfig>>,
    >,
) {
    let bounds = load_dynamic_bounds(pool).await;
    if let Some(arb_config) = arb_executor_config {
        if let Err(e) = apply_startup_snapshot_to_arb_executor(pool, arb_config, &bounds).await {
            warn!(error = %e, "Failed applying startup dynamic config snapshot to arb executor");
        }
    }
    if let Some(quant_config) = quant_executor_config {
        if let Err(e) = apply_startup_snapshot_to_quant_executor(pool, quant_config, &bounds).await
        {
            warn!(error = %e, "Failed applying startup dynamic config snapshot to quant executor");
        }
    }
}

async fn run_dynamic_config_subscriber(
    redis_url: &str,
    pool: PgPool,
    arb_executor_config: Option<Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>>,
    quant_executor_config: Option<
        Arc<RwLock<crate::quant_signal_executor::QuantSignalExecutorConfig>>,
    >,
) -> anyhow::Result<()> {
    let allowed_sources = load_allowed_update_sources();
    let bounds = load_dynamic_bounds(&pool).await;

    let client = redis::Client::open(redis_url)?;
    let conn = client.get_async_connection().await?;
    let mut pubsub = conn.into_pubsub();

    pubsub.subscribe(channels::CONFIG_UPDATES).await?;
    info!(
        channel = channels::CONFIG_UPDATES,
        "Subscribed to dynamic config updates"
    );

    // Updates published while disconnected were missed: the stored values
    // are applied again on every (re)connect.
    apply_dynamic_config_snapshot(
        &pool,
        arb_executor_config.as_ref(),
        quant_executor_config.as_ref(),
    )
    .await;

    let mut stream = pubsub.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "Invalid dynamic config payload");
                continue;
            }
        };

        let mut update: DynamicConfigUpdate = match serde_json::from_str(&payload) {
            Ok(update) => update,
            Err(e) => {
                warn!(error = %e, payload = %payload, "Failed parsing dynamic config update");
                continue;
            }
        };

        if !source_allowed(&update.source, &allowed_sources) {
            warn!(
                source = %update.source,
                key = %update.key,
                "Ignoring dynamic config update from unauthorized source"
            );
            continue;
        }

        let Some(validated) = clamp_dynamic_value(&update.key, update.value, &bounds) else {
            warn!(key = %update.key, "Ignoring dynamic config update for unsupported key");
            continue;
        };
        if validated != update.value {
            warn!(
                key = %update.key,
                source = %update.source,
                old = %update.value,
                new = %validated,
                "Clamped dynamic config update to allowed bounds"
            );
        }
        update.value = validated;

        // Apply arb executor config updates
        if let Some(ref arb_config) = arb_executor_config {
            let applied = match update.key.as_str() {
                KEY_ARB_POSITION_SIZE => {
                    arb_config.write().await.position_size = update.value;
                    true
                }
                KEY_ARB_MIN_NET_PROFIT => {
                    arb_config.write().await.min_net_profit = update.value;
                    true
                }
                KEY_ARB_MIN_BOOK_DEPTH => {
                    arb_config.write().await.min_book_depth = update.value;
                    true
                }
                KEY_ARB_MAX_SIGNAL_AGE_SECS => {
                    if let Some(secs) = update.value.to_i64() {
                        arb_config.write().await.max_signal_age_secs = secs;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if applied {
                info!(
                    key = %update.key,
                    value = %update.value,
                    source = %update.source,
                    "Applied arb executor config update at runtime"
                );
            }
        }

        if let Some(ref quant_config) = quant_executor_config {
            let applied = match update.key.as_str() {
                KEY_QUANT_BASE_POSITION_SIZE => {
                    quant_config.write().await.base_position_size_usd = update.value;
                    true
                }
                _ => false,
            };
            if applied {
                info!(
                    key = %update.key,
                    value = %update.value,
                    source = %update.source,
                    "Applied quant executor config update at runtime"
                );
            }
        }
    }

    Ok(())
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

async fn load_dynamic_bounds(pool: &PgPool) -> HashMap<String, (Decimal, Decimal)> {
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
            warn!(error = %e, "Failed loading dynamic config bounds; using fallback bounds");
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

/// Applies the current dynamic config snapshot to the arb executor config.
///
/// Used at startup to prefer DB-backed runtime configuration when available,
/// while still allowing env defaults as fallback.
async fn apply_startup_snapshot_to_arb_executor(
    pool: &PgPool,
    arb_config: &Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>,
    bounds: &HashMap<String, (Decimal, Decimal)>,
) -> anyhow::Result<()> {
    let rows: Vec<DynamicValueRow> = sqlx::query_as(
        r#"
        SELECT key, current_value
        FROM dynamic_config
        WHERE enabled = TRUE
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut applied = 0usize;
    for row in rows {
        let Some(value) = clamp_dynamic_value(&row.key, row.current_value, bounds) else {
            continue;
        };
        let did_apply = match row.key.as_str() {
            KEY_ARB_POSITION_SIZE => {
                arb_config.write().await.position_size = value;
                true
            }
            KEY_ARB_MIN_NET_PROFIT => {
                arb_config.write().await.min_net_profit = value;
                true
            }
            KEY_ARB_MIN_BOOK_DEPTH => {
                arb_config.write().await.min_book_depth = value;
                true
            }
            KEY_ARB_MAX_SIGNAL_AGE_SECS => {
                if let Some(secs) = value.to_i64() {
                    arb_config.write().await.max_signal_age_secs = secs;
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if did_apply {
            applied += 1;
        }
    }

    debug!(
        applied,
        "Applied the stored dynamic config to the arb executor"
    );
    Ok(())
}

/// Applies the current dynamic config snapshot to the quant executor config.
async fn apply_startup_snapshot_to_quant_executor(
    pool: &PgPool,
    quant_config: &Arc<RwLock<crate::quant_signal_executor::QuantSignalExecutorConfig>>,
    bounds: &HashMap<String, (Decimal, Decimal)>,
) -> anyhow::Result<()> {
    let rows: Vec<DynamicValueRow> = sqlx::query_as(
        r#"
        SELECT key, current_value
        FROM dynamic_config
        WHERE enabled = TRUE
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut applied = 0usize;
    for row in rows {
        let Some(value) = clamp_dynamic_value(&row.key, row.current_value, bounds) else {
            continue;
        };
        let did_apply = match row.key.as_str() {
            KEY_QUANT_BASE_POSITION_SIZE => {
                quant_config.write().await.base_position_size_usd = value;
                true
            }
            _ => false,
        };
        if did_apply {
            applied += 1;
        }
    }

    debug!(
        applied,
        "Applied the stored dynamic config to the quant executor"
    );
    Ok(())
}

fn load_allowed_update_sources() -> Vec<String> {
    // Blank is unset: an empty list would refuse every update.
    polymarket_core::config::env_set("DYNAMIC_CONFIG_ALLOWED_SOURCES")
        .unwrap_or_else(|| {
            "dynamic_tuner,dynamic_tuner_rollback,dynamic_tuner_sync,workspace_manual".to_string()
        })
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn source_allowed(source: &str, allowed: &[String]) -> bool {
    if source.is_empty() {
        return false;
    }
    allowed.iter().any(|entry| entry == source)
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

fn fallback_dynamic_bounds() -> HashMap<String, (Decimal, Decimal)> {
    let mut map = HashMap::new();
    for key in [
        KEY_ARB_MIN_PROFIT_THRESHOLD,
        KEY_ARB_MONITOR_MAX_MARKETS,
        KEY_ARB_MONITOR_EXPLORATION_SLOTS,
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL,
        KEY_ARB_POSITION_SIZE,
        KEY_ARB_MIN_NET_PROFIT,
        KEY_ARB_MIN_BOOK_DEPTH,
        KEY_ARB_MAX_SIGNAL_AGE_SECS,
        KEY_QUANT_BASE_POSITION_SIZE,
    ] {
        if let Some(bounds) = fallback_bounds_for_key(key) {
            map.insert(key.to_string(), bounds);
        }
    }
    map
}

fn fallback_bounds_for_key(key: &str) -> Option<(Decimal, Decimal)> {
    match key {
        KEY_ARB_MIN_PROFIT_THRESHOLD => Some((Decimal::new(2, 3), Decimal::new(5, 3))),
        KEY_ARB_MONITOR_MAX_MARKETS => Some((Decimal::new(25, 0), Decimal::new(1500, 0))),
        KEY_ARB_MONITOR_EXPLORATION_SLOTS => Some((Decimal::new(1, 0), Decimal::new(500, 0))),
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL => Some((Decimal::ZERO, Decimal::new(2, 0))),
        KEY_ARB_POSITION_SIZE => Some((Decimal::new(5, 0), Decimal::new(25, 0))),
        KEY_ARB_MIN_NET_PROFIT => Some((Decimal::new(5, 4), Decimal::new(5, 2))),
        KEY_ARB_MIN_BOOK_DEPTH => Some((Decimal::new(10, 0), Decimal::new(200, 0))),
        KEY_ARB_MAX_SIGNAL_AGE_SECS => Some((Decimal::new(5, 0), Decimal::new(300, 0))),
        KEY_QUANT_BASE_POSITION_SIZE => Some((Decimal::new(10, 0), Decimal::new(200, 0))),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct ConfigSeed {
    key: &'static str,
    default_value: Decimal,
    min_value: Decimal,
    max_value: Decimal,
    max_step_pct: Decimal,
}

fn clamp_to_bounds(value: Decimal, min: Decimal, max: Decimal) -> Decimal {
    value.max(min).min(max)
}

fn env_aggressiveness_level() -> Decimal {
    match polymarket_core::config::env_choice("ARB_MONITOR_AGGRESSIVENESS", "balanced").as_str() {
        "stable" | "conservative" => Decimal::ZERO,
        "discovery" | "aggressive" => Decimal::new(2, 0),
        _ => Decimal::new(1, 0),
    }
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator <= 0.0 {
        return 0.0;
    }
    (numerator / denominator).clamp(0.0, 1.0)
}

fn decimal_to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

fn apply_step_limit(current: f64, target: f64, max_step_pct: f64, reference: f64) -> f64 {
    let pct = if max_step_pct <= 0.0 {
        0.12
    } else {
        max_step_pct
    };
    let base = if current.abs() > EPSILON {
        current.abs()
    } else {
        reference.abs().max(1.0)
    };
    let max_delta = (base * pct).max(0.0001);
    let delta = target - current;
    if delta.abs() <= max_delta {
        target
    } else {
        current + delta.signum() * max_delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuner() -> DynamicTuner {
        DynamicTuner::new(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://localhost/unused")
                .unwrap(),
            Arc::new(RwLock::new(MarketRegime::Ranging)),
            Arc::new(CircuitBreaker::new(Default::default())),
        )
    }

    fn row(key: &str, current: i64, default: i64) -> DynamicConfigRow {
        DynamicConfigRow {
            key: key.to_string(),
            current_value: Decimal::from(current),
            default_value: Decimal::from(default),
            min_value: Decimal::ZERO,
            max_value: Decimal::from(10_000),
            max_step_pct: Decimal::new(12, 2),
            enabled: true,
            last_good_value: Decimal::from(current),
            pending_eval: false,
            pending_baseline: None,
            last_applied_at: None,
            updated_by: None,
            updated_at: None,
        }
    }

    fn idle_metrics(top_skip: &str) -> TuningMetrics {
        TuningMetrics {
            attempts_last_window: 50.0,
            fills_last_window: 0.0,
            top_skip_reason: Some(top_skip.to_string()),
            depth_proxy: 1.0,
            updates_per_minute: 60.0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn signals_held_back_by_the_threshold_wake_the_watchdog() {
        let tuner = tuner();
        let rows = HashMap::from([(
            KEY_ARB_MIN_PROFIT_THRESHOLD.to_string(),
            DynamicConfigRow {
                current_value: Decimal::new(5, 3),
                default_value: Decimal::new(5, 3),
                ..row(KEY_ARB_MIN_PROFIT_THRESHOLD, 0, 0)
            },
        )]);
        // Every signal skipped below the threshold: no position, no attempt.
        let skipped = TuningMetrics {
            gated_skips_last_window: 50.0,
            top_skip_reason: Some("below_min_profit".to_string()),
            updates_per_minute: 60.0,
            ..Default::default()
        };
        assert!(tuner.no_trade_watchdog(&skipped));
        let target = tuner.compute_targets(&rows, &skipped, MarketRegime::Ranging)
            [KEY_ARB_MIN_PROFIT_THRESHOLD];
        assert!(target < 0.005);

        // A fill in the window means trades are happening.
        let filled = TuningMetrics {
            fills_last_window: 1.0,
            ..skipped.clone()
        };
        assert!(!tuner.no_trade_watchdog(&filled));
    }

    #[tokio::test]
    async fn a_relaxed_depth_floor_returns_to_its_default() {
        let tuner = tuner();
        let rows = HashMap::from([(
            KEY_ARB_MIN_BOOK_DEPTH.to_string(),
            row(KEY_ARB_MIN_BOOK_DEPTH, 70, 100),
        )]);
        let blocked = idle_metrics("insufficient_depth");
        assert!(
            tuner.compute_targets(&rows, &blocked, MarketRegime::Ranging)[KEY_ARB_MIN_BOOK_DEPTH]
                < 70.0
        );
        let clear = idle_metrics("active_position_exists");
        assert_eq!(
            tuner.compute_targets(&rows, &clear, MarketRegime::Ranging)[KEY_ARB_MIN_BOOK_DEPTH],
            100.0
        );
        // One an operator set above the default is left alone.
        let raised = HashMap::from([(
            KEY_ARB_MIN_BOOK_DEPTH.to_string(),
            row(KEY_ARB_MIN_BOOK_DEPTH, 150, 100),
        )]);
        assert!(!tuner
            .compute_targets(&raised, &clear, MarketRegime::Ranging)
            .contains_key(KEY_ARB_MIN_BOOK_DEPTH));
    }

    #[tokio::test]
    async fn market_count_growth_stops_at_a_multiple_of_the_default() {
        let tuner = tuner();
        let busy = TuningMetrics {
            updates_per_minute: 1_000.0,
            ..Default::default()
        };
        let at = |current| {
            let rows = HashMap::from([(
                KEY_ARB_MONITOR_MAX_MARKETS.to_string(),
                row(KEY_ARB_MONITOR_MAX_MARKETS, current, 300),
            )]);
            tuner.compute_targets(&rows, &busy, MarketRegime::Ranging)[KEY_ARB_MONITOR_MAX_MARKETS]
        };
        assert_eq!(at(300), 330.0);
        assert_eq!(at(580), 600.0);
        assert_eq!(at(600), 600.0);
        // Past the cap already (set by an operator): held, not cut.
        assert_eq!(at(900), 900.0);
    }

    #[tokio::test]
    async fn watchdog_only_relaxes_the_threshold_for_threshold_skips() {
        let tuner = tuner();
        let rows = HashMap::new();
        let baseline = tuner.compute_targets(
            &rows,
            &idle_metrics("active_position_exists"),
            MarketRegime::Ranging,
        )[KEY_ARB_MIN_PROFIT_THRESHOLD];
        let relaxed = tuner.compute_targets(
            &rows,
            &idle_metrics("below_min_profit"),
            MarketRegime::Ranging,
        )[KEY_ARB_MIN_PROFIT_THRESHOLD];
        assert!(relaxed < baseline);
        let stale = tuner.compute_targets(&rows, &idle_metrics("too_stale"), MarketRegime::Ranging)
            [KEY_ARB_MIN_PROFIT_THRESHOLD];
        assert_eq!(stale, baseline);
    }

    #[tokio::test]
    async fn quiet_healthy_stream_restores_market_cap_instead_of_shrinking() {
        let tuner = tuner();
        let rows: HashMap<_, _> = [(
            KEY_ARB_MONITOR_MAX_MARKETS.to_string(),
            row(KEY_ARB_MONITOR_MAX_MARKETS, 150, 300),
        )]
        .into_iter()
        .collect();
        let quiet = TuningMetrics {
            updates_per_minute: 60.0,
            ..Default::default()
        };
        assert_eq!(
            tuner.compute_targets(&rows, &quiet, MarketRegime::Ranging)
                [KEY_ARB_MONITOR_MAX_MARKETS],
            300.0
        );

        let stalling = TuningMetrics {
            updates_per_minute: 60.0,
            ws_stall_rate: 0.5,
            ..Default::default()
        };
        assert!(
            tuner.compute_targets(&rows, &stalling, MarketRegime::Ranging)
                [KEY_ARB_MONITOR_MAX_MARKETS]
                < 150.0
        );
    }

    #[test]
    fn step_limit_caps_changes() {
        let next = apply_step_limit(100.0, 140.0, 0.1, 100.0);
        assert!((next - 110.0).abs() < 1e-6);

        let down = apply_step_limit(100.0, 50.0, 0.1, 100.0);
        assert!((down - 90.0).abs() < 1e-6);
    }

    #[test]
    fn ratio_handles_zero() {
        assert_eq!(ratio(1.0, 0.0), 0.0);
        assert_eq!(ratio(0.0, 10.0), 0.0);
        assert_eq!(ratio(5.0, 10.0), 0.5);
    }

    #[test]
    fn clamp_to_bounds_enforces_limits() {
        assert_eq!(
            clamp_to_bounds(Decimal::new(1, 0), Decimal::new(2, 0), Decimal::new(50, 0)),
            Decimal::new(2, 0)
        );
        assert_eq!(
            clamp_to_bounds(Decimal::new(60, 0), Decimal::new(2, 0), Decimal::new(50, 0)),
            Decimal::new(50, 0)
        );
        assert_eq!(
            clamp_to_bounds(Decimal::new(10, 0), Decimal::new(2, 0), Decimal::new(50, 0)),
            Decimal::new(10, 0)
        );
    }
}
