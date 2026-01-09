//! Risk Manager
//!
//! Stop-loss management, position limits, and circuit breakers for trading safety.

pub mod advanced_stops;
pub mod circuit_breaker;
pub mod stop_loss;

pub use advanced_stops::{
    AdvancedStopConfig, BreakEvenStop, CompoundLogic, CompoundStop,
    SessionStop, StepTrailingStop, StopCondition, StopContext,
    TimeDecayStop, VolatilityStop,
};
pub use circuit_breaker::CircuitBreaker;
pub use stop_loss::{StopLossManager, StopLossRule, StopType};
