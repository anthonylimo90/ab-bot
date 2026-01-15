//! Trading Engine
//!
//! Low-latency order execution, copy trading, and position management for Polymarket.

pub mod copy_trader;
pub mod executor;
pub mod position_manager;
pub mod recommendation;

pub use copy_trader::CopyTrader;
pub use executor::OrderExecutor;
pub use position_manager::PositionManager;
pub use recommendation::{
    Evidence, HoldingPeriod, Recommendation, RecommendationEngine, RecommendationType,
    RecommendedAction, RiskLevel, RiskProfile, Urgency,
};
