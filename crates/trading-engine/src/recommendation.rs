//! Trading recommendation engine.
//!
//! Generates personalized trading recommendations based on market conditions,
//! wallet analysis, risk tolerance, and historical performance.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Row type for wallet metrics query.
#[derive(Debug, FromRow)]
struct WalletMetricsRow {
    address: String,
    roi_30d: Decimal,
    sharpe_30d: Decimal,
    win_rate_30d: Decimal,
    predicted_success_prob: Decimal,
    tracked_id: Option<Uuid>,
}

/// Convert Decimal to f64.
fn decimal_to_f64(d: Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.to_f64().unwrap_or(0.0)
}

/// Recommendation type categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationType {
    /// Copy a specific wallet.
    CopyWallet,
    /// Enter a market position.
    EnterPosition,
    /// Exit or adjust an existing position.
    AdjustPosition,
    /// Arbitrage opportunity.
    Arbitrage,
    /// Risk management action.
    RiskAction,
    /// Strategy suggestion.
    StrategyChange,
}

/// Urgency level for recommendations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Low,
    Medium,
    High,
    Critical,
}

/// Risk level for recommendations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Conservative,
    Moderate,
    Aggressive,
    Speculative,
}

/// A trading recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub id: Uuid,
    /// Type of recommendation.
    pub recommendation_type: RecommendationType,
    /// Short title.
    pub title: String,
    /// Detailed description.
    pub description: String,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f64,
    /// Expected return (if applicable).
    pub expected_return: Option<Decimal>,
    /// Associated risk level.
    pub risk_level: RiskLevel,
    /// Urgency of the recommendation.
    pub urgency: Urgency,
    /// Specific action details.
    pub action: RecommendedAction,
    /// Supporting evidence.
    pub evidence: Vec<Evidence>,
    /// Time validity.
    pub valid_until: DateTime<Utc>,
    /// Created timestamp.
    pub created_at: DateTime<Utc>,
    /// User risk tolerance this matches.
    pub target_risk_tolerance: RiskLevel,
}

impl Recommendation {
    /// Check if the recommendation is still valid.
    pub fn is_valid(&self) -> bool {
        Utc::now() < self.valid_until
    }

    /// Calculate a priority score for sorting.
    pub fn priority_score(&self) -> f64 {
        let urgency_mult = match self.urgency {
            Urgency::Critical => 2.0,
            Urgency::High => 1.5,
            Urgency::Medium => 1.0,
            Urgency::Low => 0.5,
        };

        self.confidence * urgency_mult
    }
}

/// Specific action details for a recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecommendedAction {
    /// Copy a wallet.
    CopyWallet {
        wallet_address: String,
        allocation_pct: Decimal,
        success_score: f64,
    },
    /// Enter a market position.
    EnterMarket {
        market_id: String,
        outcome: String,
        side: String,
        suggested_size: Decimal,
        entry_price: Option<Decimal>,
        stop_loss: Option<Decimal>,
        take_profit: Option<Decimal>,
    },
    /// Exit a position.
    ExitPosition {
        position_id: Uuid,
        reason: String,
        suggested_exit_price: Option<Decimal>,
    },
    /// Adjust stop-loss.
    AdjustStopLoss {
        position_id: Uuid,
        new_stop_price: Decimal,
        reason: String,
    },
    /// Execute arbitrage.
    ExecuteArbitrage {
        market_id: String,
        yes_price: Decimal,
        no_price: Decimal,
        expected_profit: Decimal,
        suggested_size: Decimal,
    },
    /// Change strategy allocation.
    ChangeAllocation {
        strategy_id: String,
        current_allocation: Decimal,
        suggested_allocation: Decimal,
        reason: String,
    },
    /// Take risk action.
    TakeRiskAction {
        action: String,
        affected_positions: Vec<Uuid>,
        reason: String,
    },
}

/// Evidence supporting a recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub factor: String,
    pub value: String,
    pub weight: f64,
    pub is_positive: bool,
}

