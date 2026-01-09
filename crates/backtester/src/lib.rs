//! Backtester
//!
//! Historical simulation framework for testing trading strategies.
//!
//! # Features
//!
//! - **Strategy Trait**: Pluggable strategy interface for custom implementations
//! - **Historical Data Store**: TimescaleDB-backed storage for orderbook snapshots
//! - **Backtest Simulator**: Full simulation with slippage and fee models
//! - **Built-in Strategies**: Arbitrage, momentum, and mean reversion strategies
//!
//! # Example
//!
//! ```ignore
//! use backtester::{
//!     BacktestSimulator, SimulatorConfig, DataQuery,
//!     HistoricalDataStore, ArbitrageStrategy,
//! };
//!
//! let data_store = HistoricalDataStore::new(pool);
//! let simulator = BacktestSimulator::new(data_store, SimulatorConfig::default());
//!
//! let mut strategy = ArbitrageStrategy::default();
//! let query = DataQuery::last_days(30).resolution(TimeResolution::Minute5);
//!
//! let result = simulator.run(&mut strategy, query).await?;
//! println!("Return: {:.2}%", result.return_pct * 100.0);
//! ```

pub mod data_store;
pub mod simulator;
pub mod strategy;

// Re-exports
pub use data_store::{
    DataQuery, HistoricalDataStore, HistoricalTrade, MarketSnapshot,
    TimeResolution, TradeSide,
};
pub use simulator::{
    BacktestResult, BacktestSimulator, SimulatorConfig, SlippageModel,
    TradeRecord, TradeType,
};
pub use strategy::{
    ArbitrageStrategy, MeanReversionStrategy, MomentumStrategy,
    Position, Signal, SignalType, Strategy, StrategyContext,
};
