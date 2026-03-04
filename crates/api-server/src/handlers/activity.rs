//! Activity feed handler — serves recent execution reports as activity items.

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
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
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
struct ExecutionReportRow {
    id: Uuid,
    market_id: String,
    side: i16,
    status: i16,
    filled_quantity: Decimal,
    average_price: Decimal,
    error_message: Option<String>,
    source: i16,
    executed_at: DateTime<Utc>,
}

/// List recent activity from execution reports.
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
    // TODO: copy_trade_history has been dropped; activity is now sourced from execution_reports.
    let rows: Vec<ExecutionReportRow> = sqlx::query_as(
        r#"
        SELECT
            id, market_id, side, status, filled_quantity,
            average_price, error_message, source, executed_at
        FROM execution_reports
        ORDER BY executed_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let activities: Vec<ActivityResponse> = rows
        .into_iter()
        .map(|row| {
            let direction = if row.side == 0 { "buy" } else { "sell" };
            let error_message = row.error_message.clone();
            let market_short = if row.market_id.len() > 20 {
                format!("{}...", &row.market_id[..20])
            } else {
                row.market_id.clone()
            };

            let source_label = match row.source {
                1 => "arbitrage",
                2 => "legacy",
                3 => "stop_loss",
                _ => "manual",
            };

            let (activity_type, message) = match row.status {
                // filled
                3 => (
                    "TRADE_EXECUTED".to_string(),
                    format!(
                        "Executed {} {} qty={} @ ${} ({})",
                        direction,
                        market_short,
                        row.filled_quantity,
                        row.average_price,
                        source_label,
                    ),
                ),
                // cancelled / rejected / expired
                4..=6 => (
                    "TRADE_FAILED".to_string(),
                    format!(
                        "Failed {} {}{}",
                        direction,
                        market_short,
                        error_message
                            .as_ref()
                            .map(|e| format!(": {e}"))
                            .unwrap_or_default(),
                    ),
                ),
                _ => (
                    "TRADE_PENDING".to_string(),
                    format!("Pending {} {}", direction, market_short),
                ),
            };

            let details = Some(serde_json::json!({
                "market_id": row.market_id,
                "direction": direction,
                "source": source_label,
                "status": row.status,
            }));

            ActivityResponse {
                id: row.id.to_string(),
                activity_type,
                message,
                skip_reason: None,
                error_message,
                details,
                pnl: None,
                created_at: row.executed_at,
            }
        })
        .collect();

    Ok(Json(activities))
}
