//! Wallet data handlers (metrics and trade history).

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Wallet metrics response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WalletMetricsResponse {
    /// Wallet address.
    pub address: String,
    /// Overall ROI.
    pub roi: Decimal,
    /// Sharpe ratio.
    pub sharpe_ratio: Decimal,
    /// Maximum drawdown.
    pub max_drawdown: Decimal,
    /// Average trade size.
    pub avg_trade_size: Decimal,
    /// Average hold time in hours.
    pub avg_hold_time_hours: f64,
    /// Profit factor (gross profit / gross loss).
    pub profit_factor: Decimal,
    /// Recent performance (30 days).
    pub recent_pnl_30d: Decimal,
    /// Win rate by market category.
    pub category_win_rates: serde_json::Value,
    /// Calculated timestamp.
    pub calculated_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct MetricsRow {
    roi: Decimal,
    sharpe_ratio: Decimal,
    max_drawdown: Decimal,
    avg_trade_size: Decimal,
    avg_hold_time_hours: f64,
    profit_factor: Decimal,
    recent_pnl_30d: Decimal,
    category_win_rates: serde_json::Value,
    calculated_at: DateTime<Utc>,
}

/// Get wallet metrics.
#[utoipa::path(
    get,
    path = "/api/v1/wallets/{address}/metrics",
    tag = "wallets",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet metrics", body = WalletMetricsResponse),
        (status = 404, description = "Wallet not found")
    )
)]
pub async fn get_wallet_metrics(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> ApiResult<Json<WalletMetricsResponse>> {
    // Try to get cached metrics
    let metrics: Option<MetricsRow> = sqlx::query_as(
        r#"
        SELECT roi, sharpe_ratio, max_drawdown, avg_trade_size,
               avg_hold_time_hours, profit_factor, recent_pnl_30d,
               category_win_rates, calculated_at
        FROM wallet_success_metrics
        WHERE wallet_address = $1
        ORDER BY calculated_at DESC
        LIMIT 1
        "#,
    )
    .bind(&address)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match metrics {
        Some(m) => Ok(Json(WalletMetricsResponse {
            address,
            roi: m.roi,
            sharpe_ratio: m.sharpe_ratio,
            max_drawdown: m.max_drawdown,
            avg_trade_size: m.avg_trade_size,
            avg_hold_time_hours: m.avg_hold_time_hours,
            profit_factor: m.profit_factor,
            recent_pnl_30d: m.recent_pnl_30d,
            category_win_rates: m.category_win_rates,
            calculated_at: m.calculated_at,
        })),
        None => {
            // Return default metrics if none calculated yet
            Ok(Json(WalletMetricsResponse {
                address,
                roi: Decimal::ZERO,
                sharpe_ratio: Decimal::ZERO,
                max_drawdown: Decimal::ZERO,
                avg_trade_size: Decimal::ZERO,
                avg_hold_time_hours: 0.0,
                profit_factor: Decimal::ZERO,
                recent_pnl_30d: Decimal::ZERO,
                category_win_rates: serde_json::json!({}),
                calculated_at: Utc::now(),
            }))
        }
    }
}

/// Query parameters for listing wallet trades.
#[derive(Debug, Deserialize, IntoParams)]
pub struct WalletTradesQuery {
    /// Maximum results.
    #[serde(default = "default_trades_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_trades_limit() -> i64 {
    20
}

/// Wallet trade response.
#[derive(Debug, Serialize, Deserialize, ToSchema, FromRow)]
pub struct WalletTradeResponse {
    /// Transaction hash.
    pub transaction_hash: String,
    /// Wallet address.
    pub wallet_address: String,
    /// Asset identifier.
    pub asset_id: String,
    /// Trade side (BUY/SELL).
    pub side: String,
    /// Trade price.
    pub price: Decimal,
    /// Trade quantity.
    pub quantity: Decimal,
    /// Trade value (price * quantity).
    pub value: Decimal,
    /// Trade timestamp.
    pub timestamp: DateTime<Utc>,
    /// Market title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Outcome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// Get trades for a specific wallet.
#[utoipa::path(
    get,
    path = "/api/v1/wallets/{address}/trades",
    tag = "wallets",
    params(
        ("address" = String, Path, description = "Wallet address"),
        WalletTradesQuery,
    ),
    responses(
        (status = 200, description = "Wallet trades", body = Vec<WalletTradeResponse>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_wallet_trades(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(query): Query<WalletTradesQuery>,
) -> ApiResult<Json<Vec<WalletTradeResponse>>> {
    let trades: Vec<WalletTradeResponse> = sqlx::query_as(
        r#"
        SELECT transaction_hash, wallet_address, asset_id, side,
               price, quantity, value, timestamp, title, outcome
        FROM wallet_trades
        WHERE LOWER(wallet_address) = LOWER($1)
        ORDER BY timestamp DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&address)
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(trades))
}
