//! Risk-based scoring and allocation system for wallet portfolios.
//!
//! This module calculates composite risk scores for wallets based on multiple metrics
//! and provides dynamic allocation recommendations with volatility scaling.

use anyhow::Result;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{debug, warn};

/// Wallet risk score and allocation recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletRiskScore {
    pub address: String,
    /// Composite risk score (0-1, higher is better).
    pub composite_score: f64,
    /// Individual component scores.
    pub components: RiskComponents,
    /// Recommended allocation percentage (0-100).
    pub recommended_allocation_pct: f64,
    /// Current allocation percentage (0-100).
    pub current_allocation_pct: Option<f64>,
}

/// Individual components of the risk score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskComponents {
    /// Sortino ratio (normalized 0-1).
    pub sortino_normalized: f64,
    /// Consistency score (0-1).
    pub consistency: f64,
    /// ROI/MaxDrawdown ratio (normalized 0-1).
    pub roi_drawdown_ratio: f64,
    /// Win rate (0-1).
    pub win_rate: f64,
    /// Volatility (raw value).
    pub volatility: f64,
}

/// Configuration for risk scoring.
#[derive(Debug, Clone)]
pub struct RiskScorerConfig {
    /// Weight for Sortino ratio (default 0.3).
    pub sortino_weight: f64,
    /// Weight for consistency score (default 0.25).
    pub consistency_weight: f64,
    /// Weight for ROI/MaxDD ratio (default 0.25).
    pub roi_drawdown_weight: f64,
    /// Weight for win rate (default 0.2).
    pub win_rate_weight: f64,
    /// Maximum allocation percentage per wallet (default 25%).
    pub max_allocation_pct: f64,
    /// Minimum allocation percentage per wallet (default 5%).
    pub min_allocation_pct: f64,
    /// Volatility scaling factor (default 1.0).
    pub volatility_scale: f64,
}

impl Default for RiskScorerConfig {
    fn default() -> Self {
        Self {
            sortino_weight: 0.3,
            consistency_weight: 0.25,
            roi_drawdown_weight: 0.25,
            win_rate_weight: 0.2,
            max_allocation_pct: 25.0,
            min_allocation_pct: 5.0,
            volatility_scale: 1.0,
        }
    }
}

/// Risk scorer for calculating composite scores and allocations.
pub struct RiskScorer {
    pool: PgPool,
    config: RiskScorerConfig,
}

/// Row structure for fetching wallet metrics from database.
#[derive(sqlx::FromRow)]
struct WalletMetricsRow {
    address: String,
    roi_30d: Decimal,
    sharpe_30d: Decimal,
    sortino_30d: Decimal,
    max_drawdown_30d: Decimal,
    volatility_30d: Decimal,
    consistency_score: Decimal,
    win_rate_30d: Decimal,
}

