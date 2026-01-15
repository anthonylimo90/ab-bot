//! Wallet tracking handlers for copy trading.

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

/// Tracked wallet response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TrackedWalletResponse {
    /// Wallet address.
    pub address: String,
    /// Display name/label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether copy trading is enabled.
    pub copy_enabled: bool,
    /// Allocation percentage (0-100).
    pub allocation_pct: Decimal,
    /// Maximum position size in USD.
    pub max_position_size: Decimal,
    /// Success score (0-100).
    pub success_score: Decimal,
    /// Total profit/loss.
    pub total_pnl: Decimal,
    /// Win rate percentage.
    pub win_rate: Decimal,
    /// Total trades count.
    pub total_trades: i64,
    /// Added timestamp.
    pub added_at: DateTime<Utc>,
    /// Last activity timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<DateTime<Utc>>,
}

/// Request to add a tracked wallet.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddWalletRequest {
    /// Wallet address.
    pub address: String,
    /// Display name/label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Enable copy trading immediately.
    #[serde(default)]
    pub copy_enabled: bool,
    /// Allocation percentage (0-100).
    #[serde(default = "default_allocation")]
    pub allocation_pct: Decimal,
    /// Maximum position size in USD.
    #[serde(default = "default_max_position")]
    pub max_position_size: Decimal,
}

fn default_allocation() -> Decimal {
    Decimal::new(10, 0) // 10%
}

fn default_max_position() -> Decimal {
    Decimal::new(1000, 0) // $1000
}

/// Request to update a tracked wallet.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateWalletRequest {
    /// Display name/label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Enable/disable copy trading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_enabled: Option<bool>,
    /// Allocation percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocation_pct: Option<Decimal>,
    /// Maximum position size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_position_size: Option<Decimal>,
}

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

/// Query parameters for listing wallets.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListWalletsQuery {
    /// Filter by copy trading enabled.
    pub copy_enabled: Option<bool>,
    /// Minimum success score.
    pub min_score: Option<Decimal>,
    /// Maximum results.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, FromRow)]
struct WalletRow {
    address: String,
    label: Option<String>,
    copy_enabled: bool,
    allocation_pct: Decimal,
    max_position_size: Decimal,
    success_score: Decimal,
    total_pnl: Decimal,
    win_rate: Decimal,
    total_trades: i64,
    added_at: DateTime<Utc>,
    last_activity: Option<DateTime<Utc>>,
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

/// List tracked wallets.
#[utoipa::path(
    get,
    path = "/api/v1/wallets",
    tag = "wallets",
    params(ListWalletsQuery),
    responses(
        (status = 200, description = "List of tracked wallets", body = Vec<TrackedWalletResponse>),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_tracked_wallets(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListWalletsQuery>,
) -> ApiResult<Json<Vec<TrackedWalletResponse>>> {
    let rows: Vec<WalletRow> = sqlx::query_as(
        r#"
        SELECT address, label, copy_enabled, allocation_pct, max_position_size,
               success_score, total_pnl, win_rate, total_trades, added_at, last_activity
        FROM tracked_wallets
        WHERE ($1::bool IS NULL OR copy_enabled = $1)
          AND ($2::decimal IS NULL OR success_score >= $2)
        ORDER BY success_score DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(query.copy_enabled)
    .bind(query.min_score)
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let wallets: Vec<TrackedWalletResponse> = rows
        .into_iter()
        .map(|row| TrackedWalletResponse {
            address: row.address,
            label: row.label,
            copy_enabled: row.copy_enabled,
            allocation_pct: row.allocation_pct,
            max_position_size: row.max_position_size,
            success_score: row.success_score,
            total_pnl: row.total_pnl,
            win_rate: row.win_rate,
            total_trades: row.total_trades,
            added_at: row.added_at,
            last_activity: row.last_activity,
        })
        .collect();

    Ok(Json(wallets))
}

/// Add a wallet to track.
#[utoipa::path(
    post,
    path = "/api/v1/wallets",
    tag = "wallets",
    request_body = AddWalletRequest,
    responses(
        (status = 201, description = "Wallet added", body = TrackedWalletResponse),
        (status = 400, description = "Invalid request"),
        (status = 409, description = "Wallet already tracked")
    )
)]
pub async fn add_tracked_wallet(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddWalletRequest>,
) -> ApiResult<Json<TrackedWalletResponse>> {
    // Validate address format (basic check)
    if !request.address.starts_with("0x") || request.address.len() != 42 {
        return Err(ApiError::BadRequest(
            "Invalid wallet address format".to_string(),
        ));
    }

    // Check if already tracked
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT COUNT(*) FROM tracked_wallets WHERE address = $1")
            .bind(&request.address)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    if existing.map(|r| r.0).unwrap_or(0) > 0 {
        return Err(ApiError::Conflict("Wallet already tracked".to_string()));
    }

    // Insert new tracked wallet
    let now = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO tracked_wallets
        (address, label, copy_enabled, allocation_pct, max_position_size,
         success_score, total_pnl, win_rate, total_trades, added_at)
        VALUES ($1, $2, $3, $4, $5, 0, 0, 0, 0, $6)
        "#,
    )
    .bind(&request.address)
    .bind(&request.label)
    .bind(request.copy_enabled)
    .bind(request.allocation_pct)
    .bind(request.max_position_size)
    .bind(now)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(TrackedWalletResponse {
        address: request.address,
        label: request.label,
        copy_enabled: request.copy_enabled,
        allocation_pct: request.allocation_pct,
        max_position_size: request.max_position_size,
        success_score: Decimal::ZERO,
        total_pnl: Decimal::ZERO,
        win_rate: Decimal::ZERO,
        total_trades: 0,
        added_at: now,
        last_activity: None,
    }))
}

