//! Risk-based allocation recalculation API handlers.

use crate::error::ApiError;
use crate::state::AppState;
use auth::Claims;
use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;
use wallet_tracker::risk_scorer::{RiskScorer, WalletRiskScore};

/// Request to recalculate risk-based allocations.
#[derive(Debug, Deserialize)]
pub struct RecalculateAllocationsRequest {
    /// Portfolio tier ("active", "bench", or "all").
    pub tier: String,
    /// Whether to automatically apply the new allocations.
    #[serde(default)]
    pub auto_apply: bool,
}

/// Response with recommended allocations.
#[derive(Debug, Serialize)]
pub struct RecalculateAllocationsResponse {
    /// Recommended allocations for the tier.
    pub previews: Vec<AllocationPreview>,
    /// Whether the allocations were automatically applied.
    pub applied: bool,
    /// Total number of wallets processed.
    pub wallet_count: usize,
}

/// Preview of a recommended allocation change.
#[derive(Debug, Serialize)]
pub struct AllocationPreview {
    pub address: String,
    pub current_allocation_pct: Option<f64>,
    pub recommended_allocation_pct: f64,
    pub change_pct: f64,
    pub composite_score: f64,
    pub components: wallet_tracker::risk_scorer::RiskComponents,
}

/// Recalculate risk-based allocations for a portfolio tier.
pub async fn recalculate_allocations(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<RecalculateAllocationsRequest>,
) -> Result<Json<RecalculateAllocationsResponse>, ApiError> {
    info!(
        tier = %req.tier,
        auto_apply = req.auto_apply,
        "Recalculating risk-based allocations"
    );

    // Extract user_id from claims
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    // Resolve user's workspace
    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace configured".into()))?;

    // Validate tier
    if !matches!(req.tier.as_str(), "active" | "bench" | "all") {
        return Err(ApiError::BadRequest(
            "Invalid tier. Must be 'active', 'bench', or 'all'".to_string(),
        ));
    }

    let risk_scorer = RiskScorer::new(state.pool.clone(), workspace_id);

    let tiers = if req.tier == "all" {
        vec!["active".to_string(), "bench".to_string()]
    } else {
        vec![req.tier.clone()]
    };

    let mut all_previews = Vec::new();

    for tier in tiers {
        // Calculate risk-based allocations
        let allocations = risk_scorer
            .calculate_allocations(&tier)
            .await
            .map_err(|e| {
                warn!(error = %e, tier = %tier, "Failed to calculate allocations");
                ApiError::Internal(format!("Failed to calculate allocations: {}", e))
            })?;

        if allocations.is_empty() {
            warn!(tier = %tier, "No wallets with metrics found for allocation");
            continue;
        }

        // Convert to previews
        let previews: Vec<AllocationPreview> =
            allocations.iter().map(allocation_to_preview).collect();

        all_previews.extend(previews);

        // Auto-apply if requested
        if req.auto_apply {
            risk_scorer
                .apply_allocations(&tier, &allocations)
                .await
                .map_err(|e| {
                    warn!(error = %e, tier = %tier, "Failed to apply allocations");
                    ApiError::Internal(format!("Failed to apply allocations: {}", e))
                })?;

            // Log to audit trail
            log_allocation_change(&state, workspace_id, &tier, &allocations).await?;

            info!(
                tier = %tier,
                wallet_count = allocations.len(),
                "Applied risk-based allocations"
            );
        }
    }

    let wallet_count = all_previews.len();

    Ok(Json(RecalculateAllocationsResponse {
        previews: all_previews,
        applied: req.auto_apply,
        wallet_count,
    }))
}

fn allocation_to_preview(score: &WalletRiskScore) -> AllocationPreview {
    let change_pct = if let Some(current) = score.current_allocation_pct {
        score.recommended_allocation_pct - current
    } else {
        score.recommended_allocation_pct
    };

    AllocationPreview {
        address: score.address.clone(),
        current_allocation_pct: score.current_allocation_pct,
        recommended_allocation_pct: score.recommended_allocation_pct,
        change_pct,
        composite_score: score.composite_score,
        components: score.components.clone(),
    }
}

async fn log_allocation_change(
    state: &AppState,
    workspace_id: Uuid,
    tier: &str,
    allocations: &[WalletRiskScore],
) -> Result<(), ApiError> {
    for allocation in allocations {
        sqlx::query(
            r#"
            INSERT INTO auto_rotation_history (
                workspace_id,
                action_type,
                wallet_address,
                previous_tier,
                new_tier,
                reason,
                created_at
            )
            VALUES (
                $1,
                'allocation_adjustment',
                $2,
                $3,
                $3,
                $4,
                NOW()
            )
            "#,
        )
        .bind(workspace_id)
        .bind(&allocation.address)
        .bind(tier)
        .bind(format!(
            "Risk-based allocation: score={:.3}, allocation={:.1}%",
            allocation.composite_score, allocation.recommended_allocation_pct
        ))
        .execute(&state.pool)
        .await
        .map_err(|e| {
            warn!(error = %e, "Failed to log allocation change to audit trail");
            ApiError::Internal("Failed to log allocation change".to_string())
        })?;
    }

    Ok(())
}

/// Get user's current workspace ID.
async fn get_current_workspace(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let settings: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT default_workspace_id FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

    Ok(settings.and_then(|(workspace_id,)| workspace_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocation_preview_calculation() {
        let score = WalletRiskScore {
            address: "0x1234".to_string(),
            composite_score: 0.75,
            components: wallet_tracker::risk_scorer::RiskComponents {
                sortino_normalized: 0.8,
                consistency: 0.7,
                roi_drawdown_ratio: 0.6,
                win_rate: 0.65,
                volatility: 0.15,
            },
            recommended_allocation_pct: 15.0,
            current_allocation_pct: Some(10.0),
        };

        let preview = allocation_to_preview(&score);
        assert_eq!(preview.address, "0x1234");
        assert_eq!(preview.current_allocation_pct, Some(10.0));
        assert_eq!(preview.recommended_allocation_pct, 15.0);
        assert_eq!(preview.change_pct, 5.0);
    }

    #[test]
    fn test_allocation_preview_no_current() {
        let score = WalletRiskScore {
            address: "0x5678".to_string(),
            composite_score: 0.65,
            components: wallet_tracker::risk_scorer::RiskComponents {
                sortino_normalized: 0.7,
                consistency: 0.6,
                roi_drawdown_ratio: 0.5,
                win_rate: 0.6,
                volatility: 0.2,
            },
            recommended_allocation_pct: 12.0,
            current_allocation_pct: None,
        };

        let preview = allocation_to_preview(&score);
        assert_eq!(preview.current_allocation_pct, None);
        assert_eq!(preview.change_pct, 12.0);
    }
}