impl RiskScorer {
    /// Create a new risk scorer.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            config: RiskScorerConfig::default(),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(pool: PgPool, config: RiskScorerConfig) -> Self {
        Self { pool, config }
    }

    /// Calculate risk scores for all wallets with metrics.
    pub async fn calculate_scores(&self) -> Result<Vec<WalletRiskScore>> {
        let metrics = self.fetch_wallet_metrics().await?;

        if metrics.is_empty() {
            warn!("No wallet metrics found for risk scoring");
            return Ok(vec![]);
        }

        let mut scores: Vec<WalletRiskScore> = metrics
            .into_iter()
            .map(|m| self.calculate_wallet_score(m))
            .collect();

        // Sort by composite score descending
        scores.sort_by(|a, b| {
            b.composite_score
                .partial_cmp(&a.composite_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!(count = scores.len(), "Calculated risk scores for wallets");

        Ok(scores)
    }

    /// Calculate recommended allocations for a portfolio tier.
    pub async fn calculate_allocations(&self, tier: &str) -> Result<Vec<WalletRiskScore>> {
        // Get current allocations
        let current_allocations = self.fetch_current_allocations(tier).await?;

        // Calculate risk scores
        let mut scores = self.calculate_scores().await?;

        // Merge current allocations
        for score in &mut scores {
            score.current_allocation_pct = current_allocations.get(&score.address).copied();
        }

        // Apply volatility scaling and calculate recommended allocations
        let total_score: f64 = scores.iter().map(|s| s.composite_score).sum();

        if total_score == 0.0 {
            warn!("Total risk score is zero, cannot calculate allocations");
            return Ok(scores);
        }

        for score in &mut scores {
            // Base allocation from score proportion
            let base_allocation = (score.composite_score / total_score) * 100.0;

            // Apply volatility scaling (reduce allocation for high volatility)
            let volatility_factor = if score.components.volatility > 0.0 {
                1.0 / (1.0 + score.components.volatility * self.config.volatility_scale)
            } else {
                1.0
            };

            let scaled_allocation = base_allocation * volatility_factor;

            // Clamp to min/max bounds
            score.recommended_allocation_pct = scaled_allocation
                .max(self.config.min_allocation_pct)
                .min(self.config.max_allocation_pct);
        }

        // Normalize to sum to 100%
        let total_allocation: f64 = scores.iter().map(|s| s.recommended_allocation_pct).sum();

        if total_allocation > 0.0 {
            for score in &mut scores {
                score.recommended_allocation_pct =
                    (score.recommended_allocation_pct / total_allocation) * 100.0;
            }
        }

        debug!(
            tier = %tier,
            wallet_count = scores.len(),
            "Calculated risk-based allocations"
        );

        Ok(scores)
    }

    /// Apply allocations to the database.
    pub async fn apply_allocations(
        &self,
        tier: &str,
        allocations: &[WalletRiskScore],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for allocation in allocations {
            let decimal_pct =
                Decimal::from_f64(allocation.recommended_allocation_pct).unwrap_or_default();

            sqlx::query(
                r#"
                UPDATE workspace_wallet_allocations
                SET allocation_pct = $1,
                    updated_at = NOW()
                WHERE tier = $2
                  AND LOWER(wallet_address) = LOWER($3)
                "#,
            )
            .bind(decimal_pct)
            .bind(tier)
            .bind(&allocation.address)
            .execute(&mut *tx)
            .await?;

            debug!(
                address = %allocation.address,
                allocation_pct = %allocation.recommended_allocation_pct,
                "Applied risk-based allocation"
            );
        }

        tx.commit().await?;

        debug!(tier = %tier, "Applied all risk-based allocations");

        Ok(())
    }

    // Private methods

    async fn fetch_wallet_metrics(&self) -> Result<Vec<WalletMetricsRow>> {
        let rows = sqlx::query_as::<_, WalletMetricsRow>(
            r#"
            SELECT
                address,
                roi_30d,
                sharpe_30d,
                sortino_30d,
                max_drawdown_30d,
                volatility_30d,
                consistency_score,
                win_rate_30d
            FROM wallet_success_metrics
            WHERE last_computed >= NOW() - INTERVAL '48 hours'
              AND roi_30d > 0
              AND sortino_30d IS NOT NULL
            ORDER BY roi_30d DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    async fn fetch_current_allocations(
        &self,
        tier: &str,
    ) -> Result<std::collections::HashMap<String, f64>> {
        let rows: Vec<(String, Decimal)> = sqlx::query_as(
            r#"
            SELECT LOWER(wallet_address) as address, allocation_pct
            FROM workspace_wallet_allocations
            WHERE tier = $1
            "#,
        )
        .bind(tier)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(addr, pct)| (addr, pct.to_f64().unwrap_or(0.0)))
            .collect())
    }

    fn calculate_wallet_score(&self, metrics: WalletMetricsRow) -> WalletRiskScore {
        // Normalize Sortino (typical range 0-3, good values > 1)
        let sortino_raw = metrics.sortino_30d.to_f64().unwrap_or(0.0);
        let sortino_normalized = (sortino_raw / 3.0).min(1.0).max(0.0);

        // Consistency is already 0-1
        let consistency = metrics.consistency_score.to_f64().unwrap_or(0.0);

        // ROI/MaxDrawdown ratio (normalize typical range 0-2)
        let roi = metrics.roi_30d.to_f64().unwrap_or(0.0);
        let max_dd = metrics.max_drawdown_30d.to_f64().unwrap_or(0.01).max(0.01);
        let roi_drawdown_ratio = ((roi / max_dd) / 2.0).min(1.0).max(0.0);

        // Win rate is already 0-1
        let win_rate = metrics.win_rate_30d.to_f64().unwrap_or(0.0);

        // Volatility (raw, used for scaling)
        let volatility = metrics.volatility_30d.to_f64().unwrap_or(0.0);

        // Calculate composite score
        let composite_score = (sortino_normalized * self.config.sortino_weight)
            + (consistency * self.config.consistency_weight)
            + (roi_drawdown_ratio * self.config.roi_drawdown_weight)
            + (win_rate * self.config.win_rate_weight);

        WalletRiskScore {
            address: metrics.address,
            composite_score,
            components: RiskComponents {
                sortino_normalized,
                consistency,
                roi_drawdown_ratio,
                win_rate,
                volatility,
            },
            recommended_allocation_pct: 0.0, // Will be calculated later
            current_allocation_pct: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RiskScorerConfig::default();
        assert_eq!(config.sortino_weight, 0.3);
        assert_eq!(config.consistency_weight, 0.25);
        assert_eq!(config.roi_drawdown_weight, 0.25);
        assert_eq!(config.win_rate_weight, 0.2);

        // Weights should sum to 1.0
        let total_weight = config.sortino_weight
            + config.consistency_weight
            + config.roi_drawdown_weight
            + config.win_rate_weight;
        assert!((total_weight - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_risk_components_serialization() {
        let components = RiskComponents {
            sortino_normalized: 0.8,
            consistency: 0.75,
            roi_drawdown_ratio: 0.6,
            win_rate: 0.65,
            volatility: 0.15,
        };

        let json = serde_json::to_string(&components).unwrap();
        let deserialized: RiskComponents = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.sortino_normalized, 0.8);
        assert_eq!(deserialized.consistency, 0.75);
    }
}
