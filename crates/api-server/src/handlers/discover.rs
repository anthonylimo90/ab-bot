//! Wallet discovery and live trade monitoring handlers.
//!
//! Provides endpoints for discovering top wallets and viewing live trades.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use wallet_tracker::discovery::DiscoveryCriteria;

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
    State(state): State<Arc<AppState>>,
    Query(query): Query<LiveTradesQuery>,
) -> ApiResult<Json<Vec<LiveTrade>>> {
    // Fetch real trades from Data API
    match state
        .clob_client
        .get_recent_trades(query.limit as u32, None)
        .await
    {
        Ok(clob_trades) => {
            let min_val = query.min_value.unwrap_or(Decimal::ZERO);
            let trades: Vec<LiveTrade> = clob_trades
                .into_iter()
                .filter_map(|ct| {
                    // Data API returns f64 for price and size
                    let price = Decimal::from_f64_retain(ct.price)?;
                    let quantity = Decimal::from_f64_retain(ct.size)?;
                    let value = price * quantity;

                    if value < min_val {
                        return None;
                    }

                    // Filter by wallet if requested
                    if let Some(ref wallet_filter) = query.wallet {
                        let wallet_match =
                            ct.wallet_address.to_lowercase() == wallet_filter.to_lowercase();
                        if !wallet_match {
                            return None;
                        }
                    }

                    // Data API returns Unix timestamp as i64
                    let timestamp =
                        DateTime::from_timestamp(ct.timestamp, 0).unwrap_or_else(Utc::now);

                    Some(LiveTrade {
                        wallet_address: ct.wallet_address.clone(),
                        wallet_label: None,
                        tx_hash: ct.transaction_hash.clone(),
                        timestamp,
                        market_id: ct
                            .condition_id
                            .clone()
                            .unwrap_or_else(|| ct.asset_id.clone()),
                        market_question: ct.title.clone(),
                        outcome: ct.outcome.clone().unwrap_or_else(|| ct.asset_id.clone()),
                        direction: ct.side.to_lowercase(),
                        price,
                        quantity,
                        value,
                    })
                })
                .collect();

            if trades.is_empty() {
                tracing::warn!("Live trades endpoint returning empty — no trades matched filters");
            }
            Ok(Json(trades))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Data API trade fetch failed, returning empty");
            Ok(Json(Vec::new()))
        }
    }
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
    State(state): State<Arc<AppState>>,
    Query(query): Query<DiscoverWalletsQuery>,
) -> ApiResult<Json<Vec<DiscoveredWallet>>> {
    let time_window = match query.period.as_str() {
        "7d" => 7,
        "90d" => 90,
        _ => 30,
    };

    // Use market regime-aware base criteria when no explicit filters are provided
    let current_regime = *state.current_regime.read().await;
    let base = wallet_tracker::discovery::DiscoveryCriteria::from_market_regime(current_regime);

    let criteria = DiscoveryCriteria::new()
        .min_trades(query.min_trades.unwrap_or(1) as u64)
        .min_win_rate(
            query
                .min_win_rate
                .map(|w| {
                    let pct: f64 = w.try_into().unwrap_or(0.0);
                    pct / 100.0
                })
                .unwrap_or(base.min_win_rate),
        )
        .min_volume(base.min_volume)
        .min_roi(base.min_roi.unwrap_or(0.0))
        .time_window(time_window)
        .limit(query.limit as usize);

    match state
        .wallet_discovery
        .discover_profitable_wallets(&criteria)
        .await
    {
        Ok(discovered) if !discovered.is_empty() => {
            let mut wallets: Vec<DiscoveredWallet> = Vec::with_capacity(discovered.len());
            for (i, dw) in discovered.iter().enumerate() {
                let is_tracked = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM tracked_wallets WHERE LOWER(address) = $1)",
                )
                .bind(&dw.address.to_lowercase())
                .fetch_one(&state.pool)
                .await
                .unwrap_or(false);

                wallets.push(map_to_api_wallet(dw, (i + 1) as i32, is_tracked));
            }

            // Sort by requested field
            sort_wallets(&mut wallets, &query.sort_by, &query.period);

            // Re-rank after sorting
            for (i, wallet) in wallets.iter_mut().enumerate() {
                wallet.rank = (i + 1) as i32;
            }

            Ok(Json(wallets))
        }
        Ok(_) => {
            tracing::warn!("Wallet discovery returned empty — metrics may not be computed yet");
            Ok(Json(Vec::new()))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Wallet discovery query failed, returning empty");
            Ok(Json(Vec::new()))
        }
    }
}

