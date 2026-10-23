//! Risk Manager
//!
//! The circuit breaker that halts trading on losses, drawdown or failures.

pub mod circuit_breaker;
pub mod circuit_breaker_repo;

pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerState, RecoveryState, TripReason,
};
pub use circuit_breaker_repo::{CircuitBreakerRepository, SaveOutcome};
