//! Bot scoring utilities.

use polymarket_core::types::{BotScore, WalletClassification, WalletFeatures};

/// Generate a detailed analysis report for a wallet.
#[allow(dead_code)]
pub fn generate_report(features: &WalletFeatures, score: &BotScore) -> String {
    let mut report = String::new();

    report.push_str("=== Wallet Analysis Report ===\n");
    report.push_str(&format!("Address: {}\n", features.address));
    report.push_str("\n--- Trading Activity ---\n");
    report.push_str(&format!("Total Trades: {}\n", features.total_trades));
    report.push_str(&format!("Markets Traded: {}\n", features.markets_traded));
    report.push_str(&format!("Total Volume: ${}\n", features.total_volume));

    if let (Some(first), Some(last)) = (&features.first_trade, &features.last_trade) {
        report.push_str(&format!(
            "Active Period: {} to {}\n",
            first.date_naive(),
            last.date_naive()
        ));
    }

    report.push_str("\n--- Behavioral Features ---\n");

    if let Some(cv) = features.interval_cv {
        let consistency = if cv < 0.1 {
            "Very Consistent (Bot-like)"
        } else if cv < 0.3 {
            "Moderately Consistent"
        } else if cv < 0.6 {
            "Variable (Human-like)"
        } else {
            "Highly Variable"
        };
        report.push_str(&format!("Interval CV: {:.3} - {}\n", cv, consistency));
    }

    if let Some(wr) = features.win_rate {
        let wr_assessment = if wr > 0.9 {
            "Suspiciously High"
        } else if wr > 0.7 {
            "Very Good"
        } else if wr > 0.55 {
            "Above Average"
        } else {
            "Normal"
        };
        report.push_str(&format!(
            "Win Rate: {:.1}% - {}\n",
            wr * 100.0,
            wr_assessment
        ));
    }

    if let Some(latency) = features.avg_latency_ms {
        let speed = if latency < 500.0 {
            "Bot Speed"
        } else if latency < 2000.0 {
            "Fast"
        } else if latency < 10000.0 {
            "Normal"
        } else {
            "Slow (Human-like)"
        };
        report.push_str(&format!("Avg Latency: {:.0}ms - {}\n", latency, speed));
    }

    report.push_str(&format!(
        "Activity Spread: {:.0}% of hours active\n",
        features.activity_spread * 100.0
    ));

    if features.has_opposing_positions {
        report.push_str(&format!(
            "Opposing Positions: {} instances (Arbitrage Signature)\n",
            features.opposing_position_count
        ));
    }

    report.push_str("\n--- Bot Score ---\n");
    report.push_str(&format!("Total Score: {} / 100\n", score.total_score));
    report.push_str(&format!(
        "Classification: {}\n",
        match score.classification {
            WalletClassification::LikelyHuman => "Likely Human",
            WalletClassification::Suspicious => "Suspicious",
            WalletClassification::LikelyBot => "Likely Bot",
        }
    ));

    if !score.signals.is_empty() {
        report.push_str("\nTriggered Signals:\n");
        for signal in &score.signals {
            report.push_str(&format!("  - {:?}\n", signal));
        }
    }

    report.push_str("\n=============================\n");

    report
}

/// Batch analyze multiple wallets and rank by bot likelihood.
#[allow(dead_code)]
pub fn rank_wallets(wallets: &[(WalletFeatures, BotScore)]) -> Vec<&(WalletFeatures, BotScore)> {
    let mut ranked: Vec<_> = wallets.iter().collect();
    ranked.sort_by(|a, b| b.1.total_score.cmp(&a.1.total_score));
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    #[test]
    fn test_generate_report() {
        let features = WalletFeatures {
            address: "0x123".to_string(),
            total_trades: 150,
            interval_cv: Some(0.05),
            win_rate: Some(0.95),
            avg_latency_ms: Some(100.0),
            markets_traded: 25,
            has_opposing_positions: true,
            opposing_position_count: 10,
            hourly_distribution: [1; 24],
            activity_spread: 1.0,
            total_volume: Decimal::new(100000, 2),
            ..Default::default()
        };

        let score = BotScore::new("0x123".to_string(), &features);
        let report = generate_report(&features, &score);

        assert!(report.contains("Likely Bot"));
        assert!(report.contains("Very Consistent (Bot-like)"));
        assert!(report.contains("Bot Speed"));
    }

    #[test]
    fn test_generate_report_human_wallet() {
        let features = WalletFeatures {
            address: "0x456".to_string(),
            total_trades: 5,
            interval_cv: Some(0.8),
            win_rate: Some(0.50),
            avg_latency_ms: Some(15000.0),
            markets_traded: 3,
            has_opposing_positions: false,
            opposing_position_count: 0,
            hourly_distribution: {
                let mut h = [0u64; 24];
                h[9] = 2;
                h[14] = 3;
                h
            },
            activity_spread: 2.0 / 24.0,
            total_volume: Decimal::new(500, 2),
            ..Default::default()
        };

        let score = BotScore::new("0x456".to_string(), &features);
        let report = generate_report(&features, &score);

        assert!(report.contains("Likely Human"));
        assert!(report.contains("Slow (Human-like)"));
    }

    #[test]
    fn test_rank_wallets() {
        let w1 = WalletFeatures {
            address: "0xlow".to_string(),
            total_trades: 5,
            ..Default::default()
        };
        let s1 = BotScore::new("0xlow".to_string(), &w1);

        let w2 = WalletFeatures {
            address: "0xhigh".to_string(),
            total_trades: 200,
            interval_cv: Some(0.02),
            win_rate: Some(0.99),
            avg_latency_ms: Some(50.0),
            activity_spread: 1.0,
            has_opposing_positions: true,
            opposing_position_count: 20,
            hourly_distribution: [1; 24],
            ..Default::default()
        };
        let s2 = BotScore::new("0xhigh".to_string(), &w2);

        let wallets = vec![(w1, s1), (w2, s2)];
        let ranked = rank_wallets(&wallets);

        assert_eq!(ranked.len(), 2);
        // Higher bot score should be first
        assert!(ranked[0].1.total_score >= ranked[1].1.total_score);
    }
}
