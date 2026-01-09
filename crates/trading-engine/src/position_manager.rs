//! Position management for tracking and sizing trades across strategies.

use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use polymarket_core::types::{Position, PositionState};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Source/strategy that originated a position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSource {
    /// Manual entry.
    Manual,
    /// Arbitrage detection.
    Arbitrage,
    /// Copied from another wallet.
    CopyTrade { source_wallet: String },
    /// Signal from recommendation engine.
    Recommendation { signal_id: Uuid },
}

/// Extended position with management metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedPosition {
    /// The underlying position.
    pub position: Position,
    /// Source/strategy that created this position.
    pub source: PositionSource,
    /// Stop-loss rule ID if any.
    pub stop_loss_id: Option<Uuid>,
    /// Tags for filtering/grouping.
    pub tags: Vec<String>,
    /// Notes/comments.
    pub notes: Option<String>,
    /// Last updated timestamp.
    pub updated_at: DateTime<Utc>,
}

impl ManagedPosition {
    pub fn new(position: Position, source: PositionSource) -> Self {
        Self {
            position,
            source,
            stop_loss_id: None,
            tags: Vec::new(),
            notes: None,
            updated_at: Utc::now(),
        }
    }

    pub fn with_stop_loss(mut self, stop_loss_id: Uuid) -> Self {
        self.stop_loss_id = Some(stop_loss_id);
        self
    }

    pub fn add_tag(&mut self, tag: impl Into<String>) {
        self.tags.push(tag.into());
    }

    pub fn set_notes(&mut self, notes: impl Into<String>) {
        self.notes = Some(notes.into());
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// Configuration for position limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionLimits {
    /// Maximum total positions.
    pub max_total_positions: usize,
    /// Maximum positions per market.
    pub max_per_market: usize,
    /// Maximum total exposure (sum of all position costs).
    pub max_total_exposure: Decimal,
    /// Maximum exposure per market.
    pub max_market_exposure: Decimal,
    /// Maximum single position size.
    pub max_position_size: Decimal,
}

impl Default for PositionLimits {
    fn default() -> Self {
        Self {
            max_total_positions: 100,
            max_per_market: 5,
            max_total_exposure: Decimal::new(100000, 0),
            max_market_exposure: Decimal::new(10000, 0),
            max_position_size: Decimal::new(1000, 0),
        }
    }
}

/// Position manager for tracking all positions across strategies.
pub struct PositionManager {
    /// Active positions keyed by position ID.
    positions: DashMap<Uuid, ManagedPosition>,
    /// Position limits.
    limits: Arc<RwLock<PositionLimits>>,
    /// Total unrealized P&L.
    total_unrealized_pnl: Arc<RwLock<Decimal>>,
    /// Total realized P&L.
    total_realized_pnl: Arc<RwLock<Decimal>>,
}

impl PositionManager {
    /// Create a new position manager.
    pub fn new(limits: PositionLimits) -> Self {
        Self {
            positions: DashMap::new(),
            limits: Arc::new(RwLock::new(limits)),
            total_unrealized_pnl: Arc::new(RwLock::new(Decimal::ZERO)),
            total_realized_pnl: Arc::new(RwLock::new(Decimal::ZERO)),
        }
    }

    /// Add a new position.
    pub async fn add_position(&self, position: ManagedPosition) -> Result<()> {
        let limits = self.limits.read().await;

        // Check limits
        if self.positions.len() >= limits.max_total_positions {
            anyhow::bail!("Maximum total positions ({}) reached", limits.max_total_positions);
        }

        let market_positions = self.positions_for_market(&position.position.market_id);
        if market_positions.len() >= limits.max_per_market {
            anyhow::bail!(
                "Maximum positions per market ({}) reached for {}",
                limits.max_per_market,
                position.position.market_id
            );
        }

        if position.position.entry_cost() > limits.max_position_size {
            anyhow::bail!(
                "Position size {} exceeds maximum {}",
                position.position.entry_cost(),
                limits.max_position_size
            );
        }

        let total_exposure = self.total_exposure();
        if total_exposure + position.position.entry_cost() > limits.max_total_exposure {
            anyhow::bail!(
                "Total exposure would exceed maximum {} (current: {}, new: {})",
                limits.max_total_exposure,
                total_exposure,
                position.position.entry_cost()
            );
        }

        let market_exposure = self.market_exposure(&position.position.market_id);
        if market_exposure + position.position.entry_cost() > limits.max_market_exposure {
            anyhow::bail!(
                "Market exposure would exceed maximum {} for {} (current: {}, new: {})",
                limits.max_market_exposure,
                position.position.market_id,
                market_exposure,
                position.position.entry_cost()
            );
        }

        drop(limits);

        info!(
            position_id = %position.position.id,
            market = %position.position.market_id,
            source = ?position.source,
            cost = %position.position.entry_cost(),
            "Adding managed position"
        );

        self.positions.insert(position.position.id, position);
        Ok(())
    }

