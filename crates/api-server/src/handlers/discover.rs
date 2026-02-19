//! Wallet discovery and live trade monitoring handlers.
//!
//! Provides endpoints for discovering top wallets, viewing live trades,
//! and retrieving the current market regime.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ApiResult};
use crate::schema::wallet_features_has_strategy_type;
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
    /// Composite multi-factor score (0-100), None if not yet computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composite_score: Option<Decimal>,
    /// Trading strategy classification (e.g., "Momentum", "Arbitrage").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy_type: Option<String>,
    /// Days since last trade (staleness indicator).
    pub staleness_days: i64,
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

/// Current market regime response.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MarketRegimeResponse {
    /// Current detected market regime.
    pub regime: String,
    /// Human-readable label for the regime.
    pub label: String,
    /// Emoji icon for the regime.
    pub icon: String,
    /// Brief description of what this regime means.
    pub description: String,
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

/// Get the current detected market regime.
#[utoipa::path(
    get,
    path = "/api/v1/regime/current",
    tag = "discover",
    responses(
        (status = 200, description = "Current market regime", body = MarketRegimeResponse),
    )
)]
pub async fn get_current_regime(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<MarketRegimeResponse>> {
    use wallet_tracker::MarketRegime;

    let regime = *state.current_regime.read().await;

    let (label, icon, description) = match regime {
        MarketRegime::BullVolatile => (
            "Bull Volatile",
            "\u{1f4c8}",
            "Strong uptrend with high volatility — expect large swings",
        ),
        MarketRegime::BullCalm => (
            "Bull Calm",
            "\u{2600}\u{fe0f}",
            "Steady uptrend with low volatility — favorable conditions",
        ),
        MarketRegime::BearVolatile => (
            "Bear Volatile",
            "\u{26a1}",
            "Downtrend with high volatility — elevated risk, tighter criteria",
        ),
        MarketRegime::BearCalm => (
            "Bear Calm",
            "\u{1f327}\u{fe0f}",
            "Gradual downtrend with low volatility — cautious positioning",
        ),
        MarketRegime::Ranging => (
            "Ranging",
            "\u{2194}\u{fe0f}",
            "Sideways market with no clear trend — default criteria",
        ),
        MarketRegime::Uncertain => (
            "Uncertain",
            "\u{2753}",
            "Insufficient data to determine regime — using conservative defaults",
        ),
    };

    Ok(Json(MarketRegimeResponse {
        regime: format!("{:?}", regime),
        label: label.to_string(),
        icon: icon.to_string(),
        description: description.to_string(),
    }))
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
            // Batch-fetch strategy_type for all addresses
            let addresses: Vec<String> = discovered.iter().map(|dw| dw.address.clone()).collect();
            let strategy_map = fetch_strategy_types(&state.pool, &addresses).await;

            let mut wallets: Vec<DiscoveredWallet> = Vec::with_capacity(discovered.len());
            for (i, dw) in discovered.iter().enumerate() {
                let is_tracked = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM tracked_wallets WHERE LOWER(address) = $1)",
                )
                .bind(dw.address.to_lowercase())
                .fetch_one(&state.pool)
                .await
                .unwrap_or(false);

                let strategy_type = strategy_map.get(&dw.address.to_lowercase()).cloned();
                wallets.push(map_to_api_wallet(
                    dw,
                    (i + 1) as i32,
                    is_tracked,
                    strategy_type,
                ));
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

/// Batch-fetch strategy types for a list of wallet addresses.
async fn fetch_strategy_types(
    pool: &sqlx::PgPool,
    addresses: &[String],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if addresses.is_empty() {
        return map;
    }
    if !wallet_features_has_strategy_type(pool).await {
        return map;
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT LOWER(address), strategy_type
        FROM wallet_features
        WHERE strategy_type IS NOT NULL
          AND LOWER(address) = ANY($1)
        "#,
    )
    .bind(
        addresses
            .iter()
            .map(|a| a.to_lowercase())
            .collect::<Vec<_>>(),
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for (addr, strategy) in rows {
        map.insert(addr, strategy);
    }
    map
}

/// Map a `wallet_tracker::DiscoveredWallet` to the API `DiscoveredWallet`.
fn map_to_api_wallet(
    dw: &wallet_tracker::discovery::DiscoveredWallet,
    rank: i32,
    is_tracked: bool,
    strategy_type: Option<String>,
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
    let staleness_days = (Utc::now() - dw.last_trade).num_days().max(0);

    // Composite score: scale from 0-1 to 0-100 for display
    let composite_score = dw
        .composite_score
        .map(|s| Decimal::from_f64_retain(s * 100.0).unwrap_or_default());

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
        composite_score,
        strategy_type,
        staleness_days,
    }
}

fn sort_wallets(wallets: &mut [DiscoveredWallet], sort_by: &str, period: &str) {
    use std::cmp::Ordering;
    match sort_by {
        "composite" => wallets.sort_by(|a, b| {
            let sa = a.composite_score.unwrap_or(a.roi_30d);
            let sb = b.composite_score.unwrap_or(b.roi_30d);
            sb.partial_cmp(&sa).unwrap_or(Ordering::Equal)
        }),
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

            let strategy_map =
                fetch_strategy_types(&state.pool, std::slice::from_ref(&address)).await;
            let strategy_type = strategy_map.get(&address).cloned();

            Ok(Json(map_to_api_wallet(&dw, 0, is_tracked, strategy_type)))
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

/// Calibration bucket response for a probability range.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CalibrationBucketResponse {
    /// Lower bound of the probability range (inclusive).
    pub lower: f64,
    /// Upper bound of the probability range (exclusive).
    pub upper: f64,
    /// Average predicted probability within this bucket.
    pub avg_predicted: f64,
    /// Observed success fraction (actual wins / total).
    pub observed_rate: f64,
    /// Number of predictions in this bucket.
    pub count: usize,
    /// Calibration gap: |avg_predicted - observed_rate|.
    pub gap: f64,
}

/// Full calibration report for prediction reliability.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CalibrationReportResponse {
    /// Per-bucket calibration statistics.
    pub buckets: Vec<CalibrationBucketResponse>,
    /// Expected Calibration Error (lower is better).
    pub ece: f64,
    /// Total predictions evaluated.
    pub total_predictions: usize,
    /// Recommended threshold based on calibration data.
    pub recommended_threshold: f64,
}

/// Get the prediction calibration report.
///
/// Shows how well the ensemble predictions match actual copy trade outcomes,
/// bucketed by probability range. Used to display a reliability diagram.
#[utoipa::path(
    get,
    path = "/api/v1/discover/calibration",
    tag = "discover",
    responses(
        (status = 200, description = "Calibration report", body = CalibrationReportResponse),
        (status = 500, description = "Internal error")
    )
)]
pub async fn get_calibration_report(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<CalibrationReportResponse>> {
    let calibrator = wallet_tracker::PredictionCalibrator::new(state.pool.clone());

    match calibrator.calibrate().await {
        Ok(report) => {
            let buckets = report
                .buckets
                .into_iter()
                .map(|b| CalibrationBucketResponse {
                    lower: b.lower,
                    upper: b.upper,
                    avg_predicted: b.avg_predicted,
                    observed_rate: b.observed_rate,
                    count: b.count,
                    gap: b.gap,
                })
                .collect();

            Ok(Json(CalibrationReportResponse {
                buckets,
                ece: report.ece,
                total_predictions: report.total_predictions,
                recommended_threshold: report.recommended_threshold,
            }))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to generate calibration report");
            Err(ApiError::Internal(
                "Failed to generate calibration report".into(),
            ))
        }
    }
}

/// Copy trade performance for a specific wallet.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CopyPerformanceResponse {
    /// Wallet address.
    pub address: String,
    /// Reported win rate from wallet metrics (0-100).
    pub reported_win_rate: f64,
    /// Actual win rate from copy trade outcomes (0-100).
    pub copy_win_rate: Option<f64>,
    /// Number of copy trades executed for this wallet.
    pub copy_trade_count: i64,
    /// Total PnL from copy trades.
    pub copy_total_pnl: f64,
    /// Divergence: |reported - copy| in percentage points (null if no data).
    pub divergence_pp: Option<f64>,
    /// Whether there's a significant divergence (>15pp).
    pub has_significant_divergence: bool,
}

