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

        let mut ticker = interval(TokioDuration::from_secs(self.config.interval_secs));

        if let Err(e) = self
            .run_cycle(
                &mut stable_regime,
                &mut candidate_regime,
                &mut candidate_count,
            )
            .await
        {
            warn!(error = %e, "Initial dynamic tuning cycle failed");
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
            .await;
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

        if metrics.cb_tripped || metrics.recent_drawdown >= self.config.max_drawdown_freeze {
            self.record_history(
                None,
                None,
                None,
                "frozen",
                "risk guard active: circuit breaker/drawdown",
                Some(&metrics),
                None,
            )
            .await?;
            info!(
                cb_tripped = metrics.cb_tripped,
                drawdown = metrics.recent_drawdown,
                "Dynamic tuner frozen by risk guard"
            );
            return Ok(());
        }

        let mut by_key: HashMap<String, DynamicConfigRow> = HashMap::new();
        for row in rows {
            by_key.insert(row.key.clone(), row);
        }

        let targets = self.compute_targets(&by_key, &metrics, resolved);

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
                "auto tune: regime={} fill_rate={:.3} slip_p90={:.4} pnl={:.2} drawdown={:.3}",
                metrics.current_regime,
                metrics.successful_fill_rate,
                metrics.realized_slippage_p90,
                metrics.recent_pnl,
                metrics.recent_drawdown
            );

            if self.config.apply_changes {
                self.apply_change(row, new_value, &reason, &metrics).await?;
                self.publish_update(
                    redis_manager.as_mut(),
                    &DynamicConfigUpdate {
                        key: key.clone(),
                        value: new_value,
                        reason: reason.clone(),
                        source: "dynamic_tuner".to_string(),
                        timestamp: Utc::now(),
                        metrics: serde_json::to_value(&metrics).unwrap_or(serde_json::json!({})),
                    },
                )
                .await;
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
            }
        }

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
                .await;
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
            .unwrap_or(10.0);
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
        let desired_min_trade = min_trade_current
            * (1.0 - below_min_push_down + slippage_push_up + pnl_push_up)
            * regime_min_trade_mult;
        targets.insert(KEY_COPY_MIN_TRADE_VALUE.to_string(), desired_min_trade);

        // COPY_MAX_SLIPPAGE_PCT: set near recent p90 + buffer.
        let fill_relax = if metrics.successful_fill_rate < 0.35 {
            0.0010
        } else {
            0.0
        };
        let desired_slippage = (metrics.realized_slippage_p90 + regime_buffer + fill_relax)
            .max(slippage_current * 0.7)
            .min(slippage_current * 1.4);
        targets.insert(KEY_COPY_MAX_SLIPPAGE_PCT.to_string(), desired_slippage);

        // ARB_MIN_PROFIT_THRESHOLD: expected slippage + safety margin.
        let expected_arb_slippage = (metrics.realized_slippage_p90 * 2.0).max(0.0015);
        let low_depth_penalty = (0.0010 - metrics.depth_proxy).max(0.0) * 0.3;
        let vol_penalty = metrics.volatility_proxy * 0.01;
        let desired_arb_profit =
            expected_arb_slippage + regime_safety_margin + low_depth_penalty + vol_penalty;
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
            WHERE created_at >= NOW() - INTERVAL '2 hours'
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

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
                default_value: env_decimal("COPY_MIN_TRADE_VALUE", Decimal::new(10, 0)),
                min_value: Decimal::new(2, 0),
                max_value: Decimal::new(50, 0),
                max_step_pct: Decimal::new(12, 2),
            },
            ConfigSeed {
                key: KEY_COPY_MAX_SLIPPAGE_PCT,
                default_value: env_decimal("COPY_MAX_SLIPPAGE_PCT", Decimal::new(1, 2)),
                min_value: Decimal::new(25, 4),
                max_value: Decimal::new(5, 2),
                max_step_pct: Decimal::new(15, 2),
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
        ];

        for seed in seeds {
            sqlx::query(
                r#"
                INSERT INTO dynamic_config (
                    key, current_value, default_value, min_value, max_value,
                    max_step_pct, enabled, last_good_value, updated_by, last_reason
                ) VALUES ($1, $2, $2, $3, $4, $5, TRUE, $2, 'bootstrap', 'initial seed')
                ON CONFLICT (key) DO NOTHING
                "#,
            )
            .bind(seed.key)
            .bind(seed.default_value)
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
    ) {
        let Some(redis) = redis_manager else {
            warn!(key = %update.key, "No Redis connection, skipping dynamic config publish");
            return;
        };

        let payload = match serde_json::to_string(update) {
            Ok(payload) => payload,
            Err(e) => {
                warn!(error = %e, "Failed serializing dynamic config update");
                return;
            }
        };

        let result: redis::RedisResult<()> = redis.publish(channels::CONFIG_UPDATES, payload).await;
        if let Err(e) = result {
            warn!(error = %e, key = %update.key, "Failed publishing dynamic config update");
        }
    }
}

