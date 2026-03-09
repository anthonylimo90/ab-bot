use crate::learning::{ArbShadowPredictionInput, QuantShadowPredictionInput};
use crate::learning_models::LearningModelRuntime;
use chrono::{Duration, Utc};
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::Decimal;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone)]
pub struct LearningRolloutController {
    pool: PgPool,
    refresh_secs: u64,
    schema_missing: Arc<AtomicBool>,
    cache: Arc<RwLock<Option<CachedRollouts>>>,
    runtime: LearningModelRuntime,
}

#[derive(Clone)]
struct CachedRollouts {
    fetched_at: Instant,
    rollouts: Vec<ActiveRollout>,
}

#[derive(Debug, Clone)]
struct ActiveRollout {
    model_id: Uuid,
    model_key: String,
    strategy_scope: String,
    target: String,
    rollout_mode: String,
    authority_level: String,
    bounds: serde_json::Value,
}

#[derive(Debug, FromRow)]
struct ActiveRolloutRow {
    model_id: Uuid,
    model_key: String,
    strategy_scope: String,
    target: String,
    rollout_mode: String,
    authority_level: String,
    bounds: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct RolloutDecision {
    pub skip_reason: Option<String>,
    pub size_multiplier: Decimal,
}

impl Default for RolloutDecision {
    fn default() -> Self {
        Self {
            skip_reason: None,
            size_multiplier: Decimal::ONE,
        }
    }
}

impl LearningRolloutController {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            refresh_secs: std::env::var("LEARNING_ROLLOUT_REFRESH_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            schema_missing: Arc::new(AtomicBool::new(false)),
            cache: Arc::new(RwLock::new(None)),
            runtime: LearningModelRuntime::new(pool.clone()),
        }
    }

    pub async fn evaluate_arb(
        &self,
        input: &ArbShadowPredictionInput,
        execution_mode: &str,
    ) -> RolloutDecision {
        self.evaluate("arb", execution_mode, |model| model.score_arb(input))
            .await
    }

    pub async fn evaluate_quant(
        &self,
        input: &QuantShadowPredictionInput,
        execution_mode: &str,
    ) -> RolloutDecision {
        self.evaluate("quant", execution_mode, |model| model.score_quant(input))
            .await
    }

    async fn evaluate<F>(
        &self,
        strategy_scope: &str,
        execution_mode: &str,
        score_model: F,
    ) -> RolloutDecision
    where
        F: Fn(&crate::learning_models::LoadedModel) -> Option<crate::learning_models::ModelScore>,
    {
        if execution_mode != "live" || self.schema_missing.load(Ordering::Relaxed) {
            return RolloutDecision::default();
        }

        let rollouts = match self.active_rollouts_for_scope(strategy_scope).await {
            Ok(rollouts) => rollouts,
            Err(error) => {
                self.handle_error(&error);
                return RolloutDecision::default();
            }
        };
        let models = match self.runtime.models_for_scope(strategy_scope).await {
            Ok(models) => models,
            Err(error) => {
                self.handle_error(&error);
                return RolloutDecision::default();
            }
        };
        let model_map: HashMap<Uuid, crate::learning_models::LoadedModel> =
            models.into_iter().map(|model| (model.id, model)).collect();

        let mut decision = RolloutDecision::default();

        for rollout in rollouts {
            let Some(model) = model_map.get(&rollout.model_id) else {
                continue;
            };
            let Some(score) = score_model(model) else {
                continue;
            };

            if can_reject(&rollout.authority_level) && score.recommended_action == "skip" {
                decision.skip_reason = Some(format!(
                    "learning_rollout:{}:{}:{}",
                    rollout.rollout_mode, rollout.model_key, rollout.target
                ));
                return decision;
            }

            if can_resize(&rollout.authority_level)
                && matches!(
                    score.recommended_action.as_str(),
                    "downsize" | "deprioritize" | "skip"
                )
            {
                let reduction = rollout
                    .bounds
                    .get("max_size_reduction_pct")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.20)
                    .clamp(0.0, 0.95);
                let multiplier = Decimal::from_f64(1.0 - reduction).unwrap_or(Decimal::ONE);
                if multiplier < decision.size_multiplier {
                    decision.size_multiplier = multiplier;
                }
            }
        }

