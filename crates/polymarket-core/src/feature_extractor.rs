//! Feature extraction from wallet trading history.
//!
//! Shared by both `bot-scanner` and `api-server` (wallet harvester).

use crate::api::polygon::AssetTransfer;
use crate::types::WalletFeatures;
use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};

/// FIFO realized-P&L engine over a wallet's BUY/SELL fills.
///
/// Shared by `wallet-tracker` (profitability metrics) and `bot-scanner`
/// (win-rate feature) so both derive results from the same definition of a
/// realized trade instead of from raw cash flows.
///
/// Definitions
/// -----------
/// * Fills are matched **per asset** (`asset_id` = one outcome token). A SELL
///   closes the oldest open lots (FIFO) of the *same* asset.
/// * `realized_pnl = proceeds - cost_basis`, where both sides cover only the
///   **matched** quantity (`matched_qty * sell_price` vs. `matched_qty * lot_price`).
/// * A SELL (or the part of a SELL) that exceeds the lots known in the supplied
///   fills has an unknown cost basis (the buy happened before the window, or
///   the position came from a split/transfer). That part is **ignored**: it is
///   never counted as profit.
/// * Open lots (buys not yet sold) produce no realized P&L; a buy-and-hold
///   position is therefore neither a win nor a loss until it is sold.
/// * One [`ClosedSale`] is emitted per SELL fill that matched at least one lot.
///   Win/loss is decided **per closing sale** (`realized_pnl > 0` / `< 0`).
pub mod fifo {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use std::collections::{HashMap, VecDeque};