/// Get a specific tracked wallet.
#[utoipa::path(
    get,
    path = "/api/v1/wallets/{address}",
    tag = "wallets",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet details", body = TrackedWalletResponse),
        (status = 404, description = "Wallet not found")
    )
)]
pub async fn get_wallet(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> ApiResult<Json<TrackedWalletResponse>> {
    let row: Option<WalletRow> = sqlx::query_as(
        r#"
        SELECT address, label, copy_enabled, allocation_pct, max_position_size,
               success_score, total_pnl, win_rate, total_trades, added_at, last_activity
        FROM tracked_wallets
        WHERE address = $1
        "#,
    )
    .bind(&address)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    match row {
        Some(row) => Ok(Json(TrackedWalletResponse {
            address: row.address,
            label: row.label,
            copy_enabled: row.copy_enabled,
            allocation_pct: row.allocation_pct,
            max_position_size: row.max_position_size,
            success_score: row.success_score,
            total_pnl: row.total_pnl,
            win_rate: row.win_rate,
            total_trades: row.total_trades,
            added_at: row.added_at,
            last_activity: row.last_activity,
        })),
        None => Err(ApiError::NotFound(format!(
            "Wallet {} not tracked",
            address
        ))),
    }
}

/// Update a tracked wallet.
#[utoipa::path(
    put,
    path = "/api/v1/wallets/{address}",
    tag = "wallets",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    request_body = UpdateWalletRequest,
    responses(
        (status = 200, description = "Wallet updated", body = TrackedWalletResponse),
        (status = 404, description = "Wallet not found")
    )
)]
pub async fn update_wallet(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Json(request): Json<UpdateWalletRequest>,
) -> ApiResult<Json<TrackedWalletResponse>> {
    // Check if wallet exists
    let existing: Option<WalletRow> = sqlx::query_as(
        r#"
        SELECT address, label, copy_enabled, allocation_pct, max_position_size,
               success_score, total_pnl, win_rate, total_trades, added_at, last_activity
        FROM tracked_wallets
        WHERE address = $1
        "#,
    )
    .bind(&address)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let existing = match existing {
        Some(w) => w,
        None => {
            return Err(ApiError::NotFound(format!(
                "Wallet {} not tracked",
                address
            )))
        }
    };

    // Apply updates
    let label = request.label.or(existing.label);
    let copy_enabled = request.copy_enabled.unwrap_or(existing.copy_enabled);
    let allocation_pct = request.allocation_pct.unwrap_or(existing.allocation_pct);
    let max_position_size = request
        .max_position_size
        .unwrap_or(existing.max_position_size);

    sqlx::query(
        r#"
        UPDATE tracked_wallets
        SET label = $1, copy_enabled = $2, allocation_pct = $3, max_position_size = $4
        WHERE address = $5
        "#,
    )
    .bind(&label)
    .bind(copy_enabled)
    .bind(allocation_pct)
    .bind(max_position_size)
    .bind(&address)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(TrackedWalletResponse {
        address: existing.address,
        label,
        copy_enabled,
        allocation_pct,
        max_position_size,
        success_score: existing.success_score,
        total_pnl: existing.total_pnl,
        win_rate: existing.win_rate,
        total_trades: existing.total_trades,
        added_at: existing.added_at,
        last_activity: existing.last_activity,
    }))
}