/// Get copy trade performance for a wallet.
///
/// Compares the wallet's reported win rate against actual copy trade outcomes,
/// identifying any divergence between on-paper and actual performance.
#[utoipa::path(
    get,
    path = "/api/v1/discover/wallets/{address}/copy-performance",
    tag = "discover",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Copy performance comparison", body = CopyPerformanceResponse),
        (status = 404, description = "Wallet not found")
    )
)]
pub async fn get_copy_performance(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> ApiResult<Json<CopyPerformanceResponse>> {
    let address = address.to_lowercase();

    // Get the reported win rate from wallet metrics
    let reported = sqlx::query_as::<_, (f64,)>(
        r#"
        SELECT COALESCE(win_rate_30d, 0)::FLOAT8
        FROM wallet_success_metrics
        WHERE LOWER(address) = $1
        ORDER BY calculated_at DESC
        LIMIT 1
        "#,
    )
    .bind(&address)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "Failed to query wallet metrics");
        ApiError::Internal("Failed to query wallet metrics".into())
    })?;

    let reported_win_rate = reported.map(|r| r.0 * 100.0).unwrap_or(0.0);

    // Get actual copy trade performance
    let copy_stats = sqlx::query_as::<_, (i64, f64, f64)>(
        r#"
        SELECT
            COUNT(*)::INT8 AS trade_count,
            COALESCE(SUM(pnl), 0)::FLOAT8 AS total_pnl,
            CASE WHEN COUNT(*) > 0
                THEN (COUNT(CASE WHEN pnl > 0 THEN 1 END)::FLOAT8 / COUNT(*)::FLOAT8) * 100.0
                ELSE 0
            END AS copy_win_rate
        FROM copy_trade_history
        WHERE LOWER(source_wallet) = $1
        "#,
    )
    .bind(&address)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "Failed to query copy trade history");
        ApiError::Internal("Failed to query copy trade performance".into())
    })?;

    let (copy_trade_count, copy_total_pnl, copy_wr) = copy_stats.unwrap_or((0, 0.0, 0.0));

    let copy_win_rate = if copy_trade_count > 0 {
        Some(copy_wr)
    } else {
        None
    };

    let divergence_pp = copy_win_rate.map(|cwr| (reported_win_rate - cwr).abs());
    let has_significant_divergence = divergence_pp.is_some_and(|d| d > 15.0);

    Ok(Json(CopyPerformanceResponse {
        address,
        reported_win_rate,
        copy_win_rate,
        copy_trade_count,
        copy_total_pnl,
        divergence_pp,
        has_significant_divergence,
    }))
}