    /// One executed fill of an outcome token.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Fill {
        pub timestamp: DateTime<Utc>,
        /// Outcome-token identifier; FIFO matching never crosses assets.
        pub asset_id: String,
        /// `true` for BUY, `false` for SELL.
        pub is_buy: bool,
        /// Price per share in USD (0..=1 on Polymarket).
        pub price: Decimal,
        /// Number of shares.
        pub quantity: Decimal,
    }

    /// A SELL fill (matched portion only) that closed FIFO lots.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ClosedSale {
        pub timestamp: DateTime<Utc>,
        pub asset_id: String,
        /// Shares matched against open lots.
        pub matched_quantity: Decimal,
        /// Shares of the SELL with no known lots (ignored for P&L).
        pub unmatched_quantity: Decimal,
        /// Cost of the matched lots (`sum(matched_qty * lot_price)`).
        pub cost_basis: Decimal,
        /// Proceeds of the matched shares (`matched_qty * sell_price`).
        pub proceeds: Decimal,
        /// `proceeds - cost_basis`.
        pub realized_pnl: Decimal,
    }

    impl ClosedSale {
        /// Return on the cost basis of this sale (`pnl / cost`), `None` when the
        /// matched lots were free (zero cost) and the ratio is undefined.
        pub fn return_on_cost(&self) -> Option<f64> {
            use rust_decimal::prelude::ToPrimitive;
            if self.cost_basis > Decimal::ZERO {
                (self.realized_pnl / self.cost_basis).to_f64()
            } else {
                None
            }
        }
    }

    /// Result of replaying a wallet's fills through the FIFO engine.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct FifoSummary {
        /// Closing sales in chronological order.
        pub closed_sales: Vec<ClosedSale>,
        /// Cost basis of lots still open after the last fill.
        pub open_cost_basis: Decimal,
        /// Highest concurrent open cost basis seen while replaying; the
        /// "capital at risk" used as the return denominator.
        pub peak_open_cost_basis: Decimal,
        /// Total SELL shares that had no matching lot (ignored for P&L).
        pub unmatched_sell_quantity: Decimal,
    }

    impl FifoSummary {
        /// Sum of realized P&L over all closing sales.
        pub fn total_realized_pnl(&self) -> Decimal {
            self.closed_sales.iter().map(|c| c.realized_pnl).sum()
        }

        /// Sum of the cost basis of all matched (closed) lots.
        pub fn matched_cost_basis(&self) -> Decimal {
            self.closed_sales.iter().map(|c| c.cost_basis).sum()
        }

        /// Number of closing sales with positive realized P&L.
        pub fn winning_sales(&self) -> usize {
            self.closed_sales
                .iter()
                .filter(|c| c.realized_pnl > Decimal::ZERO)
                .count()
        }

        /// Number of closing sales with negative realized P&L.
        pub fn losing_sales(&self) -> usize {
            self.closed_sales
                .iter()
                .filter(|c| c.realized_pnl < Decimal::ZERO)
                .count()
        }

        /// Share of closing sales that were profitable (breakeven sales count as
        /// closed but not as wins). `None` when nothing was closed.
        pub fn win_rate(&self) -> Option<f64> {
            if self.closed_sales.is_empty() {
                None
            } else {
                Some(self.winning_sales() as f64 / self.closed_sales.len() as f64)
            }
        }
    }

    #[derive(Debug)]
    struct Lot {
        quantity: Decimal,
        price: Decimal,
    }

    /// Replay `fills` through per-asset FIFO lots.
    ///
    /// The input need not be sorted: fills are ordered by timestamp, with BUYs
    /// before SELLs at identical timestamps (so a same-instant sell can find its
    /// basis). Fills with a non-positive quantity or negative price are skipped.
    pub fn compute_fifo(fills: &[Fill]) -> FifoSummary {
        let mut ordered: Vec<&Fill> = fills
            .iter()
            .filter(|f| f.quantity > Decimal::ZERO && f.price >= Decimal::ZERO)
            .collect();
        // Stable sort; `!is_buy` is false for BUY so BUYs come first on ties.
        ordered.sort_by_key(|f| (f.timestamp, !f.is_buy));

        let mut lots: HashMap<&str, VecDeque<Lot>> = HashMap::new();
        let mut summary = FifoSummary::default();

        for fill in ordered {
            if fill.is_buy {
                lots.entry(fill.asset_id.as_str())
                    .or_default()
                    .push_back(Lot {
                        quantity: fill.quantity,
                        price: fill.price,
                    });
                summary.open_cost_basis += fill.quantity * fill.price;
                if summary.open_cost_basis > summary.peak_open_cost_basis {
                    summary.peak_open_cost_basis = summary.open_cost_basis;
                }
                continue;
            }

            let mut remaining = fill.quantity;
            let mut matched = Decimal::ZERO;
            let mut cost = Decimal::ZERO;

            if let Some(queue) = lots.get_mut(fill.asset_id.as_str()) {
                while remaining > Decimal::ZERO {
                    let Some(lot) = queue.front_mut() else { break };
                    let take = remaining.min(lot.quantity);
                    cost += take * lot.price;
                    matched += take;
                    remaining -= take;
                    lot.quantity -= take;
                    if lot.quantity.is_zero() {
                        queue.pop_front();
                    }
                }
            }

            summary.unmatched_sell_quantity += remaining;
            if matched > Decimal::ZERO {
                let proceeds = matched * fill.price;
                summary.open_cost_basis -= cost;
                summary.closed_sales.push(ClosedSale {
                    timestamp: fill.timestamp,
                    asset_id: fill.asset_id.clone(),
                    matched_quantity: matched,
                    unmatched_quantity: remaining,
                    cost_basis: cost,
                    proceeds,
                    realized_pnl: proceeds - cost,
                });
            }
        }

        summary
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use chrono::{Duration, TimeZone};

        fn base() -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()
        }

        fn fill(hours: i64, asset: &str, is_buy: bool, price: &str, qty: &str) -> Fill {
            Fill {
                timestamp: base() + Duration::hours(hours),
                asset_id: asset.to_string(),
                is_buy,
                price: price.parse().unwrap(),
                quantity: qty.parse().unwrap(),
            }
        }

        fn d(s: &str) -> Decimal {
            s.parse().unwrap()
        }

        #[test]
        fn buy_and_hold_has_no_realized_pnl() {
            let s = compute_fifo(&[fill(0, "A", true, "0.40", "100")]);
            assert!(s.closed_sales.is_empty());
            assert_eq!(s.total_realized_pnl(), Decimal::ZERO);
            assert_eq!(s.win_rate(), None);
            assert_eq!(s.open_cost_basis, d("40"));
            assert_eq!(s.peak_open_cost_basis, d("40"));
        }

        #[test]
        fn round_trip_win() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0.40", "100"),
                fill(1, "A", false, "0.60", "100"),
            ]);
            assert_eq!(s.closed_sales.len(), 1);
            let c = &s.closed_sales[0];
            assert_eq!(c.cost_basis, d("40"));
            assert_eq!(c.proceeds, d("60"));
            assert_eq!(c.realized_pnl, d("20"));
            assert_eq!(c.return_on_cost(), Some(0.5));
            assert_eq!(s.win_rate(), Some(1.0));
            assert_eq!(s.open_cost_basis, Decimal::ZERO);
        }

        #[test]
        fn round_trip_loss() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0.60", "100"),
                fill(1, "A", false, "0.45", "100"),
            ]);
            assert_eq!(s.total_realized_pnl(), d("-15"));
            assert_eq!(s.winning_sales(), 0);
            assert_eq!(s.losing_sales(), 1);
            assert_eq!(s.win_rate(), Some(0.0));
            assert_eq!(s.closed_sales[0].return_on_cost(), Some(-0.25));
        }

        #[test]
        fn partial_sells_consume_oldest_lots_first() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0.40", "100"),
                fill(1, "A", true, "0.50", "100"),
                // Closes lot 1 fully (100 @ 0.40) and half of lot 2 (50 @ 0.50).
                fill(2, "A", false, "0.60", "150"),
                // Closes the remaining 50 @ 0.50 at a loss.
                fill(3, "A", false, "0.45", "50"),
            ]);
            assert_eq!(s.closed_sales.len(), 2);

            let first = &s.closed_sales[0];
            assert_eq!(first.matched_quantity, d("150"));
            assert_eq!(first.cost_basis, d("65")); // 40 + 25
            assert_eq!(first.proceeds, d("90"));
            assert_eq!(first.realized_pnl, d("25"));

            let second = &s.closed_sales[1];
            assert_eq!(second.cost_basis, d("25"));
            assert_eq!(second.proceeds, d("22.50"));
            assert_eq!(second.realized_pnl, d("-2.50"));

            assert_eq!(s.total_realized_pnl(), d("22.50"));
            assert_eq!(s.matched_cost_basis(), d("90"));
            assert_eq!(s.winning_sales(), 1);
            assert_eq!(s.losing_sales(), 1);
            assert_eq!(s.win_rate(), Some(0.5));
            assert_eq!(s.open_cost_basis, Decimal::ZERO);
            // 100@0.40 + 100@0.50 = 90 was the most capital ever open.
            assert_eq!(s.peak_open_cost_basis, d("90"));
        }

        #[test]
        fn sell_without_basis_is_ignored_not_profit() {
            let s = compute_fifo(&[fill(0, "A", false, "0.90", "100")]);
            assert!(s.closed_sales.is_empty());
            assert_eq!(s.total_realized_pnl(), Decimal::ZERO);
            assert_eq!(s.unmatched_sell_quantity, d("100"));
            assert_eq!(s.win_rate(), None);
        }

        #[test]
        fn oversized_sell_only_counts_the_matched_part() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0.50", "40"),
                fill(1, "A", false, "0.70", "100"),
            ]);
            assert_eq!(s.closed_sales.len(), 1);
            let c = &s.closed_sales[0];
            assert_eq!(c.matched_quantity, d("40"));
            assert_eq!(c.unmatched_quantity, d("60"));
            assert_eq!(c.cost_basis, d("20"));
            assert_eq!(c.proceeds, d("28"));
            assert_eq!(c.realized_pnl, d("8"));
            assert_eq!(s.unmatched_sell_quantity, d("60"));
        }

        #[test]
        fn matching_never_crosses_assets() {
            // Buying YES then selling NO must not realize anything.
            let s = compute_fifo(&[
                fill(0, "YES", true, "0.40", "100"),
                fill(1, "NO", false, "0.90", "100"),
            ]);
            assert!(s.closed_sales.is_empty());
            assert_eq!(s.unmatched_sell_quantity, d("100"));
            assert_eq!(s.open_cost_basis, d("40"));
        }

        #[test]
        fn input_order_does_not_matter_and_buys_win_ties() {
            let s = compute_fifo(&[
                fill(1, "A", false, "0.60", "10"),
                fill(1, "A", true, "0.50", "10"),
            ]);
            assert_eq!(s.closed_sales.len(), 1);
            assert_eq!(s.total_realized_pnl(), d("1"));
        }

        #[test]
        fn zero_quantity_and_negative_price_rows_are_skipped() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0.50", "0"),
                fill(1, "A", true, "-0.50", "10"),
                fill(2, "A", false, "0.50", "10"),
            ]);
            assert!(s.closed_sales.is_empty());
            assert_eq!(s.unmatched_sell_quantity, d("10"));
        }

        #[test]
        fn breakeven_sale_is_closed_but_not_a_win() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0.50", "10"),
                fill(1, "A", false, "0.50", "10"),
            ]);
            assert_eq!(s.closed_sales.len(), 1);
            assert_eq!(s.winning_sales(), 0);
            assert_eq!(s.losing_sales(), 0);
            assert_eq!(s.win_rate(), Some(0.0));
        }

        #[test]
        fn free_lots_have_no_defined_return() {
            let s = compute_fifo(&[
                fill(0, "A", true, "0", "10"),
                fill(1, "A", false, "0.50", "10"),
            ]);
            assert_eq!(s.closed_sales[0].return_on_cost(), None);
            assert_eq!(s.total_realized_pnl(), d("5"));
        }
    }
}

