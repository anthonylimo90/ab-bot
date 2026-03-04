//! Wallet Tracker
//!
//! Discover and analyze profitable wallets on Polymarket.

pub mod advanced_predictor;
pub mod calibration;
pub mod discovery;
pub mod profitability;
pub mod risk_scorer;
pub mod scoring;
pub mod strategy_classifier;
pub mod success_predictor;

pub use advanced_predictor::{
    AdvancedPredictor, EnsemblePrediction, MarketConditionAnalyzer, MarketRegime,
    PredictionFeatures,
};
pub use calibration::CalibrationReport;
pub use discovery::{DiscoveredWallet, DiscoveryCriteria, WalletDiscovery};
pub use profitability::{ProfitabilityAnalyzer, WalletMetrics};
pub use risk_scorer::{RiskScorer, RiskScorerConfig, WalletRiskScore};
pub use scoring::{ScoringWeights, WalletScore};
pub use strategy_classifier::{ClassifierConfig, ExtendedFeatures, StrategyClassifier};
pub use success_predictor::{PredictionModel, SuccessPredictor};
