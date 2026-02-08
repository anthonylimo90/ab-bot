//! Feature extraction from wallet trading history.
//!
//! Shared by both `bot-scanner` and `api-server` (wallet harvester).

use crate::api::polygon::AssetTransfer;
use crate::types::WalletFeatures;
use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

/// Extract behavioral features from a wallet's transfer history.
pub fn extract_features(address: &str, transfers: &[AssetTransfer]) -> Result<WalletFeatures> {
    let mut features = WalletFeatures {
        address: address.to_string(),
        ..Default::default()
    };

    if transfers.is_empty() {
        return Ok(features);
    }

    features.total_trades = transfers.len() as u64;

    // Parse timestamps and calculate intervals
    let timestamps = extract_timestamps(transfers);
    if timestamps.len() >= 2 {
        let intervals = calculate_intervals(&timestamps);
        features.interval_cv = Some(coefficient_of_variation(&intervals));
    }

    // Track unique markets
    let markets: HashSet<_> = transfers.iter().filter_map(|t| t.asset.as_ref()).collect();
    features.markets_traded = markets.len() as u64;

    // Calculate hourly distribution
    for ts in &timestamps {
        let hour = ts.hour() as usize;
        features.hourly_distribution[hour] += 1;
    }
    features.activity_spread = calculate_activity_spread(&features.hourly_distribution);

    // Track first and last trade
    if let (Some(first), Some(last)) = (timestamps.first(), timestamps.last()) {
        features.first_trade = Some(*first);
        features.last_trade = Some(*last);
    }

    // Calculate total volume
    features.total_volume = transfers
        .iter()
        .filter_map(|t| t.value.map(|v| Decimal::try_from(v).unwrap_or_default()))
        .sum();

    // Detect opposing positions (simplified: check for rapid buy/sell of same asset)
    let opposing = detect_opposing_positions(transfers);
    features.has_opposing_positions = opposing > 0;
    features.opposing_position_count = opposing;

    Ok(features)
}

/// Extract timestamps from transfers.
fn extract_timestamps(transfers: &[AssetTransfer]) -> Vec<DateTime<Utc>> {
    transfers
        .iter()
        .filter_map(|t| {
            t.metadata
                .as_ref()
                .and_then(|m| m.block_timestamp.as_ref())
                .and_then(|ts| ts.parse().ok())
        })
        .collect()
}

/// Calculate time intervals between consecutive trades (in seconds).
fn calculate_intervals(timestamps: &[DateTime<Utc>]) -> Vec<f64> {
    timestamps
        .windows(2)
        .map(|w| (w[1] - w[0]).num_seconds() as f64)
        .filter(|&i| i > 0.0)
        .collect()
}

/// Calculate coefficient of variation (std_dev / mean).
pub fn coefficient_of_variation(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 1.0;
    }

    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;

    if mean == 0.0 {
        return 1.0;
    }

    let variance = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let std_dev = variance.sqrt();

    std_dev / mean
}

/// Calculate activity spread across hours (1.0 = perfectly even, lower = clustered).
pub fn calculate_activity_spread(distribution: &[u64; 24]) -> f64 {
    let total: u64 = distribution.iter().sum();
    if total == 0 {
        return 0.0;
    }

    let active_hours = distribution.iter().filter(|&&c| c > 0).count();
    active_hours as f64 / 24.0
}

/// Detect opposing position patterns (buying both sides of same market).
pub fn detect_opposing_positions(transfers: &[AssetTransfer]) -> u64 {
    if transfers.is_empty() {
        return 0;
    }

    // Group transfers by block (same block = potentially simultaneous)
    let mut by_block: HashMap<&str, Vec<&AssetTransfer>> = HashMap::new();
    for transfer in transfers {
        by_block
            .entry(&transfer.block_num)
            .or_default()
            .push(transfer);
    }

    let wallet_address = transfers[0].from.to_lowercase();

    // Count blocks where wallet both sent and received (opposing positions)
    let mut opposing_count = 0u64;
    for (_, block_transfers) in by_block {
        if block_transfers.len() >= 2 {
            // Simple heuristic: multiple transfers in same block could indicate arb
            let from_count = block_transfers
                .iter()
                .filter(|t| t.from.to_lowercase() == wallet_address)
                .count();
            let to_count = block_transfers
                .iter()
                .filter(|t| t.to.to_lowercase() == wallet_address)
                .count();

            if from_count > 0 && to_count > 0 {
                opposing_count += 1;
            }
        }
    }

    opposing_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coefficient_of_variation() {
        // Consistent intervals (low CV)
        let consistent = vec![10.0, 10.0, 10.0, 10.0, 10.0];
        assert!(coefficient_of_variation(&consistent) < 0.01);

        // Variable intervals (high CV)
        let variable = vec![1.0, 100.0, 5.0, 50.0, 10.0];
        assert!(coefficient_of_variation(&variable) > 0.5);
    }

    #[test]
    fn test_coefficient_of_variation_edge_cases() {
        assert_eq!(coefficient_of_variation(&[]), 1.0);
        assert_eq!(coefficient_of_variation(&[0.0, 0.0, 0.0]), 1.0);
        assert!(coefficient_of_variation(&[5.0]).abs() < f64::EPSILON);
    }

    #[test]
    fn test_activity_spread() {
        // Active all hours
        let all_hours = [1u64; 24];
        assert_eq!(calculate_activity_spread(&all_hours), 1.0);

        // Active only 12 hours
        let mut half_hours = [0u64; 24];
        for i in 0..12 {
            half_hours[i] = 5;
        }
        assert_eq!(calculate_activity_spread(&half_hours), 0.5);
    }

    #[test]
    fn test_activity_spread_edge_cases() {
        let empty = [0u64; 24];
        assert_eq!(calculate_activity_spread(&empty), 0.0);

        let mut one_hour = [0u64; 24];
        one_hour[12] = 100;
        let spread = calculate_activity_spread(&one_hour);
        assert!((spread - 1.0 / 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_extract_features_empty_transfers() {
        let features = extract_features("0xabc", &[]).unwrap();
        assert_eq!(features.address, "0xabc");
        assert_eq!(features.total_trades, 0);
        assert!(!features.has_opposing_positions);
    }

    #[test]
    fn test_detect_opposing_positions_empty() {
        assert_eq!(detect_opposing_positions(&[]), 0);
    }

    #[test]
    fn test_calculate_intervals() {
        let ts1 = "2025-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ts2 = "2025-01-01T00:00:10Z".parse::<DateTime<Utc>>().unwrap();
        let ts3 = "2025-01-01T00:00:30Z".parse::<DateTime<Utc>>().unwrap();

        let intervals = calculate_intervals(&[ts1, ts2, ts3]);
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0], 10.0);
        assert_eq!(intervals[1], 20.0);
    }

    #[test]
    fn test_calculate_intervals_single() {
        let ts1 = "2025-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert!(calculate_intervals(&[ts1]).is_empty());
    }
}
