//! Wallet discovery and live trade monitoring handlers.
//!
//! Provides endpoints for discovering top wallets and viewing live trades.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiResult;
use crate::state::AppState;

/// A live trade from a wallet.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LiveTrade {
    /// Wallet address that made the trade.
    pub wallet_address: String,
    /// Wallet label if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_label: Option<String>,
    /// Transaction hash.
    pub tx_hash: String,
    /// Trade timestamp.
    pub timestamp: DateTime<Utc>,
    /// Market/asset identifier.
    pub market_id: String,
    /// Market question.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_question: Option<String>,
    /// Token/outcome (Yes/No).
    pub outcome: String,
    /// Trade direction (buy/sell).
    pub direction: String,
    /// Price per share (0.01 to 0.99).
    pub price: Decimal,
    /// Quantity of shares.
    pub quantity: Decimal,
    /// Total value in USD.
    pub value: Decimal,
}

/// A discovered top-performing wallet.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DiscoveredWallet {
    /// Wallet address.
    pub address: String,
    /// Rank in the leaderboard.
    pub rank: i32,
    /// 7-day ROI percentage.
    pub roi_7d: Decimal,
    /// 30-day ROI percentage.
    pub roi_30d: Decimal,
    /// 90-day ROI percentage.
    pub roi_90d: Decimal,
    /// Sharpe ratio.
    pub sharpe_ratio: Decimal,
    /// Total trades count.
    pub total_trades: i64,
    /// Win rate percentage.
    pub win_rate: Decimal,
    /// Maximum drawdown percentage.
    pub max_drawdown: Decimal,
    /// Prediction category.
    pub prediction: PredictionCategory,
    /// Confidence score (0-100).
    pub confidence: i32,
    /// Whether this wallet is already being tracked.
    pub is_tracked: bool,
    /// Recent trade activity (trades in last 24h).
    pub trades_24h: i64,
    /// Total PnL in USD.
    pub total_pnl: Decimal,
}

/// Prediction category for a wallet.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PredictionCategory {
    HighPotential,
    Moderate,
    LowPotential,
    InsufficientData,
}

/// Equity curve point for demo simulation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EquityPoint {
    /// Date as ISO string.
    pub date: String,
    /// Portfolio value.
    pub value: Decimal,
}

/// Demo P&L simulation result.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DemoPnlSimulation {
    /// Initial investment amount.
    pub initial_amount: Decimal,
    /// Current simulated value.
    pub current_value: Decimal,
    /// Absolute P&L.
    pub pnl: Decimal,
    /// P&L percentage.
    pub pnl_pct: Decimal,
    /// Simulated equity curve.
    pub equity_curve: Vec<EquityPoint>,
    /// Wallets included in simulation.
    pub wallets: Vec<WalletSimulation>,
}

/// Individual wallet contribution to simulation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WalletSimulation {
    /// Wallet address.
    pub address: String,
    /// Allocation percentage.
    pub allocation_pct: Decimal,
    /// P&L contribution.
    pub pnl: Decimal,
    /// Number of trades simulated.
    pub trades: i64,
}

/// Query parameters for live trades.
#[derive(Debug, Deserialize, IntoParams)]
pub struct LiveTradesQuery {
    /// Filter by wallet address.
    pub wallet: Option<String>,
    /// Maximum number of trades.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Minimum trade value in USD.
    pub min_value: Option<Decimal>,
}

fn default_limit() -> i64 {
    50
}

/// Query parameters for wallet discovery.
#[derive(Debug, Deserialize, IntoParams)]
pub struct DiscoverWalletsQuery {
    /// Sort by field.
    #[serde(default = "default_sort")]
    pub sort_by: String,
    /// Time period for metrics.
    #[serde(default = "default_period")]
    pub period: String,
    /// Minimum trades count.
    pub min_trades: Option<i64>,
    /// Minimum win rate percentage.
    pub min_win_rate: Option<Decimal>,
    /// Maximum results.
    #[serde(default = "default_discover_limit")]
    pub limit: i64,
}

fn default_sort() -> String {
    "roi".to_string()
}

fn default_period() -> String {
    "30d".to_string()
}

fn default_discover_limit() -> i64 {
    20
}

/// Query parameters for demo P&L simulation.
#[derive(Debug, Deserialize, IntoParams)]
pub struct DemoSimulationQuery {
    /// Initial investment amount in USD.
    #[serde(default = "default_amount")]
    pub amount: Decimal,
    /// Time period for simulation.
    #[serde(default = "default_period")]
    pub period: String,
    /// Wallet addresses to include (comma-separated).
    pub wallets: Option<String>,
}

fn default_amount() -> Decimal {
    Decimal::new(100, 0)
}

