//! Wallet Tracker
//!
//! Discover and analyze profitable wallets on Polymarket for copy trading.

pub mod discovery;
pub mod profitability;
pub mod success_predictor;
pub mod trade_monitor;

pub use discovery::{DiscoveryCriteria, DiscoveredWallet, WalletDiscovery};
pub use profitability::{ProfitabilityAnalyzer, WalletMetrics};
pub use success_predictor::{PredictionModel, SuccessPredictor};
pub use trade_monitor::{TradeMonitor, WalletTrade};
