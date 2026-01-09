//! Stop-loss management for automated position protection.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use polymarket_core::types::{BinaryMarketBook, ExecutionReport, MarketOrder, OrderSide};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use trading_engine::OrderExecutor;
use uuid::Uuid;

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
        Self::Fixed { trigger_price: price }
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
            StopType::Trailing { offset_pct, peak_price } => {
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
        if let StopType::Trailing { ref mut peak_price, .. } = self.stop_type {
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
            StopType::Trailing { offset_pct, peak_price } => {
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

/// Manager for stop-loss rules.
pub struct StopLossManager {
    /// Active stop-loss rules keyed by rule ID.
    rules: DashMap<Uuid, StopLossRule>,
    /// Rules indexed by position ID.
    rules_by_position: DashMap<Uuid, Vec<Uuid>>,
    /// Order executor for exit orders.
    executor: Arc<OrderExecutor>,
    /// Channel for triggered stop notifications.
    trigger_tx: mpsc::Sender<TriggeredStop>,
    /// Receiver for triggered stops (taken once).
    trigger_rx: Option<mpsc::Receiver<TriggeredStop>>,
}

impl StopLossManager {
    /// Create a new stop-loss manager.
    pub fn new(executor: Arc<OrderExecutor>) -> Self {
        let (trigger_tx, trigger_rx) = mpsc::channel(1000);
        Self {
            rules: DashMap::new(),
            rules_by_position: DashMap::new(),
            executor,
            trigger_tx,
            trigger_rx: Some(trigger_rx),
        }
    }

    /// Take the trigger receiver (can only be called once).
    pub fn take_trigger_receiver(&mut self) -> Option<mpsc::Receiver<TriggeredStop>> {
        self.trigger_rx.take()
    }

    /// Add a new stop-loss rule.
    pub fn add_rule(&self, mut rule: StopLossRule) {
        rule.activate();
        info!(
            rule_id = %rule.id,
            position_id = %rule.position_id,
            stop_type = ?rule.stop_type,
            "Adding stop-loss rule"
        );

        // Index by position
        self.rules_by_position
            .entry(rule.position_id)
            .or_default()
            .push(rule.id);

        self.rules.insert(rule.id, rule);
    }

    /// Remove a stop-loss rule.
    pub fn remove_rule(&self, rule_id: Uuid) -> Option<StopLossRule> {
        if let Some((_, rule)) = self.rules.remove(&rule_id) {
            // Remove from position index
            if let Some(mut rules) = self.rules_by_position.get_mut(&rule.position_id) {
                rules.retain(|&id| id != rule_id);
            }
            info!(rule_id = %rule_id, "Removed stop-loss rule");
            Some(rule)
        } else {
            None
        }
    }

    /// Remove all rules for a position.
    pub fn remove_rules_for_position(&self, position_id: Uuid) -> Vec<StopLossRule> {
        let mut removed = Vec::new();
        if let Some((_, rule_ids)) = self.rules_by_position.remove(&position_id) {
            for rule_id in rule_ids {
                if let Some((_, rule)) = self.rules.remove(&rule_id) {
                    removed.push(rule);
                }
            }
        }
        removed
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
    pub async fn check_triggers(
        &self,
        market_prices: &std::collections::HashMap<String, BinaryMarketBook>,
    ) -> Vec<TriggeredStop> {
        let mut triggered = Vec::new();

        for entry in self.rules.iter() {
            let rule = entry.value();
            if !rule.activated || rule.executed {
                continue;
            }

            // Get current price for this rule's market/outcome
            let current_price = match market_prices.get(&rule.market_id) {
                Some(book) => {
                    // Use best bid as exit price
                    if rule.outcome_id == book.yes_book.outcome_id {
                        book.yes_book.bids.first().map(|l| l.price)
                    } else {
                        book.no_book.bids.first().map(|l| l.price)
                    }
                }
                None => None,
            };

            let current_price = match current_price {
                Some(p) => p,
                None => continue,
            };

            // Update trailing stop peak
            if matches!(rule.stop_type, StopType::Trailing { .. }) {
                if let Some(mut rule_mut) = self.rules.get_mut(&rule.id) {
                    rule_mut.update_peak(current_price);
                }
            }

            // Check if triggered
            if rule.is_triggered(current_price) {
                let trigger_price = rule.current_trigger_price().unwrap_or(current_price);
                triggered.push(TriggeredStop {
                    rule: rule.clone(),
                    trigger_price,
                    current_price,
                });
            }
        }

        triggered
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

        let by_type = |is_type: fn(&StopType) -> bool| {
            rules.iter().filter(|r| is_type(&r.stop_type)).count()
        };

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
}