        decision
    }

    async fn active_rollouts_for_scope(
        &self,
        strategy_scope: &str,
    ) -> Result<Vec<ActiveRollout>, sqlx::Error> {
        let now = Instant::now();
        if let Some(cached) = self.cache.read().await.as_ref() {
            if now.duration_since(cached.fetched_at).as_secs() < self.refresh_secs {
                return Ok(cached
                    .rollouts
                    .iter()
                    .filter(|rollout| rollout.strategy_scope == strategy_scope)
                    .cloned()
                    .collect());
            }
        }

        let rows: Vec<ActiveRolloutRow> = sqlx::query_as(
            r#"
            SELECT
                mr.id AS model_id,
                mr.model_key,
                ro.strategy_scope,
                mr.target,
                ro.rollout_mode,
                ro.authority_level,
                ro.bounds
            FROM learning_model_rollouts ro
            INNER JOIN learning_model_registry mr
                ON mr.id = ro.model_id
            WHERE ro.status = 'active'
              AND ro.rollout_mode <> 'shadow'
            ORDER BY ro.started_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let rollouts: Vec<ActiveRollout> = rows
            .into_iter()
            .map(|row| ActiveRollout {
                model_id: row.model_id,
                model_key: row.model_key,
                strategy_scope: row.strategy_scope,
                target: row.target,
                rollout_mode: row.rollout_mode,
                authority_level: row.authority_level,
                bounds: row.bounds,
            })
            .collect();

        *self.cache.write().await = Some(CachedRollouts {
            fetched_at: now,
            rollouts: rollouts.clone(),
        });

        Ok(rollouts
            .into_iter()
            .filter(|rollout| rollout.strategy_scope == strategy_scope)
            .collect())
    }

    fn handle_error(&self, error: &sqlx::Error) {
        if is_schema_missing(error) {
            self.schema_missing.store(true, Ordering::Relaxed);
        }
        warn!(error = %error, "Learning rollout lookup failed");
    }
}

pub fn spawn_learning_rollout_observer(config: LearningRolloutObserverConfig, pool: PgPool) {
    if !config.enabled {
        tracing::info!(
            "Learning rollout observer disabled (LEARNING_ROLLOUT_OBSERVER_ENABLED != true)"
        );
        return;
    }

    tracing::info!(
        interval_secs = config.interval_secs,
        startup_delay_secs = config.startup_delay_secs,
        "Spawning learning rollout observer"
    );

    tokio::spawn(run_observer_loop(config, pool));
}

#[derive(Debug, Clone)]
pub struct LearningRolloutObserverConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub startup_delay_secs: u64,
}

impl LearningRolloutObserverConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("LEARNING_ROLLOUT_OBSERVER_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("LEARNING_ROLLOUT_OBSERVER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(900),
            startup_delay_secs: std::env::var("LEARNING_ROLLOUT_OBSERVER_STARTUP_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
        }
    }
}

#[derive(Debug, FromRow)]
struct RolloutObservationCandidateRow {
    id: Uuid,
    strategy_scope: String,
    authority_level: String,
    baseline_window_hours: i32,
    guardrails: serde_json::Value,
}

#[derive(Debug, FromRow)]
struct ArbObservationMetricsRow {
    executed_count: i64,
    baseline_count: i64,
    failure_rate: Option<f64>,
    one_legged_rate: Option<f64>,
    drawdown_pct: Option<f64>,
    latency_p90_ms: Option<f64>,
    edge_capture_ratio: Option<f64>,
    realized_pnl_total: Option<f64>,
    baseline_realized_pnl_total: Option<f64>,
    gross_exposure: Option<f64>,
    baseline_gross_exposure: Option<f64>,
}

