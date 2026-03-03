//! Quantitative signal generators.
//!
//! Each generator runs as an independent background task, polling feature tables
//! (flow features, orderbook aggregates, market metadata) at its own cadence.
//! When conditions are met, it emits a `QuantSignal` on the broadcast channel
//! for the `QuantSignalExecutor` to evaluate and potentially execute.
//!
//! Generators can be independently enabled/disabled via environment variables.

pub mod cross_market_signal;
pub mod flow_signal;
pub mod mean_reversion_signal;
pub mod resolution_signal;

pub use cross_market_signal::spawn_cross_market_signal_generator;
pub use flow_signal::spawn_flow_signal_generator;
pub use mean_reversion_signal::spawn_mean_reversion_signal_generator;
pub use resolution_signal::spawn_resolution_signal_generator;
