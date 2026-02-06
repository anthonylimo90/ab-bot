//! Stop-loss management for automated position protection.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use polymarket_core::types::{BinaryMarketBook, ExecutionReport, MarketOrder, OrderSide};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;
use uuid::Uuid;

use crate::stop_loss_repo::StopLossRepository;

/// Type of stop-loss trigger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopType {
    /// Fixed price stop - triggers when price falls below threshold.
    Fixed { trigger_price: Decimal },
    /// Percentage-based stop - triggers on percentage loss from entry.
    Percentage { loss_pct: Decimal },
    /// Trailing stop - follows price up, triggers on pullback.
    Trailing {
        offset_pct: Decimal,
        peak_price: Decimal,
    },
    /// Time-based exit - triggers at a specific deadline.
    TimeBased { deadline: DateTime<Utc> },
}

impl StopType {
    /// Create a fixed price stop-loss.
    pub fn fixed(price: Decimal) -> Self {
        Self::Fixed {
            trigger_price: price,
        }
    }

    /// Create a percentage-based stop-loss.
    pub fn percentage(loss_pct: Decimal) -> Self {
        Self::Percentage { loss_pct }
    }

    /// Create a trailing stop-loss.
    pub fn trailing(offset_pct: Decimal) -> Self {
        Self::Trailing {
            offset_pct,
            peak_price: Decimal::ZERO,
        }
    }

    /// Create a time-based exit.
    pub fn time_based(deadline: DateTime<Utc>) -> Self {
        Self::TimeBased { deadline }
    }
}

/// A stop-loss rule attached to a position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopLossRule {
    pub id: Uuid,
    /// Position this stop-loss is attached to.
    pub position_id: Uuid,
    /// Market ID for the position.
    pub market_id: String,
    /// Which outcome to monitor (YES or NO token).
    pub outcome_id: String,
    /// Entry price of the position.
    pub entry_price: Decimal,
    /// Quantity held.
    pub quantity: Decimal,
    /// Type of stop-loss.
    pub stop_type: StopType,
    /// Whether the stop has been activated (monitoring).
    pub activated: bool,
    /// When the stop was activated.
    pub activated_at: Option<DateTime<Utc>>,
    /// Whether the stop has been executed.
    pub executed: bool,
    /// When the stop was executed.
    pub executed_at: Option<DateTime<Utc>>,
    /// When the rule was created.
    pub created_at: DateTime<Utc>,
}

impl StopLossRule {
    /// Create a new stop-loss rule.
    pub fn new(
        position_id: Uuid,
        market_id: String,
        outcome_id: String,
        entry_price: Decimal,
        quantity: Decimal,
        stop_type: StopType,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            position_id,
            market_id,
            outcome_id,
            entry_price,
            quantity,
            stop_type,
            activated: false,
            activated_at: None,
            executed: false,
            executed_at: None,
            created_at: Utc::now(),
        }
    }

    /// Activate the stop-loss (start monitoring).
    pub fn activate(&mut self) {
        if !self.activated {
            self.activated = true;
            self.activated_at = Some(Utc::now());
        }
    }

    /// Check if this stop-loss is triggered given current price.
    pub fn is_triggered(&self, current_price: Decimal) -> bool {
        if !self.activated || self.executed {
            return false;
        }

        match &self.stop_type {
            StopType::Fixed { trigger_price } => current_price <= *trigger_price,
            StopType::Percentage { loss_pct } => {
                let loss = (self.entry_price - current_price) / self.entry_price;
                loss >= *loss_pct
            }
            StopType::Trailing {
                offset_pct,
                peak_price,
            } => {
                if *peak_price <= Decimal::ZERO {
                    return false;
                }
                let trigger = *peak_price * (Decimal::ONE - *offset_pct);
                current_price <= trigger
            }
            StopType::TimeBased { deadline } => Utc::now() >= *deadline,
        }
    }

    /// Update trailing stop with new peak price.
    pub fn update_peak(&mut self, current_price: Decimal) {
        if let StopType::Trailing {
            ref mut peak_price, ..
        } = self.stop_type
        {
            if current_price > *peak_price {
                *peak_price = current_price;
                debug!(
                    rule_id = %self.id,
                    new_peak = %current_price,
                    "Updated trailing stop peak"
                );
            }
        }
    }

    /// Get the current trigger price (for display/logging).
    pub fn current_trigger_price(&self) -> Option<Decimal> {
        match &self.stop_type {
            StopType::Fixed { trigger_price } => Some(*trigger_price),
            StopType::Percentage { loss_pct } => {
                Some(self.entry_price * (Decimal::ONE - *loss_pct))
            }
            StopType::Trailing {
                offset_pct,
                peak_price,
            } => {
                if *peak_price > Decimal::ZERO {
                    Some(*peak_price * (Decimal::ONE - *offset_pct))
                } else {
                    None
                }
            }
            StopType::TimeBased { .. } => None,
        }
    }

    /// Mark the stop-loss as executed.
    pub fn mark_executed(&mut self) {
        self.executed = true;
        self.executed_at = Some(Utc::now());
    }
}

