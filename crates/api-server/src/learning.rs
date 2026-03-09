use crate::learning_models::LearningModelRuntime;
use chrono::{DateTime, Utc};
use polymarket_core::types::signal::{QuantSignalKind, SignalDirection};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone)]
pub struct ShadowPredictionRecorder {
    pool: PgPool,
    enabled: bool,
    schema_missing: Arc<AtomicBool>,
    model_cache: Arc<RwLock<HashMap<&'static str, Uuid>>>,
    runtime: LearningModelRuntime,
}

#[derive(Clone, Copy)]
struct ModelSpec {
    key: &'static str,
    strategy_scope: &'static str,
    target: &'static str,
    feature_view: &'static str,
}

struct PredictionRow {
    model_id: Uuid,
    entity_type: &'static str,
    entity_id: String,
    strategy_scope: String,
    target: String,
    recommended_action: String,
    predicted_score: f64,
    threshold: f64,
    context: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ArbShadowPredictionInput {
    pub attempt_id: Uuid,
    pub market_id: String,
    pub execution_mode: String,
    pub signal_age_ms: i64,
    pub yes_ask: Decimal,
    pub no_ask: Decimal,
    pub total_cost: Decimal,
    pub gross_profit: Decimal,
    pub net_profit: Decimal,
    pub live_ready: bool,
}

#[derive(Debug, Clone)]
pub struct QuantShadowPredictionInput {
    pub decision_id: Uuid,
    pub kind: QuantSignalKind,
    pub condition_id: String,
    pub direction: SignalDirection,
    pub confidence: f64,
    pub suggested_size_usd: Decimal,
    pub generated_at: DateTime<Utc>,
    pub expiry: DateTime<Utc>,
    pub execution_mode: String,
    pub metadata: serde_json::Value,
    pub min_confidence: f64,
    pub max_signal_age_secs: i64,
}

const MODEL_TYPE: &str = "heuristic_shadow_baseline";
const MODEL_VERSION: &str = "v1";

const ARB_OPEN_SUCCESS_MODEL: ModelSpec = ModelSpec {
    key: "arb_open_success_baseline_v1",
    strategy_scope: "arb",
    target: "open_success_probability",
    feature_view: "canonical_arb_learning_attempts",
};

const ARB_ONE_LEGGED_MODEL: ModelSpec = ModelSpec {
    key: "arb_one_legged_risk_baseline_v1",
    strategy_scope: "arb",
    target: "one_legged_risk",
    feature_view: "canonical_arb_learning_attempts",
};

const ARB_EDGE_CAPTURE_MODEL: ModelSpec = ModelSpec {
    key: "arb_edge_capture_baseline_v1",
    strategy_scope: "arb",
    target: "realized_edge_capture",
    feature_view: "canonical_arb_learning_attempts",
};

const QUANT_EXECUTE_SUCCESS_MODEL: ModelSpec = ModelSpec {
    key: "quant_execute_success_baseline_v1",
    strategy_scope: "quant",
    target: "execute_success_probability",
    feature_view: "canonical_quant_learning_decisions",
};

const QUANT_PNL_SIGN_MODEL: ModelSpec = ModelSpec {
    key: "quant_realized_pnl_sign_baseline_v1",
    strategy_scope: "quant",
    target: "realized_pnl_sign",
    feature_view: "canonical_quant_learning_decisions",
};

const QUANT_EDGE_CAPTURE_MODEL: ModelSpec = ModelSpec {
    key: "quant_edge_capture_baseline_v1",
    strategy_scope: "quant",
    target: "realized_edge_capture",
    feature_view: "canonical_quant_learning_decisions",
};

impl ShadowPredictionRecorder {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            enabled: std::env::var("LEARNING_SHADOW_ENABLED")
                .map(|value| value != "false")
                .unwrap_or(true),
            schema_missing: Arc::new(AtomicBool::new(false)),
            model_cache: Arc::new(RwLock::new(HashMap::new())),
            runtime: LearningModelRuntime::new(pool.clone()),
        }
    }

    pub async fn record_arb_attempt_baselines(&self, input: ArbShadowPredictionInput) {
        if !self.should_record() {
            return;
        }

        if let Err(error) = self.ensure_builtin_models("arb").await {
            self.handle_error("Failed to register built-in learning models", &error);
            return;
        }

        let entity_id = input.attempt_id.to_string();
        let context = serde_json::json!({
            "market_id": input.market_id,
            "execution_mode": input.execution_mode,
            "signal_age_ms": input.signal_age_ms,
            "yes_ask": decimal_to_f64(input.yes_ask),
            "no_ask": decimal_to_f64(input.no_ask),
            "total_cost": decimal_to_f64(input.total_cost),
            "gross_profit": decimal_to_f64(input.gross_profit),
            "net_profit": decimal_to_f64(input.net_profit),
            "live_ready": input.live_ready,
        });

        let models = match self.runtime.models_for_scope("arb").await {
            Ok(models) => models,
            Err(error) => {
                self.handle_error("Failed to load learning models", &error);
                return;
            }
        };
        let rows: Vec<PredictionRow> = models
            .into_iter()
            .filter_map(|model| {
                let score = model.score_arb(&input)?;
                Some(PredictionRow {
                    model_id: model.id,
                    entity_type: "arb_attempt",
                    entity_id: entity_id.clone(),
                    strategy_scope: model.strategy_scope,
                    target: model.target,
                    recommended_action: score.recommended_action,
                    predicted_score: score.predicted_score,
                    threshold: score.threshold,
                    context: context.clone(),
                })
            })
            .collect();

        self.record_rows(rows).await;
    }

    pub async fn record_quant_decision_baselines(&self, input: QuantShadowPredictionInput) {
        if !self.should_record() {
            return;
        }

        if let Err(error) = self.ensure_builtin_models("quant").await {
            self.handle_error("Failed to register built-in learning models", &error);
            return;
        }

        let entity_id = input.decision_id.to_string();
        let age_secs = Utc::now()
            .signed_duration_since(input.generated_at)
            .num_seconds()
            .max(0);
        let time_to_expiry_secs = input
            .expiry
            .signed_duration_since(Utc::now())
            .num_seconds()
            .max(0);
        let expected_edge_bps = extract_expected_edge_bps(&input.metadata);
        let context = serde_json::json!({
            "market_id": input.condition_id,
            "kind": input.kind.as_str(),
            "direction": input.direction.as_str(),
            "execution_mode": input.execution_mode,
            "confidence": input.confidence,
            "suggested_size_usd": decimal_to_f64(input.suggested_size_usd),
            "age_secs": age_secs,
            "time_to_expiry_secs": time_to_expiry_secs,
            "expected_edge_bps": expected_edge_bps,
        });

        let models = match self.runtime.models_for_scope("quant").await {
            Ok(models) => models,
            Err(error) => {
                self.handle_error("Failed to load learning models", &error);
                return;
            }
        };
        let rows: Vec<PredictionRow> = models
            .into_iter()
            .filter_map(|model| {
                let score = model.score_quant(&input)?;
                Some(PredictionRow {
                    model_id: model.id,
                    entity_type: "quant_decision",
                    entity_id: entity_id.clone(),
                    strategy_scope: model.strategy_scope,
                    target: model.target,
                    recommended_action: score.recommended_action,
                    predicted_score: score.predicted_score,
                    threshold: score.threshold,
                    context: context.clone(),
                })
            })
            .collect();

        self.record_rows(rows).await;
    }

    fn should_record(&self) -> bool {
        self.enabled && !self.schema_missing.load(Ordering::Relaxed)
    }

    async fn record_rows(&self, rows: Vec<PredictionRow>) {
        for row in rows {
            if let Err(error) = sqlx::query(
                r#"
                INSERT INTO learning_shadow_predictions (
                    model_id, entity_type, entity_id, strategy_scope, target,
                    recommended_action, predicted_score, threshold, context, predicted_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
                ON CONFLICT (model_id, entity_type, entity_id, target) DO UPDATE SET
                    recommended_action = EXCLUDED.recommended_action,
                    predicted_score = EXCLUDED.predicted_score,
                    threshold = EXCLUDED.threshold,
                    context = EXCLUDED.context,
                    predicted_at = EXCLUDED.predicted_at
                "#,
            )
            .bind(row.model_id)
            .bind(row.entity_type)
            .bind(&row.entity_id)
            .bind(&row.strategy_scope)
            .bind(&row.target)
            .bind(&row.recommended_action)
            .bind(row.predicted_score)
            .bind(row.threshold)
            .bind(&row.context)
            .execute(&self.pool)
            .await
            {
                self.handle_error("Failed to persist shadow prediction", &error);
                return;
            }
        }
    }

    async fn ensure_builtin_models(&self, strategy_scope: &str) -> Result<(), sqlx::Error> {
        let specs: &[ModelSpec] = match strategy_scope {
            "arb" => &[
                ARB_OPEN_SUCCESS_MODEL,
                ARB_ONE_LEGGED_MODEL,
                ARB_EDGE_CAPTURE_MODEL,
            ],
            "quant" => &[
                QUANT_EXECUTE_SUCCESS_MODEL,
                QUANT_PNL_SIGN_MODEL,
                QUANT_EDGE_CAPTURE_MODEL,
            ],
            _ => &[],
        };

        for spec in specs {
            self.ensure_model_id(*spec).await?;
        }
        Ok(())
    }

    async fn ensure_model_id(&self, spec: ModelSpec) -> Result<Uuid, sqlx::Error> {
        if let Some(cached) = self.model_cache.read().await.get(spec.key).copied() {
            return Ok(cached);
        }

        let model_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO learning_model_registry (
                model_key, strategy_scope, target, model_type, version, status,
                feature_view, metrics
            )
            VALUES ($1, $2, $3, $4, $5, 'shadow', $6, $7)
            ON CONFLICT (model_key) DO UPDATE SET
                strategy_scope = EXCLUDED.strategy_scope,
                target = EXCLUDED.target,
                model_type = EXCLUDED.model_type,
                version = EXCLUDED.version,
                feature_view = EXCLUDED.feature_view,
                metrics = learning_model_registry.metrics || EXCLUDED.metrics,
                status = CASE
                    WHEN learning_model_registry.status IN ('active', 'canary', 'retired', 'disabled')
                    THEN learning_model_registry.status
                    ELSE 'shadow'
                END
            RETURNING id
            "#,
        )
        .bind(spec.key)
        .bind(spec.strategy_scope)
        .bind(spec.target)
        .bind(MODEL_TYPE)
        .bind(MODEL_VERSION)
        .bind(spec.feature_view)
        .bind(serde_json::json!({
            "family": MODEL_TYPE,
            "version": MODEL_VERSION,
            "writes_live_predictions": true,
        }))
        .fetch_one(&self.pool)
        .await?;

        self.model_cache.write().await.insert(spec.key, model_id);
        Ok(model_id)
    }

    fn handle_error(&self, message: &str, error: &sqlx::Error) {
        if is_schema_missing(error) {
            self.schema_missing.store(true, Ordering::Relaxed);
        }
        warn!(error = %error, "{message}");
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ArbShadowScores {
    pub open_success_probability: f64,
    pub one_legged_risk: f64,
    pub realized_edge_capture: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct QuantShadowScores {
    pub execute_success_probability: f64,
    pub realized_pnl_sign: f64,
    pub realized_edge_capture: f64,
}

pub fn score_arb_shadow(input: &ArbShadowPredictionInput) -> ArbShadowScores {
    let age_secs = (input.signal_age_ms.max(0) as f64) / 1000.0;
    let total_cost = decimal_to_f64(input.total_cost).max(0.01);
    let net_profit_ratio = decimal_to_f64(input.net_profit) / total_cost;
    let live_bonus = if input.live_ready { 0.10 } else { -0.05 };

    ArbShadowScores {
        open_success_probability: clamp_probability(sigmoid(
            (net_profit_ratio * 40.0) - (age_secs * 0.03) + live_bonus,
        )),
        one_legged_risk: clamp_probability(sigmoid(
            (age_secs * 0.05) - (net_profit_ratio * 50.0)
                + if input.live_ready { -0.10 } else { 0.10 },
        )),
        realized_edge_capture: clamp_probability(sigmoid(
            (net_profit_ratio * 30.0) - (age_secs * 0.02),
        )),
    }
}

pub fn score_quant_shadow(input: &QuantShadowPredictionInput) -> QuantShadowScores {
    let now = Utc::now();
    let age_secs = now
        .signed_duration_since(input.generated_at)
        .num_seconds()
        .max(0) as f64;
    let max_age_secs = input.max_signal_age_secs.max(1) as f64;
    let freshness = (1.0 - (age_secs / max_age_secs)).clamp(0.0, 1.0);
    let size_penalty = (decimal_to_f64(input.suggested_size_usd) / 150.0).clamp(0.0, 1.0) * 0.15;
    let edge_bps = extract_expected_edge_bps(&input.metadata).unwrap_or(input.confidence * 100.0);
    let normalized_edge = (edge_bps / 150.0).clamp(-1.0, 1.5);
    let kind_bias = match input.kind {
        QuantSignalKind::Flow => 0.15,
        QuantSignalKind::CrossMarket => 0.05,
        QuantSignalKind::MeanReversion => -0.03,
        QuantSignalKind::ResolutionProximity => 0.08,
    };

    QuantShadowScores {
        execute_success_probability: clamp_probability(sigmoid(
            ((input.confidence - input.min_confidence) * 4.5) + (freshness * 1.2) + kind_bias
                - size_penalty,
        )),
        realized_pnl_sign: clamp_probability(sigmoid(
            ((input.confidence - 0.5) * 3.5) + normalized_edge + kind_bias - size_penalty,
        )),
        realized_edge_capture: clamp_probability(sigmoid(
            ((input.confidence - 0.5) * 3.0) + (normalized_edge * 0.9) + (freshness * 0.5)
                - size_penalty,
        )),
    }
}

fn extract_expected_edge_bps(metadata: &serde_json::Value) -> Option<f64> {
    metadata
        .get("expected_edge_bps")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| {
            metadata
                .get("raw_expected_edge_bps")
                .and_then(serde_json::Value::as_f64)
        })
}

fn decimal_to_f64(value: Decimal) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(0.0)
}

