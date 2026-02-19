//! Wallet analysis types for bot detection.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// A trade executed by a wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub tx_hash: String,
    pub wallet_address: String,
    pub market_id: String,
    pub outcome_id: String,
    pub side: TradeSide,
    pub price: Decimal,
    pub quantity: Decimal,
    pub timestamp: DateTime<Utc>,
    pub block_number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeSide {
    Buy,
    Sell,
}

/// Behavioral features extracted from a wallet's trading history.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalletFeatures {
    /// Wallet address.
    pub address: String,

    /// Total number of trades analyzed.
    pub total_trades: u64,

    /// Coefficient of variation for trade intervals (std_dev / mean).
    /// Lower values indicate more consistent (bot-like) timing.
    pub interval_cv: Option<f64>,

    /// Win rate across all closed positions.
    pub win_rate: Option<f64>,

    /// Average latency from market event to trade (milliseconds).
    pub avg_latency_ms: Option<f64>,

    /// Number of distinct markets traded.
    pub markets_traded: u64,

    /// Whether wallet holds simultaneous opposing positions.
    pub has_opposing_positions: bool,

    /// Count of opposing position instances.
    pub opposing_position_count: u64,

    /// Activity distribution across 24 hours (hour -> trade count).
    pub hourly_distribution: [u64; 24],

    /// Activity spread: 1.0 = perfectly even, lower = clustered.
    pub activity_spread: f64,

    /// Total volume in USD equivalent.
    pub total_volume: Decimal,

    /// First trade timestamp.
    pub first_trade: Option<DateTime<Utc>>,

    /// Last trade timestamp.
    pub last_trade: Option<DateTime<Utc>>,
}

impl WalletFeatures {
    /// Check if activity is 24/7 (spread across all hours).
    pub fn is_24_7_active(&self) -> bool {
        self.hourly_distribution.iter().all(|&count| count > 0)
    }

    /// Count hours with activity.
    pub fn active_hours(&self) -> usize {
        self.hourly_distribution.iter().filter(|&&c| c > 0).count()
    }
}

/// Bot detection scoring result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotScore {
    pub address: String,
    pub total_score: u32,
    pub signals: Vec<BotSignal>,
    pub classification: WalletClassification,
    pub computed_at: DateTime<Utc>,
}

impl BotScore {
    /// Threshold for flagging as likely bot.
    pub const BOT_THRESHOLD: u32 = 50;

    pub fn new(address: String, features: &WalletFeatures) -> Self {
        let mut signals = Vec::new();
        let mut total_score = 0u32;

        // Signal: Too consistent trade intervals
        if let Some(cv) = features.interval_cv {
            if cv < 0.1 {
                signals.push(BotSignal::ConsistentIntervals { cv, points: 30 });
                total_score += 30;
            }
        }

        // Signal: Suspiciously high win rate
        if let Some(wr) = features.win_rate {
            if wr > 0.90 && features.total_trades >= 100 {
                signals.push(BotSignal::HighWinRate {
                    win_rate: wr,
                    trade_count: features.total_trades,
                    points: 25,
                });
                total_score += 25;
            }
        }

        // Signal: Opposing positions (arbitrage signature)
        if features.has_opposing_positions {
            signals.push(BotSignal::OpposingPositions {
                count: features.opposing_position_count,
                points: 20,
            });
            total_score += 20;
        }

        // Signal: Too fast reaction
        if let Some(latency) = features.avg_latency_ms {
            if latency < 500.0 {
                signals.push(BotSignal::FastLatency {
                    avg_ms: latency,
                    points: 15,
                });
                total_score += 15;
            }
        }

        // Signal: 24/7 activity
        if features.is_24_7_active() {
            signals.push(BotSignal::AlwaysActive { points: 10 });
            total_score += 10;
        }

        let classification = if total_score >= Self::BOT_THRESHOLD {
            WalletClassification::LikelyBot
        } else if total_score >= 25 {
            WalletClassification::Suspicious
        } else {
            WalletClassification::LikelyHuman
        };

        Self {
            address,
            total_score,
            signals,
            classification,
            computed_at: Utc::now(),
        }
    }
}

/// Individual bot detection signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotSignal {
    ConsistentIntervals {
        cv: f64,
        points: u32,
    },
    HighWinRate {
        win_rate: f64,
        trade_count: u64,
        points: u32,
    },
    OpposingPositions {
        count: u64,
        points: u32,
    },
    FastLatency {
        avg_ms: f64,
        points: u32,
    },
    AlwaysActive {
        points: u32,
    },
}

/// Wallet classification based on bot score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletClassification {
    LikelyHuman,
    Suspicious,
    LikelyBot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_scoring() {
        let features = WalletFeatures {
            address: "0x123".to_string(),
            total_trades: 150,
            interval_cv: Some(0.05),      // Very consistent = +30
            win_rate: Some(0.95),         // High win rate = +25
            avg_latency_ms: Some(100.0),  // Fast = +15
            has_opposing_positions: true, // Arb signature = +20
            opposing_position_count: 10,
            hourly_distribution: [1; 24], // 24/7 = +10
            activity_spread: 1.0,
            ..Default::default()
        };

        let score = BotScore::new("0x123".to_string(), &features);

        assert_eq!(score.total_score, 100); // All signals triggered
        assert_eq!(score.classification, WalletClassification::LikelyBot);
        assert_eq!(score.signals.len(), 5);
    }

    #[test]
    fn test_human_wallet() {
        let features = WalletFeatures {
            address: "0x456".to_string(),
            total_trades: 50,
            interval_cv: Some(0.8),       // Variable timing
            win_rate: Some(0.55),         // Normal win rate
            avg_latency_ms: Some(5000.0), // Slow (human)
            has_opposing_positions: false,
            opposing_position_count: 0,
            hourly_distribution: {
                let mut dist = [0u64; 24];
                // Active only during typical waking hours
                for hour in dist.iter_mut().take(22).skip(9) {
                    *hour = 5;
                }
                dist
            },
            activity_spread: 0.5,
            ..Default::default()
        };

        let score = BotScore::new("0x456".to_string(), &features);

        assert_eq!(score.total_score, 0);
        assert_eq!(score.classification, WalletClassification::LikelyHuman);
        assert!(score.signals.is_empty());
    }
}
