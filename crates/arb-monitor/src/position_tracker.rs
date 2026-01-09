//! Position lifecycle tracking for arbitrage positions.

use anyhow::Result;
use polymarket_core::db::positions::PositionRepository;
use polymarket_core::types::{
    ArbOpportunity, BinaryMarketBook, ExitStrategy, Position, PositionState,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Tracks arbitrage positions through their lifecycle.
pub struct PositionTracker {
    repo: PositionRepository,
    /// In-memory cache of active positions by market_id.
    active_positions: HashMap<String, Vec<Position>>,
    /// Minimum profit threshold for exit signals.
    exit_threshold: Decimal,
}

impl PositionTracker {
    /// Create a new position tracker.
    pub fn new(pool: PgPool) -> Self {
        Self {
            repo: PositionRepository::new(pool),
            active_positions: HashMap::new(),
            exit_threshold: Decimal::new(5, 3), // 0.005 = 0.5% minimum exit profit
        }
    }

    /// Load active positions from database into cache.
    pub async fn load_active_positions(&mut self) -> Result<()> {
        let positions = self.repo.get_active().await?;
        self.active_positions.clear();

        for position in positions {
            self.active_positions
                .entry(position.market_id.clone())
                .or_default()
                .push(position);
        }

        info!(
            "Loaded {} active positions across {} markets",
            self.active_positions.values().map(|v| v.len()).sum::<usize>(),
            self.active_positions.len()
        );

        Ok(())
    }

    /// Create a new position from an arbitrage opportunity.
    pub async fn create_position(
        &mut self,
        arb: &ArbOpportunity,
        quantity: Decimal,
        exit_strategy: ExitStrategy,
    ) -> Result<Position> {
        let position = Position::new(
            arb.market_id.clone(),
            arb.yes_ask,
            arb.no_ask,
            quantity,
            exit_strategy,
        );

        self.repo.insert(&position).await?;

        self.active_positions
            .entry(arb.market_id.clone())
            .or_default()
            .push(position.clone());

        info!(
            "Created position {} for market {} with {} shares",
            position.id, arb.market_id, quantity
        );

        Ok(position)
    }

    /// Update P&L for all positions in a market based on current prices.
    pub async fn update_market_positions(
        &mut self,
        market_id: &str,
        book: &BinaryMarketBook,
    ) -> Result<()> {
        let positions = match self.active_positions.get_mut(market_id) {
            Some(p) => p,
            None => return Ok(()),
        };

        let (yes_bid, no_bid, _) = match book.exit_value() {
            Some(v) => v,
            None => return Ok(()),
        };

        for position in positions.iter_mut() {
            if position.is_active() {
                position.update_pnl(yes_bid, no_bid, ArbOpportunity::DEFAULT_FEE);
                debug!(
                    "Position {} unrealized P&L: {}",
                    position.id, position.unrealized_pnl
                );
            }
        }

        Ok(())
    }

    /// Check for exit opportunities on positions in a market.
    pub async fn check_exit_opportunities(
        &mut self,
        market_id: &str,
        book: &BinaryMarketBook,
    ) -> Result<Vec<Uuid>> {
        let positions = match self.active_positions.get_mut(market_id) {
            Some(p) => p,
            None => return Ok(vec![]),
        };

        let (yes_bid, no_bid, _) = match book.exit_value() {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let mut exit_ready = Vec::new();

        for position in positions.iter_mut() {
            if position.state != PositionState::Open {
                continue;
            }

            if position.exit_strategy != ExitStrategy::ExitOnCorrection {
                continue;
            }

            // Calculate potential exit profit
            let exit_value = (yes_bid + no_bid) * position.quantity;
            let entry_cost = position.entry_cost();
            let fees = ArbOpportunity::DEFAULT_FEE * Decimal::TWO * position.quantity;
            let potential_profit = exit_value - entry_cost - fees;

            if potential_profit >= self.exit_threshold * position.quantity {
                position.mark_exit_ready();
                self.repo.update(position).await?;
                exit_ready.push(position.id);

                info!(
                    "EXIT READY: position {} profit={:.4}",
                    position.id, potential_profit
                );
            }
        }

        Ok(exit_ready)
    }

    /// Mark a position as open (execution confirmed).
    pub async fn mark_position_open(&mut self, position_id: Uuid) -> Result<()> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                pos.mark_open();
                self.repo.update(pos).await?;
                info!("Position {} marked as OPEN", position_id);
                return Ok(());
            }
        }
        warn!("Position {} not found in cache", position_id);
        Ok(())
    }

    /// Close a position via market exit.
    pub async fn close_position_exit(
        &mut self,
        position_id: Uuid,
        yes_exit_price: Decimal,
        no_exit_price: Decimal,
    ) -> Result<Option<Decimal>> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                pos.close_via_exit(yes_exit_price, no_exit_price, ArbOpportunity::DEFAULT_FEE);
                self.repo.update(pos).await?;

                let pnl = pos.realized_pnl;
                info!(
                    "Position {} CLOSED via exit, realized P&L: {:?}",
                    position_id, pnl
                );

                return Ok(pnl);
            }
        }
        Ok(None)
    }

    /// Close a position via market resolution.
    pub async fn close_position_resolution(&mut self, position_id: Uuid) -> Result<Option<Decimal>> {
        for positions in self.active_positions.values_mut() {
            if let Some(pos) = positions.iter_mut().find(|p| p.id == position_id) {
                pos.close_via_resolution(ArbOpportunity::DEFAULT_FEE);
                self.repo.update(pos).await?;

                let pnl = pos.realized_pnl;
                info!(
                    "Position {} CLOSED via resolution, realized P&L: {:?}",
                    position_id, pnl
                );

                return Ok(pnl);
            }
        }
        Ok(None)
    }

    /// Get all active positions.
    pub fn get_active_positions(&self) -> Vec<&Position> {
        self.active_positions
            .values()
            .flat_map(|v| v.iter())
            .filter(|p| p.is_active())
            .collect()
    }

    /// Get positions for a specific market.
    pub fn get_market_positions(&self, market_id: &str) -> Vec<&Position> {
        self.active_positions
            .get(market_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }
}