/// Subscribes to dynamic config updates and applies them to local API runtime.
pub fn spawn_dynamic_config_subscriber(
    redis_url: String,
    copy_trader: Option<Arc<RwLock<CopyTrader>>>,
    pool: PgPool,
) {
    tokio::spawn(async move {
        loop {
            if let Err(e) =
                run_dynamic_config_subscriber(redis_url.as_str(), copy_trader.clone(), pool.clone())
                    .await
            {
                error!(error = %e, "Dynamic config subscriber failed, retrying");
                tokio::time::sleep(TokioDuration::from_secs(5)).await;
            }
        }
    });

    info!("Dynamic config subscriber spawned");
}

async fn run_dynamic_config_subscriber(
    redis_url: &str,
    copy_trader: Option<Arc<RwLock<CopyTrader>>>,
    pool: PgPool,
) -> anyhow::Result<()> {
    let allowed_sources = load_allowed_update_sources();
    let bounds = load_dynamic_bounds(&pool).await;
    if let Some(ref trader) = copy_trader {
        if let Err(e) = apply_startup_snapshot_to_copy_trader(&pool, trader, &bounds).await {
            warn!(error = %e, "Failed applying startup dynamic config snapshot to copy trader");
        }
    }

    let client = redis::Client::open(redis_url)?;
    let conn = client.get_async_connection().await?;
    let mut pubsub = conn.into_pubsub();

    pubsub.subscribe(channels::CONFIG_UPDATES).await?;
    info!(
        channel = channels::CONFIG_UPDATES,
        "Subscribed to dynamic config updates"
    );

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
            key: row.key,
            value,
            reason: "startup snapshot".to_string(),
            source: "dynamic_tuner_sync".to_string(),
            timestamp: Utc::now(),
            metrics: serde_json::json!({ "sync": "subscriber_bootstrap" }),
        };
        if apply_to_copy_trader(copy_trader, &update).await {
            applied += 1;
        }
    }

    info!(
        applied,
        "Applied startup dynamic config snapshot to copy trader"
    );
    Ok(())
}

fn load_allowed_update_sources() -> Vec<String> {
    std::env::var("DYNAMIC_CONFIG_ALLOWED_SOURCES")
        .unwrap_or_else(|_| "dynamic_tuner,dynamic_tuner_rollback,dynamic_tuner_sync".to_string())
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
        KEY_ARB_MIN_PROFIT_THRESHOLD,
        KEY_ARB_MONITOR_MAX_MARKETS,
    ] {
        if let Some(bounds) = fallback_bounds_for_key(key) {
            map.insert(key.to_string(), bounds);
        }
    }
    map
}

fn fallback_bounds_for_key(key: &str) -> Option<(Decimal, Decimal)> {
    match key {
        KEY_COPY_MIN_TRADE_VALUE => Some((Decimal::new(2, 0), Decimal::new(50, 0))),
        KEY_COPY_MAX_SLIPPAGE_PCT => Some((Decimal::new(25, 4), Decimal::new(5, 2))),
        KEY_ARB_MIN_PROFIT_THRESHOLD => Some((Decimal::new(2, 3), Decimal::new(5, 2))),
        KEY_ARB_MONITOR_MAX_MARKETS => Some((Decimal::new(25, 0), Decimal::new(1500, 0))),
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

fn env_decimal(name: &str, fallback: Decimal) -> Decimal {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
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
}