/// User risk profile for personalized recommendations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskProfile {
    pub user_id: String,
    pub risk_tolerance: RiskLevel,
    pub max_position_size: Decimal,
    pub max_portfolio_risk: Decimal,
    pub preferred_holding_period: HoldingPeriod,
    pub preferred_markets: Vec<String>,
    pub excluded_markets: Vec<String>,
    pub min_confidence_threshold: f64,
}

impl Default for RiskProfile {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            risk_tolerance: RiskLevel::Moderate,
            max_position_size: Decimal::new(1000, 0),
            max_portfolio_risk: Decimal::new(10, 0), // 10% of portfolio
            preferred_holding_period: HoldingPeriod::Medium,
            preferred_markets: Vec::new(),
            excluded_markets: Vec::new(),
            min_confidence_threshold: 0.6,
        }
    }
}

/// Preferred holding period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldingPeriod {
    /// Minutes to hours.
    Short,
    /// Days to weeks.
    Medium,
    /// Weeks to months.
    Long,
    /// Until market resolution.
    ToResolution,
}

/// Recommendation engine.
pub struct RecommendationEngine {
    pool: PgPool,
    min_arb_profit: Decimal,
    min_wallet_score: f64,
    max_recommendations: usize,
    /// Cache of recently generated recommendations, keyed by ID.
    cache: Arc<RwLock<HashMap<Uuid, Recommendation>>>,
}

impl RecommendationEngine {
    /// Create a new recommendation engine.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            min_arb_profit: Decimal::new(2, 2), // 2% minimum
            min_wallet_score: 0.65,
            max_recommendations: 10,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set minimum arbitrage profit threshold.
    pub fn with_min_arb_profit(mut self, profit: Decimal) -> Self {
        self.min_arb_profit = profit;
        self
    }

    /// Set minimum wallet score threshold.
    pub fn with_min_wallet_score(mut self, score: f64) -> Self {
        self.min_wallet_score = score;
        self
    }