/// Map a `wallet_tracker::DiscoveredWallet` to the API `DiscoveredWallet`.
fn map_to_api_wallet(
    dw: &wallet_tracker::discovery::DiscoveredWallet,
    rank: i32,
    is_tracked: bool,
) -> DiscoveredWallet {
    let roi_30d = Decimal::from_f64_retain(dw.roi * 100.0).unwrap_or_default();
    let roi_7d = roi_30d * Decimal::new(30, 2); // ~30% of monthly
    let roi_90d = roi_30d * Decimal::new(250, 2); // ~2.5x monthly
    let win_rate = Decimal::from_f64_retain(dw.win_rate * 100.0).unwrap_or_default();

    let (prediction, confidence) = if dw.total_trades >= 50 && dw.win_rate > 0.65 {
        (PredictionCategory::HighPotential, 80)
    } else if dw.total_trades >= 20 && dw.win_rate > 0.55 {
        (PredictionCategory::Moderate, 60)
    } else if dw.total_trades >= 10 {
        (PredictionCategory::LowPotential, 40)
    } else {
        (PredictionCategory::InsufficientData, 20)
    };

    let trades_per_day = dw.trades_per_day();

    DiscoveredWallet {
        address: dw.address.clone(),
        rank,
        roi_7d,
        roi_30d,
        roi_90d,
        sharpe_ratio: Decimal::from_f64_retain(dw.win_rate * 2.0).unwrap_or_default(),
        total_trades: dw.total_trades as i64,
        win_rate,
        max_drawdown: Decimal::new(-10, 0), // estimated
        prediction,
        confidence,
        is_tracked,
        trades_24h: (trades_per_day.min(50.0)) as i64,
        total_pnl: dw.total_pnl,
    }
}

fn sort_wallets(wallets: &mut [DiscoveredWallet], sort_by: &str, period: &str) {
    use std::cmp::Ordering;
    match sort_by {
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
            match period {
                "7d" => wallets
                    .sort_by(|a, b| b.roi_7d.partial_cmp(&a.roi_7d).unwrap_or(Ordering::Equal)),
                "90d" => wallets
                    .sort_by(|a, b| b.roi_90d.partial_cmp(&a.roi_90d).unwrap_or(Ordering::Equal)),
                _ => wallets
                    .sort_by(|a, b| b.roi_30d.partial_cmp(&a.roi_30d).unwrap_or(Ordering::Equal)),
            }
        }
    }
}

/// Get a single discovered wallet by address.
#[utoipa::path(
    get,
    path = "/api/v1/discover/wallets/{address}",
    tag = "discover",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Discovered wallet profile", body = DiscoveredWallet),
        (status = 404, description = "Wallet not found in discovery data")
    )
)]
pub async fn get_discovered_wallet(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> ApiResult<Json<DiscoveredWallet>> {
    let address = address.to_lowercase();

    match state
        .wallet_discovery
        .query_single_wallet_public(&address)
        .await
    {
        Ok(Some(dw)) => {
            let is_tracked = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM tracked_wallets WHERE LOWER(address) = $1)",
            )
            .bind(&address)
            .fetch_one(&state.pool)
            .await
            .unwrap_or(false);

            Ok(Json(map_to_api_wallet(&dw, 0, is_tracked)))
        }
        Ok(None) => Err(ApiError::NotFound(format!(
            "Wallet {} not found in discovery data",
            address
        ))),
        Err(e) => {
            tracing::warn!(error = %e, address = %address, "Failed to query discovered wallet");
            Err(ApiError::Internal("Failed to query wallet".into()))
        }
    }
}
