//! Wallet Tracker
//!
//! Discover and analyze profitable wallets on Polymarket for copy trading.

pub mod advanced_predictor;
pub mod discovery;
pub mod profitability;
pub mod strategy_classifier;
pub mod success_predictor;
pub mod trade_monitor;

pub use advanced_predictor::{
    AdvancedPredictor, EnsemblePrediction, MarketConditionAnalyzer, MarketRegime,
    PredictionFeatures,
};
pub use discovery::{DiscoveredWallet, DiscoveryCriteria, WalletDiscovery};
pub use profitability::{ProfitabilityAnalyzer, WalletMetrics};
pub use strategy_classifier::{ClassifierConfig, ExtendedFeatures, StrategyClassifier};
pub use success_predictor::{PredictionModel, SuccessPredictor};
pub use trade_monitor::{TradeMonitor, WalletTrade};