/// Remove a tracked wallet.
#[utoipa::path(
    delete,
    path = "/api/v1/wallets/{address}",
    tag = "wallets",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 204, description = "Wallet removed"),
        (status = 404, description = "Wallet not found")
    )
)]
pub async fn remove_wallet(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> ApiResult<()> {
    let result = sqlx::query("DELETE FROM tracked_wallets WHERE address = $1")
        .bind(&address)
        .execute(&state.pool)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!(
            "Wallet {} not tracked",
            address
        )));
    }

    Ok(())
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
    // Check wallet exists
    let exists: Option<(i64,)> =
        sqlx::query_as("SELECT COUNT(*) FROM tracked_wallets WHERE address = $1")
            .bind(&address)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    if exists.map(|r| r.0).unwrap_or(0) == 0 {
        return Err(ApiError::NotFound(format!(
            "Wallet {} not tracked",
            address
        )));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_response_serialization() {
        let wallet = TrackedWalletResponse {
            address: "0x1234567890123456789012345678901234567890".to_string(),
            label: Some("Top Trader".to_string()),
            copy_enabled: true,
            allocation_pct: Decimal::new(20, 0),
            max_position_size: Decimal::new(5000, 0),
            success_score: Decimal::new(85, 0),
            total_pnl: Decimal::new(15000, 2),
            win_rate: Decimal::new(72, 0),
            total_trades: 150,
            added_at: Utc::now(),
            last_activity: Some(Utc::now()),
        };

        let json = serde_json::to_string(&wallet).unwrap();
        assert!(json.contains("0x1234"));
        assert!(json.contains("Top Trader"));
    }

    #[test]
    fn test_add_wallet_request() {
        let request = AddWalletRequest {
            address: "0x1234567890123456789012345678901234567890".to_string(),
            label: Some("New Wallet".to_string()),
            copy_enabled: false,
            allocation_pct: Decimal::new(15, 0),
            max_position_size: Decimal::new(2000, 0),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("0x1234"));
    }

    #[test]
    fn test_metrics_response() {
        let metrics = WalletMetricsResponse {
            address: "0x1234567890123456789012345678901234567890".to_string(),
            roi: Decimal::new(150, 0),
            sharpe_ratio: Decimal::new(185, 2),
            max_drawdown: Decimal::new(12, 0),
            avg_trade_size: Decimal::new(500, 0),
            avg_hold_time_hours: 48.5,
            profit_factor: Decimal::new(220, 2),
            recent_pnl_30d: Decimal::new(3500, 2),
            category_win_rates: serde_json::json!({"crypto": 75, "politics": 68}),
            calculated_at: Utc::now(),
        };

        let json = serde_json::to_string(&metrics).unwrap();
        assert!(json.contains("sharpe_ratio"));
        assert!(json.contains("crypto"));
    }
}