/// Result of a triggered stop-loss.
#[derive(Debug, Clone)]
pub struct TriggeredStop {
    pub rule: StopLossRule,
    pub trigger_price: Decimal,
    pub current_price: Decimal,
}

/// Reason why a stop-loss rule check was skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSkipReason {
    /// Market data not available.
    MarketDataMissing,
    /// No bids available for the outcome.
    NoBidsAvailable,
    /// Rule is not activated.
    RuleNotActive,
    /// Rule already executed.
    RuleAlreadyExecuted,
    /// Trailing stop has no peak set.
    TrailingNoPeak,
}

/// Result of checking a single stop-loss rule.
#[derive(Debug, Clone)]
pub struct RuleCheckResult {
    pub rule_id: Uuid,
    pub position_id: Uuid,
    pub market_id: String,
    pub outcome: RuleCheckOutcome,
}

/// Outcome of checking a stop-loss rule.
#[derive(Debug, Clone)]
pub enum RuleCheckOutcome {
    /// Rule was triggered.
    Triggered {
        trigger_price: Decimal,
        current_price: Decimal,
    },
    /// Rule was checked but not triggered.
    NotTriggered { current_price: Decimal },
    /// Rule check was skipped for a reason.
    Skipped { reason: CheckSkipReason },
}

/// Summary of check_triggers results.
#[derive(Debug, Clone, Default)]
pub struct CheckTriggersSummary {
    pub total_rules: usize,
    pub triggered: usize,
    pub not_triggered: usize,
    pub skipped_market_missing: usize,
    pub skipped_no_bids: usize,
    pub skipped_not_active: usize,
    pub skipped_already_executed: usize,
    pub skipped_trailing_no_peak: usize,
}

/// Manager for stop-loss rules.
pub struct StopLossManager {
    /// Active stop-loss rules keyed by rule ID.
    rules: DashMap<Uuid, StopLossRule>,
    /// Rules indexed by position ID.
    rules_by_position: DashMap<Uuid, Vec<Uuid>>,
    /// Order executor for exit orders.
    executor: Arc<OrderExecutor>,
    /// Database repository for persistence.
    repo: Option<StopLossRepository>,
    /// Channel for triggered stop notifications.
    trigger_tx: mpsc::Sender<TriggeredStop>,
    /// Receiver for triggered stops (taken once).
    trigger_rx: Option<mpsc::Receiver<TriggeredStop>>,
}

impl StopLossManager {
    /// Create a new stop-loss manager without database persistence.
    pub fn new(executor: Arc<OrderExecutor>) -> Self {
        let (trigger_tx, trigger_rx) = mpsc::channel(1000);
        Self {
            rules: DashMap::new(),
            rules_by_position: DashMap::new(),
            executor,
            repo: None,
            trigger_tx,
            trigger_rx: Some(trigger_rx),
        }
    }

    /// Create a new stop-loss manager with database persistence.
    pub fn with_persistence(executor: Arc<OrderExecutor>, pool: PgPool) -> Self {
        let (trigger_tx, trigger_rx) = mpsc::channel(1000);
        Self {
            rules: DashMap::new(),
            rules_by_position: DashMap::new(),
            executor,
            repo: Some(StopLossRepository::new(pool)),
            trigger_tx,
            trigger_rx: Some(trigger_rx),
        }
    }

    /// Load active rules from database on startup.
    /// This should be called once during initialization.
    pub async fn load_active_rules(&self) -> Result<usize> {
        let repo = match &self.repo {
            Some(r) => r,
            None => {
                warn!("Cannot load rules: no database connection configured");
                return Ok(0);
            }
        };

        let rules = repo.get_active().await?;
        let count = rules.len();

        for rule in rules {
            // Index by position
            self.rules_by_position
                .entry(rule.position_id)
                .or_default()
                .push(rule.id);

            self.rules.insert(rule.id, rule);
        }

        info!(
            count = count,
            "Recovered active stop-loss rules from database"
        );
        Ok(count)
    }

