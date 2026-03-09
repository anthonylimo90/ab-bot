//! Wallet data handlers (metrics and trade history).

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use polymarket_core::api::ClobTrade;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const MAX_WALLET_TRADES_LIMIT: usize = 100;
const MAX_WALLET_TRADES_FETCH: usize = 1_000;

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
        SELECT
            COALESCE(roi, roi_all_time, roi_30d, 0) AS roi,
            COALESCE(sharpe_30d, 0) AS sharpe_ratio,
            COALESCE(max_drawdown_30d, 0) AS max_drawdown,
            COALESCE(avg_trade_size, 0) AS avg_trade_size,
            COALESCE(avg_hold_time_hours, 0) AS avg_hold_time_hours,
            COALESCE(profit_factor, 0) AS profit_factor,
            COALESCE(recent_pnl_30d, 0) AS recent_pnl_30d,
            COALESCE(category_win_rates, '{}'::jsonb) AS category_win_rates,
            COALESCE(calculated_at, last_computed, NOW()) AS calculated_at
        FROM wallet_success_metrics
        WHERE LOWER(COALESCE(wallet_address, address)) = LOWER($1)
        ORDER BY COALESCE(calculated_at, last_computed, NOW()) DESC
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
    /// Market condition identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition_id: Option<String>,
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
    /// Market slug.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
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
    let (limit, offset, fetch_target) = normalize_wallet_trade_window(&query);

    let db_trades = match sqlx::query_as(
        r#"
        SELECT transaction_hash, wallet_address, asset_id, condition_id, side,
               price, quantity, value, timestamp, title, slug, outcome
        FROM wallet_trades
        WHERE LOWER(wallet_address) = LOWER($1)
        ORDER BY timestamp DESC
        LIMIT $2
        "#,
    )
    .bind(&address)
    .bind(fetch_target as i64)
    .fetch_all(&state.pool)
    .await
    {
        Ok(trades) => trades,
        Err(error) => {
            warn!(
                wallet_address = %address,
                error = %error,
                "Failed loading wallet trades from database; falling back to Polymarket activity"
            );
            Vec::new()
        }
    };

    let live_trades = if db_trades.len() < fetch_target {
        let page_size = fetch_target.min(MAX_WALLET_TRADES_LIMIT) as u32;
        let max_pages = fetch_target.div_ceil(page_size as usize);
        match state
            .clob_client
            .get_wallet_activity_paginated(&address, page_size, max_pages)
            .await
        {
            Ok(trades) => trades
                .into_iter()
                .map(wallet_trade_response_from_live_trade)
                .collect(),
            Err(error) if db_trades.is_empty() => {
                return Err(ApiError::Internal(format!(
                    "Failed to fetch wallet trades from Polymarket activity: {error}"
                )));
            }
            Err(error) => {
                warn!(
                    wallet_address = %address,
                    error = %error,
                    "Failed loading wallet activity fallback; returning database-backed trades"
                );
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let trades = merge_wallet_trades(db_trades, live_trades)
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();

    Ok(Json(trades))
}

fn normalize_wallet_trade_window(query: &WalletTradesQuery) -> (usize, usize, usize) {
    let limit = if query.limit <= 0 {
        default_trades_limit() as usize
    } else {
        query.limit as usize
    }
    .min(MAX_WALLET_TRADES_LIMIT);

    let offset = query.offset.max(0) as usize;
    let fetch_target = offset
        .saturating_add(limit)
        .clamp(limit.max(1), MAX_WALLET_TRADES_FETCH);

    (limit.max(1), offset, fetch_target)
}

fn wallet_trade_response_from_live_trade(trade: ClobTrade) -> WalletTradeResponse {
    let price = Decimal::from_f64_retain(trade.price).unwrap_or(Decimal::ZERO);
    let quantity = Decimal::from_f64_retain(trade.size).unwrap_or(Decimal::ZERO);
    let value = price * quantity;
    let timestamp = DateTime::from_timestamp(trade.timestamp, 0).unwrap_or_else(Utc::now);

    WalletTradeResponse {
        transaction_hash: trade.transaction_hash,
        wallet_address: trade.wallet_address.to_lowercase(),
        asset_id: trade.asset_id,
        condition_id: trade.condition_id,
        side: trade.side,
        price,
        quantity,
        value,
        timestamp,
        title: trade.title,
        slug: trade.slug,
        outcome: trade.outcome,
    }
}

fn merge_wallet_trades(
    db_trades: Vec<WalletTradeResponse>,
    live_trades: Vec<WalletTradeResponse>,
) -> Vec<WalletTradeResponse> {
    let mut merged: HashMap<String, WalletTradeResponse> = live_trades
        .into_iter()
        .map(|trade| (trade.transaction_hash.clone(), trade))
        .collect();

    for mut trade in db_trades {
        if let Some(live_trade) = merged.get(&trade.transaction_hash) {
            if trade.condition_id.is_none() {
                trade.condition_id = live_trade.condition_id.clone();
            }
            if trade.title.is_none() {
                trade.title = live_trade.title.clone();
            }
            if trade.slug.is_none() {
                trade.slug = live_trade.slug.clone();
            }
            if trade.outcome.is_none() {
                trade.outcome = live_trade.outcome.clone();
            }
        }
        merged.insert(trade.transaction_hash.clone(), trade);
    }

    let mut trades: Vec<_> = merged.into_values().collect();
    trades.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| right.transaction_hash.cmp(&left.transaction_hash))
    });
    trades
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_trade(transaction_hash: &str, timestamp: i64) -> WalletTradeResponse {
        WalletTradeResponse {
            transaction_hash: transaction_hash.to_string(),
            wallet_address: "0xabc".to_string(),
            asset_id: "asset".to_string(),
            condition_id: None,
            side: "BUY".to_string(),
            price: Decimal::new(55, 2),
            quantity: Decimal::new(2, 0),
            value: Decimal::new(110, 2),
            timestamp: DateTime::from_timestamp(timestamp, 0).unwrap(),
            title: None,
            slug: None,
            outcome: None,
        }
    }

    #[test]
    fn normalize_wallet_trade_window_clamps_limit_and_offset() {
        let query = WalletTradesQuery {
            limit: 500,
            offset: -25,
        };

        let (limit, offset, fetch_target) = normalize_wallet_trade_window(&query);

        assert_eq!(limit, MAX_WALLET_TRADES_LIMIT);
        assert_eq!(offset, 0);
        assert_eq!(fetch_target, MAX_WALLET_TRADES_LIMIT);
    }

    #[test]
    fn merge_wallet_trades_prefers_db_rows_and_hydrates_missing_fields() {
        let mut db_trade = sample_trade("tx-1", 10);
        db_trade.title = Some("DB title".to_string());

        let mut live_trade = sample_trade("tx-1", 10);
        live_trade.condition_id = Some("condition-1".to_string());
        live_trade.slug = Some("market-slug".to_string());
        live_trade.outcome = Some("Yes".to_string());

        let merged = merge_wallet_trades(vec![db_trade], vec![live_trade]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].title.as_deref(), Some("DB title"));
        assert_eq!(merged[0].condition_id.as_deref(), Some("condition-1"));
        assert_eq!(merged[0].slug.as_deref(), Some("market-slug"));
        assert_eq!(merged[0].outcome.as_deref(), Some("Yes"));
    }

    #[test]
    fn merge_wallet_trades_sorts_newest_first() {
        let older = sample_trade("tx-older", 10);
        let newer = sample_trade("tx-newer", 20);

        let merged = merge_wallet_trades(vec![older], vec![newer]);

        assert_eq!(merged[0].transaction_hash, "tx-newer");
        assert_eq!(merged[1].transaction_hash, "tx-older");
    }
}
