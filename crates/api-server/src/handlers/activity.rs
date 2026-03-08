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

fn source_label(source: i16) -> &'static str {
    match source {
        1 => "arbitrage",
        2 => "legacy",
        3 => "quant",
        _ => "manual",
    }
}

fn classify_activity_type(source: i16, status: i16, side: i16) -> &'static str {
    match source {
        1 => match status {
            3 => {
                if side == 0 {
                    "ARB_POSITION_OPENED"
                } else {
                    "ARB_POSITION_CLOSED"
                }
            }
            4..=6 => {
                if side == 0 {
                    "ARB_EXECUTION_FAILED"
                } else {
                    "ARB_EXIT_FAILED"
                }
            }
            _ => "ARBITRAGE_DETECTED",
        },
        3 => match status {
            3 => {
                if side == 0 {
                    "POSITION_OPENED"
                } else {
                    "POSITION_CLOSED"
                }
            }
            4..=6 => "TRADE_FAILED",
            _ => "TRADE_PENDING",
        },
        _ => match status {
            3 => {
                if side == 0 {
                    "POSITION_OPENED"
                } else {
                    "POSITION_CLOSED"
                }
            }
            4..=6 => "TRADE_FAILED",
            _ => "TRADE_PENDING",
        },
    }
}

fn build_activity_message(
    source: i16,
    status: i16,
    side: i16,
    market_short: &str,
    filled_quantity: Decimal,
    average_price: Decimal,
    error_message: Option<&str>,
) -> String {
    let direction = if side == 0 { "buy" } else { "sell" };

    match source {
        1 => match status {
            3 => {
                if side == 0 {
                    format!(
                        "Arb position opened on {} qty={} @ ${}",
                        market_short, filled_quantity, average_price
                    )
                } else {
                    format!(
                        "Arb position closed on {} qty={} @ ${}",
                        market_short, filled_quantity, average_price
                    )
                }
            }
            4..=6 => {
                let prefix = if side == 0 {
                    "Arb execution failed"
                } else {
                    "Arb exit failed"
                };
                format!(
                    "{} on {}{}",
                    prefix,
                    market_short,
                    error_message
                        .map(|error| format!(": {error}"))
                        .unwrap_or_default(),
                )
            }
            _ => format!("Arbitrage activity on {}", market_short),
        },
        3 => match status {
            3 => {
                if side == 0 {
                    format!(
                        "Quant position opened on {} qty={} @ ${}",
                        market_short, filled_quantity, average_price
                    )
                } else {
                    format!(
                        "Quant position closed on {} qty={} @ ${}",
                        market_short, filled_quantity, average_price
                    )
                }
            }
            4..=6 => format!(
                "Quant trade failed on {}{}",
                market_short,
                error_message
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default(),
            ),
            _ => format!("Quant trade pending on {}", market_short),
        },
        _ => match status {
            3 => format!(
                "{} {} qty={} @ ${}",
                if side == 0 { "Opened" } else { "Closed" },
                market_short,
                filled_quantity,
                average_price,
            ),
            4..=6 => format!(
                "Failed {} {}{}",
                direction,
                market_short,
                error_message
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default(),
            ),
            _ => format!("Pending {} {}", direction, market_short),
        },
    }
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
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {}", e)))?;

    let activities: Vec<ActivityResponse> = rows
        .into_iter()
        .map(|row| {
            let error_message = row.error_message.clone();
            let market_short = if row.market_id.len() > 20 {
                format!("{}...", &row.market_id[..20])
            } else {
                row.market_id.clone()
            };
            let source_label = source_label(row.source);
            let direction = if row.side == 0 { "buy" } else { "sell" };
            let activity_type =
                classify_activity_type(row.source, row.status, row.side).to_string();
            let message = build_activity_message(
                row.source,
                row.status,
                row.side,
                &market_short,
                row.filled_quantity,
                row.average_price,
                error_message.as_deref(),
            );

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
