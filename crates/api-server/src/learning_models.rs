use crate::learning::{
    score_arb_shadow, score_quant_shadow, ArbShadowPredictionInput, QuantShadowPredictionInput,
};
use chrono::Utc;
use polymarket_core::types::signal::{QuantSignalKind, SignalDirection};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

#[derive(Clone)]
pub struct LearningModelRuntime {
    pool: PgPool,
    refresh_secs: u64,
    cache: Arc<RwLock<Option<CachedModels>>>,
}

#[derive(Clone)]
struct CachedModels {
    fetched_at: Instant,
    models: Vec<LoadedModel>,
}

#[derive(Debug, Clone)]
pub struct LoadedModel {
    pub id: Uuid,
    pub model_key: String,
    pub strategy_scope: String,
    pub target: String,
    pub model_type: String,
    pub version: String,
    pub status: String,
    pub feature_view: String,
    pub metrics: serde_json::Value,
    inference: ModelInferenceDefinition,
}

#[derive(Debug, Clone)]
enum ModelInferenceDefinition {
    HeuristicBaseline,
    Linear(LinearArtifact),
}

#[derive(Debug, Clone)]
pub struct ModelScore {
    pub predicted_score: f64,
    pub threshold: f64,
    pub recommended_action: String,
}

#[derive(Debug, FromRow)]
struct RegistryModelRow {
    id: Uuid,
    model_key: String,
    strategy_scope: String,
    target: String,
    model_type: String,
    version: String,
    status: String,
    feature_view: String,
    metrics: serde_json::Value,
    artifact_uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearArtifact {
    #[serde(default)]
    intercept: f64,
    #[serde(default, alias = "coefficients")]
    weights: HashMap<String, f64>,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(default)]
    positive_action: Option<String>,
    #[serde(default)]
    negative_action: Option<String>,
    #[serde(default)]
    transform: Option<String>,
    #[serde(default)]
    clip_min: Option<f64>,
    #[serde(default)]
    clip_max: Option<f64>,
}

impl LearningModelRuntime {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            refresh_secs: std::env::var("LEARNING_MODEL_REFRESH_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            cache: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn models_for_scope(
        &self,
        strategy_scope: &str,
    ) -> Result<Vec<LoadedModel>, sqlx::Error> {
        let now = Instant::now();
        if let Some(cached) = self.cache.read().await.as_ref() {
            if now.duration_since(cached.fetched_at).as_secs() < self.refresh_secs {
                return Ok(cached
                    .models
                    .iter()
                    .filter(|model| model.strategy_scope == strategy_scope)
                    .cloned()
                    .collect());
            }
        }

        let rows: Vec<RegistryModelRow> = sqlx::query_as(
            r#"
            SELECT
                id,
                model_key,
                strategy_scope,
                target,
                model_type,
                version,
                status,
                feature_view,
                metrics,
                artifact_uri
            FROM learning_model_registry
            WHERE status IN ('shadow', 'canary', 'active')
            ORDER BY
                CASE status
                    WHEN 'active' THEN 0
                    WHEN 'canary' THEN 1
                    ELSE 2
                END,
                created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut models = Vec::new();
        for row in rows {
            match build_loaded_model(row).await {
                Ok(Some(model)) => models.push(model),
                Ok(None) => {}
                Err(error) => warn!(error = %error, "Skipping unsupported learning model"),
            }
        }

        *self.cache.write().await = Some(CachedModels {
            fetched_at: now,
            models: models.clone(),
        });

        Ok(models
            .into_iter()
            .filter(|model| model.strategy_scope == strategy_scope)
            .collect())
    }
}

impl LoadedModel {
    pub fn score_arb(&self, input: &ArbShadowPredictionInput) -> Option<ModelScore> {
        if self.strategy_scope != "arb" {
            return None;
        }

        let predicted_score = match &self.inference {
            ModelInferenceDefinition::HeuristicBaseline => {
                let scores = score_arb_shadow(input);
                match self.target.as_str() {
                    "open_success_probability" => scores.open_success_probability,
                    "one_legged_risk" => scores.one_legged_risk,
                    "realized_edge_capture" => scores.realized_edge_capture,
                    _ => return None,
                }
            }
            ModelInferenceDefinition::Linear(artifact) => {
                let features = arb_feature_map(input);
                run_linear_artifact(artifact, &features)
            }
        };

        Some(ModelScore {
            predicted_score,
            threshold: self.threshold(),
            recommended_action: self.recommended_action(predicted_score),
        })
    }