#[derive(Debug, FromRow)]
struct QuantObservationMetricsRow {
    executed_count: i64,
    baseline_count: i64,
    failure_rate: Option<f64>,
    drawdown_pct: Option<f64>,
    edge_capture_ratio: Option<f64>,
    realized_pnl_total: Option<f64>,
    baseline_realized_pnl_total: Option<f64>,
    gross_exposure: Option<f64>,
    baseline_gross_exposure: Option<f64>,
}

async fn run_observer_loop(config: LearningRolloutObserverConfig, pool: PgPool) {
    tokio::time::sleep(std::time::Duration::from_secs(config.startup_delay_secs)).await;
    let interval = std::time::Duration::from_secs(config.interval_secs);

    loop {
        match observe_rollouts(&pool).await {
            Ok(count) => tracing::info!(
                observations_written = count,
                "Learning rollout observation cycle completed"
            ),
            Err(error) => warn!(error = %error, "Learning rollout observation cycle failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

async fn observe_rollouts(
    pool: &PgPool,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let rows: Vec<RolloutObservationCandidateRow> = sqlx::query_as(
        r#"
        SELECT
            ro.id,
            ro.strategy_scope,
            ro.authority_level,
            ro.baseline_window_hours,
            ro.guardrails
        FROM learning_model_rollouts ro
        WHERE ro.status = 'active'
          AND ro.rollout_mode <> 'shadow'
        ORDER BY ro.started_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut written = 0usize;

    for row in rows {
        let window_start = Utc::now() - Duration::hours(row.baseline_window_hours.max(1) as i64);
        let observation = if row.strategy_scope == "arb" {
            observe_arb_rollout(pool, row.id, window_start, row.authority_level.as_str()).await?
        } else {
            observe_quant_rollout(pool, row.id, window_start, row.authority_level.as_str()).await?
        };

        let Some((
            failure_rate,
            one_legged_rate,
            drawdown_pct,
            latency_p90_ms,
            edge_capture_ratio,
            notes,
        )) = observation
        else {
            continue;
        };

        let (guardrail_state, rollback_reason) = evaluate_guardrails(
            &row.guardrails,
            failure_rate,
            one_legged_rate,
            drawdown_pct,
            latency_p90_ms,
            edge_capture_ratio,
        );

        sqlx::query(
            r#"
            INSERT INTO learning_rollout_observations (
                rollout_id, observed_at, failure_rate, one_legged_rate, drawdown_pct,
                latency_p90_ms, edge_capture_ratio, guardrail_state, notes
            )
            VALUES ($1, NOW(), $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(row.id)
        .bind(failure_rate)
        .bind(one_legged_rate)
        .bind(drawdown_pct)
        .bind(latency_p90_ms)
        .bind(edge_capture_ratio.map(decimal_from_f64))
        .bind(guardrail_state)
        .bind(notes)
        .execute(pool)
        .await?;
        written += 1;

        if let Some(reason) = rollback_reason {
            sqlx::query(
                r#"
                UPDATE learning_model_rollouts
                SET status = 'rolled_back',
                    ended_at = COALESCE(ended_at, NOW()),
                    rollback_reason = $2
                WHERE id = $1
                  AND status = 'active'
                "#,
            )
            .bind(row.id)
            .bind(reason)
            .execute(pool)
            .await?;
        }
    }

    Ok(written)
}

async fn observe_arb_rollout(
    pool: &PgPool,
    rollout_id: Uuid,
    from: chrono::DateTime<Utc>,
    authority_level: &str,
) -> Result<
    Option<(
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        serde_json::Value,
    )>,
    sqlx::Error,
> {
    let execute_only = can_reject(authority_level);
    let row: ArbObservationMetricsRow = sqlx::query_as(
        r#"
        WITH matched AS (
            SELECT
                ca.attempt_time AS occurred_at,
                ca.attempt_id AS entity_id,
                ca.outcome,
                ca.one_legged,
                ca.total_time_ms,
                ca.realized_edge_capture_ratio::double precision AS edge_capture_ratio,
                COALESCE(ca.realized_pnl, 0)::double precision AS realized_pnl,
                ABS(COALESCE(ca.requested_size_usd, 0))::double precision AS requested_size_usd,
                sp.recommended_action
            FROM learning_model_rollouts ro
            INNER JOIN learning_shadow_predictions sp
                ON sp.model_id = ro.model_id
            INNER JOIN canonical_arb_learning_attempts ca
                ON sp.entity_type = 'arb_attempt'
               AND sp.entity_id = ca.attempt_id
            WHERE ro.id = $1
              AND sp.predicted_at >= $2
              AND ca.attempt_time >= $2
        ),
        executed AS (
            SELECT *
            FROM matched
            WHERE ($3::boolean = FALSE OR recommended_action <> 'skip')
        ),
        ordered AS (
            SELECT
                *,
                SUM(realized_pnl) OVER (
                    ORDER BY occurred_at, entity_id
                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                ) AS cumulative_pnl
            FROM executed
        ),
        drawdowns AS (
            SELECT
                *,
                MAX(cumulative_pnl) OVER (
                    ORDER BY occurred_at, entity_id
                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                ) - cumulative_pnl AS drawdown_abs
            FROM ordered
        )
        SELECT
            COUNT(*)::bigint AS executed_count,
            (SELECT COUNT(*)::bigint FROM matched) AS baseline_count,
            AVG(CASE WHEN outcome = 'failed' THEN 1.0 ELSE 0.0 END) AS failure_rate,
            AVG(CASE WHEN one_legged THEN 1.0 ELSE 0.0 END) AS one_legged_rate,
            CASE
                WHEN COALESCE(SUM(requested_size_usd), 0) = 0 THEN NULL
                ELSE COALESCE((SELECT MAX(drawdown_abs) FROM drawdowns), 0.0) / SUM(requested_size_usd)
            END AS drawdown_pct,
            percentile_cont(0.9) WITHIN GROUP (ORDER BY total_time_ms)
                FILTER (WHERE total_time_ms IS NOT NULL) AS latency_p90_ms,
            AVG(edge_capture_ratio) AS edge_capture_ratio,
            SUM(realized_pnl) AS realized_pnl_total,
            (SELECT SUM(realized_pnl) FROM matched) AS baseline_realized_pnl_total,
            SUM(requested_size_usd) AS gross_exposure,
            (SELECT SUM(requested_size_usd) FROM matched) AS baseline_gross_exposure
        FROM executed
        "#,
    )
    .bind(rollout_id)
    .bind(from)
    .bind(execute_only)
    .fetch_one(pool)
    .await?;

    if row.executed_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        row.failure_rate,
        row.one_legged_rate,
        row.drawdown_pct,
        row.latency_p90_ms,
        row.edge_capture_ratio,
        serde_json::json!({
            "executed_count": row.executed_count,
            "baseline_count": row.baseline_count,
            "execute_only_subset": execute_only,
            "realized_pnl_total": row.realized_pnl_total,
            "baseline_realized_pnl_total": row.baseline_realized_pnl_total,
            "gross_exposure": row.gross_exposure,
            "baseline_gross_exposure": row.baseline_gross_exposure,
            "pnl_per_dollar": ratio(row.realized_pnl_total, row.gross_exposure),
            "baseline_pnl_per_dollar": ratio(
                row.baseline_realized_pnl_total,
                row.baseline_gross_exposure,
            ),
            "selection_lift_pnl": difference(
                row.realized_pnl_total,
                row.baseline_realized_pnl_total,
            ),
            "attribution_mode": "ordered_realized_pnl_drawdown",
        }),
    )))
}

async fn observe_quant_rollout(
    pool: &PgPool,
    rollout_id: Uuid,
    from: chrono::DateTime<Utc>,
    authority_level: &str,
) -> Result<
    Option<(
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        serde_json::Value,
    )>,
    sqlx::Error,
> {
    let execute_only = can_reject(authority_level);
    let row: QuantObservationMetricsRow = sqlx::query_as(
        r#"
        WITH matched AS (
            SELECT
                cq.decision_time AS occurred_at,
                cq.decision_id::text AS entity_id,
                cq.decision_outcome,
                cq.realized_edge_capture_ratio::double precision AS edge_capture_ratio,
                COALESCE(cq.realized_pnl, 0)::double precision AS realized_pnl,
                ABS(COALESCE(cq.requested_size_usd, 0))::double precision AS requested_size_usd,
                sp.recommended_action
            FROM learning_model_rollouts ro
            INNER JOIN learning_shadow_predictions sp
                ON sp.model_id = ro.model_id
            INNER JOIN canonical_quant_learning_decisions cq
                ON sp.entity_type = 'quant_decision'
               AND sp.entity_id = cq.decision_id::text
            WHERE ro.id = $1
              AND sp.predicted_at >= $2
              AND cq.decision_time >= $2
        ),
        executed AS (
            SELECT *
            FROM matched
            WHERE ($3::boolean = FALSE OR recommended_action <> 'skip')
        ),
        ordered AS (
            SELECT
                *,
                SUM(realized_pnl) OVER (
                    ORDER BY occurred_at, entity_id
                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                ) AS cumulative_pnl
            FROM executed
        ),
        drawdowns AS (
            SELECT
                *,
                MAX(cumulative_pnl) OVER (
                    ORDER BY occurred_at, entity_id
                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                ) - cumulative_pnl AS drawdown_abs
            FROM ordered
        )
        SELECT
            COUNT(*)::bigint AS executed_count,
            (SELECT COUNT(*)::bigint FROM matched) AS baseline_count,
            AVG(CASE WHEN decision_outcome = 'failed' THEN 1.0 ELSE 0.0 END) AS failure_rate,
            CASE
                WHEN COALESCE(SUM(requested_size_usd), 0) = 0 THEN NULL
                ELSE COALESCE((SELECT MAX(drawdown_abs) FROM drawdowns), 0.0) / SUM(requested_size_usd)
            END AS drawdown_pct,
            AVG(edge_capture_ratio) AS edge_capture_ratio,
            SUM(realized_pnl) AS realized_pnl_total,
            (SELECT SUM(realized_pnl) FROM matched) AS baseline_realized_pnl_total,
            SUM(requested_size_usd) AS gross_exposure,
            (SELECT SUM(requested_size_usd) FROM matched) AS baseline_gross_exposure
        FROM executed
        "#,
    )
    .bind(rollout_id)
    .bind(from)
    .bind(execute_only)
    .fetch_one(pool)
    .await?;

    if row.executed_count == 0 {
        return Ok(None);
    }

    Ok(Some((
        row.failure_rate,
        None,
        row.drawdown_pct,
        None,
        row.edge_capture_ratio,
        serde_json::json!({
            "executed_count": row.executed_count,
            "baseline_count": row.baseline_count,
            "execute_only_subset": execute_only,
            "realized_pnl_total": row.realized_pnl_total,
            "baseline_realized_pnl_total": row.baseline_realized_pnl_total,
            "gross_exposure": row.gross_exposure,
            "baseline_gross_exposure": row.baseline_gross_exposure,
            "pnl_per_dollar": ratio(row.realized_pnl_total, row.gross_exposure),
            "baseline_pnl_per_dollar": ratio(
                row.baseline_realized_pnl_total,
                row.baseline_gross_exposure,
            ),
            "selection_lift_pnl": difference(
                row.realized_pnl_total,
                row.baseline_realized_pnl_total,
            ),
            "attribution_mode": "ordered_realized_pnl_drawdown",
        }),
    )))
}

fn evaluate_guardrails(
    guardrails: &serde_json::Value,
    failure_rate: Option<f64>,
    one_legged_rate: Option<f64>,
    drawdown_pct: Option<f64>,
    latency_p90_ms: Option<f64>,
    edge_capture_ratio: Option<f64>,
) -> (&'static str, Option<String>) {
    let mut warn = false;
    let mut violations = Vec::new();

    check_max_guardrail(
        guardrails,
        "max_failure_rate",
        failure_rate,
        &mut warn,
        &mut violations,
    );
    check_max_guardrail(
        guardrails,
        "max_one_legged_rate",
        one_legged_rate,
        &mut warn,
        &mut violations,
    );
    check_max_guardrail(
        guardrails,
        "max_drawdown_pct",
        drawdown_pct,
        &mut warn,
        &mut violations,
    );
    check_max_guardrail(
        guardrails,
        "max_latency_p90_ms",
        latency_p90_ms,
        &mut warn,
        &mut violations,
    );

    if let Some(min_value) = guardrails
        .get("min_edge_capture_ratio")
        .and_then(serde_json::Value::as_f64)
    {
        if let Some(actual) = edge_capture_ratio {
            if actual < min_value {
                violations.push(format!("edge_capture_ratio {actual:.4} < {min_value:.4}"));
            } else if actual < min_value * 1.2 {
                warn = true;
            }
        }
    }

    if !violations.is_empty() {
        return ("rollback", Some(violations.join("; ")));
    }
    if warn {
        return ("warn", None);
    }
    ("ok", None)
}

fn check_max_guardrail(
    guardrails: &serde_json::Value,
    key: &str,
    actual: Option<f64>,
    warn: &mut bool,
    violations: &mut Vec<String>,
) {
    if let Some(max_value) = guardrails.get(key).and_then(serde_json::Value::as_f64) {
        if let Some(actual_value) = actual {
            if actual_value > max_value {
                violations.push(format!("{key} {actual_value:.4} > {max_value:.4}"));
            } else if actual_value > max_value * 0.8 {
                *warn = true;
            }
        }
    }
}

fn can_reject(authority_level: &str) -> bool {
    matches!(authority_level, "tail_reject" | "full")
}

fn can_resize(authority_level: &str) -> bool {
    matches!(authority_level, "size_adjust" | "full")
}

fn ratio(numerator: Option<f64>, denominator: Option<f64>) -> Option<f64> {
    match (numerator, denominator) {
        (Some(numerator), Some(denominator)) if denominator.abs() > f64::EPSILON => {
            Some(numerator / denominator)
        }
        _ => None,
    }
}

fn difference(lhs: Option<f64>, rhs: Option<f64>) -> Option<f64> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => Some(lhs - rhs),
        _ => None,
    }
}

fn decimal_from_f64(value: f64) -> Decimal {
    Decimal::from_f64(value).unwrap_or(Decimal::ZERO)
}

fn is_schema_missing(error: &sqlx::Error) -> bool {
    let message = error.to_string();
    message.contains("learning_model_rollouts")
        || message.contains("learning_model_registry")
        || message.contains("learning_rollout_observations")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_logic_matches_target_direction() {
        assert!(should_reject("one_legged_risk", 0.7, 0.4));
        assert!(should_reject("open_success_probability", 0.4, 0.6));
        assert!(!should_reject("open_success_probability", 0.9, 0.6));
    }

    #[test]
    fn guardrails_trigger_warn_and_rollback() {
        let (state, reason) = evaluate_guardrails(
            &serde_json::json!({ "max_failure_rate": 0.1 }),
            Some(0.12),
            None,
            None,
            None,
            None,
        );
        assert_eq!(state, "rollback");
        assert!(reason.is_some());
    }
}
