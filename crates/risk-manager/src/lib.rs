//! Risk Manager
//!
//! Stop-loss management, position limits, and circuit breakers for trading safety.

pub mod advanced_stops;
pub mod circuit_breaker;
pub mod circuit_breaker_repo;
pub mod stop_loss;
pub mod stop_loss_repo;

pub use advanced_stops::{
    AdvancedStopConfig, BreakEvenStop, CompoundLogic, CompoundStop, SessionStop, StepTrailingStop,
    StopCondition, StopContext, TimeDecayStop, VolatilityStop,
};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerState, RecoveryState, TripReason,
};
pub use circuit_breaker_repo::CircuitBreakerRepository;
pub use stop_loss::{
    CheckSkipReason, CheckTriggersSummary, RuleCheckOutcome, RuleCheckResult, StopLossManager,
    StopLossRule, StopLossStats, StopType, TriggeredStop,
};
pub use stop_loss_repo::StopLossRepository;
