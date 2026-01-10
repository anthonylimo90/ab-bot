//! Rotation recommendations handler.
//!
//! Provides recommendations for roster changes based on wallet performance.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// Recommendation type for roster changes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecommendationType {
    /// Demote wallet from Active to Bench.
    Demote,
    /// Promote wallet from Bench to Active.
    Promote,
    /// Alert about concerning pattern.
    Alert,
}

/// Reason for the recommendation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationReason {
    /// Performance has degraded significantly.
    AlphaDecay,
    /// Wallet is using martingale-style doubling.
    MartingalePattern,
    /// Strategy has changed from original pattern.
    StrategyDrift,
    /// Suspicious wallet behavior.
    HoneypotWarning,
    /// Bench wallet outperforming Active roster.
    Outperforming,
    /// Excessive risk taking.
    HighRisk,
    /// Consistently losing.
    ConsistentLosses,
}

/// Urgency level for the recommendation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    Medium,
    High,
}

/// A rotation recommendation.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RotationRecommendation {
    pub id: String,
    #[serde(rename = "type")]
    pub recommendation_type: RecommendationType,
    pub wallet_address: String,
    pub wallet_label: Option<String>,
    pub reason: RecommendationReason,
    pub evidence: Vec<String>,
    pub urgency: Urgency,
    pub suggested_action: String,
    pub created_at: String,
}

/// Query parameters for recommendations.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RecommendationsQuery {
    /// Filter by urgency level.
    pub urgency: Option<String>,
    /// Maximum number of recommendations to return.
    pub limit: Option<i32>,
}

/// Database row for tracked wallet metrics.
#[derive(Debug, FromRow)]
struct WalletMetricsRow {
    address: String,
    label: Option<String>,
    roi_30d: Option<Decimal>,
    roi_7d: Option<Decimal>,
    sharpe_30d: Option<Decimal>,
    win_rate_30d: Option<Decimal>,
    trades_30d: Option<i64>,
    max_drawdown_30d: Option<Decimal>,
    enabled: bool,
}

fn decimal_to_f64(d: Option<Decimal>) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.and_then(|v| v.to_f64()).unwrap_or(0.0)
}