/// Minimum number of closing sales before a realized win rate is reported.
///
/// A win rate over a handful of sales (e.g. 3/3) says nothing about bot-like
/// consistency, and [`crate::types::BotScore`] awards 25 points for a win rate
/// above 90%. Below this sample size the feature stays `None`.
pub const MIN_CLOSED_SALES_FOR_WIN_RATE: usize = 20;

/// Realized win rate of a wallet from its BUY/SELL fills (see [`fifo`]).
///
/// Each closing sale is one observation; breakeven sales count as non-wins.
/// Returns `None` when fewer than [`MIN_CLOSED_SALES_FOR_WIN_RATE`] sales were
/// closed, so callers never fabricate a rate from too little data.
pub fn realized_win_rate(fills: &[fifo::Fill]) -> Option<f64> {
    let summary = fifo::compute_fifo(fills);
    if summary.closed_sales.len() < MIN_CLOSED_SALES_FOR_WIN_RATE {
        return None;
    }
    summary.win_rate()
}

/// Extract behavioral features from a wallet's transfer history.
///
/// `transfers` should contain the wallet's transfers in **both** directions
/// (see `PolygonClient::get_asset_transfers`); `address` is the wallet the
/// features describe.
///
/// Only features derivable from transfers are populated. `win_rate` needs
/// realized trade results and is filled in by callers that have them (see
/// [`realized_win_rate`]); `avg_latency_ms` needs the timestamp of the market
/// event that triggered each trade, which is not available, so it stays `None`
/// rather than being invented.
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
    let opposing = detect_opposing_positions(address, transfers);
    features.has_opposing_positions = opposing > 0;
    features.opposing_position_count = opposing;

    Ok(features)
}