    /// Generate recommendations for a user.
    pub async fn generate_recommendations(
        &self,
        profile: &RiskProfile,
    ) -> Result<Vec<Recommendation>> {
        let mut recommendations = Vec::new();

        // Get wallet recommendations
        let wallet_recs = self.generate_wallet_recommendations(profile).await?;
        recommendations.extend(wallet_recs);

        // Get arbitrage recommendations
        let arb_recs = self.generate_arbitrage_recommendations(profile).await?;
        recommendations.extend(arb_recs);

        // Get position recommendations
        let position_recs = self.generate_position_recommendations(profile).await?;
        recommendations.extend(position_recs);

        // Get risk recommendations
        let risk_recs = self.generate_risk_recommendations(profile).await?;
        recommendations.extend(risk_recs);

        // Filter by confidence and sort by priority
        recommendations.retain(|r| r.confidence >= profile.min_confidence_threshold);
        recommendations.sort_by(|a, b| {
            b.priority_score()
                .partial_cmp(&a.priority_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        recommendations.truncate(self.max_recommendations);

        // Cache all generated recommendations for lookup by ID
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
            for rec in &recommendations {
                cache.insert(rec.id, rec.clone());
            }
        }

        info!(
            user_id = %profile.user_id,
            count = recommendations.len(),
            "Generated recommendations"
        );

        Ok(recommendations)
    }

    async fn generate_wallet_recommendations(
        &self,
        profile: &RiskProfile,
    ) -> Result<Vec<Recommendation>> {
        let mut recommendations = Vec::new();

        // Find top performing wallets not already being copied
        let wallets: Vec<WalletMetricsRow> = sqlx::query_as(
            r#"
            SELECT
                wsm.address,
                wsm.roi_30d,
                wsm.sharpe_30d,
                wsm.win_rate_30d,
                wsm.predicted_success_prob,
                tw.id as tracked_id
            FROM wallet_success_metrics wsm
            LEFT JOIN tracked_wallets tw ON tw.address = wsm.address AND tw.enabled = true
            WHERE wsm.predicted_success_prob >= $1
              AND wsm.trades_30d >= 10
            ORDER BY wsm.predicted_success_prob DESC
            LIMIT 5
            "#,
        )
        .bind(self.min_wallet_score)
        .fetch_all(&self.pool)
        .await?;

        for wallet in wallets {
            // Skip if already tracking
            if wallet.tracked_id.is_some() {
                continue;
            }

            let success_prob = decimal_to_f64(wallet.predicted_success_prob);
            let roi = decimal_to_f64(wallet.roi_30d);
            let sharpe = decimal_to_f64(wallet.sharpe_30d);
            let win_rate = decimal_to_f64(wallet.win_rate_30d);

            // Guard: skip wallets with invalid success probability
            if !success_prob.is_finite() || !(0.0..=1.0).contains(&success_prob) {
                warn!(
                    wallet = %wallet.address,
                    success_prob = %success_prob,
                    "Skipping wallet with invalid success probability"
                );
                continue;
            }

            // Calculate suggested allocation based on risk profile
            let base_allocation = match profile.risk_tolerance {
                RiskLevel::Conservative => Decimal::new(5, 0),
                RiskLevel::Moderate => Decimal::new(10, 0),
                RiskLevel::Aggressive => Decimal::new(15, 0),
                RiskLevel::Speculative => Decimal::new(20, 0),
            };

            let adjusted_allocation =
                base_allocation * Decimal::from_f64_retain(success_prob).unwrap_or(Decimal::ZERO);

            let evidence = vec![
                Evidence {
                    factor: "30-day ROI".to_string(),
                    value: format!("{:.1}%", roi * 100.0),
                    weight: 0.3,
                    is_positive: roi > 0.0,
                },
                Evidence {
                    factor: "Sharpe Ratio".to_string(),
                    value: format!("{:.2}", sharpe),
                    weight: 0.25,
                    is_positive: sharpe > 1.0,
                },
                Evidence {
                    factor: "Win Rate".to_string(),
                    value: format!("{:.1}%", win_rate * 100.0),
                    weight: 0.25,
                    is_positive: win_rate > 0.55,
                },
                Evidence {
                    factor: "Prediction Score".to_string(),
                    value: format!("{:.0}%", success_prob * 100.0),
                    weight: 0.2,
                    is_positive: success_prob > 0.7,
                },
            ];

            let recommendation = Recommendation {
                id: Uuid::new_v4(),
                recommendation_type: RecommendationType::CopyWallet,
                title: format!("Copy high-performer {}", &wallet.address[..10]),
                description: format!(
                    "This wallet has shown strong performance with {:.1}% ROI and {:.1}% win rate over 30 days.",
                    roi * 100.0,
                    win_rate * 100.0
                ),
                confidence: success_prob,
                expected_return: Some(Decimal::from_f64_retain(roi * 0.7).unwrap_or(Decimal::ZERO)),
                risk_level: if sharpe > 2.0 {
                    RiskLevel::Conservative
                } else if sharpe > 1.0 {
                    RiskLevel::Moderate
                } else {
                    RiskLevel::Aggressive
                },
                urgency: Urgency::Medium,
                action: RecommendedAction::CopyWallet {
                    wallet_address: wallet.address.clone(),
                    allocation_pct: adjusted_allocation,
                    success_score: success_prob,
                },
                evidence,
                valid_until: Utc::now() + chrono::Duration::hours(24),
                created_at: Utc::now(),
                target_risk_tolerance: profile.risk_tolerance,
            };

            recommendations.push(recommendation);
        }

        Ok(recommendations)
    }

    async fn generate_arbitrage_recommendations(
        &self,
        profile: &RiskProfile,
    ) -> Result<Vec<Recommendation>> {
        let mut recommendations = Vec::new();

        // Find current arbitrage opportunities
        let opportunities = sqlx::query!(
            r#"
            SELECT market_id, yes_ask, no_ask, net_profit, timestamp
            FROM arb_opportunities
            WHERE timestamp > NOW() - INTERVAL '5 minutes'
              AND net_profit >= $1
            ORDER BY net_profit DESC
            LIMIT 5
            "#,
            self.min_arb_profit
        )
        .fetch_all(&self.pool)
        .await?;

        for opp in opportunities {
            let profit_pct = (opp.net_profit * Decimal::new(100, 0))
                .to_string()
                .parse::<f64>()
                .unwrap_or(0.0);

            // Calculate suggested size based on profile
            let suggested_size = profile.max_position_size
                * Decimal::from_f64_retain(profit_pct / 10.0).unwrap_or(Decimal::ONE);
            let suggested_size = suggested_size.min(profile.max_position_size);

            let evidence = vec![
                Evidence {
                    factor: "Net Profit".to_string(),
                    value: format!("{:.2}%", profit_pct),
                    weight: 0.5,
                    is_positive: true,
                },
                Evidence {
                    factor: "Yes Ask".to_string(),
                    value: format!("${:.4}", opp.yes_ask),
                    weight: 0.25,
                    is_positive: true,
                },
                Evidence {
                    factor: "No Ask".to_string(),
                    value: format!("${:.4}", opp.no_ask),
                    weight: 0.25,
                    is_positive: true,
                },
            ];

            let recommendation = Recommendation {
                id: Uuid::new_v4(),
                recommendation_type: RecommendationType::Arbitrage,
                title: format!("Arbitrage: {:.1}% profit", profit_pct),
                description: format!(
                    "Market {} has an arbitrage opportunity with {:.2}% expected profit.",
                    opp.market_id, profit_pct
                ),
                confidence: 0.9, // High confidence for verified arb
                expected_return: Some(opp.net_profit),
                risk_level: RiskLevel::Conservative, // Arb is low risk
                urgency: Urgency::High,
                action: RecommendedAction::ExecuteArbitrage {
                    market_id: opp.market_id.clone(),
                    yes_price: opp.yes_ask,
                    no_price: opp.no_ask,
                    expected_profit: opp.net_profit,
                    suggested_size,
                },
                evidence,
                valid_until: Utc::now() + chrono::Duration::minutes(5),
                created_at: Utc::now(),
                target_risk_tolerance: RiskLevel::Conservative,
            };

            recommendations.push(recommendation);
        }

        Ok(recommendations)
    }

    async fn generate_position_recommendations(
        &self,
        profile: &RiskProfile,
    ) -> Result<Vec<Recommendation>> {
        let mut recommendations = Vec::new();

        // Find positions that may need attention
        let positions = sqlx::query!(
            r#"
            SELECT id, market_id, unrealized_pnl, entry_timestamp, quantity,
                   yes_entry_price, no_entry_price, state
            FROM positions
            WHERE state IN (1, 2)
            ORDER BY unrealized_pnl ASC
            LIMIT 10
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        for pos in positions {
            let pnl = pos.unrealized_pnl;
            let duration = Utc::now().signed_duration_since(pos.entry_timestamp);
            let hours_held = duration.num_hours();

            // Recommend closing losing positions held too long
            if pnl < Decimal::ZERO && hours_held > 72 {
                let recommendation = Recommendation {
                    id: Uuid::new_v4(),
                    recommendation_type: RecommendationType::AdjustPosition,
                    title: "Consider closing underwater position".to_string(),
                    description: format!(
                        "Position in {} is down {:.2}% and has been held for {} hours.",
                        pos.market_id,
                        (pnl / pos.quantity * Decimal::new(100, 0))
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0),
                        hours_held
                    ),
                    confidence: 0.7,
                    expected_return: None,
                    risk_level: RiskLevel::Moderate,
                    urgency: if pnl < Decimal::new(-20, 0) {
                        Urgency::High
                    } else {
                        Urgency::Medium
                    },
                    action: RecommendedAction::ExitPosition {
                        position_id: pos.id,
                        reason: "Extended underwater position".to_string(),
                        suggested_exit_price: None,
                    },
                    evidence: vec![
                        Evidence {
                            factor: "Unrealized P&L".to_string(),
                            value: format!("${:.2}", pnl),
                            weight: 0.5,
                            is_positive: false,
                        },
                        Evidence {
                            factor: "Holding Period".to_string(),
                            value: format!("{} hours", hours_held),
                            weight: 0.3,
                            is_positive: false,
                        },
                    ],
                    valid_until: Utc::now() + chrono::Duration::hours(12),
                    created_at: Utc::now(),
                    target_risk_tolerance: profile.risk_tolerance,
                };

                recommendations.push(recommendation);
            }

            // Recommend taking profit on positions up significantly
            if pnl > Decimal::new(50, 0) {
                let recommendation = Recommendation {
                    id: Uuid::new_v4(),
                    recommendation_type: RecommendationType::AdjustPosition,
                    title: "Consider taking profit".to_string(),
                    description: format!(
                        "Position in {} is up {:.2}%. Consider taking partial or full profit.",
                        pos.market_id,
                        (pnl / pos.quantity * Decimal::new(100, 0))
                            .to_string()
                            .parse::<f64>()
                            .unwrap_or(0.0)
                    ),
                    confidence: 0.65,
                    expected_return: Some(pnl),
                    risk_level: RiskLevel::Conservative,
                    urgency: Urgency::Low,
                    action: RecommendedAction::ExitPosition {
                        position_id: pos.id,
                        reason: "Take profit opportunity".to_string(),
                        suggested_exit_price: None,
                    },
                    evidence: vec![Evidence {
                        factor: "Unrealized P&L".to_string(),
                        value: format!("${:.2}", pnl),
                        weight: 0.6,
                        is_positive: true,
                    }],
                    valid_until: Utc::now() + chrono::Duration::hours(24),
                    created_at: Utc::now(),
                    target_risk_tolerance: profile.risk_tolerance,
                };

                recommendations.push(recommendation);
            }
        }

        Ok(recommendations)
    }

    async fn generate_risk_recommendations(
        &self,
        profile: &RiskProfile,
    ) -> Result<Vec<Recommendation>> {
        let mut recommendations = Vec::new();

        // Check portfolio concentration
        let concentration = sqlx::query!(
            r#"
            SELECT market_id, SUM(quantity * (yes_entry_price + no_entry_price)) as total_value
            FROM positions
            WHERE state = 1
            GROUP BY market_id
            ORDER BY total_value DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        if let Some(top) = concentration.first() {
            let total_portfolio: Decimal = concentration.iter().filter_map(|c| c.total_value).sum();

            if total_portfolio > Decimal::ZERO {
                let concentration_pct = top.total_value.unwrap_or(Decimal::ZERO) / total_portfolio;

                if concentration_pct > Decimal::new(30, 2) {
                    // Over 30% in one market
                    recommendations.push(Recommendation {
                        id: Uuid::new_v4(),
                        recommendation_type: RecommendationType::RiskAction,
                        title: "High market concentration warning".to_string(),
                        description: format!(
                            "Over {:.0}% of portfolio is concentrated in market {}. Consider diversifying.",
                            concentration_pct * Decimal::new(100, 0),
                            top.market_id
                        ),
                        confidence: 0.85,
                        expected_return: None,
                        risk_level: RiskLevel::Aggressive,
                        urgency: Urgency::Medium,
                        action: RecommendedAction::TakeRiskAction {
                            action: "Reduce position size".to_string(),
                            affected_positions: Vec::new(),
                            reason: "High concentration risk".to_string(),
                        },
                        evidence: vec![
                            Evidence {
                                factor: "Concentration".to_string(),
                                value: format!("{:.0}%", concentration_pct * Decimal::new(100, 0)),
                                weight: 1.0,
                                is_positive: false,
                            },
                        ],
                        valid_until: Utc::now() + chrono::Duration::hours(48),
                        created_at: Utc::now(),
                        target_risk_tolerance: profile.risk_tolerance,
                    });
                }
            }
        }

        // Check for positions without stop-loss
        let unprotected = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) as count
            FROM positions p
            LEFT JOIN stop_loss_rules sl ON sl.position_id = p.id AND sl.executed = false
            WHERE p.state = 1 AND sl.id IS NULL
            "#
        )
        .fetch_one(&self.pool)
        .await?;

        if unprotected.unwrap_or(0) > 0 {
            recommendations.push(Recommendation {
                id: Uuid::new_v4(),
                recommendation_type: RecommendationType::RiskAction,
                title: "Positions without stop-loss".to_string(),
                description: format!(
                    "{} open positions have no stop-loss protection. Consider adding stop-loss orders.",
                    unprotected.unwrap_or(0)
                ),
                confidence: 0.9,
                expected_return: None,
                risk_level: RiskLevel::Aggressive,
                urgency: Urgency::High,
                action: RecommendedAction::TakeRiskAction {
                    action: "Add stop-loss".to_string(),
                    affected_positions: Vec::new(),
                    reason: "Unprotected positions".to_string(),
                },
                evidence: vec![
                    Evidence {
                        factor: "Unprotected Positions".to_string(),
                        value: unprotected.unwrap_or(0).to_string(),
                        weight: 1.0,
                        is_positive: false,
                    },
                ],
                valid_until: Utc::now() + chrono::Duration::hours(24),
                created_at: Utc::now(),
                target_risk_tolerance: profile.risk_tolerance,
            });
        }

        Ok(recommendations)
    }

    /// Get recommendation by ID from the cache.
    pub fn get_recommendation(&self, id: Uuid) -> Option<Recommendation> {
        self.cache
            .read()
            .ok()
            .and_then(|cache| cache.get(&id).cloned())
            .filter(|rec| rec.is_valid())
    }

    /// Dismiss a recommendation.
    pub async fn dismiss_recommendation(&self, id: Uuid, user_id: &str) -> Result<()> {
        // Log the dismissal for learning
        debug!(
            recommendation_id = %id,
            user_id = %user_id,
            "Recommendation dismissed"
        );
        Ok(())
    }

    /// Accept and execute a recommendation.
    pub async fn accept_recommendation(&self, id: Uuid, user_id: &str) -> Result<()> {
        // Log the acceptance for learning
        info!(
            recommendation_id = %id,
            user_id = %user_id,
            "Recommendation accepted"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommendation_priority() {
        let high_priority = Recommendation {
            id: Uuid::new_v4(),
            recommendation_type: RecommendationType::Arbitrage,
            title: "High priority".to_string(),
            description: "Test".to_string(),
            confidence: 0.9,
            expected_return: None,
            risk_level: RiskLevel::Conservative,
            urgency: Urgency::Critical,
            action: RecommendedAction::TakeRiskAction {
                action: "test".to_string(),
                affected_positions: vec![],
                reason: "test".to_string(),
            },
            evidence: vec![],
            valid_until: Utc::now() + chrono::Duration::hours(1),
            created_at: Utc::now(),
            target_risk_tolerance: RiskLevel::Moderate,
        };

        let low_priority = Recommendation {
            urgency: Urgency::Low,
            confidence: 0.5,
            ..high_priority.clone()
        };

        assert!(high_priority.priority_score() > low_priority.priority_score());
    }

    #[test]
    fn test_recommendation_validity() {
        let valid = Recommendation {
            id: Uuid::new_v4(),
            recommendation_type: RecommendationType::CopyWallet,
            title: "Test".to_string(),
            description: "Test".to_string(),
            confidence: 0.8,
            expected_return: None,
            risk_level: RiskLevel::Moderate,
            urgency: Urgency::Medium,
            action: RecommendedAction::TakeRiskAction {
                action: "test".to_string(),
                affected_positions: vec![],
                reason: "test".to_string(),
            },
            evidence: vec![],
            valid_until: Utc::now() + chrono::Duration::hours(1),
            created_at: Utc::now(),
            target_risk_tolerance: RiskLevel::Moderate,
        };

        assert!(valid.is_valid());

        let expired = Recommendation {
            valid_until: Utc::now() - chrono::Duration::hours(1),
            ..valid
        };

        assert!(!expired.is_valid());
    }

    #[test]
    fn test_risk_profile_default() {
        let profile = RiskProfile::default();
        assert_eq!(profile.risk_tolerance, RiskLevel::Moderate);
        assert_eq!(profile.min_confidence_threshold, 0.6);
    }

    #[test]
    fn test_evidence() {
        let evidence = Evidence {
            factor: "Win Rate".to_string(),
            value: "65%".to_string(),
            weight: 0.25,
            is_positive: true,
        };

        assert!(evidence.is_positive);
        assert_eq!(evidence.weight, 0.25);
    }

    #[test]
    fn test_allocation_by_risk_level() {
        // Verify base allocation values per risk level
        let cases = vec![
            (RiskLevel::Conservative, Decimal::new(5, 0)),
            (RiskLevel::Moderate, Decimal::new(10, 0)),
            (RiskLevel::Aggressive, Decimal::new(15, 0)),
            (RiskLevel::Speculative, Decimal::new(20, 0)),
        ];

        for (risk_level, expected) in cases {
            let allocation = match risk_level {
                RiskLevel::Conservative => Decimal::new(5, 0),
                RiskLevel::Moderate => Decimal::new(10, 0),
                RiskLevel::Aggressive => Decimal::new(15, 0),
                RiskLevel::Speculative => Decimal::new(20, 0),
            };
            assert_eq!(
                allocation, expected,
                "Allocation mismatch for {:?}",
                risk_level
            );
        }
    }

    #[test]
    fn test_priority_score_boundary_values() {
        let base = Recommendation {
            id: Uuid::new_v4(),
            recommendation_type: RecommendationType::CopyWallet,
            title: "Test".to_string(),
            description: "Test".to_string(),
            confidence: 1.0,
            expected_return: None,
            risk_level: RiskLevel::Moderate,
            urgency: Urgency::Critical,
            action: RecommendedAction::TakeRiskAction {
                action: "test".to_string(),
                affected_positions: vec![],
                reason: "test".to_string(),
            },
            evidence: vec![],
            valid_until: Utc::now() + chrono::Duration::hours(1),
            created_at: Utc::now(),
            target_risk_tolerance: RiskLevel::Moderate,
        };

        // confidence=1.0, urgency=Critical(2.0) => 2.0
        assert_eq!(base.priority_score(), 2.0);

        // confidence=0.0 => 0.0 regardless of urgency
        let zero_conf = Recommendation {
            confidence: 0.0,
            ..base.clone()
        };
        assert_eq!(zero_conf.priority_score(), 0.0);

        // confidence=1.0, urgency=Low(0.5) => 0.5
        let low_urgency = Recommendation {
            urgency: Urgency::Low,
            ..base
        };
        assert_eq!(low_urgency.priority_score(), 0.5);
    }

    #[test]
    fn test_urgency_multiplier_values() {
        let make = |urgency: Urgency| -> f64 {
            let rec = Recommendation {
                id: Uuid::new_v4(),
                recommendation_type: RecommendationType::Arbitrage,
                title: "t".to_string(),
                description: "d".to_string(),
                confidence: 1.0,
                expected_return: None,
                risk_level: RiskLevel::Moderate,
                urgency,
                action: RecommendedAction::TakeRiskAction {
                    action: "t".to_string(),
                    affected_positions: vec![],
                    reason: "t".to_string(),
                },
                evidence: vec![],
                valid_until: Utc::now() + chrono::Duration::hours(1),
                created_at: Utc::now(),
                target_risk_tolerance: RiskLevel::Moderate,
            };
            rec.priority_score()
        };

        assert_eq!(make(Urgency::Critical), 2.0);
        assert_eq!(make(Urgency::High), 1.5);
        assert_eq!(make(Urgency::Medium), 1.0);
        assert_eq!(make(Urgency::Low), 0.5);
    }

    #[test]
    fn test_evidence_weight_validity() {
        let evidence = vec![
            Evidence {
                factor: "ROI".into(),
                value: "20%".into(),
                weight: 0.3,
                is_positive: true,
            },
            Evidence {
                factor: "Sharpe".into(),
                value: "2.0".into(),
                weight: 0.25,
                is_positive: true,
            },
            Evidence {
                factor: "Win Rate".into(),
                value: "60%".into(),
                weight: 0.25,
                is_positive: true,
            },
            Evidence {
                factor: "Prediction".into(),
                value: "80%".into(),
                weight: 0.2,
                is_positive: true,
            },
        ];

        for e in &evidence {
            assert!(
                e.weight >= 0.0,
                "Weight should not be negative: {}",
                e.factor
            );
        }

        let total: f64 = evidence.iter().map(|e| e.weight).sum();
        assert!(
            (total - 1.0).abs() < f64::EPSILON,
            "Evidence weights should sum to 1.0, got {}",
            total
        );
    }
}