    /// Take the trigger receiver (can only be called once).
    pub fn take_trigger_receiver(&mut self) -> Option<mpsc::Receiver<TriggeredStop>> {
        self.trigger_rx.take()
    }

    /// Add a new stop-loss rule.
    pub async fn add_rule(&self, mut rule: StopLossRule) -> Result<()> {
        rule.activate();
        info!(
            rule_id = %rule.id,
            position_id = %rule.position_id,
            stop_type = ?rule.stop_type,
            "Adding stop-loss rule"
        );

        // Persist to database if configured
        if let Some(repo) = &self.repo {
            repo.insert(&rule).await?;
        }

        // Index by position
        self.rules_by_position
            .entry(rule.position_id)
            .or_default()
            .push(rule.id);

        self.rules.insert(rule.id, rule);
        Ok(())
    }

    /// Remove a stop-loss rule.
    pub async fn remove_rule(&self, rule_id: Uuid) -> Result<Option<StopLossRule>> {
        if let Some((_, rule)) = self.rules.remove(&rule_id) {
            // Remove from position index
            if let Some(mut rules) = self.rules_by_position.get_mut(&rule.position_id) {
                rules.retain(|&id| id != rule_id);
            }

            // Delete from database if configured
            if let Some(repo) = &self.repo {
                repo.delete(rule_id).await?;
            }

            info!(rule_id = %rule_id, "Removed stop-loss rule");
            Ok(Some(rule))
        } else {
            Ok(None)
        }
    }

    /// Remove all rules for a position.
    pub async fn remove_rules_for_position(&self, position_id: Uuid) -> Result<Vec<StopLossRule>> {
        let mut removed = Vec::new();
        if let Some((_, rule_ids)) = self.rules_by_position.remove(&position_id) {
            for rule_id in rule_ids {
                if let Some((_, rule)) = self.rules.remove(&rule_id) {
                    removed.push(rule);
                }
            }
        }

        // Delete from database if configured
        if let Some(repo) = &self.repo {
            repo.delete_by_position(position_id).await?;
        }

        Ok(removed)
    }

    /// Get a rule by ID.
    pub fn get_rule(&self, rule_id: Uuid) -> Option<StopLossRule> {
        self.rules.get(&rule_id).map(|r| r.clone())
    }

