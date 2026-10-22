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
    /// 7-day ROI percentage. `null`: only the 30-day window is computed.
    pub roi_7d: Option<Decimal>,
    /// 30-day ROI percentage (realized P&L over the cost of closed lots).
    pub roi_30d: Decimal,
    /// 90-day ROI percentage. `null`: only the 30-day window is computed.
    pub roi_90d: Option<Decimal>,
    /// 30-day Sharpe ratio. `null` when the wallet's metrics are not computed.
    pub sharpe_ratio: Option<Decimal>,
    /// Total trades count.
    pub total_trades: i64,
    /// Win rate percentage.
    pub win_rate: Decimal,
    /// 30-day maximum drawdown as a negative percentage (e.g. `-12.5`). `null`
    /// when the wallet's metrics are not computed.
    pub max_drawdown: Option<Decimal>,
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

/// Most wallets `/discover/wallets` returns, whatever the client asks for.
const MAX_DISCOVER_LIMIT: i64 = 100;

/// Most trades `/discover/trades` fetches and returns.
const MAX_LIVE_TRADES_LIMIT: i64 = 200;

/// Clamp a client-supplied page size to `1..=max`. A negative value must not
/// wrap when converted to an unsigned size.
fn clamp_limit(requested: i64, max: i64) -> i64 {
    requested.clamp(1, max)
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
    let limit = clamp_limit(query.limit, MAX_LIVE_TRADES_LIMIT);
    match state.clob_client.get_recent_trades(limit as u32).await {
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

    let limit = clamp_limit(query.limit, MAX_DISCOVER_LIMIT);
    let criteria = DiscoveryCriteria::new()
        .min_trades(query.min_trades.unwrap_or(1).max(0) as u64)
        .min_win_rate(
            query
                .min_win_rate
                .map(|w| {
                    let pct: f64 = w.try_into().unwrap_or(0.0);
                    (pct / 100.0).clamp(0.0, 1.0)
                })
                .unwrap_or(base.min_win_rate),
        )
        .min_volume(base.min_volume)
        .min_roi(base.min_roi.unwrap_or(0.0))
        .time_window(time_window)
        .limit(limit as usize);

    match state
        .wallet_discovery
        .discover_profitable_wallets(&criteria)
        .await
    {
        Ok(discovered) if !discovered.is_empty() => {
            // Batch-fetch strategy_type for all addresses
            let addresses: Vec<String> = discovered.iter().map(|dw| dw.address.clone()).collect();
            let strategy_map = fetch_strategy_types(&state.pool, &addresses).await;
            let risk_map = fetch_risk_metrics(&state.pool, &addresses).await;

            let mut wallets: Vec<DiscoveredWallet> = Vec::with_capacity(discovered.len());
            for (i, dw) in discovered.iter().enumerate() {
                // TODO: tracked_wallets table has been dropped with the copy-trading system.
                let is_tracked = false;

                let key = dw.address.to_lowercase();
                let strategy_type = strategy_map.get(&key).cloned();
                wallets.push(map_to_api_wallet(
                    dw,
                    (i + 1) as i32,
                    is_tracked,
                    strategy_type,
                    risk_map.get(&key),
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

/// Computed 30-day risk metrics of a wallet (from `wallet_success_metrics`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct WalletRiskMetrics {
    sharpe_ratio: f64,
    /// Maximum drawdown as a positive ratio (0.2 = 20%).
    max_drawdown: f64,
}

/// Batch-fetch the computed Sharpe ratio and max drawdown of wallets.
///
/// Rows whose metrics were never computed (freshness timestamp at the epoch, as
/// written when only a longer window's ROI was stored) are skipped, so a missing
/// entry means "not computed".
async fn fetch_risk_metrics(
    pool: &sqlx::PgPool,
    addresses: &[String],
) -> std::collections::HashMap<String, WalletRiskMetrics> {
    let mut map = std::collections::HashMap::new();
    if addresses.is_empty() {
        return map;
    }

    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        r#"
        SELECT LOWER(address),
               sharpe_30d::FLOAT8,
               max_drawdown_30d::FLOAT8
        FROM wallet_success_metrics
        WHERE LOWER(address) = ANY($1)
          AND COALESCE(calculated_at, last_computed) > TIMESTAMPTZ '1970-01-02'
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

    for (address, sharpe_ratio, max_drawdown) in rows {
        map.insert(
            address,
            WalletRiskMetrics {
                sharpe_ratio,
                max_drawdown,
            },
        );
    }
    map
}

/// Map a `wallet_tracker::DiscoveredWallet` to the API `DiscoveredWallet`.
///
/// Metrics that are not computed are `None` (JSON `null`) rather than estimated
/// from others: only the 30-day ROI exists, so the 7- and 90-day ROIs are
/// `null`; Sharpe ratio and max drawdown come from `risk` when the wallet's
/// metrics were computed.
fn map_to_api_wallet(
    dw: &wallet_tracker::discovery::DiscoveredWallet,
    rank: i32,
    is_tracked: bool,
    strategy_type: Option<String>,
    risk: Option<&WalletRiskMetrics>,
) -> DiscoveredWallet {
    let roi_30d = Decimal::from_f64_retain(dw.roi * 100.0).unwrap_or_default();
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
        roi_7d: None,
        roi_30d,
        roi_90d: None,
        sharpe_ratio: risk.and_then(|r| Decimal::from_f64_retain(r.sharpe_ratio)),
        total_trades: dw.total_trades as i64,
        win_rate,
        // Negative percentage; `0.0 - x` keeps a zero drawdown at +0 rather than -0.
        max_drawdown: risk.and_then(|r| Decimal::from_f64_retain(0.0 - r.max_drawdown * 100.0)),
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

/// Order `a` before `b` when `a`'s optional value is larger; missing values
/// always sort last.
fn desc_none_last(a: Option<Decimal>, b: Option<Decimal>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Sort the page of wallets by the requested field. Only 30-day metrics exist,
/// so `roi` orders by the 30-day ROI whatever `period` is. Wallets without a
/// value for the sort field (composite score, Sharpe ratio) come last.
fn sort_wallets(wallets: &mut [DiscoveredWallet], sort_by: &str, _period: &str) {
    match sort_by {
        "composite" => wallets.sort_by(|a, b| desc_none_last(a.composite_score, b.composite_score)),
        "sharpe" => wallets.sort_by(|a, b| desc_none_last(a.sharpe_ratio, b.sharpe_ratio)),
        "winRate" => wallets.sort_by_key(|wallet| std::cmp::Reverse(wallet.win_rate)),
        "trades" => wallets.sort_by_key(|wallet| std::cmp::Reverse(wallet.total_trades)),
        _ => wallets.sort_by_key(|wallet| std::cmp::Reverse(wallet.roi_30d)),
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
            // TODO: tracked_wallets table has been dropped with the copy-trading system.
            let is_tracked = false;

            let strategy_map =
                fetch_strategy_types(&state.pool, std::slice::from_ref(&address)).await;
            let strategy_type = strategy_map.get(&address).cloned();
            let risk_map = fetch_risk_metrics(&state.pool, std::slice::from_ref(&address)).await;

            Ok(Json(map_to_api_wallet(
                &dw,
                0,
                is_tracked,
                strategy_type,
                risk_map.get(&address),
            )))
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

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_tracker::discovery::DiscoveredWallet as CoreWallet;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    fn core_wallet(address: &str, win_rate: f64, roi: f64) -> CoreWallet {
        CoreWallet {
            address: address.to_string(),
            total_trades: 80,
            win_count: (80.0 * win_rate) as u64,
            loss_count: 80 - (80.0 * win_rate) as u64,
            win_rate,
            total_volume: d("12000"),
            total_pnl: d("900"),
            roi,
            first_trade: Utc::now() - chrono::Duration::days(40),
            last_trade: Utc::now() - chrono::Duration::days(1),
            is_bot: false,
            bot_score: Some(5),
            discovered_at: Utc::now(),
            composite_score: Some(0.7),
        }
    }

    #[test]
    fn page_sizes_are_clamped_and_never_wrap() {
        assert_eq!(clamp_limit(-5, 100), 1);
        assert_eq!(clamp_limit(i64::MIN, 100), 1);
        assert_eq!(clamp_limit(0, 100), 1);
        assert_eq!(clamp_limit(1, 100), 1);
        assert_eq!(clamp_limit(20, 100), 20);
        assert_eq!(clamp_limit(100, 100), 100);
        assert_eq!(clamp_limit(101, 100), 100);
        assert_eq!(clamp_limit(i64::MAX, 100), 100);
        // `as usize` / `as u32` on the clamped value is always a sane size.
        assert_eq!(clamp_limit(-1, MAX_DISCOVER_LIMIT) as usize, 1);
        assert_eq!(clamp_limit(-1, MAX_LIVE_TRADES_LIMIT) as u32, 1);
        assert_eq!(clamp_limit(1_000_000, MAX_LIVE_TRADES_LIMIT) as u32, 200);
    }

    #[test]
    fn uncomputed_metrics_are_null_not_estimated() {
        let wallet = core_wallet("0xabc", 0.90, 0.25);
        let api = map_to_api_wallet(&wallet, 1, false, None, None);

        // Only the 30-day ROI exists.
        assert_eq!(api.roi_30d, d("25"));
        assert_eq!(api.roi_7d, None, "was roi_30d * 0.30");
        assert_eq!(api.roi_90d, None, "was roi_30d * 2.5");
        // Neither Sharpe nor drawdown is derivable from the win rate.
        assert_eq!(api.sharpe_ratio, None, "was win_rate * 2");
        assert_eq!(api.max_drawdown, None, "was a constant -10");

        let json = serde_json::to_value(&api).unwrap();
        for field in ["roi_7d", "roi_90d", "sharpe_ratio", "max_drawdown"] {
            assert!(
                json.get(field).is_some_and(serde_json::Value::is_null),
                "{field} must be present and null"
            );
        }
        assert_eq!(json["win_rate"], serde_json::json!(d("90")));
    }

    #[test]
    fn computed_risk_metrics_are_reported_as_computed() {
        let wallet = core_wallet("0xabc", 0.60, 0.10);
        let risk = WalletRiskMetrics {
            sharpe_ratio: 1.75,
            max_drawdown: 0.125,
        };
        let api = map_to_api_wallet(&wallet, 1, false, Some("Momentum".into()), Some(&risk));
        assert_eq!(api.sharpe_ratio, Some(d("1.75")));
        assert_eq!(api.max_drawdown, Some(d("-12.5")));
        assert_eq!(api.roi_7d, None);
        assert_eq!(api.roi_90d, None);
        assert_eq!(api.strategy_type.as_deref(), Some("Momentum"));

        // A zero drawdown is a plain zero.
        let flat = WalletRiskMetrics {
            sharpe_ratio: 0.0,
            max_drawdown: 0.0,
        };
        let api = map_to_api_wallet(&wallet, 1, false, None, Some(&flat));
        assert_eq!(api.max_drawdown, Some(Decimal::ZERO));
        assert_eq!(api.max_drawdown.unwrap().to_string(), "0");
    }

    fn ranked(addresses: &[&str]) -> Vec<DiscoveredWallet> {
        addresses
            .iter()
            .enumerate()
            .map(|(i, a)| {
                map_to_api_wallet(&core_wallet(a, 0.6, 0.1), i as i32 + 1, false, None, None)
            })
            .collect()
    }

    #[test]
    fn missing_values_sort_last() {
        let mut wallets = ranked(&["0xa", "0xb", "0xc"]);
        wallets[0].sharpe_ratio = None;
        wallets[1].sharpe_ratio = Some(d("0.5"));
        wallets[2].sharpe_ratio = Some(d("2.0"));
        sort_wallets(&mut wallets, "sharpe", "30d");
        let order: Vec<_> = wallets.iter().map(|w| w.address.as_str()).collect();
        assert_eq!(order, vec!["0xc", "0xb", "0xa"]);

        let mut wallets = ranked(&["0xa", "0xb", "0xc"]);
        wallets[0].composite_score = None;
        wallets[1].composite_score = Some(d("40"));
        wallets[2].composite_score = Some(d("90"));
        sort_wallets(&mut wallets, "composite", "30d");
        let order: Vec<_> = wallets.iter().map(|w| w.address.as_str()).collect();
        assert_eq!(order, vec!["0xc", "0xb", "0xa"]);
    }

    #[test]
    fn roi_sort_uses_the_only_computed_window() {
        for period in ["7d", "30d", "90d"] {
            let mut wallets = ranked(&["0xa", "0xb", "0xc"]);
            wallets[0].roi_30d = d("5");
            wallets[1].roi_30d = d("50");
            wallets[2].roi_30d = d("20");
            sort_wallets(&mut wallets, "roi", period);
            let order: Vec<_> = wallets.iter().map(|w| w.address.as_str()).collect();
            assert_eq!(order, vec!["0xb", "0xc", "0xa"], "period {period}");
        }
    }

    #[test]
    fn other_sort_fields_keep_working() {
        let mut wallets = ranked(&["0xa", "0xb", "0xc"]);
        wallets[0].win_rate = d("55");
        wallets[1].win_rate = d("80");
        wallets[2].win_rate = d("65");
        sort_wallets(&mut wallets, "winRate", "30d");
        assert_eq!(wallets[0].address, "0xb");

        let mut wallets = ranked(&["0xa", "0xb", "0xc"]);
        wallets[0].total_trades = 10;
        wallets[1].total_trades = 300;
        wallets[2].total_trades = 40;
        sort_wallets(&mut wallets, "trades", "30d");
        let order: Vec<_> = wallets.iter().map(|w| w.address.as_str()).collect();
        assert_eq!(order, vec!["0xb", "0xc", "0xa"]);
    }
}