    pub fn score_quant(&self, input: &QuantShadowPredictionInput) -> Option<ModelScore> {
        if self.strategy_scope != "quant" {
            return None;
        }

        let predicted_score = match &self.inference {
            ModelInferenceDefinition::HeuristicBaseline => {
                let scores = score_quant_shadow(input);
                match self.target.as_str() {
                    "execute_success_probability" => scores.execute_success_probability,
                    "realized_pnl_sign" => scores.realized_pnl_sign,
                    "realized_edge_capture" => scores.realized_edge_capture,
                    _ => return None,
                }
            }
            ModelInferenceDefinition::Linear(artifact) => {
                let features = quant_feature_map(input);
                run_linear_artifact(artifact, &features)
            }
        };

        Some(ModelScore {
            predicted_score,
            threshold: self.threshold(),
            recommended_action: self.recommended_action(predicted_score),
        })
    }

    fn threshold(&self) -> f64 {
        match &self.inference {
            ModelInferenceDefinition::HeuristicBaseline => default_threshold(&self.target),
            ModelInferenceDefinition::Linear(artifact) => artifact
                .threshold
                .unwrap_or_else(|| default_threshold(&self.target)),
        }
    }

    fn recommended_action(&self, predicted_score: f64) -> String {
        let threshold = self.threshold();
        let positive_action = match &self.inference {
            ModelInferenceDefinition::Linear(artifact) => artifact
                .positive_action
                .clone()
                .unwrap_or_else(|| default_positive_action(&self.strategy_scope, &self.target)),
            ModelInferenceDefinition::HeuristicBaseline => {
                default_positive_action(&self.strategy_scope, &self.target)
            }
        };
        let negative_action = match &self.inference {
            ModelInferenceDefinition::Linear(artifact) => artifact
                .negative_action
                .clone()
                .unwrap_or_else(|| default_negative_action(&self.strategy_scope, &self.target)),
            ModelInferenceDefinition::HeuristicBaseline => {
                default_negative_action(&self.strategy_scope, &self.target)
            }
        };

        if target_is_positive_when_high(&self.target) {
            if predicted_score >= threshold {
                positive_action
            } else {
                negative_action
            }
        } else if predicted_score >= threshold {
            negative_action
        } else {
            positive_action
        }
    }
}

async fn build_loaded_model(row: RegistryModelRow) -> Result<Option<LoadedModel>, String> {
    let inference = match row.model_type.as_str() {
        "heuristic_shadow_baseline" => ModelInferenceDefinition::HeuristicBaseline,
        "trained_linear_probability_v1" | "trained_linear_regression_v1" => {
            let artifact = load_linear_artifact(&row).await?;
            ModelInferenceDefinition::Linear(artifact)
        }
        other => return Err(format!("unsupported model_type {other}")),
    };

    Ok(Some(LoadedModel {
        id: row.id,
        model_key: row.model_key,
        strategy_scope: row.strategy_scope,
        target: row.target,
        model_type: row.model_type,
        version: row.version,
        status: row.status,
        feature_view: row.feature_view,
        metrics: row.metrics,
        inference,
    }))
}

async fn load_linear_artifact(row: &RegistryModelRow) -> Result<LinearArtifact, String> {
    if let Some(value) = row.metrics.get("artifact") {
        return serde_json::from_value(value.clone()).map_err(|e| e.to_string());
    }

    let Some(uri) = row.artifact_uri.as_deref() else {
        return Err(format!(
            "trained model {} is missing artifact_uri or metrics.artifact",
            row.model_key
        ));
    };

    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let raw = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("failed reading artifact {path}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("invalid artifact json {path}: {e}"))
}

fn run_linear_artifact(artifact: &LinearArtifact, features: &HashMap<&'static str, f64>) -> f64 {
    let mut value = artifact.intercept;
    for (feature, weight) in &artifact.weights {
        value += features.get(feature.as_str()).copied().unwrap_or(0.0) * weight;
    }

    let transformed = match artifact.transform.as_deref().unwrap_or("sigmoid") {
        "identity" => value,
        "clamp_0_1" => value.clamp(0.0, 1.0),
        _ => 1.0 / (1.0 + (-value).exp()),
    };

    transformed.clamp(
        artifact.clip_min.unwrap_or(0.0),
        artifact.clip_max.unwrap_or(1.0),
    )
}

fn arb_feature_map(input: &ArbShadowPredictionInput) -> HashMap<&'static str, f64> {
    let signal_age_secs = (input.signal_age_ms.max(0) as f64) / 1000.0;
    let total_cost = decimal_to_f64(input.total_cost).max(0.01);
    let gross_profit = decimal_to_f64(input.gross_profit);
    let net_profit = decimal_to_f64(input.net_profit);
    let mut features = HashMap::new();
    features.insert("signal_age_secs", signal_age_secs);
    features.insert("yes_ask", decimal_to_f64(input.yes_ask));
    features.insert("no_ask", decimal_to_f64(input.no_ask));
    features.insert("total_cost", total_cost);
    features.insert("gross_profit", gross_profit);
    features.insert("net_profit", net_profit);
    features.insert("gross_profit_ratio", gross_profit / total_cost);
    features.insert("net_profit_ratio", net_profit / total_cost);
    features.insert("live_ready", if input.live_ready { 1.0 } else { 0.0 });
    features
}

fn quant_feature_map(input: &QuantShadowPredictionInput) -> HashMap<&'static str, f64> {
    let now = Utc::now();
    let age_secs = now
        .signed_duration_since(input.generated_at)
        .num_seconds()
        .max(0) as f64;
    let time_to_expiry_secs = input.expiry.signed_duration_since(now).num_seconds().max(0) as f64;
    let max_signal_age_secs = input.max_signal_age_secs.max(1) as f64;
    let freshness = (1.0 - (age_secs / max_signal_age_secs)).clamp(0.0, 1.0);
    let expected_edge_bps =
        extract_expected_edge_bps(&input.metadata).unwrap_or(input.confidence * 100.0);
    let normalized_edge = (expected_edge_bps / 150.0).clamp(-1.0, 1.5);

