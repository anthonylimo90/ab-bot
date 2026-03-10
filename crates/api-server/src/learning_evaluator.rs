//! Periodic evaluator for shadow learning models.
//!
//! Scores current shadow predictions against canonical arb/quant outcomes and
//! persists replay-style summaries into `learning_offline_evaluations`.

use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{FromRow, PgPool};
use std::time;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct LearningEvaluatorConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub startup_delay_secs: u64,
    pub lookback_hours: i64,
    pub max_models_per_cycle: i64,
}

impl LearningEvaluatorConfig {
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("LEARNING_EVALUATOR_ENABLED")
                .map(|v| v == "true")
                .unwrap_or(true),
            interval_secs: std::env::var("LEARNING_EVALUATOR_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1800),
            startup_delay_secs: std::env::var("LEARNING_EVALUATOR_STARTUP_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(240),
            lookback_hours: std::env::var("LEARNING_EVALUATOR_LOOKBACK_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(168),
            max_models_per_cycle: std::env::var("LEARNING_EVALUATOR_MAX_MODELS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(24),
        }
    }
}

#[derive(Debug, FromRow)]
struct ModelRow {
    id: Uuid,
    strategy_scope: String,
    target: String,
}

#[derive(Debug, FromRow)]
struct BinaryEvalRow {
    sample_size: i64,
    avg_predicted: Option<f64>,
    avg_actual: Option<f64>,
    brier_score: Option<f64>,
    avg_threshold: Option<f64>,
    execute_count: i64,
    execute_actual_rate: Option<f64>,
}

#[derive(Debug, FromRow)]
struct RegressionEvalRow {
    sample_size: i64,
    avg_predicted: Option<f64>,
    avg_actual: Option<f64>,
    mae: Option<f64>,
    rmse: Option<f64>,
    avg_threshold: Option<f64>,
    execute_count: i64,
    execute_actual_average: Option<f64>,
}

struct EvaluationPayload {
    scope: &'static str,
    metrics: serde_json::Value,
    decision_policy: serde_json::Value,
}

pub fn spawn_learning_evaluator(config: LearningEvaluatorConfig, pool: PgPool) {
    if !config.enabled {
        info!("Learning evaluator disabled (LEARNING_EVALUATOR_ENABLED != true)");
        return;
    }

    info!(
        interval_secs = config.interval_secs,
        startup_delay_secs = config.startup_delay_secs,
        lookback_hours = config.lookback_hours,
        max_models_per_cycle = config.max_models_per_cycle,
        "Spawning learning evaluator"
    );

    tokio::spawn(run_loop(config, pool));
}

async fn run_loop(config: LearningEvaluatorConfig, pool: PgPool) {
    tokio::time::sleep(time::Duration::from_secs(config.startup_delay_secs)).await;
    let interval = time::Duration::from_secs(config.interval_secs);

    loop {
        match compute_cycle(&pool, &config).await {
            Ok(inserted) => info!(
                evaluations_inserted = inserted,
                "Learning evaluator cycle completed"
            ),
            Err(error) => warn!(error = %error, "Learning evaluator cycle failed"),
        }

        tokio::time::sleep(interval).await;
    }
}

async fn compute_cycle(
    pool: &PgPool,
    config: &LearningEvaluatorConfig,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();
    let window_start = now - Duration::hours(config.lookback_hours.max(1));
    let models = load_candidate_models(pool, config.max_models_per_cycle).await?;
    let dataset_name = format!("shadow_eval_{}h", config.lookback_hours.max(1));
    let mut inserted = 0usize;

    for model in models {
        let Some(payload) = evaluate_model(pool, &model, window_start, now).await? else {
            continue;
        };

        sqlx::query(
            r#"
            INSERT INTO learning_offline_evaluations (
                model_id, dataset_name, evaluation_scope, window_start, window_end,
                metrics, decision_policy, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())
            "#,
        )
        .bind(model.id)
        .bind(&dataset_name)
        .bind(payload.scope)
        .bind(window_start)
        .bind(now)
        .bind(payload.metrics)
        .bind(payload.decision_policy)
        .execute(pool)
        .await?;
        inserted += 1;
    }

    Ok(inserted)
}

async fn load_candidate_models(pool: &PgPool, limit: i64) -> Result<Vec<ModelRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT id, strategy_scope, target
        FROM learning_model_registry
        WHERE status IN ('shadow', 'canary', 'active')
        ORDER BY created_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .await
}

async fn evaluate_model(
    pool: &PgPool,
    model: &ModelRow,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
) -> Result<Option<EvaluationPayload>, sqlx::Error> {
    match (model.strategy_scope.as_str(), model.target.as_str()) {
        ("arb", "open_success_probability") => evaluate_arb_binary_model(
            pool,
            model,
            from,
            to,
            "CASE WHEN ca.outcome = 'opened' THEN 1.0 ELSE 0.0 END",
        )
        .await,
        ("arb", "one_legged_risk") => evaluate_arb_binary_model(
            pool,
            model,
            from,
            to,
            "CASE WHEN ca.one_legged THEN 1.0 ELSE 0.0 END",
        )
        .await,
        ("arb", "realized_edge_capture") => evaluate_arb_regression_model(
            pool,
            model,
            from,
            to,
            "LEAST(GREATEST(COALESCE(ca.realized_edge_capture_ratio::double precision, 0.0), 0.0), 1.0)",
        )
        .await,
        ("quant", "execute_success_probability") => evaluate_quant_binary_model(
            pool,
            model,
            from,
            to,
            "CASE WHEN cq.execution_status = 'executed' THEN 1.0 ELSE 0.0 END",
            false,
        )
        .await,
        ("quant", "realized_pnl_sign") => evaluate_quant_binary_model(
            pool,
            model,
            from,
            to,
            "CASE WHEN cq.realized_pnl > 0 THEN 1.0 ELSE 0.0 END",
            true,
        )
        .await,
        ("quant", "realized_edge_capture") => evaluate_quant_regression_model(
            pool,
            model,
            from,
            to,
            "LEAST(GREATEST(COALESCE(cq.realized_edge_capture_ratio::double precision, 0.0), 0.0), 1.0)",
            true,
        )
        .await,
        _ => Ok(None),
    }
}

async fn evaluate_arb_binary_model(
    pool: &PgPool,
    model: &ModelRow,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    actual_expr: &str,
) -> Result<Option<EvaluationPayload>, sqlx::Error> {
    let query = format!(
        r#"
        WITH matched AS (
            SELECT
                sp.predicted_score,
                sp.threshold,
                sp.recommended_action,
                {actual_expr} AS actual
            FROM learning_shadow_predictions sp
            INNER JOIN canonical_arb_learning_attempts ca
                ON sp.entity_type = 'arb_attempt'
               AND sp.entity_id = ca.attempt_id
            WHERE sp.model_id = $1
              AND sp.target = $2
              AND sp.predicted_at >= $3
              AND sp.predicted_at <= $4
              AND ca.attempt_time >= $3
              AND ca.attempt_time <= $4
        )
        SELECT
            COUNT(*)::bigint AS sample_size,
            AVG(predicted_score)::double precision AS avg_predicted,
            AVG(actual)::double precision AS avg_actual,
            AVG(POWER(predicted_score - actual, 2))::double precision AS brier_score,
            AVG(threshold)::double precision AS avg_threshold,
            COUNT(*) FILTER (WHERE recommended_action = 'execute')::bigint AS execute_count,
            AVG(actual) FILTER (WHERE recommended_action = 'execute')::double precision AS execute_actual_rate
        FROM matched
        "#,
    );

    let row: BinaryEvalRow = sqlx::query_as(&query)
        .bind(model.id)
        .bind(&model.target)
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;

    if row.sample_size == 0 {
        return Ok(None);
    }

    let baseline = row.avg_actual.unwrap_or(0.0);
    let execute_rate = row.execute_actual_rate.unwrap_or(baseline);
    let threshold = row.avg_threshold.unwrap_or(0.0);

    Ok(Some(EvaluationPayload {
        scope: "arb",
        metrics: json!({
            "sample_size": row.sample_size,
            "avg_predicted": row.avg_predicted,
            "avg_actual": row.avg_actual,
            "brier_score": row.brier_score,
            "execute_actual_rate": row.execute_actual_rate,
            "baseline_actual_rate": row.avg_actual,
            "lift_vs_baseline": execute_rate - baseline,
        }),
        decision_policy: json!({
            "mode": "shadow_replay",
            "threshold": threshold,
            "positive_action": "execute",
            "execute_count": row.execute_count,
            "skip_count": row.sample_size - row.execute_count,
        }),
    }))
}

async fn evaluate_arb_regression_model(
    pool: &PgPool,
    model: &ModelRow,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    actual_expr: &str,
) -> Result<Option<EvaluationPayload>, sqlx::Error> {
    let query = format!(
        r#"
        WITH matched AS (
            SELECT
                sp.predicted_score,
                sp.threshold,
                sp.recommended_action,
                {actual_expr} AS actual
            FROM learning_shadow_predictions sp
            INNER JOIN canonical_arb_learning_attempts ca
                ON sp.entity_type = 'arb_attempt'
               AND sp.entity_id = ca.attempt_id
            WHERE sp.model_id = $1
              AND sp.target = $2
              AND sp.predicted_at >= $3
              AND sp.predicted_at <= $4
              AND ca.attempt_time >= $3
              AND ca.attempt_time <= $4
              AND ca.realized_edge_capture_ratio IS NOT NULL
        )
        SELECT
            COUNT(*)::bigint AS sample_size,
            AVG(predicted_score)::double precision AS avg_predicted,
            AVG(actual)::double precision AS avg_actual,
            AVG(ABS(predicted_score - actual))::double precision AS mae,
            SQRT(AVG(POWER(predicted_score - actual, 2)))::double precision AS rmse,
            AVG(threshold)::double precision AS avg_threshold,
            COUNT(*) FILTER (WHERE recommended_action = 'execute')::bigint AS execute_count,
            AVG(actual) FILTER (WHERE recommended_action = 'execute')::double precision AS execute_actual_average
        FROM matched
        "#,
    );

    let row: RegressionEvalRow = sqlx::query_as(&query)
        .bind(model.id)
        .bind(&model.target)
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;

    if row.sample_size == 0 {
        return Ok(None);
    }

    let baseline = row.avg_actual.unwrap_or(0.0);
    let execute_avg = row.execute_actual_average.unwrap_or(baseline);

    Ok(Some(EvaluationPayload {
        scope: "arb",
        metrics: json!({
            "sample_size": row.sample_size,
            "avg_predicted": row.avg_predicted,
            "avg_actual": row.avg_actual,
            "mae": row.mae,
            "rmse": row.rmse,
            "execute_actual_average": row.execute_actual_average,
            "baseline_actual_average": row.avg_actual,
            "lift_vs_baseline": execute_avg - baseline,
        }),
        decision_policy: json!({
            "mode": "shadow_replay",
            "threshold": row.avg_threshold,
            "positive_action": "execute",
            "actual_transform": "clamp_0_1",
            "execute_count": row.execute_count,
            "skip_count": row.sample_size - row.execute_count,
        }),
    }))
}

async fn evaluate_quant_binary_model(
    pool: &PgPool,
    model: &ModelRow,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    actual_expr: &str,
    require_realized: bool,
) -> Result<Option<EvaluationPayload>, sqlx::Error> {
    let realized_filter = if require_realized {
        "AND cq.realized_pnl IS NOT NULL"
    } else {
        ""
    };
    let query = format!(
        r#"
        WITH matched AS (
            SELECT
                sp.predicted_score,
                sp.threshold,
                sp.recommended_action,
                {actual_expr} AS actual
            FROM learning_shadow_predictions sp
            INNER JOIN canonical_quant_learning_decisions cq
                ON sp.entity_type = 'quant_decision'
               AND sp.entity_id = cq.decision_id::text
            WHERE sp.model_id = $1
              AND sp.target = $2
              AND sp.predicted_at >= $3
              AND sp.predicted_at <= $4
              AND cq.decision_time >= $3
              AND cq.decision_time <= $4
              {realized_filter}
        )
        SELECT
            COUNT(*)::bigint AS sample_size,
            AVG(predicted_score)::double precision AS avg_predicted,
            AVG(actual)::double precision AS avg_actual,
            AVG(POWER(predicted_score - actual, 2))::double precision AS brier_score,
            AVG(threshold)::double precision AS avg_threshold,
            COUNT(*) FILTER (WHERE recommended_action = 'execute')::bigint AS execute_count,
            AVG(actual) FILTER (WHERE recommended_action = 'execute')::double precision AS execute_actual_rate
        FROM matched
        "#,
    );

    let row: BinaryEvalRow = sqlx::query_as(&query)
        .bind(model.id)
        .bind(&model.target)
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;

    if row.sample_size == 0 {
        return Ok(None);
    }

    let baseline = row.avg_actual.unwrap_or(0.0);
    let execute_rate = row.execute_actual_rate.unwrap_or(baseline);

    Ok(Some(EvaluationPayload {
        scope: "quant",
        metrics: json!({
            "sample_size": row.sample_size,
            "avg_predicted": row.avg_predicted,
            "avg_actual": row.avg_actual,
            "brier_score": row.brier_score,
            "execute_actual_rate": row.execute_actual_rate,
            "baseline_actual_rate": row.avg_actual,
            "lift_vs_baseline": execute_rate - baseline,
        }),
        decision_policy: json!({
            "mode": "shadow_replay",
            "threshold": row.avg_threshold,
            "positive_action": "execute",
            "execute_count": row.execute_count,
            "skip_count": row.sample_size - row.execute_count,
        }),
    }))
}

async fn evaluate_quant_regression_model(
    pool: &PgPool,
    model: &ModelRow,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    actual_expr: &str,
    require_realized: bool,
) -> Result<Option<EvaluationPayload>, sqlx::Error> {
    let realized_filter = if require_realized {
        "AND cq.realized_edge_capture_ratio IS NOT NULL"
    } else {
        ""
    };
    let query = format!(
        r#"
        WITH matched AS (
            SELECT
                sp.predicted_score,
                sp.threshold,
                sp.recommended_action,
                {actual_expr} AS actual
            FROM learning_shadow_predictions sp
            INNER JOIN canonical_quant_learning_decisions cq
                ON sp.entity_type = 'quant_decision'
               AND sp.entity_id = cq.decision_id::text
            WHERE sp.model_id = $1
              AND sp.target = $2
              AND sp.predicted_at >= $3
              AND sp.predicted_at <= $4
              AND cq.decision_time >= $3
              AND cq.decision_time <= $4
              {realized_filter}
        )
        SELECT
            COUNT(*)::bigint AS sample_size,
            AVG(predicted_score)::double precision AS avg_predicted,
            AVG(actual)::double precision AS avg_actual,
            AVG(ABS(predicted_score - actual))::double precision AS mae,
            SQRT(AVG(POWER(predicted_score - actual, 2)))::double precision AS rmse,
            AVG(threshold)::double precision AS avg_threshold,
            COUNT(*) FILTER (WHERE recommended_action = 'execute')::bigint AS execute_count,
            AVG(actual) FILTER (WHERE recommended_action = 'execute')::double precision AS execute_actual_average
        FROM matched
        "#,
    );

    let row: RegressionEvalRow = sqlx::query_as(&query)
        .bind(model.id)
        .bind(&model.target)
        .bind(from)
        .bind(to)
        .fetch_one(pool)
        .await?;

    if row.sample_size == 0 {
        return Ok(None);
    }

    let baseline = row.avg_actual.unwrap_or(0.0);
    let execute_avg = row.execute_actual_average.unwrap_or(baseline);

    Ok(Some(EvaluationPayload {
        scope: "quant",
        metrics: json!({
            "sample_size": row.sample_size,
            "avg_predicted": row.avg_predicted,
            "avg_actual": row.avg_actual,
            "mae": row.mae,
            "rmse": row.rmse,
            "execute_actual_average": row.execute_actual_average,
            "baseline_actual_average": row.avg_actual,
            "lift_vs_baseline": execute_avg - baseline,
        }),
        decision_policy: json!({
            "mode": "shadow_replay",
            "threshold": row.avg_threshold,
            "positive_action": "execute",
            "actual_transform": "clamp_0_1",
            "execute_count": row.execute_count,
            "skip_count": row.sample_size - row.execute_count,
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env_has_sane_defaults() {
        let config = LearningEvaluatorConfig::from_env();
        assert!(config.interval_secs > 0);
        assert!(config.startup_delay_secs > 0);
        assert!(config.lookback_hours > 0);
        assert!(config.max_models_per_cycle > 0);
    }
}