    /// Get a position by ID.
    pub fn get_position(&self, id: Uuid) -> Option<ManagedPosition> {
        self.positions.get(&id).map(|p| p.clone())
    }

    /// Get all positions.
    pub fn all_positions(&self) -> Vec<ManagedPosition> {
        self.positions.iter().map(|e| e.value().clone()).collect()
    }

    /// Get active (non-closed) positions.
    pub fn active_positions(&self) -> Vec<ManagedPosition> {
        self.positions
            .iter()
            .filter(|e| e.value().position.is_active())
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get positions for a specific market.
    pub fn positions_for_market(&self, market_id: &str) -> Vec<ManagedPosition> {
        self.positions
            .iter()
            .filter(|e| e.value().position.market_id == market_id)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get positions by source.
    pub fn positions_by_source(&self, source: &PositionSource) -> Vec<ManagedPosition> {
        self.positions
            .iter()
            .filter(|e| &e.value().source == source)
            .map(|e| e.value().clone())
            .collect()
    }

    /// Get positions with a specific tag.
    pub fn positions_with_tag(&self, tag: &str) -> Vec<ManagedPosition> {
        self.positions
            .iter()
            .filter(|e| e.value().tags.contains(&tag.to_string()))
            .map(|e| e.value().clone())
            .collect()
    }

    /// Update a position.
    pub fn update_position(&self, id: Uuid, f: impl FnOnce(&mut ManagedPosition)) -> bool {
        if let Some(mut entry) = self.positions.get_mut(&id) {
            f(entry.value_mut());
            entry.value_mut().touch();
            true
        } else {
            false
        }
    }

    /// Remove a position.
    pub fn remove_position(&self, id: Uuid) -> Option<ManagedPosition> {
        self.positions.remove(&id).map(|(_, p)| p)
    }

    /// Close a position and update P&L.
    pub async fn close_position(&self, id: Uuid, realized_pnl: Decimal) -> Result<()> {
        if let Some(mut entry) = self.positions.get_mut(&id) {
            entry.position.state = PositionState::Closed;
            entry.position.realized_pnl = Some(realized_pnl);
            entry.position.exit_timestamp = Some(Utc::now());
            entry.touch();

            let mut total = self.total_realized_pnl.write().await;
            *total += realized_pnl;

            info!(
                position_id = %id,
                realized_pnl = %realized_pnl,
                "Position closed"
            );
            Ok(())
        } else {
            anyhow::bail!("Position {} not found", id)
        }
    }

    /// Calculate total exposure across all active positions.
    pub fn total_exposure(&self) -> Decimal {
        self.active_positions()
            .iter()
            .map(|p| p.position.entry_cost())
            .sum()
    }

    /// Calculate exposure for a specific market.
    pub fn market_exposure(&self, market_id: &str) -> Decimal {
        self.positions_for_market(market_id)
            .iter()
            .filter(|p| p.position.is_active())
            .map(|p| p.position.entry_cost())
            .sum()
    }

    /// Update position limits.
    pub async fn update_limits(&self, limits: PositionLimits) {
        let mut current = self.limits.write().await;
        *current = limits;
        info!("Position limits updated");
    }

    /// Get current limits.
    pub async fn get_limits(&self) -> PositionLimits {
        self.limits.read().await.clone()
    }

    /// Get summary statistics.
    pub async fn stats(&self) -> PositionManagerStats {
        let positions = self.all_positions();
        let active: Vec<_> = positions.iter().filter(|p| p.position.is_active()).collect();

        let total_unrealized: Decimal = active.iter().map(|p| p.position.unrealized_pnl).sum();
        let total_realized = *self.total_realized_pnl.read().await;

        let by_source = |source: &PositionSource| {
            positions.iter().filter(|p| &p.source == source).count()
        };

        PositionManagerStats {
            total_positions: positions.len(),
            active_positions: active.len(),
            total_exposure: self.total_exposure(),
            total_unrealized_pnl: total_unrealized,
            total_realized_pnl: total_realized,
            arbitrage_positions: by_source(&PositionSource::Arbitrage),
            copy_trade_positions: positions.iter().filter(|p| matches!(&p.source, PositionSource::CopyTrade { .. })).count(),
            manual_positions: by_source(&PositionSource::Manual),
        }
    }

    /// Check if a new position of given size would be allowed.
    pub async fn can_open_position(&self, market_id: &str, size: Decimal) -> Result<(), String> {
        let limits = self.limits.read().await;

        if self.positions.len() >= limits.max_total_positions {
            return Err(format!("Max positions ({}) reached", limits.max_total_positions));
        }

        if self.positions_for_market(market_id).len() >= limits.max_per_market {
            return Err(format!("Max positions per market ({}) reached", limits.max_per_market));
        }

        if size > limits.max_position_size {
            return Err(format!("Size {} exceeds max {}", size, limits.max_position_size));
        }

        let new_total = self.total_exposure() + size;
        if new_total > limits.max_total_exposure {
            return Err(format!("Total exposure {} would exceed max {}", new_total, limits.max_total_exposure));
        }

        let new_market = self.market_exposure(market_id) + size;
        if new_market > limits.max_market_exposure {
            return Err(format!("Market exposure {} would exceed max {}", new_market, limits.max_market_exposure));
        }

        Ok(())
    }
}

/// Summary statistics for position manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionManagerStats {
    pub total_positions: usize,
    pub active_positions: usize,
    pub total_exposure: Decimal,
    pub total_unrealized_pnl: Decimal,
    pub total_realized_pnl: Decimal,
    pub arbitrage_positions: usize,
    pub copy_trade_positions: usize,
    pub manual_positions: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use polymarket_core::types::ExitStrategy;

    fn create_test_position(market_id: &str, cost: Decimal) -> Position {
        let price = cost / Decimal::new(2, 0);
        Position::new(
            market_id.to_string(),
            price,
            price,
            Decimal::ONE,
            ExitStrategy::HoldToResolution,
        )
    }

    #[tokio::test]
    async fn test_add_position_within_limits() {
        let manager = PositionManager::new(PositionLimits::default());
        let position = create_test_position("market1", Decimal::new(100, 0));
        let managed = ManagedPosition::new(position, PositionSource::Manual);

        assert!(manager.add_position(managed).await.is_ok());
        assert_eq!(manager.active_positions().len(), 1);
    }

    #[tokio::test]
    async fn test_position_size_limit() {
        let limits = PositionLimits {
            max_position_size: Decimal::new(100, 0),
            ..Default::default()
        };
        let manager = PositionManager::new(limits);

        let position = create_test_position("market1", Decimal::new(200, 0));
        let managed = ManagedPosition::new(position, PositionSource::Manual);

        assert!(manager.add_position(managed).await.is_err());
    }

    #[tokio::test]
    async fn test_market_position_limit() {
        let limits = PositionLimits {
            max_per_market: 2,
            ..Default::default()
        };
        let manager = PositionManager::new(limits);

        for i in 0..3 {
            let position = create_test_position("market1", Decimal::new(10, 0));
            let managed = ManagedPosition::new(position, PositionSource::Manual);
            let result = manager.add_position(managed).await;

            if i < 2 {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
            }
        }
    }

    #[tokio::test]
    async fn test_close_position_updates_pnl() {
        let manager = PositionManager::new(PositionLimits::default());
        let position = create_test_position("market1", Decimal::new(100, 0));
        let id = position.id;
        let managed = ManagedPosition::new(position, PositionSource::Arbitrage);

        manager.add_position(managed).await.unwrap();
        manager.close_position(id, Decimal::new(10, 0)).await.unwrap();

        let stats = manager.stats().await;
        assert_eq!(stats.total_realized_pnl, Decimal::new(10, 0));
        assert_eq!(stats.active_positions, 0);
    }
}