    let mut features = HashMap::new();
    features.insert("confidence", input.confidence);
    features.insert(
        "suggested_size_usd",
        decimal_to_f64(input.suggested_size_usd),
    );
    features.insert("age_secs", age_secs);
    features.insert("time_to_expiry_secs", time_to_expiry_secs);
    features.insert("freshness", freshness);
    features.insert("expected_edge_bps", expected_edge_bps);
    features.insert("normalized_edge", normalized_edge);
    features.insert("min_confidence", input.min_confidence);
    features.insert("max_signal_age_secs", max_signal_age_secs);
    features.insert(
        "direction_buy_yes",
        if matches!(input.direction, SignalDirection::BuyYes) {
            1.0
        } else {
            0.0
        },
    );
    features.insert(
        "direction_buy_no",
        if matches!(input.direction, SignalDirection::BuyNo) {
            1.0
        } else {
            0.0
        },
    );
    features.insert(
        "kind_flow",
        if matches!(input.kind, QuantSignalKind::Flow) {
            1.0
        } else {
            0.0
        },
    );
    features.insert(
        "kind_cross_market",
        if matches!(input.kind, QuantSignalKind::CrossMarket) {
            1.0
        } else {
            0.0
        },
    );
    features.insert(
        "kind_mean_reversion",
        if matches!(input.kind, QuantSignalKind::MeanReversion) {
            1.0
        } else {
            0.0
        },
    );
    features.insert(
        "kind_resolution_proximity",
        if matches!(input.kind, QuantSignalKind::ResolutionProximity) {
            1.0
        } else {
            0.0
        },
    );
    features
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

fn default_threshold(target: &str) -> f64 {
    match target {
        "open_success_probability" | "execute_success_probability" => 0.60,
        "one_legged_risk" => 0.40,
        "realized_pnl_sign" => 0.55,
        "realized_edge_capture" => 0.55,
        _ => 0.50,
    }
}

fn target_is_positive_when_high(target: &str) -> bool {
    !matches!(target, "one_legged_risk")
}

fn default_positive_action(strategy_scope: &str, target: &str) -> String {
    match (strategy_scope, target) {
        (_, "one_legged_risk") => "execute".to_string(),
        _ => "execute".to_string(),
    }
}

fn default_negative_action(strategy_scope: &str, target: &str) -> String {
    match (strategy_scope, target) {
        (_, "one_legged_risk") => "skip".to_string(),
        ("arb", "realized_edge_capture") => "deprioritize".to_string(),
        ("quant", "realized_edge_capture") => "downsize".to_string(),
        _ => "skip".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_artifact_respects_sigmoid_threshold_defaults() {
        let artifact = LinearArtifact {
            intercept: 0.2,
            weights: HashMap::from([(String::from("confidence"), 2.0)]),
            threshold: None,
            positive_action: None,
            negative_action: None,
            transform: Some("sigmoid".to_string()),
            clip_min: None,
            clip_max: None,
        };
        let mut features = HashMap::new();
        features.insert("confidence", 0.7);
        let score = run_linear_artifact(&artifact, &features);
        assert!((0.0..=1.0).contains(&score));
        assert!(score > 0.5);
    }

    #[test]
    fn negative_direction_targets_flip_actions() {
        let model = LoadedModel {
            id: Uuid::new_v4(),
            model_key: "risk".to_string(),
            strategy_scope: "arb".to_string(),
            target: "one_legged_risk".to_string(),
            model_type: "heuristic_shadow_baseline".to_string(),
            version: "v1".to_string(),
            status: "active".to_string(),
            feature_view: "canonical_arb_learning_attempts".to_string(),
            metrics: serde_json::json!({}),
            inference: ModelInferenceDefinition::HeuristicBaseline,
        };

        assert_eq!(model.recommended_action(0.7), "skip");
        assert_eq!(model.recommended_action(0.2), "execute");
    }
}