    /// Get all rules for a position.
    pub fn rules_for_position(&self, position_id: Uuid) -> Vec<StopLossRule> {
        self.rules_by_position
            .get(&position_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.rules.get(id).map(|r| r.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all active (non-executed) rules.
    pub fn active_rules(&self) -> Vec<StopLossRule> {
        self.rules
            .iter()
            .filter(|e| e.value().activated && !e.value().executed)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Check all rules against current market prices.
    /// Returns triggered stops for backward compatibility.
    pub async fn check_triggers(
        &self,
        market_prices: &std::collections::HashMap<String, BinaryMarketBook>,
    ) -> Vec<TriggeredStop> {
        let (triggered, _) = self.check_triggers_detailed(market_prices).await;
        triggered
    }

    /// Check all rules against current market prices with detailed results.
    /// Returns both triggered stops and a summary of all check outcomes.
    pub async fn check_triggers_detailed(
        &self,
        market_prices: &std::collections::HashMap<String, BinaryMarketBook>,
    ) -> (Vec<TriggeredStop>, CheckTriggersSummary) {
        let mut triggered = Vec::new();
        let mut rules_to_update = Vec::new();
        let mut summary = CheckTriggersSummary::default();

        for entry in self.rules.iter() {
            let rule = entry.value();
            summary.total_rules += 1;

            // Check if rule is in a checkable state
            if !rule.activated {
                debug!(
                    rule_id = %rule.id,
                    position_id = %rule.position_id,
                    "Skipping rule check: not activated"
                );
                summary.skipped_not_active += 1;
                continue;
            }

            if rule.executed {
                debug!(
                    rule_id = %rule.id,
                    position_id = %rule.position_id,
                    "Skipping rule check: already executed"
                );
                summary.skipped_already_executed += 1;
                continue;
            }

            // Get market data
            let book = match market_prices.get(&rule.market_id) {
                Some(b) => b,
                None => {
                    warn!(
                        rule_id = %rule.id,
                        position_id = %rule.position_id,
                        market_id = %rule.market_id,
                        "Cannot check stop-loss: market data missing"
                    );
                    summary.skipped_market_missing += 1;
                    continue;
                }
            };

            // Get current price for this rule's outcome
            let current_price = if rule.outcome_id == book.yes_book.outcome_id {
                book.yes_book.bids.first().map(|l| l.price)
            } else if rule.outcome_id == book.no_book.outcome_id {
                book.no_book.bids.first().map(|l| l.price)
            } else {
                warn!(
                    rule_id = %rule.id,
                    position_id = %rule.position_id,
                    market_id = %rule.market_id,
                    outcome_id = %rule.outcome_id,
                    "Cannot check stop-loss: outcome ID doesn't match market"
                );
                summary.skipped_market_missing += 1;
                continue;
            };

            let current_price = match current_price {
                Some(p) => p,
                None => {
                    warn!(
                        rule_id = %rule.id,
                        position_id = %rule.position_id,
                        market_id = %rule.market_id,
                        outcome_id = %rule.outcome_id,
                        "Cannot check stop-loss: no bids available for outcome"
                    );
                    summary.skipped_no_bids += 1;
                    continue;
                }
            };

            // Update trailing stop peak
            if let StopType::Trailing { peak_price, .. } = &rule.stop_type {
                // Check if trailing stop has a valid peak
                if *peak_price <= Decimal::ZERO && current_price <= rule.entry_price {
                    // First update - set peak to current price or entry price
                    if let Some(mut rule_mut) = self.rules.get_mut(&rule.id) {
                        rule_mut.update_peak(current_price.max(rule.entry_price));
                        rules_to_update.push(rule_mut.clone());
                    }
                }

                if let Some(mut rule_mut) = self.rules.get_mut(&rule.id) {
                    let old_peak = match &rule_mut.stop_type {
                        StopType::Trailing { peak_price, .. } => *peak_price,
                        _ => Decimal::ZERO,
                    };
                    rule_mut.update_peak(current_price);
                    // Track if peak was updated for persistence
                    let new_peak = match &rule_mut.stop_type {
                        StopType::Trailing { peak_price, .. } => *peak_price,
                        _ => Decimal::ZERO,
                    };
                    if new_peak > old_peak {
                        rules_to_update.push(rule_mut.clone());
                    }
                }
            }

            // Check if triggered
            if rule.is_triggered(current_price) {
                let trigger_price = rule.current_trigger_price().unwrap_or(current_price);
                info!(
                    rule_id = %rule.id,
                    position_id = %rule.position_id,
                    market_id = %rule.market_id,
                    trigger_price = %trigger_price,
                    current_price = %current_price,
                    stop_type = ?rule.stop_type,
                    "Stop-loss triggered"
                );
                triggered.push(TriggeredStop {
                    rule: rule.clone(),
                    trigger_price,
                    current_price,
                });
                summary.triggered += 1;
            } else {
                debug!(
                    rule_id = %rule.id,
                    position_id = %rule.position_id,
                    current_price = %current_price,
                    trigger_price = ?rule.current_trigger_price(),
                    "Stop-loss not triggered"
                );
                summary.not_triggered += 1;
            }
        }

        // Persist trailing stop peak updates to database (batched — only rules with new peaks)
        if let Some(repo) = &self.repo {
            if !rules_to_update.is_empty() {
                debug!(
                    count = rules_to_update.len(),
                    "Persisting trailing stop peak updates"
                );
                for rule in rules_to_update {
                    if let Err(e) = repo.update(&rule).await {
                        error!(rule_id = %rule.id, error = %e, "Failed to persist trailing stop peak update");
                    }
                }
            }
        }

        // Log summary if there were any issues
        let total_skipped = summary.skipped_market_missing
            + summary.skipped_no_bids
            + summary.skipped_not_active
            + summary.skipped_already_executed
            + summary.skipped_trailing_no_peak;

        if total_skipped > 0 {
            warn!(
                total = summary.total_rules,
                triggered = summary.triggered,
                not_triggered = summary.not_triggered,
                skipped_market_missing = summary.skipped_market_missing,
                skipped_no_bids = summary.skipped_no_bids,
                skipped_not_active = summary.skipped_not_active,
                skipped_already_executed = summary.skipped_already_executed,
                "Stop-loss check completed with skipped rules"
            );
        } else if summary.total_rules > 0 {
            debug!(
                total = summary.total_rules,
                triggered = summary.triggered,
                not_triggered = summary.not_triggered,
                "Stop-loss check completed"
            );
        }

        (triggered, summary)
    }

    /// Get all rules that couldn't be checked due to missing market data.
    pub fn rules_missing_market_data(
        &self,
        market_prices: &std::collections::HashMap<String, BinaryMarketBook>,
    ) -> Vec<StopLossRule> {
        self.rules
            .iter()
            .filter(|entry| {
                let rule = entry.value();
                if !rule.activated || rule.executed {
                    return false;
                }
                !market_prices.contains_key(&rule.market_id)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Execute a triggered stop-loss.
    pub async fn execute_stop(&self, stop: &TriggeredStop) -> Result<ExecutionReport> {
        info!(
            rule_id = %stop.rule.id,
            position_id = %stop.rule.position_id,
            trigger_price = %stop.trigger_price,
            current_price = %stop.current_price,
            "Executing stop-loss"
        );

        // Create sell order
        let order = MarketOrder::new(
            stop.rule.market_id.clone(),
            stop.rule.outcome_id.clone(),
            OrderSide::Sell,
            stop.rule.quantity,
        );

        let report = self.executor.execute_market_order(order).await?;

        // Mark rule as executed
        if let Some(mut rule) = self.rules.get_mut(&stop.rule.id) {
            rule.mark_executed();

            // Persist executed state to database
            if let Some(repo) = &self.repo {
                if let Err(e) = repo.update(&rule).await {
                    error!(rule_id = %rule.id, error = %e, "Failed to persist stop-loss execution");
                }
            }
        }

        // Send notification
        if self.trigger_tx.send(stop.clone()).await.is_err() {
            warn!("No receiver for stop-loss trigger notification");
        }

        Ok(report)
    }

    /// Manually exit a position immediately.
    pub async fn manual_exit(&self, position_id: Uuid) -> Result<Vec<ExecutionReport>> {
        info!(position_id = %position_id, "Executing manual exit");

        let rules = self.rules_for_position(position_id);
        let mut reports = Vec::new();

        for rule in rules {
            if rule.executed {
                continue;
            }

            let order = MarketOrder::new(
                rule.market_id.clone(),
                rule.outcome_id.clone(),
                OrderSide::Sell,
                rule.quantity,
            );

            match self.executor.execute_market_order(order).await {
                Ok(report) => {
                    if let Some(mut r) = self.rules.get_mut(&rule.id) {
                        r.mark_executed();

                        // Persist executed state to database
                        if let Some(repo) = &self.repo {
                            if let Err(e) = repo.update(&r).await {
                                error!(rule_id = %r.id, error = %e, "Failed to persist manual exit execution");
                            }
                        }
                    }
                    reports.push(report);
                }
                Err(e) => {
                    error!(rule_id = %rule.id, error = %e, "Failed to execute manual exit");
                }
            }
        }

        Ok(reports)
    }

    /// Get summary statistics.
    pub fn stats(&self) -> StopLossStats {
        let rules: Vec<_> = self.rules.iter().map(|e| e.value().clone()).collect();
        let active = rules.iter().filter(|r| r.activated && !r.executed).count();
        let executed = rules.iter().filter(|r| r.executed).count();

        let by_type =
            |is_type: fn(&StopType) -> bool| rules.iter().filter(|r| is_type(&r.stop_type)).count();

        StopLossStats {
            total_rules: rules.len(),
            active_rules: active,
            executed_rules: executed,
            fixed_stops: by_type(|t| matches!(t, StopType::Fixed { .. })),
            percentage_stops: by_type(|t| matches!(t, StopType::Percentage { .. })),
            trailing_stops: by_type(|t| matches!(t, StopType::Trailing { .. })),
            time_based_stops: by_type(|t| matches!(t, StopType::TimeBased { .. })),
        }
    }
}

/// Summary statistics for stop-loss manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopLossStats {
    pub total_rules: usize,
    pub active_rules: usize,
    pub executed_rules: usize,
    pub fixed_stops: usize,
    pub percentage_stops: usize,
    pub trailing_stops: usize,
    pub time_based_stops: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_rule(stop_type: StopType) -> StopLossRule {
        StopLossRule::new(
            Uuid::new_v4(),
            "market1".to_string(),
            "yes_token".to_string(),
            Decimal::new(50, 2), // 0.50 entry
            Decimal::new(100, 0),
            stop_type,
        )
    }

    #[test]
    fn test_fixed_stop_trigger() {
        let mut rule = create_test_rule(StopType::fixed(Decimal::new(40, 2))); // 0.40 trigger
        rule.activate();

        // Above trigger - not triggered
        assert!(!rule.is_triggered(Decimal::new(45, 2)));

        // At trigger - triggered
        assert!(rule.is_triggered(Decimal::new(40, 2)));

        // Below trigger - triggered
        assert!(rule.is_triggered(Decimal::new(35, 2)));
    }

    #[test]
    fn test_percentage_stop_trigger() {
        let mut rule = create_test_rule(StopType::percentage(Decimal::new(20, 2))); // 20% loss
        rule.activate();

        // Entry: 0.50, 20% loss trigger at 0.40
        assert!(!rule.is_triggered(Decimal::new(45, 2))); // 10% loss
        assert!(rule.is_triggered(Decimal::new(40, 2))); // 20% loss
        assert!(rule.is_triggered(Decimal::new(35, 2))); // 30% loss
    }

    #[test]
    fn test_trailing_stop_trigger() {
        let mut rule = create_test_rule(StopType::trailing(Decimal::new(10, 2))); // 10% trailing
        rule.activate();

        // Update peak to 0.60
        rule.update_peak(Decimal::new(60, 2));

        // Trigger at 0.54 (0.60 - 10%)
        assert!(!rule.is_triggered(Decimal::new(58, 2))); // Above trigger
        assert!(rule.is_triggered(Decimal::new(54, 2))); // At trigger
        assert!(rule.is_triggered(Decimal::new(50, 2))); // Below trigger

        // Update peak higher
        rule.update_peak(Decimal::new(70, 2));
        // New trigger at 0.63
        assert!(!rule.is_triggered(Decimal::new(65, 2))); // Above new trigger
        assert!(rule.is_triggered(Decimal::new(63, 2))); // At new trigger
    }

    #[test]
    fn test_time_based_stop_trigger() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let mut rule = create_test_rule(StopType::time_based(past));
        rule.activate();

        // Past deadline - should trigger regardless of price
        assert!(rule.is_triggered(Decimal::new(100, 2)));

        let future = Utc::now() + chrono::Duration::hours(1);
        let mut rule2 = create_test_rule(StopType::time_based(future));
        rule2.activate();

        // Future deadline - should not trigger
        assert!(!rule2.is_triggered(Decimal::new(1, 2)));
    }

    #[test]
    fn test_current_trigger_price() {
        let rule1 = create_test_rule(StopType::fixed(Decimal::new(40, 2)));
        assert_eq!(rule1.current_trigger_price(), Some(Decimal::new(40, 2)));

        let rule2 = create_test_rule(StopType::percentage(Decimal::new(20, 2)));
        // Entry 0.50, 20% loss = 0.40 trigger
        assert_eq!(rule2.current_trigger_price(), Some(Decimal::new(40, 2)));

        let mut rule3 = create_test_rule(StopType::trailing(Decimal::new(10, 2)));
        rule3.update_peak(Decimal::new(60, 2));
        // Peak 0.60, 10% trailing = 0.54 trigger
        assert_eq!(rule3.current_trigger_price(), Some(Decimal::new(54, 2)));
    }

    #[test]
    fn test_inactive_rule_not_triggered() {
        let rule = create_test_rule(StopType::fixed(Decimal::new(40, 2)));
        // Not activated - should never trigger
        assert!(!rule.is_triggered(Decimal::new(1, 2)));
    }

    #[test]
    fn test_executed_rule_not_triggered() {
        let mut rule = create_test_rule(StopType::fixed(Decimal::new(40, 2)));
        rule.activate();
        rule.mark_executed();

        // Already executed - should never trigger again
        assert!(!rule.is_triggered(Decimal::new(1, 2)));
    }

    #[test]
    fn test_check_triggers_summary_default() {
        let summary = super::CheckTriggersSummary::default();
        assert_eq!(summary.total_rules, 0);
        assert_eq!(summary.triggered, 0);
        assert_eq!(summary.not_triggered, 0);
        assert_eq!(summary.skipped_market_missing, 0);
        assert_eq!(summary.skipped_no_bids, 0);
        assert_eq!(summary.skipped_not_active, 0);
        assert_eq!(summary.skipped_already_executed, 0);
    }

    #[test]
    fn test_check_skip_reason_serialization() {
        use super::CheckSkipReason;

        let reason = CheckSkipReason::MarketDataMissing;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"market_data_missing\"");

        let reason2 = CheckSkipReason::NoBidsAvailable;
        let json2 = serde_json::to_string(&reason2).unwrap();
        assert_eq!(json2, "\"no_bids_available\"");
    }
}
