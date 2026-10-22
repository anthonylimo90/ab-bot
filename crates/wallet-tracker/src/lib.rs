//! Wallet Tracker
//!
//! Discover and analyze profitable wallets on Polymarket.

pub mod discovery;
pub mod market_regime;
pub mod profitability;
pub mod strategy_classifier;

pub use discovery::{DiscoveredWallet, DiscoveryCriteria, WalletDiscovery};
pub use market_regime::{MarketConditionAnalyzer, MarketRegime};
pub use profitability::{ProfitabilityAnalyzer, WalletMetrics};
pub use strategy_classifier::{ClassifierConfig, ExtendedFeatures, StrategyClassifier};