fn sigmoid(value: f64) -> f64 {
    1.0 / (1.0 + (-value).exp())
}

fn clamp_probability(value: f64) -> f64 {
    value.clamp(0.01, 0.99)
}

fn is_schema_missing(error: &sqlx::Error) -> bool {
    let message = error.to_string();
    message.contains("learning_model_registry") || message.contains("learning_shadow_predictions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn arb_shadow_scores_stay_bounded_and_freshness_matters() {
        let fresh = score_arb_shadow(&ArbShadowPredictionInput {
            attempt_id: Uuid::new_v4(),
            market_id: "mkt".to_string(),
            execution_mode: "paper".to_string(),
            signal_age_ms: 250,
            yes_ask: Decimal::new(48, 2),
            no_ask: Decimal::new(49, 2),
            total_cost: Decimal::new(97, 2),
            gross_profit: Decimal::new(3, 2),
            net_profit: Decimal::new(2, 2),
            live_ready: true,
        });
        let stale = score_arb_shadow(&ArbShadowPredictionInput {
            signal_age_ms: 20_000,
            ..ArbShadowPredictionInput {
                attempt_id: Uuid::new_v4(),
                market_id: "mkt".to_string(),
                execution_mode: "paper".to_string(),
                signal_age_ms: 250,
                yes_ask: Decimal::new(48, 2),
                no_ask: Decimal::new(49, 2),
                total_cost: Decimal::new(97, 2),
                gross_profit: Decimal::new(3, 2),
                net_profit: Decimal::new(2, 2),
                live_ready: true,
            }
        });

        assert!((0.0..=1.0).contains(&fresh.open_success_probability));
        assert!(fresh.open_success_probability > stale.open_success_probability);
    }

    #[test]
    fn quant_shadow_scores_respect_confidence_and_expected_edge() {
        let now = Utc::now();
        let low = score_quant_shadow(&QuantShadowPredictionInput {
            decision_id: Uuid::new_v4(),
            kind: QuantSignalKind::Flow,
            condition_id: "mkt".to_string(),
            direction: SignalDirection::BuyYes,
            confidence: 0.55,
            suggested_size_usd: Decimal::new(25, 0),
            generated_at: now,
            expiry: now + Duration::minutes(10),
            execution_mode: "paper".to_string(),
            metadata: serde_json::json!({ "expected_edge_bps": 20.0 }),
            min_confidence: 0.60,
            max_signal_age_secs: 120,
        });
        let high = score_quant_shadow(&QuantShadowPredictionInput {
            confidence: 0.82,
            metadata: serde_json::json!({ "expected_edge_bps": 140.0 }),
            ..QuantShadowPredictionInput {
                decision_id: Uuid::new_v4(),
                kind: QuantSignalKind::Flow,
                condition_id: "mkt".to_string(),
                direction: SignalDirection::BuyYes,
                confidence: 0.55,
                suggested_size_usd: Decimal::new(25, 0),
                generated_at: now,
                expiry: now + Duration::minutes(10),
                execution_mode: "paper".to_string(),
                metadata: serde_json::json!({ "expected_edge_bps": 20.0 }),
                min_confidence: 0.60,
                max_signal_age_secs: 120,
            }
        });

        assert!(high.execute_success_probability > low.execute_success_probability);
        assert!(high.realized_pnl_sign > low.realized_pnl_sign);
        assert!(high.realized_edge_capture > low.realized_edge_capture);
    }
}