/// Get live trades from monitored wallets.
#[utoipa::path(
    get,
    path = "/api/v1/discover/trades",
    tag = "discover",
    params(LiveTradesQuery),
    responses(
        (status = 200, description = "List of live trades", body = Vec<LiveTrade>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_live_trades(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<LiveTradesQuery>,
) -> ApiResult<Json<Vec<LiveTrade>>> {
    // Generate realistic mock trades for demo
    let trades = generate_mock_trades(query.limit as usize, query.min_value);
    Ok(Json(trades))
}

/// Discover top-performing wallets.
#[utoipa::path(
    get,
    path = "/api/v1/discover/wallets",
    tag = "discover",
    params(DiscoverWalletsQuery),
    responses(
        (status = 200, description = "List of top wallets", body = Vec<DiscoveredWallet>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn discover_wallets(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<DiscoverWalletsQuery>,
) -> ApiResult<Json<Vec<DiscoveredWallet>>> {
    // Generate mock discovered wallets for demo
    let mut wallets = generate_mock_wallets(query.limit as usize);

    // Apply filters
    if let Some(min_trades) = query.min_trades {
        wallets.retain(|w| w.total_trades >= min_trades);
    }
    if let Some(min_win_rate) = query.min_win_rate {
        wallets.retain(|w| w.win_rate >= min_win_rate);
    }

    // Sort by requested field
    use std::cmp::Ordering;
    match query.sort_by.as_str() {
        "sharpe" => wallets.sort_by(|a, b| {
            b.sharpe_ratio
                .partial_cmp(&a.sharpe_ratio)
                .unwrap_or(Ordering::Equal)
        }),
        "winRate" => wallets.sort_by(|a, b| {
            b.win_rate
                .partial_cmp(&a.win_rate)
                .unwrap_or(Ordering::Equal)
        }),
        "trades" => wallets.sort_by(|a, b| b.total_trades.cmp(&a.total_trades)),
        _ => {
            // Default: roi
            match query.period.as_str() {
                "7d" => wallets
                    .sort_by(|a, b| b.roi_7d.partial_cmp(&a.roi_7d).unwrap_or(Ordering::Equal)),
                "90d" => wallets
                    .sort_by(|a, b| b.roi_90d.partial_cmp(&a.roi_90d).unwrap_or(Ordering::Equal)),
                _ => wallets
                    .sort_by(|a, b| b.roi_30d.partial_cmp(&a.roi_30d).unwrap_or(Ordering::Equal)),
            }
        }
    }

    // Re-rank after sorting
    for (i, wallet) in wallets.iter_mut().enumerate() {
        wallet.rank = (i + 1) as i32;
    }

    Ok(Json(wallets))
}

/// Run a demo P&L simulation.
#[utoipa::path(
    get,
    path = "/api/v1/discover/simulate",
    tag = "discover",
    params(DemoSimulationQuery),
    responses(
        (status = 200, description = "Simulation results", body = DemoPnlSimulation),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn simulate_demo_pnl(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<DemoSimulationQuery>,
) -> ApiResult<Json<DemoPnlSimulation>> {
    let simulation = generate_simulation(query.amount, &query.period, query.wallets.as_deref());
    Ok(Json(simulation))
}

// Mock data generators

fn generate_mock_trades(count: usize, min_value: Option<Decimal>) -> Vec<LiveTrade> {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let wallets = [
        (
            "0x1234567890abcdef1234567890abcdef12345678",
            Some("WhaleTrader"),
        ),
        (
            "0xabcdef1234567890abcdef1234567890abcdef12",
            Some("SharpBettor"),
        ),
        ("0x5678901234abcdef5678901234abcdef56789012", None),
        (
            "0x9876543210fedcba9876543210fedcba98765432",
            Some("PoliticsGuru"),
        ),
        ("0xfedcba9876543210fedcba9876543210fedcba98", None),
    ];

    let markets = [
        ("0xmarket1", "Will Trump win the 2024 election?"),
        ("0xmarket2", "Will BTC reach $100k by end of year?"),
        ("0xmarket3", "Will there be a Fed rate cut in Q1?"),
        ("0xmarket4", "Will AI breakthrough happen in 2024?"),
        ("0xmarket5", "Super Bowl winner: Chiefs vs 49ers"),
    ];

    let min_val = min_value.unwrap_or(Decimal::ZERO);
    let now = Utc::now();

    (0..count)
        .filter_map(|i| {
            let (wallet, label) = wallets[rng.gen_range(0..wallets.len())];
            let (market_id, question) = markets[rng.gen_range(0..markets.len())];

            let price = Decimal::new(rng.gen_range(15..85), 2);
            let quantity = Decimal::new(rng.gen_range(50..2000), 0);
            let value = price * quantity;

            if value < min_val {
                return None;
            }

            Some(LiveTrade {
                wallet_address: wallet.to_string(),
                wallet_label: label.map(String::from),
                tx_hash: format!("0x{:064x}", rng.gen::<u64>()),
                timestamp: now
                    - chrono::Duration::seconds(rng.gen_range(0..3600) + (i as i64 * 60)),
                market_id: market_id.to_string(),
                market_question: Some(question.to_string()),
                outcome: if rng.gen_bool(0.5) { "Yes" } else { "No" }.to_string(),
                direction: if rng.gen_bool(0.6) { "buy" } else { "sell" }.to_string(),
                price,
                quantity,
                value,
            })
        })
        .collect()
}

fn generate_mock_wallets(count: usize) -> Vec<DiscoveredWallet> {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    (0..count)
        .map(|i| {
            let base_roi = 50.0 - (i as f64 * 3.0) + rng.gen_range(-5.0..5.0);
            let roi_30d =
                Decimal::from_f64_retain(base_roi.max(5.0)).unwrap_or(Decimal::new(20, 0));
            let roi_7d = roi_30d * Decimal::new(30, 2); // ~30% of monthly
            let roi_90d = roi_30d * Decimal::new(250, 2); // ~2.5x monthly

            let win_rate = Decimal::new(rng.gen_range(52..78), 0);
            let trades = rng.gen_range(30..300);

            let prediction = match i {
                0..=2 => PredictionCategory::HighPotential,
                3..=7 => PredictionCategory::Moderate,
                _ => PredictionCategory::LowPotential,
            };

            let confidence = match prediction {
                PredictionCategory::HighPotential => rng.gen_range(75..95),
                PredictionCategory::Moderate => rng.gen_range(55..75),
                PredictionCategory::LowPotential => rng.gen_range(35..55),
                PredictionCategory::InsufficientData => rng.gen_range(10..35),
            };

            DiscoveredWallet {
                address: format!(
                    "0x{:040x}",
                    rng.gen::<u64>() as u128 * rng.gen::<u64>() as u128
                ),
                rank: (i + 1) as i32,
                roi_7d,
                roi_30d,
                roi_90d,
                sharpe_ratio: Decimal::new(rng.gen_range(100..280), 2),
                total_trades: trades,
                win_rate,
                max_drawdown: Decimal::new(-rng.gen_range(5..25), 0),
                prediction,
                confidence,
                is_tracked: i < 2, // First 2 are tracked for demo
                trades_24h: rng.gen_range(0..15),
                total_pnl: Decimal::new(rng.gen_range(500..50000), 2),
            }
        })
        .collect()
}

fn generate_simulation(amount: Decimal, period: &str, _wallets: Option<&str>) -> DemoPnlSimulation {
    use rand::Rng;
    let mut rng = rand::thread_rng();

    let days = match period {
        "7d" => 7,
        "90d" => 90,
        _ => 30,
    };

    // Generate equity curve
    let mut value = amount;
    let daily_return = 0.008; // ~0.8% daily average
    let volatility = 0.03;

    let mut equity_curve = Vec::with_capacity(days);
    let now = Utc::now();

    for i in (0..=days).rev() {
        let date = (now - chrono::Duration::days(i as i64))
            .format("%Y-%m-%d")
            .to_string();

        let random_factor = 1.0 + (rng.gen::<f64>() - 0.5) * volatility * 2.0;
        value = value
            * Decimal::from_f64_retain(1.0 + daily_return).unwrap_or(Decimal::ONE)
            * Decimal::from_f64_retain(random_factor).unwrap_or(Decimal::ONE);

        equity_curve.push(EquityPoint {
            date,
            value: value.round_dp(2),
        });
    }

    let final_value = equity_curve.last().map(|e| e.value).unwrap_or(amount);
    let pnl = final_value - amount;
    let pnl_pct = if amount > Decimal::ZERO {
        (pnl / amount) * Decimal::new(100, 0)
    } else {
        Decimal::ZERO
    };

    DemoPnlSimulation {
        initial_amount: amount,
        current_value: final_value,
        pnl,
        pnl_pct: pnl_pct.round_dp(2),
        equity_curve,
        wallets: vec![
            WalletSimulation {
                address: "0x1234...5678".to_string(),
                allocation_pct: Decimal::new(40, 0),
                pnl: pnl * Decimal::new(40, 2),
                trades: 23,
            },
            WalletSimulation {
                address: "0xabcd...ef12".to_string(),
                allocation_pct: Decimal::new(35, 0),
                pnl: pnl * Decimal::new(35, 2),
                trades: 18,
            },
            WalletSimulation {
                address: "0x5678...9012".to_string(),
                allocation_pct: Decimal::new(25, 0),
                pnl: pnl * Decimal::new(25, 2),
                trades: 12,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_mock_trades() {
        let trades = generate_mock_trades(10, None);
        assert_eq!(trades.len(), 10);

        for trade in &trades {
            assert!(trade.wallet_address.starts_with("0x"));
            assert!(trade.price > Decimal::ZERO);
            assert!(trade.quantity > Decimal::ZERO);
        }
    }

    #[test]
    fn test_generate_mock_wallets() {
        let wallets = generate_mock_wallets(5);
        assert_eq!(wallets.len(), 5);

        for (i, wallet) in wallets.iter().enumerate() {
            assert_eq!(wallet.rank, (i + 1) as i32);
            assert!(wallet.address.starts_with("0x"));
        }
    }

    #[test]
    fn test_simulation_pnl() {
        let sim = generate_simulation(Decimal::new(100, 0), "30d", None);
        assert_eq!(sim.initial_amount, Decimal::new(100, 0));
        assert!(!sim.equity_curve.is_empty());
        assert_eq!(sim.wallets.len(), 3);
    }
}