/// Get rotation recommendations.
#[utoipa::path(
    get,
    path = "/api/v1/recommendations/rotation",
    tag = "recommendations",
    params(
        ("urgency" = Option<String>, Query, description = "Filter by urgency (low, medium, high)"),
        ("limit" = Option<i32>, Query, description = "Maximum recommendations to return")
    ),
    responses(
        (status = 200, description = "List of rotation recommendations", body = Vec<RotationRecommendation>),
        (status = 500, description = "Internal server error", body = crate::error::ErrorResponse)
    )
)]
pub async fn get_rotation_recommendations(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecommendationsQuery>,
) -> Result<Json<Vec<RotationRecommendation>>, ApiError> {
    let limit = params.limit.unwrap_or(10).min(50);
    let mut recommendations = Vec::new();

    // Get active wallets with their metrics
    let active_wallets: Vec<WalletMetricsRow> = sqlx::query_as(
        r#"
        SELECT
            tw.address,
            tw.label,
            wsm.roi_30d,
            wsm.roi_7d,
            wsm.sharpe_30d,
            wsm.win_rate_30d,
            wsm.trades_30d,
            wsm.max_drawdown_30d,
            tw.enabled
        FROM tracked_wallets tw
        LEFT JOIN wallet_success_metrics wsm ON wsm.address = tw.address
        WHERE tw.enabled = true
        ORDER BY wsm.roi_30d DESC NULLS LAST
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Get bench wallets (disabled but tracked)
    let bench_wallets: Vec<WalletMetricsRow> = sqlx::query_as(
        r#"
        SELECT
            tw.address,
            tw.label,
            wsm.roi_30d,
            wsm.roi_7d,
            wsm.sharpe_30d,
            wsm.win_rate_30d,
            wsm.trades_30d,
            wsm.max_drawdown_30d,
            tw.enabled
        FROM tracked_wallets tw
        LEFT JOIN wallet_success_metrics wsm ON wsm.address = tw.address
        WHERE tw.enabled = false
        ORDER BY wsm.roi_30d DESC NULLS LAST
        "#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Calculate average active wallet performance
    let avg_active_roi = if !active_wallets.is_empty() {
        active_wallets.iter().map(|w| decimal_to_f64(w.roi_30d)).sum::<f64>()
            / active_wallets.len() as f64
    } else {
        0.0
    };

    // Check active wallets for demotion candidates
    for wallet in &active_wallets {
        let roi_30d = decimal_to_f64(wallet.roi_30d);
        let roi_7d = decimal_to_f64(wallet.roi_7d);
        let sharpe = decimal_to_f64(wallet.sharpe_30d);
        let win_rate = decimal_to_f64(wallet.win_rate_30d);
        let max_dd = decimal_to_f64(wallet.max_drawdown_30d);

        // Alpha Decay: ROI dropped significantly
        if roi_7d < roi_30d * 0.5 && roi_30d > 0.0 {
            let decay_pct = ((roi_30d - roi_7d) / roi_30d * 100.0).abs();
            recommendations.push(RotationRecommendation {
                id: Uuid::new_v4().to_string(),
                recommendation_type: RecommendationType::Demote,
                wallet_address: wallet.address.clone(),
                wallet_label: wallet.label.clone(),
                reason: RecommendationReason::AlphaDecay,
                evidence: vec![
                    format!("30-day ROI dropped from +{:.1}% to +{:.1}%", roi_30d, roi_7d),
                    format!("Performance decay of {:.0}%", decay_pct),
                    if sharpe < 1.0 {
                        format!("Sharpe ratio below 1.0 ({:.2})", sharpe)
                    } else {
                        format!("Sharpe ratio: {:.2}", sharpe)
                    },
                ],
                urgency: if decay_pct > 50.0 { Urgency::High } else { Urgency::Medium },
                suggested_action: "Demote to Bench for monitoring".to_string(),
                created_at: Utc::now().to_rfc3339(),
            });
        }

        // Low win rate with negative ROI
        if win_rate < 0.5 && roi_30d < 0.0 {
            recommendations.push(RotationRecommendation {
                id: Uuid::new_v4().to_string(),
                recommendation_type: RecommendationType::Demote,
                wallet_address: wallet.address.clone(),
                wallet_label: wallet.label.clone(),
                reason: RecommendationReason::ConsistentLosses,
                evidence: vec![
                    format!("Win rate of only {:.1}%", win_rate * 100.0),
                    format!("Negative ROI of {:.1}%", roi_30d),
                    format!("Max drawdown: {:.1}%", max_dd.abs()),
                ],
                urgency: Urgency::High,
                suggested_action: "Demote to Bench immediately".to_string(),
                created_at: Utc::now().to_rfc3339(),
            });
        }

        // High risk behavior
        if max_dd.abs() > 30.0 {
            recommendations.push(RotationRecommendation {
                id: Uuid::new_v4().to_string(),
                recommendation_type: RecommendationType::Alert,
                wallet_address: wallet.address.clone(),
                wallet_label: wallet.label.clone(),
                reason: RecommendationReason::HighRisk,
                evidence: vec![
                    format!("Maximum drawdown of {:.1}%", max_dd.abs()),
                    "Excessive risk exposure detected".to_string(),
                ],
                urgency: Urgency::Medium,
                suggested_action: "Monitor closely and consider reducing allocation".to_string(),
                created_at: Utc::now().to_rfc3339(),
            });
        }
    }

    // Check bench wallets for promotion candidates
    for wallet in &bench_wallets {
        let roi_30d = decimal_to_f64(wallet.roi_30d);
        let sharpe = decimal_to_f64(wallet.sharpe_30d);
        let win_rate = decimal_to_f64(wallet.win_rate_30d);
        let trades = wallet.trades_30d.unwrap_or(0);

        // Outperforming bench wallet
        if roi_30d > avg_active_roi * 1.15 && trades >= 20 && win_rate > 0.6 {
            recommendations.push(RotationRecommendation {
                id: Uuid::new_v4().to_string(),
                recommendation_type: RecommendationType::Promote,
                wallet_address: wallet.address.clone(),
                wallet_label: wallet.label.clone(),
                reason: RecommendationReason::Outperforming,
                evidence: vec![
                    format!(
                        "Outperforming Active 5 average by {:.0}%",
                        (roi_30d - avg_active_roi)
                    ),
                    format!("Consistent win rate of {:.1}%", win_rate * 100.0),
                    format!("{}+ trades with stable strategy", trades),
                ],
                urgency: Urgency::Low,
                suggested_action: "Consider promoting to Active 5".to_string(),
                created_at: Utc::now().to_rfc3339(),
            });
        }
    }

    // Filter by urgency if specified
    if let Some(urgency_filter) = params.urgency {
        let target = match urgency_filter.to_lowercase().as_str() {
            "low" => Urgency::Low,
            "medium" => Urgency::Medium,
            "high" => Urgency::High,
            _ => Urgency::Low,
        };
        recommendations.retain(|r| r.urgency == target);
    }

    // Sort by urgency (high first) then by date
    recommendations.sort_by(|a, b| b.urgency.cmp(&a.urgency));

    // Apply limit
    recommendations.truncate(limit as usize);

    Ok(Json(recommendations))
}

/// Dismiss a recommendation.
#[utoipa::path(
    post,
    path = "/api/v1/recommendations/{id}/dismiss",
    tag = "recommendations",
    params(
        ("id" = String, Path, description = "Recommendation ID to dismiss")
    ),
    responses(
        (status = 200, description = "Recommendation dismissed"),
        (status = 404, description = "Recommendation not found")
    )
)]
pub async fn dismiss_recommendation(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // In a real implementation, this would mark the recommendation as dismissed in the database
    // For now, we just acknowledge it
    tracing::info!(recommendation_id = %id, "Recommendation dismissed");
    Ok(Json(serde_json::json!({ "status": "dismissed", "id": id })))
}

/// Accept a recommendation.
#[utoipa::path(
    post,
    path = "/api/v1/recommendations/{id}/accept",
    tag = "recommendations",
    params(
        ("id" = String, Path, description = "Recommendation ID to accept")
    ),
    responses(
        (status = 200, description = "Recommendation accepted"),
        (status = 404, description = "Recommendation not found")
    )
)]
pub async fn accept_recommendation(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // In a real implementation, this would execute the recommended action
    tracing::info!(recommendation_id = %id, "Recommendation accepted");
    Ok(Json(serde_json::json!({ "status": "accepted", "id": id })))
}
