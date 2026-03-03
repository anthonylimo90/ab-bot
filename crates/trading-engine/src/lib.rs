//! Trading Engine
//!
//! Low-latency order execution and position management for Polymarket.

pub mod executor;
pub mod position_manager;
pub mod recommendation;

pub use executor::OrderExecutor;
pub use position_manager::PositionManager;
pub use recommendation::{
    Evidence, HoldingPeriod, Recommendation, RecommendationEngine, RecommendationType,
    RecommendedAction, RiskLevel, RiskProfile, Urgency,
};
