//! Activity feed handler — serves persisted copy trade history as activity items.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::error::ApiResult;
use crate::state::AppState;

/// Activity item returned by the API.
#[derive(Debug, Serialize, ToSchema)]
pub struct ActivityResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub activity_type: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pnl: Option<Decimal>,
    pub created_at: DateTime<Utc>,
}

/// Query parameters for the activity endpoint.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ActivityQuery {
    /// Maximum results (default 50).
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, sqlx::FromRow)]
struct ActivityRow {
    id: Uuid,
    status: i16,
    source_wallet: String,
    source_market_id: String,
    source_direction: i16,
    copy_price: Option<Decimal>,
    copy_quantity: Option<Decimal>,
    slippage: Option<Decimal>,
    created_at: DateTime<Utc>,
}

/// List recent activity from copy trade history.
#[utoipa::path(
    get,
    path = "/api/v1/activity",
    params(ActivityQuery),
    responses(
        (status = 200, description = "List of recent activity", body = Vec<ActivityResponse>),
        (status = 500, description = "Internal server error"),
    ),
    tag = "activity"
)]
pub async fn list_activity(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActivityQuery>,
) -> ApiResult<Json<Vec<ActivityResponse>>> {
    let rows: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT
            id, status, source_wallet, source_market_id,
            source_direction, copy_price, copy_quantity,
            slippage, created_at
        FROM copy_trade_history
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(&state.pool)
    .await?;

    let activities: Vec<ActivityResponse> = rows
        .into_iter()
        .map(|row| {
            let direction = if row.source_direction == 0 {
                "buy"
            } else {
                "sell"
            };
            let market_short = if row.source_market_id.len() > 20 {
                format!("{}...", &row.source_market_id[..20])
            } else {
                row.source_market_id.clone()
            };

            let (activity_type, message) = match row.status {
                1 => {
                    let qty = row.copy_quantity.map(|q| q.to_string()).unwrap_or_default();
                    let price = row
                        .copy_price
                        .map(|p| format!("@ ${}", p))
                        .unwrap_or_default();
                    (
                        "TRADE_COPIED".to_string(),
                        format!(
                            "Copied {} {} {qty} {price} from {}",
                            direction,
                            market_short,
                            &row.source_wallet[..8.min(row.source_wallet.len())]
                        ),
                    )
                }
                3 => (
                    "TRADE_COPY_SKIPPED".to_string(),
                    format!(
                        "Skipped {} {} from {}",
                        direction,
                        market_short,
                        &row.source_wallet[..8.min(row.source_wallet.len())]
                    ),
                ),
                4 => (
                    "TRADE_COPY_FAILED".to_string(),
                    format!(
                        "Failed {} {} from {}",
                        direction,
                        market_short,
                        &row.source_wallet[..8.min(row.source_wallet.len())]
                    ),
                ),
                _ => (
                    "TRADE_COPIED".to_string(),
                    format!(
                        "Trade {} {} from {}",
                        direction,
                        market_short,
                        &row.source_wallet[..8.min(row.source_wallet.len())]
                    ),
                ),
            };

            let pnl = row.slippage.filter(|s| *s != Decimal::ZERO);

            ActivityResponse {
                id: row.id.to_string(),
                activity_type,
                message,
                pnl,
                created_at: row.created_at,
            }
        })
        .collect();

    Ok(Json(activities))
}
