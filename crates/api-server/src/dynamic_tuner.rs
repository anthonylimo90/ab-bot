//! Dynamic runtime tuning for copy-trading and arbitrage thresholds.
//!
//! The tuner senses execution quality + market conditions every few minutes,
//! computes bounded targets, applies gradual updates, and broadcasts config
//! changes over Redis so services can adapt without restart.

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use redis::AsyncCommands;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{error, info, warn};
use wallet_tracker::MarketRegime;

use risk_manager::circuit_breaker::CircuitBreaker;
use trading_engine::copy_trader::CopyTrader;

const KEY_COPY_MIN_TRADE_VALUE: &str = "COPY_MIN_TRADE_VALUE";
const KEY_COPY_MAX_SLIPPAGE_PCT: &str = "COPY_MAX_SLIPPAGE_PCT";
const KEY_ARB_MIN_PROFIT_THRESHOLD: &str = "ARB_MIN_PROFIT_THRESHOLD";
const KEY_ARB_MONITOR_MAX_MARKETS: &str = "ARB_MONITOR_MAX_MARKETS";
const KEY_ARB_MONITOR_EXPLORATION_SLOTS: &str = "ARB_MONITOR_EXPLORATION_SLOTS";
const KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL: &str = "ARB_MONITOR_AGGRESSIVENESS_LEVEL";
const KEY_COPY_MAX_LATENCY_SECS: &str = "COPY_MAX_LATENCY_SECS";
const KEY_COPY_DAILY_CAPITAL_LIMIT: &str = "COPY_DAILY_CAPITAL_LIMIT";
const KEY_COPY_MAX_OPEN_POSITIONS: &str = "COPY_MAX_OPEN_POSITIONS";
const KEY_COPY_STOP_LOSS_PCT: &str = "COPY_STOP_LOSS_PCT";
const KEY_COPY_TAKE_PROFIT_PCT: &str = "COPY_TAKE_PROFIT_PCT";
const KEY_COPY_MAX_HOLD_HOURS: &str = "COPY_MAX_HOLD_HOURS";
const KEY_ARB_POSITION_SIZE: &str = "ARB_POSITION_SIZE";
const KEY_ARB_MIN_NET_PROFIT: &str = "ARB_MIN_NET_PROFIT";
const KEY_ARB_MIN_BOOK_DEPTH: &str = "ARB_MIN_BOOK_DEPTH";
const KEY_ARB_MAX_SIGNAL_AGE_SECS: &str = "ARB_MAX_SIGNAL_AGE_SECS";
const KEY_COPY_TOTAL_CAPITAL: &str = "COPY_TOTAL_CAPITAL";
const KEY_COPY_NEAR_RESOLUTION_MARGIN: &str = "COPY_NEAR_RESOLUTION_MARGIN";

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
            enabled: std::env::var("DYNAMIC_TUNER_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            interval_secs: std::env::var("DYNAMIC_TUNER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            apply_changes: std::env::var("DYNAMIC_TUNER_APPLY")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            regime_hysteresis_intervals: std::env::var("DYNAMIC_TUNER_REGIME_STREAK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            max_drawdown_freeze: std::env::var("DYNAMIC_TUNER_FREEZE_DRAWDOWN")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.20),
            evaluation_delay_minutes: std::env::var("DYNAMIC_TUNER_EVAL_DELAY_MINUTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            fill_rate_degrade_delta: std::env::var("DYNAMIC_TUNER_FILL_DEGRADE_DELTA")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.08),
            pnl_degrade_delta: std::env::var("DYNAMIC_TUNER_PNL_DEGRADE_DELTA")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(75.0),
            bootstrap_enabled: std::env::var("DYNAMIC_TUNER_BOOTSTRAP_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            bootstrap_max_attempts: std::env::var("DYNAMIC_TUNER_BOOTSTRAP_MAX_ATTEMPTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            no_trade_window_minutes: std::env::var("DYNAMIC_TUNER_NO_TRADE_WINDOW_MINUTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(120),
            no_trade_min_attempts: std::env::var("DYNAMIC_TUNER_NO_TRADE_MIN_ATTEMPTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
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
}

pub struct DynamicTuner {
    pool: PgPool,
    config: DynamicTunerConfig,
    current_regime: Arc<RwLock<MarketRegime>>,
    circuit_breaker: Arc<CircuitBreaker>,
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

        let mut ticker = interval(TokioDuration::from_secs(self.config.interval_secs));
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
        let no_trade_watchdog = metrics.attempts_last_window
            >= self.config.no_trade_min_attempts as f64
            && metrics.fills_last_window <= EPSILON;
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
            if row.pending_eval {
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

            let Some(new_value) = Decimal::from_f64_retain(bounded) else {
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
                self.apply_change(row, new_value, &reason, &metrics).await?;
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
                    warn!(
                        key = %key,
                        attempted_value = %new_value,
                        error = %e,
                        "Dynamic config publish failed; reverting DB change to preserve runtime/DB consistency"
                    );
                    self.rollback_unpublished_change(row, new_value, &reason, &e)
                        .await?;
                    continue;
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

                sqlx::query(
                    r#"
                    UPDATE dynamic_config
                    SET current_value = $2,
                        pending_eval = FALSE,
                        pending_baseline = NULL,
                        last_reason = $3,
                        updated_by = 'dynamic_tuner'
                    WHERE key = $1
                    "#,
                )
                .bind(&row.key)
                .bind(row.last_good_value)
                .bind(&reason)
                .execute(&self.pool)
                .await?;

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
                sqlx::query(
                    r#"
                    UPDATE dynamic_config
                    SET pending_eval = FALSE,
                        pending_baseline = NULL,
                        last_good_value = current_value,
                        updated_by = 'dynamic_tuner'
                    WHERE key = $1
                    "#,
                )
                .bind(&row.key)
                .execute(&self.pool)
                .await?;

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

    fn compute_targets(
        &self,
        rows: &HashMap<String, DynamicConfigRow>,
        metrics: &TuningMetrics,
        regime: MarketRegime,
    ) -> HashMap<String, f64> {
        let mut targets = HashMap::new();

        let min_trade_current = rows
            .get(KEY_COPY_MIN_TRADE_VALUE)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(2.0);
        let slippage_current = rows
            .get(KEY_COPY_MAX_SLIPPAGE_PCT)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(0.01);
        let arb_profit_current = rows
            .get(KEY_ARB_MIN_PROFIT_THRESHOLD)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(0.005);
        let max_markets_current = rows
            .get(KEY_ARB_MONITOR_MAX_MARKETS)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(300.0);

        let bootstrap_active = self.config.bootstrap_enabled
            && metrics.attempts_last_window < self.config.bootstrap_max_attempts as f64;
        let no_trade_watchdog = metrics.attempts_last_window
            >= self.config.no_trade_min_attempts as f64
            && metrics.fills_last_window <= EPSILON;
        let top_skip = metrics
            .top_skip_reason
            .as_deref()
            .unwrap_or("none")
            .to_string();

        let (regime_min_trade_mult, regime_buffer, regime_safety_margin) = match regime {
            MarketRegime::BullCalm => (0.95, 0.0012, 0.0010),
            MarketRegime::BullVolatile => (1.03, 0.0020, 0.0018),
            MarketRegime::BearCalm => (1.05, 0.0018, 0.0022),
            MarketRegime::BearVolatile => (1.10, 0.0028, 0.0030),
            MarketRegime::Ranging => (1.00, 0.0016, 0.0018),
            MarketRegime::Uncertain => (1.07, 0.0022, 0.0025),
        };

        // COPY_MIN_TRADE_VALUE: lower when many useful trades are skipped by minimum,
        // raise when slippage/noise dominates.
        let below_min_push_down = ((metrics.below_min_skip_rate - 0.20).max(0.0) * 0.35).min(0.20);
        let slippage_push_up = ((metrics.slippage_skip_rate - 0.15).max(0.0) * 0.45).min(0.25);
        let pnl_push_up = if metrics.recent_pnl < 0.0 { 0.05 } else { 0.0 };
        let mut desired_min_trade = min_trade_current
            * (1.0 - below_min_push_down + slippage_push_up + pnl_push_up)
            * regime_min_trade_mult;
        if bootstrap_active {
            desired_min_trade = desired_min_trade.min(5.0);
        }
        if no_trade_watchdog {
            desired_min_trade = match top_skip.as_str() {
                "below_minimum" => desired_min_trade.min((min_trade_current * 0.70).max(0.50)),
                "too_stale" => desired_min_trade.min((min_trade_current * 0.85).max(0.50)),
                // Near-resolution and market_not_active are infrastructure/cache problems,
                // not parameter problems. Pin to current value to prevent drift —
                // the base formula already moved desired_min_trade away from current.
                "near_resolution" | "market_not_active" => min_trade_current,
                _ => desired_min_trade.min((min_trade_current * 0.80).max(0.50)),
            };
        }
        targets.insert(KEY_COPY_MIN_TRADE_VALUE.to_string(), desired_min_trade);

        // COPY_MAX_SLIPPAGE_PCT: set near recent p90 + buffer.
        let fill_relax = if metrics.successful_fill_rate < 0.35 {
            0.0010
        } else {
            0.0
        };
        let mut desired_slippage = (metrics.realized_slippage_p90 + regime_buffer + fill_relax)
            .max(slippage_current * 0.7)
            .min(slippage_current * 1.4);
        if bootstrap_active {
            desired_slippage = desired_slippage.max(0.02);
        }
        if no_trade_watchdog {
            desired_slippage = match top_skip.as_str() {
                "slippage" => desired_slippage.max((slippage_current * 1.50).max(0.015)),
                "too_stale" => desired_slippage.max((slippage_current * 1.10).max(0.012)),
                // Near-resolution and market_not_active are infrastructure/cache problems.
                // Cannot be fixed by relaxing slippage — pin to current to prevent drift.
                // The base formula drives desired_slippage toward 0 when p90 = 0 (no fills).
                "near_resolution" | "market_not_active" => slippage_current,
                _ => desired_slippage.max((slippage_current * 1.15).max(0.012)),
            };
        }
        targets.insert(KEY_COPY_MAX_SLIPPAGE_PCT.to_string(), desired_slippage);

        // COPY_MAX_LATENCY_SECS: relax when trades are rejected as too_stale,
        // tighten slightly otherwise to prefer fresher fills.
        let latency_current = rows
            .get(KEY_COPY_MAX_LATENCY_SECS)
            .map(|r| decimal_to_f64(r.current_value))
            .unwrap_or(120.0);
        let desired_latency = if no_trade_watchdog && top_skip == "too_stale" {
            // Most skips are stale trades — relax the threshold to let them through.
            (latency_current * 1.30).min(300.0)
        } else if no_trade_watchdog || bootstrap_active {
            // No fills yet (watchdog or bootstrap) but staleness isn't the top issue.
            // Hold latency steady — tightening during 0-fill phases is counter-productive.
            latency_current
        } else {
            // Normal operation — tighten slightly toward fresher fills.
            (latency_current * 0.95).max(60.0)
        };
        targets.insert(KEY_COPY_MAX_LATENCY_SECS.to_string(), desired_latency);

        // ARB_MIN_PROFIT_THRESHOLD: expected slippage + safety margin.
        let expected_arb_slippage = (metrics.realized_slippage_p90 * 2.0).max(0.0015);
        let low_depth_penalty = (0.0010 - metrics.depth_proxy).max(0.0) * 0.3;
        let vol_penalty = metrics.volatility_proxy * 0.01;
        let mut desired_arb_profit =
            expected_arb_slippage + regime_safety_margin + low_depth_penalty + vol_penalty;
        if no_trade_watchdog && top_skip != "slippage" {
            desired_arb_profit *= 0.90;
        }
        targets.insert(
            KEY_ARB_MIN_PROFIT_THRESHOLD.to_string(),
            desired_arb_profit.max(arb_profit_current * 0.7),
        );

        // ARB_MONITOR_MAX_MARKETS: adapt to stream health + throughput.
        let health_penalty = (metrics.ws_stall_rate * 0.9 + metrics.ws_reset_rate * 0.6).min(1.0);
        let throughput_bonus = if metrics.updates_per_minute > 500.0 {
            0.10
        } else {
            0.0
        };
        let throughput_penalty = if metrics.updates_per_minute < 120.0 {
            0.10
        } else {
            0.0
        };

        let desired_market_factor =
            (1.0 - health_penalty - throughput_penalty + throughput_bonus).clamp(0.70, 1.20);
        targets.insert(
            KEY_ARB_MONITOR_MAX_MARKETS.to_string(),
            (max_markets_current * desired_market_factor).round(),
        );

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
        let window_minutes = self.config.no_trade_window_minutes.max(5);
        let row: (i64, i64, i64, i64, Decimal) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*)::bigint AS attempts,
                COALESCE(SUM(CASE WHEN status = 1 THEN 1 ELSE 0 END), 0)::bigint AS fills,
                COALESCE(SUM(CASE WHEN status = 3 AND COALESCE(skip_reason, '') = 'slippage' THEN 1 ELSE 0 END), 0)::bigint AS slippage_skips,
                COALESCE(SUM(CASE WHEN status = 3 AND COALESCE(skip_reason, '') = 'below_minimum' THEN 1 ELSE 0 END), 0)::bigint AS below_min_skips,
                COALESCE(
                    PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY ABS(slippage))
                    FILTER (WHERE status = 1 AND slippage IS NOT NULL),
                    0
                )::numeric AS slippage_p90
            FROM copy_trade_history
            WHERE created_at >= NOW() - make_interval(mins => $1::int)
            "#,
        )
        .bind(window_minutes as i32)
        .fetch_one(&self.pool)
        .await?;

        let top_skip_reason: Option<String> = sqlx::query_scalar(
            r#"
            SELECT skip_reason
            FROM copy_trade_history
            WHERE status = 3
              AND skip_reason IS NOT NULL
              AND created_at >= NOW() - make_interval(mins => $1::int)
            GROUP BY skip_reason
            ORDER BY COUNT(*) DESC
            LIMIT 1
            "#,
        )
        .bind(window_minutes as i32)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None);

        let attempts = row.0.max(0) as f64;
        let fills = row.1.max(0) as f64;
        let slippage_skips = row.2.max(0) as f64;
        let below_min_skips = row.3.max(0) as f64;

        let recent_pnl: Decimal = sqlx::query_scalar(
            r#"
            SELECT COALESCE(SUM(realized_pnl), 0)
            FROM positions
            WHERE exit_timestamp >= NOW() - INTERVAL '6 hours'
              AND realized_pnl IS NOT NULL
              AND source IN (1, 2, 3)
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let volatility_proxy: f64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(AVG(volatility_30d::double precision), 0)
            FROM wallet_success_metrics
            WHERE last_computed >= NOW() - INTERVAL '48 hours'
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

        Ok(TuningMetrics {
            slippage_skip_rate: ratio(slippage_skips, attempts),
            below_min_skip_rate: ratio(below_min_skips, attempts),
            successful_fill_rate: ratio(fills, attempts),
            attempts_last_window: attempts,
            fills_last_window: fills,
            top_skip_reason,
            realized_slippage_p90: decimal_to_f64(row.4).abs(),
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
                key: KEY_COPY_MIN_TRADE_VALUE,
                default_value: env_decimal("COPY_MIN_TRADE_VALUE", Decimal::new(2, 0)),
                min_value: Decimal::new(50, 2), // $0.50 floor (was $2)
                max_value: Decimal::new(50, 0),
                max_step_pct: Decimal::new(18, 2), // 18% per cycle (was 12%)
            },
            ConfigSeed {
                key: KEY_COPY_MAX_SLIPPAGE_PCT,
                default_value: env_decimal("COPY_MAX_SLIPPAGE_PCT", Decimal::new(1, 2)),
                min_value: Decimal::new(25, 4),    // 0.25% floor
                max_value: Decimal::new(15, 2),    // 15% ceiling (was 5%)
                max_step_pct: Decimal::new(25, 2), // 25% per cycle (was 15%)
            },
            ConfigSeed {
                key: KEY_ARB_MIN_PROFIT_THRESHOLD,
                default_value: env_decimal("ARB_MIN_PROFIT_THRESHOLD", Decimal::new(5, 3)),
                min_value: Decimal::new(2, 3),
                max_value: Decimal::new(5, 2),
                max_step_pct: Decimal::new(12, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MONITOR_MAX_MARKETS,
                default_value: env_decimal("ARB_MONITOR_MAX_MARKETS", Decimal::new(300, 0)),
                min_value: Decimal::new(25, 0),
                max_value: Decimal::new(1500, 0),
                max_step_pct: Decimal::new(15, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MONITOR_EXPLORATION_SLOTS,
                default_value: env_decimal("ARB_MONITOR_EXPLORATION_SLOTS", Decimal::new(5, 0)),
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
            ConfigSeed {
                key: KEY_COPY_MAX_LATENCY_SECS,
                default_value: env_decimal("COPY_MAX_LATENCY_SECS", Decimal::new(120, 0)),
                min_value: Decimal::new(30, 0),    // 30 second floor
                max_value: Decimal::new(300, 0),   // 5 minute ceiling (was 15 min)
                max_step_pct: Decimal::new(20, 2), // 20% per cycle
            },
            ConfigSeed {
                key: KEY_COPY_DAILY_CAPITAL_LIMIT,
                default_value: env_decimal("COPY_DAILY_CAPITAL_LIMIT", Decimal::new(5000, 0)),
                min_value: Decimal::new(100, 0),   // $100 floor
                max_value: Decimal::new(50000, 0), // $50,000 ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_COPY_MAX_OPEN_POSITIONS,
                default_value: env_decimal("COPY_MAX_OPEN_POSITIONS", Decimal::new(15, 0)),
                min_value: Decimal::new(1, 0),
                max_value: Decimal::new(50, 0),
                max_step_pct: Decimal::new(25, 2),
            },
            ConfigSeed {
                key: KEY_COPY_STOP_LOSS_PCT,
                default_value: env_decimal("COPY_STOP_LOSS_PCT", Decimal::new(15, 2)),
                min_value: Decimal::new(5, 2),  // 5% floor
                max_value: Decimal::new(50, 2), // 50% ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_COPY_TAKE_PROFIT_PCT,
                default_value: env_decimal("COPY_TAKE_PROFIT_PCT", Decimal::new(25, 2)),
                min_value: Decimal::new(5, 2),   // 5% floor
                max_value: Decimal::new(100, 2), // 100% ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_COPY_MAX_HOLD_HOURS,
                default_value: env_decimal("COPY_MAX_HOLD_HOURS", Decimal::new(72, 0)),
                min_value: Decimal::new(1, 0),   // 1 hour floor
                max_value: Decimal::new(720, 0), // 30 days ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            // ── Arb executor tuning knobs ──
            ConfigSeed {
                key: KEY_ARB_POSITION_SIZE,
                default_value: env_decimal("ARB_POSITION_SIZE", Decimal::new(50, 0)),
                min_value: Decimal::new(10, 0),  // $10 floor
                max_value: Decimal::new(500, 0), // $500 ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MIN_NET_PROFIT,
                default_value: env_decimal("ARB_MIN_NET_PROFIT", Decimal::new(1, 3)),
                min_value: Decimal::new(5, 4), // 0.0005 floor
                max_value: Decimal::new(5, 2), // 0.05 ceiling
                max_step_pct: Decimal::new(15, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MIN_BOOK_DEPTH,
                default_value: env_decimal("ARB_MIN_BOOK_DEPTH", Decimal::new(100, 0)),
                min_value: Decimal::new(25, 0),   // $25 floor
                max_value: Decimal::new(1000, 0), // $1,000 ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_ARB_MAX_SIGNAL_AGE_SECS,
                default_value: env_decimal("ARB_MAX_SIGNAL_AGE_SECS", Decimal::new(30, 0)),
                min_value: Decimal::new(5, 0),   // 5s floor
                max_value: Decimal::new(300, 0), // 300s ceiling
                max_step_pct: Decimal::new(25, 2),
            },
            ConfigSeed {
                key: KEY_COPY_TOTAL_CAPITAL,
                default_value: env_decimal("COPY_TOTAL_CAPITAL", Decimal::new(10000, 0)),
                min_value: Decimal::new(100, 0),    // $100 floor
                max_value: Decimal::new(500000, 0), // $500,000 ceiling
                max_step_pct: Decimal::new(20, 2),
            },
            ConfigSeed {
                key: KEY_COPY_NEAR_RESOLUTION_MARGIN,
                default_value: env_decimal("COPY_NEAR_RESOLUTION_MARGIN", Decimal::new(3, 2)),
                min_value: Decimal::new(3, 2), // 0.03 (3%) floor — never fully disabled
                max_value: Decimal::new(25, 2), // 0.25 ceiling
                max_step_pct: Decimal::new(50, 2),
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
                    current_value = LEAST(GREATEST(dynamic_config.current_value, EXCLUDED.min_value), EXCLUDED.max_value)
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
                   pending_baseline, last_applied_at
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
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            UPDATE dynamic_config
            SET current_value = $2,
                pending_eval = TRUE,
                pending_baseline = $3,
                last_applied_at = NOW(),
                last_reason = $4,
                updated_by = 'dynamic_tuner'
            WHERE key = $1
            "#,
        )
        .bind(&row.key)
        .bind(new_value)
        .bind(serde_json::to_value(metrics).unwrap_or(serde_json::json!({})))
        .bind(reason)
        .execute(&self.pool)
        .await?;

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

        Ok(())
    }

    async fn rollback_unpublished_change(
        &self,
        row: &DynamicConfigRow,
        attempted_value: Decimal,
        reason: &str,
        publish_error: &anyhow::Error,
    ) -> anyhow::Result<()> {
        let revert_reason = format!(
            "reverted unapplied change after publish failure: {publish_error}; original_reason={reason}"
        );
        sqlx::query(
            r#"
            UPDATE dynamic_config
            SET current_value = $2,
                pending_eval = FALSE,
                pending_baseline = NULL,
                last_applied_at = NULL,
                last_reason = $3,
                updated_by = 'dynamic_tuner'
            WHERE key = $1
            "#,
        )
        .bind(&row.key)
        .bind(row.current_value)
        .bind(&revert_reason)
        .execute(&self.pool)
        .await?;

        self.record_history(
            Some(&row.key),
            Some(attempted_value),
            Some(row.current_value),
            "skipped",
            "reverted dynamic update because publish to subscribers failed",
            None,
            None,
        )
        .await?;

        Ok(())
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

    async fn redis_connection_manager(&self) -> Option<redis::aio::ConnectionManager> {
        let client = redis::Client::open(self.config.redis_url.as_str()).ok()?;
        redis::aio::ConnectionManager::new(client).await.ok()
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
#[allow(clippy::too_many_arguments)]
pub fn spawn_dynamic_config_subscriber(
    redis_url: String,
    copy_trader: Option<Arc<RwLock<CopyTrader>>>,
    pool: PgPool,
    max_latency_secs: Option<Arc<AtomicI64>>,
    copy_stop_loss_config: Option<Arc<RwLock<crate::copy_trade_stop_loss::CopyStopLossConfig>>>,
    arb_executor_config: Option<Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>>,
    copy_total_capital: Option<Arc<AtomicI64>>,
    copy_near_resolution_margin: Option<Arc<AtomicI64>>,
) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run_dynamic_config_subscriber(
                redis_url.as_str(),
                copy_trader.clone(),
                pool.clone(),
                max_latency_secs.clone(),
                copy_stop_loss_config.clone(),
                arb_executor_config.clone(),
                copy_total_capital.clone(),
                copy_near_resolution_margin.clone(),
            )
            .await
            {
                error!(error = %e, "Dynamic config subscriber failed, retrying");
                tokio::time::sleep(TokioDuration::from_secs(5)).await;
            }
        }
    });

    info!("Dynamic config subscriber spawned");
}

/// Applies the current dynamic config snapshot to the local copy-trader policy.
///
/// This is used at startup to prefer DB-backed runtime configuration when
/// available, while still allowing env defaults as fallback if dynamic config
/// tables are missing or not yet seeded.
pub async fn sync_dynamic_config_snapshot_to_copy_trader(
    pool: &PgPool,
    copy_trader: &Arc<RwLock<CopyTrader>>,
    max_latency_secs: Option<&Arc<AtomicI64>>,
    copy_total_capital: Option<&Arc<AtomicI64>>,
    copy_near_resolution_margin: Option<&Arc<AtomicI64>>,
) -> anyhow::Result<()> {
    let bounds = load_dynamic_bounds(pool).await;
    apply_startup_snapshot_to_copy_trader(
        pool,
        copy_trader,
        &bounds,
        max_latency_secs,
        copy_total_capital,
        copy_near_resolution_margin,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_dynamic_config_subscriber(
    redis_url: &str,
    copy_trader: Option<Arc<RwLock<CopyTrader>>>,
    pool: PgPool,
    max_latency_secs: Option<Arc<AtomicI64>>,
    copy_stop_loss_config: Option<Arc<RwLock<crate::copy_trade_stop_loss::CopyStopLossConfig>>>,
    arb_executor_config: Option<Arc<RwLock<crate::arb_executor::ArbExecutorConfig>>>,
    copy_total_capital: Option<Arc<AtomicI64>>,
    copy_near_resolution_margin: Option<Arc<AtomicI64>>,
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

    // Subscribe before snapshot application so updates published during startup
    // are queued and processed after snapshot reconciliation.
    if let Some(ref trader) = copy_trader {
        if let Err(e) = apply_startup_snapshot_to_copy_trader(
            &pool,
            trader,
            &bounds,
            max_latency_secs.as_ref(),
            copy_total_capital.as_ref(),
            copy_near_resolution_margin.as_ref(),
        )
        .await
        {
            warn!(error = %e, "Failed applying startup dynamic config snapshot to copy trader");
        }
    }

    // Apply startup snapshot to arb executor config
    if let Some(ref arb_config) = arb_executor_config {
        if let Err(e) = apply_startup_snapshot_to_arb_executor(&pool, arb_config, &bounds).await {
            warn!(error = %e, "Failed applying startup dynamic config snapshot to arb executor");
        }
    }

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

        if let Some(ref trader) = copy_trader {
            if apply_to_copy_trader(trader, &update).await {
                info!(
                    key = %update.key,
                    value = %update.value,
                    source = %update.source,
                    "Applied dynamic config in api-server"
                );
            }
        }

        // Apply latency threshold directly to the shared atomic
        if update.key == KEY_COPY_MAX_LATENCY_SECS {
            if let Some(ref atomic) = max_latency_secs {
                if let Some(secs) = update.value.to_i64() {
                    atomic.store(secs, Ordering::Relaxed);
                    info!(
                        key = %update.key,
                        value = %update.value,
                        source = %update.source,
                        "Applied dynamic config in api-server"
                    );
                }
            }
        }

        // Apply stop-loss/take-profit/hold-hours to shared config
        if let Some(ref sl_config) = copy_stop_loss_config {
            let applied = match update.key.as_str() {
                KEY_COPY_STOP_LOSS_PCT => {
                    sl_config.write().await.stop_loss_pct = update.value;
                    true
                }
                KEY_COPY_TAKE_PROFIT_PCT => {
                    sl_config.write().await.take_profit_pct = update.value;
                    true
                }
                KEY_COPY_MAX_HOLD_HOURS => {
                    if let Some(h) = update.value.to_i64() {
                        sl_config.write().await.max_hold_hours = h;
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
                    "Applied stop-loss config update at runtime"
                );
            }
        }

        // Apply copy total capital to shared atomic (stored as cents)
        if update.key == KEY_COPY_TOTAL_CAPITAL {
            if let Some(ref atomic) = copy_total_capital {
                if let Some(dollars) = update.value.to_i64() {
                    atomic.store(dollars * 100, Ordering::Relaxed);
                    info!(
                        key = %update.key,
                        value = %update.value,
                        source = %update.source,
                        "Applied copy total capital update at runtime"
                    );
                }
            }
        }

        // Apply near-resolution margin to shared atomic (stored as margin * 10,000)
        if update.key == KEY_COPY_NEAR_RESOLUTION_MARGIN {
            if let Some(ref atomic) = copy_near_resolution_margin {
                // value is e.g. 0.03 → store as 300; floor at 300 (3%) to match MIN_MARGIN_RAW
                let margin_raw = (update.value * Decimal::new(10_000, 0))
                    .to_i64()
                    .unwrap_or(300)
                    .max(300); // Floor at 3% (MIN_MARGIN_RAW) — never allow 0
                atomic.store(margin_raw, Ordering::Relaxed);
                info!(
                    key = %update.key,
                    value = %update.value,
                    margin_raw,
                    source = %update.source,
                    "Applied near-resolution margin update at runtime"
                );
            }
        }

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
    }

    Ok(())
}

async fn apply_to_copy_trader(
    copy_trader: &Arc<RwLock<CopyTrader>>,
    update: &DynamicConfigUpdate,
) -> bool {
    let mut trader = copy_trader.write().await;

    match update.key.as_str() {
        KEY_COPY_MIN_TRADE_VALUE => {
            trader.set_min_trade_value(update.value);
            true
        }
        KEY_COPY_MAX_SLIPPAGE_PCT => {
            trader.set_max_slippage_pct(update.value);
            true
        }
        KEY_COPY_DAILY_CAPITAL_LIMIT => {
            trader.set_daily_capital_limit(update.value);
            true
        }
        KEY_COPY_MAX_OPEN_POSITIONS => {
            if let Some(n) = update.value.to_u64() {
                trader.set_max_open_positions(n as usize);
                true
            } else {
                false
            }
        }
        _ => false,
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

async fn apply_startup_snapshot_to_copy_trader(
    pool: &PgPool,
    copy_trader: &Arc<RwLock<CopyTrader>>,
    bounds: &HashMap<String, (Decimal, Decimal)>,
    max_latency_secs: Option<&Arc<AtomicI64>>,
    copy_total_capital: Option<&Arc<AtomicI64>>,
    copy_near_resolution_margin: Option<&Arc<AtomicI64>>,
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
        let update = DynamicConfigUpdate {
            key: row.key.clone(),
            value,
            reason: "startup snapshot".to_string(),
            source: "dynamic_tuner_sync".to_string(),
            timestamp: Utc::now(),
            metrics: serde_json::json!({ "sync": "subscriber_bootstrap" }),
        };

        if apply_to_copy_trader(copy_trader, &update).await {
            applied += 1;
        }

        // Apply latency threshold to the shared atomic
        if row.key == KEY_COPY_MAX_LATENCY_SECS {
            if let Some(atomic) = max_latency_secs {
                if let Some(secs) = value.to_i64() {
                    atomic.store(secs, Ordering::Relaxed);
                    applied += 1;
                }
            }
        }

        // Apply total capital to the shared atomic (stored as cents)
        if row.key == KEY_COPY_TOTAL_CAPITAL {
            if let Some(atomic) = copy_total_capital {
                if let Some(dollars) = value.to_i64() {
                    atomic.store(dollars * 100, Ordering::Relaxed);
                    applied += 1;
                }
            }
        }

        // Apply near-resolution margin to the shared atomic (stored as margin * 10,000)
        if row.key == KEY_COPY_NEAR_RESOLUTION_MARGIN {
            if let Some(atomic) = copy_near_resolution_margin {
                let margin_raw = (value * Decimal::new(10_000, 0))
                    .to_i64()
                    .unwrap_or(300)
                    .max(300); // Floor at 3% — match MIN_MARGIN_RAW
                atomic.store(margin_raw, Ordering::Relaxed);
                applied += 1;
            }
        }
    }

    info!(
        applied,
        "Applied startup dynamic config snapshot to copy trader"
    );
    Ok(())
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

    info!(
        applied,
        "Applied startup dynamic config snapshot to arb executor"
    );
    Ok(())
}

fn load_allowed_update_sources() -> Vec<String> {
    std::env::var("DYNAMIC_CONFIG_ALLOWED_SOURCES")
        .unwrap_or_else(|_| {
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
        KEY_COPY_MIN_TRADE_VALUE,
        KEY_COPY_MAX_SLIPPAGE_PCT,
        KEY_COPY_MAX_LATENCY_SECS,
        KEY_ARB_MIN_PROFIT_THRESHOLD,
        KEY_ARB_MONITOR_MAX_MARKETS,
        KEY_ARB_MONITOR_EXPLORATION_SLOTS,
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL,
        KEY_ARB_POSITION_SIZE,
        KEY_ARB_MIN_NET_PROFIT,
        KEY_ARB_MIN_BOOK_DEPTH,
        KEY_ARB_MAX_SIGNAL_AGE_SECS,
        KEY_COPY_TOTAL_CAPITAL,
        KEY_COPY_NEAR_RESOLUTION_MARGIN,
    ] {
        if let Some(bounds) = fallback_bounds_for_key(key) {
            map.insert(key.to_string(), bounds);
        }
    }
    map
}

fn fallback_bounds_for_key(key: &str) -> Option<(Decimal, Decimal)> {
    match key {
        KEY_COPY_MIN_TRADE_VALUE => Some((Decimal::new(50, 2), Decimal::new(50, 0))),
        KEY_COPY_MAX_SLIPPAGE_PCT => Some((Decimal::new(25, 4), Decimal::new(15, 2))),
        KEY_COPY_MAX_LATENCY_SECS => Some((Decimal::new(30, 0), Decimal::new(300, 0))),
        KEY_ARB_MIN_PROFIT_THRESHOLD => Some((Decimal::new(2, 3), Decimal::new(5, 2))),
        KEY_ARB_MONITOR_MAX_MARKETS => Some((Decimal::new(25, 0), Decimal::new(1500, 0))),
        KEY_ARB_MONITOR_EXPLORATION_SLOTS => Some((Decimal::new(1, 0), Decimal::new(500, 0))),
        KEY_ARB_MONITOR_AGGRESSIVENESS_LEVEL => Some((Decimal::ZERO, Decimal::new(2, 0))),
        KEY_ARB_POSITION_SIZE => Some((Decimal::new(10, 0), Decimal::new(500, 0))),
        KEY_ARB_MIN_NET_PROFIT => Some((Decimal::new(5, 4), Decimal::new(5, 2))),
        KEY_ARB_MIN_BOOK_DEPTH => Some((Decimal::new(25, 0), Decimal::new(1000, 0))),
        KEY_ARB_MAX_SIGNAL_AGE_SECS => Some((Decimal::new(5, 0), Decimal::new(300, 0))),
        KEY_COPY_TOTAL_CAPITAL => Some((Decimal::new(100, 0), Decimal::new(500000, 0))),
        KEY_COPY_NEAR_RESOLUTION_MARGIN => Some((Decimal::new(3, 2), Decimal::new(25, 2))),
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

fn env_decimal(name: &str, fallback: Decimal) -> Decimal {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

fn env_aggressiveness_level() -> Decimal {
    match std::env::var("ARB_MONITOR_AGGRESSIVENESS")
        .unwrap_or_else(|_| "balanced".to_string())
        .to_lowercase()
        .as_str()
    {
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
