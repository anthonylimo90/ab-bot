//! AB-Bot: Polymarket Arbitrage and Copy Trading Bot
//!
//! This is the root crate that provides benchmark access to the internal modules.
//! For actual functionality, use the individual crates directly:
//!
//! - `polymarket-core`: Core types, API clients, database models
//! - `trading-engine`: Order execution, copy trading, position management
//! - `risk-manager`: Stop-loss, circuit breakers, position limits
//! - `auth`: Authentication, RBAC, audit logging
//! - `wallet-tracker`: Wallet discovery, profitability analysis
//! - `backtester`: Historical simulation, strategy testing
//! - `api-server`: REST/WebSocket API server

// Re-export for benchmarks
pub use polymarket_core as core;
pub use risk_manager as risk;
pub use trading_engine as trading;