/// Extract timestamps from transfers, sorted ascending.
///
/// Sorting makes interval and first/last-trade calculations independent of the
/// order the transfers were supplied in (outgoing and incoming transfers come
/// from separate queries).
fn extract_timestamps(transfers: &[AssetTransfer]) -> Vec<DateTime<Utc>> {
    let mut timestamps: Vec<DateTime<Utc>> = transfers
        .iter()
        .filter_map(|t| {
            t.metadata
                .as_ref()
                .and_then(|m| m.block_timestamp.as_ref())
                .and_then(|ts| ts.parse().ok())
        })
        .collect();
    timestamps.sort();
    timestamps
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

/// Detect opposing position patterns: blocks in which `address` both sent and
/// received transfers (an arbitrage-like round trip).
///
/// `address` is the wallet being analyzed. It is passed explicitly because
/// `transfers` contains both directions, so the first transfer's `from` is not
/// necessarily the wallet.
pub fn detect_opposing_positions(address: &str, transfers: &[AssetTransfer]) -> u64 {
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

    let wallet_address = address.to_lowercase();

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
        for hour in half_hours.iter_mut().take(12) {
            *hour = 5;
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
        assert_eq!(detect_opposing_positions("0xabc", &[]), 0);
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

    // -- Bot-score reachability ------------------------------------------------

    use crate::api::polygon::TransferMetadata;
    use crate::types::{BotScore, WalletClassification};
    use chrono::{Duration, SecondsFormat, TimeZone};

    const WALLET: &str = "0xb07b07b07b07b07b07b07b07b07b07b07b07b07b";
    const EXCHANGE: &str = "0x4bfb41d5b3570defd03c39a9a4d8de6bd8b8982e";
    const COUNTERPARTY: &str = "0x00000000000000000000000000000000000000c0";
    const OTHER: &str = "0x00000000000000000000000000000000000000d0";

    fn base_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 3, 1, 0, 0, 0).unwrap()
    }

    fn transfer(from: &str, to: &str, block: u64, ts: DateTime<Utc>, n: usize) -> AssetTransfer {
        AssetTransfer {
            from: from.to_string(),
            to: to.to_string(),
            value: Some(25.0),
            asset: Some("USDC".to_string()),
            hash: format!("0x{n:064x}"),
            unique_id: Some(format!("0x{n:064x}:log:0x0")),
            block_num: format!("0x{block:x}"),
            metadata: Some(TransferMetadata {
                block_timestamp: Some(ts.to_rfc3339_opts(SecondsFormat::Millis, true)),
            }),
        }
    }

    /// Outgoing and incoming transfers of an arbitrage-style bot: every 10
    /// minutes for 24 hours it pays the exchange and is paid out in the same
    /// block. Returned as the two direction queries would deliver them: all
    /// outgoing first, then all incoming.
    fn bot_transfers(include_incoming: bool) -> Vec<AssetTransfer> {
        let step = |i: usize| {
            (
                base_time() + Duration::minutes(10 * i as i64),
                1000 + 300 * i as u64,
            )
        };
        let mut transfers: Vec<AssetTransfer> = (0..144)
            .map(|i| {
                let (ts, block) = step(i);
                transfer(WALLET, EXCHANGE, block, ts, i)
            })
            .collect();
        if include_incoming {
            transfers.extend((0..144).map(|i| {
                let (ts, block) = step(i);
                transfer(EXCHANGE, WALLET, block, ts, 1000 + i)
            }));
        }
        transfers
    }

    /// A person trading by hand between 09:00 and 21:00 at irregular times,
    /// paying out only, with an occasional deposit in a block of its own.
    fn human_transfers() -> Vec<AssetTransfer> {
        // (day offset, hour, minute): bursts and long pauses, never at night.
        let slots: [(i64, i64, i64); 20] = [
            (0, 9, 12),
            (0, 9, 48),
            (0, 14, 5),
            (1, 10, 30),
            (1, 10, 41),
            (1, 19, 2),
            (3, 11, 15),
            (3, 20, 40),
            (4, 9, 5),
            (4, 9, 9),
            (7, 13, 33),
            (7, 13, 59),
            (9, 10, 0),
            (9, 17, 25),
            (12, 12, 12),
            (12, 12, 15),
            (15, 9, 45),
            (15, 18, 18),
            (21, 15, 0),
            (28, 11, 11),
        ];
        slots
            .iter()
            .enumerate()
            .map(|(i, (day, hour, minute))| {
                let ts = base_time()
                    + Duration::days(*day)
                    + Duration::hours(*hour)
                    + Duration::minutes(*minute);
                let block = 5000 + 700 * i as u64;
                if i % 5 == 4 {
                    transfer(COUNTERPARTY, WALLET, block, ts, i)
                } else {
                    transfer(WALLET, EXCHANGE, block, ts, i)
                }
            })
            .collect()
    }

    #[test]
    fn bot_fixture_reaches_the_bot_threshold_with_both_directions() {
        let transfers = bot_transfers(true);
        let features = extract_features(WALLET, &transfers).unwrap();

        assert_eq!(features.total_trades, 288);
        assert!(features.interval_cv.unwrap() < 0.1);
        assert!(features.is_24_7_active());
        assert!(features.has_opposing_positions);
        assert_eq!(features.opposing_position_count, 144);
        assert_eq!(features.win_rate, None, "never fabricated from transfers");
        assert_eq!(features.avg_latency_ms, None, "never fabricated");

        let score = BotScore::new(WALLET.to_string(), &features);
        assert_eq!(score.total_score, 30 + 20 + 10);
        assert!(score.total_score >= BotScore::BOT_THRESHOLD);
        assert_eq!(score.classification, WalletClassification::LikelyBot);
    }

    #[test]
    fn outgoing_only_data_could_never_reach_the_threshold() {
        // The shape the old one-directional fetch produced: the opposing
        // positions signal is dead and the score is capped at 40.
        let transfers = bot_transfers(false);
        let features = extract_features(WALLET, &transfers).unwrap();
        assert!(!features.has_opposing_positions);

        let score = BotScore::new(WALLET.to_string(), &features);
        assert_eq!(score.total_score, 40);
        assert!(score.total_score < BotScore::BOT_THRESHOLD);
    }

    #[test]
    fn bot_with_realized_win_rate_scores_higher_still() {
        let mut features = extract_features(WALLET, &bot_transfers(true)).unwrap();
        features.win_rate = Some(0.95);
        let score = BotScore::new(WALLET.to_string(), &features);
        assert_eq!(score.total_score, 30 + 25 + 20 + 10);
    }

    #[test]
    fn human_fixture_stays_below_the_threshold() {
        let transfers = human_transfers();
        let features = extract_features(WALLET, &transfers).unwrap();

        assert!(features.interval_cv.unwrap() > 0.5, "irregular timing");
        assert!(!features.is_24_7_active());
        assert!(!features.has_opposing_positions);

        let score = BotScore::new(WALLET.to_string(), &features);
        assert!(score.total_score < BotScore::BOT_THRESHOLD);
        assert_eq!(score.classification, WalletClassification::LikelyHuman);
    }

    #[test]
    fn regular_timing_alone_is_not_enough_to_be_a_bot() {
        // 30 (intervals) + 10 (24/7) = 40: suspicious at most, never a bot.
        let features = extract_features(WALLET, &bot_transfers(false)).unwrap();
        let score = BotScore::new(WALLET.to_string(), &features);
        assert_eq!(score.classification, WalletClassification::Suspicious);
    }

    #[test]
    fn opposing_positions_use_the_analyzed_wallet_not_the_first_transfer() {
        let ts = base_time();
        // First transfer is an incoming one from a counterparty. The old code
        // took transfers[0].from as "the wallet" and so counted the wrong side.
        let transfers = vec![
            transfer(COUNTERPARTY, WALLET, 7, ts, 1),
            transfer(WALLET, EXCHANGE, 7, ts, 2),
            // A block in which only some other wallet round-trips.
            transfer(EXCHANGE, OTHER, 8, ts, 3),
            transfer(OTHER, EXCHANGE, 8, ts, 4),
        ];
        assert_eq!(detect_opposing_positions(WALLET, &transfers), 1);
        assert_eq!(
            detect_opposing_positions(&WALLET.to_uppercase().replace("0X", "0x"), &transfers),
            1,
            "address comparison is case-insensitive"
        );
        // The wallet the old code mistook for the analyzed one never both sent
        // and received within a block.
        assert_eq!(detect_opposing_positions(COUNTERPARTY, &transfers), 0);
        assert_eq!(detect_opposing_positions(OTHER, &transfers), 1);
    }

    #[test]
    fn features_do_not_depend_on_transfer_order() {
        let mut transfers = bot_transfers(true);
        let forward = extract_features(WALLET, &transfers).unwrap();
        transfers.reverse();
        let reversed = extract_features(WALLET, &transfers).unwrap();

        assert_eq!(forward.first_trade, reversed.first_trade);
        assert_eq!(forward.last_trade, reversed.last_trade);
        assert!(forward.first_trade < forward.last_trade);
        assert_eq!(forward.interval_cv, reversed.interval_cv);
    }

    // -- Realized win rate ---------------------------------------------------

    fn fill(minute: i64, asset: &str, is_buy: bool, price: &str) -> fifo::Fill {
        fifo::Fill {
            timestamp: base_time() + Duration::minutes(minute),
            asset_id: asset.to_string(),
            is_buy,
            price: price.parse().unwrap(),
            quantity: Decimal::from(10),
        }
    }

    /// `wins` profitable and `losses` losing round trips, each on its own asset.
    fn round_trips(wins: usize, losses: usize) -> Vec<fifo::Fill> {
        let mut fills = vec![];
        for i in 0..wins + losses {
            let asset = format!("tok{i}");
            let sell = if i < wins { "0.60" } else { "0.40" };
            fills.push(fill(2 * i as i64, &asset, true, "0.50"));
            fills.push(fill(2 * i as i64 + 1, &asset, false, sell));
        }
        fills
    }

    #[test]
    fn realized_win_rate_needs_a_minimum_sample() {
        // 5/5 wins is too few closed sales to say anything.
        assert_eq!(realized_win_rate(&round_trips(5, 0)), None);
        assert_eq!(
            realized_win_rate(&round_trips(MIN_CLOSED_SALES_FOR_WIN_RATE - 1, 0)),
            None
        );
        // No trades at all.
        assert_eq!(realized_win_rate(&[]), None);
    }

    #[test]
    fn realized_win_rate_counts_closing_sales() {
        let rate = realized_win_rate(&round_trips(23, 2)).unwrap();
        assert!((rate - 23.0 / 25.0).abs() < 1e-12);

        let rate = realized_win_rate(&round_trips(10, 10)).unwrap();
        assert!((rate - 0.5).abs() < 1e-12);
    }

    #[test]
    fn realized_win_rate_ignores_open_positions_and_unmatched_sells() {
        let mut fills = round_trips(18, 2); // 20 closed sales, 90% wins
        fills.push(fill(500, "held", true, "0.30")); // open: not a loss
        fills.push(fill(501, "unknown-basis", false, "0.99")); // sell w/o basis: not a win
        let rate = realized_win_rate(&fills).unwrap();
        assert!((rate - 0.9).abs() < 1e-12);
    }

    #[test]
    fn high_realized_win_rate_feeds_the_score() {
        let mut features = extract_features(WALLET, &bot_transfers(true)).unwrap();
        features.win_rate = realized_win_rate(&round_trips(24, 1));
        assert!(features.win_rate.unwrap() > 0.9);
        let score = BotScore::new(WALLET.to_string(), &features);
        assert!(score
            .signals
            .iter()
            .any(|s| matches!(s, crate::types::BotSignal::HighWinRate { .. })));
    }
}
