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

#[derive(Debug, Deserialize, IntoParams)]
pub struct ActivityQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// The most activity rows one request returns.
const MAX_LIMIT: i64 = 200;

/// The trade lifecycle events the activity feed shows.
const ACTIVITY_EVENTS: [&str; 9] = [
    "position_open",
    "entry_filled",
    "exit_fill",
    "closed_via_exit",
    "closed_via_resolution",
    "position_closed",
    "entry_failed",
    "position_failed",
    "exit_failed",
];

#[derive(Debug, sqlx::FromRow)]
struct TradeEventRow {
    id: Uuid,
    occurred_at: DateTime<Utc>,
    strategy: String,
    execution_mode: String,
    market_id: String,
    position_id: Option<Uuid>,
    event_type: String,
    reason: Option<String>,
    fill_price: Option<Decimal>,
    filled_size_usd: Option<Decimal>,
    realized_pnl: Option<Decimal>,
}

/// The dashboard's activity type for a trade event of `strategy`.
fn activity_type(strategy: &str, event_type: &str) -> &'static str {
    let arb = strategy == "arb";
    match event_type {
        "position_open" if arb => "ARB_POSITION_OPENED",
        "position_open" => "POSITION_OPENED",
        "closed_via_exit" | "closed_via_resolution" | "position_closed" if arb => {
            "ARB_POSITION_CLOSED"
        }
        "closed_via_exit" | "closed_via_resolution" | "position_closed" => "POSITION_CLOSED",
        "entry_failed" | "position_failed" if arb => "ARB_EXECUTION_FAILED",
        "exit_failed" if arb => "ARB_EXIT_FAILED",
        "entry_failed" | "position_failed" | "exit_failed" => "TRADE_FAILED",
        _ => "TRADE_EXECUTED",
    }
}

fn activity_message(row: &TradeEventRow) -> String {
    let market = if row.market_id.chars().count() > 20 {
        format!("{}...", row.market_id.chars().take(20).collect::<String>())
    } else {
        row.market_id.clone()
    };
    let who = if row.strategy == "arb" {
        "Arb".to_string()
    } else {
        format!("{} ({})", capitalized(&row.strategy), row.execution_mode)
    };
    let reason = row
        .reason
        .as_deref()
        .map(|reason| format!(": {reason}"))
        .unwrap_or_default();
    match row.event_type.as_str() {
        "position_open" => format!("{who} position opened on {market}"),
        "closed_via_exit" => format!("{who} position closed via exit on {market}"),
        "closed_via_resolution" => format!("{who} position closed at resolution on {market}"),
        "position_closed" => format!("{who} position closed on {market}"),
        "entry_failed" | "position_failed" => format!("{who} entry failed on {market}{reason}"),
        "exit_failed" => format!("{who} exit failed on {market}{reason}"),
        "entry_filled" | "exit_fill" => {
            let side = if row.event_type == "entry_filled" {
                "bought"
            } else {
                "sold"
            };
            match (row.filled_size_usd, row.fill_price) {
                (Some(size), Some(price)) => {
                    format!(
                        "{who} {side} ${} on {market} @ {}",
                        size.round_dp(2),
                        price.normalize()
                    )
                }
                _ => format!("{who} {side} on {market}"),
            }
        }
        other => format!("{who} {other} on {market}"),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn to_activity(row: TradeEventRow) -> ActivityResponse {
    let activity_type = activity_type(&row.strategy, &row.event_type).to_string();
    let message = activity_message(&row);
    let failed = matches!(
        row.event_type.as_str(),
        "entry_failed" | "position_failed" | "exit_failed"
    );
    let closed = matches!(
        row.event_type.as_str(),
        "closed_via_exit" | "closed_via_resolution" | "position_closed"
    );
    ActivityResponse {
        id: row.id.to_string(),
        activity_type,
        message,
        skip_reason: None,
        error_message: if failed { row.reason.clone() } else { None },
        details: Some(serde_json::json!({
            "market_id": row.market_id,
            "position_id": row.position_id,
            "event_type": row.event_type,
            "strategy": row.strategy,
            "execution_mode": row.execution_mode,
            "source": if row.strategy == "arb" { "arbitrage" } else { "quant" },
        })),
        pnl: if closed { row.realized_pnl } else { None },
        created_at: row.occurred_at,
    }
}

/// List recent trading activity: positions opened and closed, fills and
/// failures, from the canonical trade event log.
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
    _access: crate::workspace_scope::WorkspaceReadAccess,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ActivityQuery>,
) -> ApiResult<Json<Vec<ActivityResponse>>> {
    Ok(Json(
        load_activity(&state.pool, query.limit, query.offset).await?,
    ))
}

async fn load_activity(
    pool: &sqlx::PgPool,
    limit: i64,
    offset: i64,
) -> ApiResult<Vec<ActivityResponse>> {
    let rows: Vec<TradeEventRow> = sqlx::query_as(
        r#"
        SELECT id, occurred_at, strategy, execution_mode, market_id, position_id,
               event_type, reason, fill_price, filled_size_usd, realized_pnl
        FROM trade_events e
        WHERE event_type = ANY($1)
          -- A retried exit that fails again for the same reason is one
          -- failure to show, however often it is retried.
          AND NOT (e.event_type = 'exit_failed' AND e.position_id IS NOT NULL AND EXISTS (
              SELECT 1 FROM (
                  SELECT p.event_type, p.reason FROM trade_events p
                  WHERE p.position_id = e.position_id AND p.event_type = ANY($1)
                    AND (p.occurred_at, p.id) < (e.occurred_at, e.id)
                  ORDER BY p.occurred_at DESC, p.id DESC
                  LIMIT 1
              ) previous
              WHERE previous.event_type = 'exit_failed'
                AND previous.reason IS NOT DISTINCT FROM e.reason))
        ORDER BY occurred_at DESC, id
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&ACTIVITY_EVENTS[..])
    .bind(limit.clamp(1, MAX_LIMIT))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await
    .map_err(|e| crate::error::ApiError::Internal(format!("DB error: {}", e)))?;
    Ok(rows.into_iter().map(to_activity).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Executor;

    #[sqlx::test(migrations = false)]
    async fn activity_comes_from_the_trade_event_log(pool: sqlx::PgPool) {
        let schema = include_str!("../../../../migrations/048_trade_events.sql")
            .replace("uuid_generate_v4()", "gen_random_uuid()");
        pool.execute(schema.as_str()).await.unwrap();
        let position = Uuid::new_v4();
        let event = |strategy: &'static str,
                     event_type: &'static str,
                     minutes_ago: i32,
                     reason: Option<&'static str>,
                     pnl: Option<Decimal>| {
            sqlx::query(
                "INSERT INTO trade_events (occurred_at, strategy, execution_mode, source, \
                 market_id, position_id, event_type, reason, realized_pnl, fill_price, \
                 filled_size_usd) \
                 VALUES (NOW() - make_interval(mins => $1), $2, 'live', 'src', \
                 '0x1234567890abcdef1234567890', $3, $4, $5, $6, 0.45, 4.5)",
            )
            .bind(minutes_ago)
            .bind(strategy)
            .bind(position)
            .bind(event_type)
            .bind(reason)
            .bind(pnl)
            .execute(&pool)
        };
        event("arb", "entry_requested", 30, None, None)
            .await
            .unwrap();
        event("arb", "position_open", 29, None, None).await.unwrap();
        event("flow", "entry_filled", 20, None, None).await.unwrap();
        event("flow", "exit_failed", 10, Some("no bids"), None)
            .await
            .unwrap();
        // Retried and failed again for the same reason: not shown again.
        for minutes_ago in [9, 8, 7] {
            event("flow", "exit_recovery_requeued", minutes_ago, None, None)
                .await
                .unwrap();
            event("flow", "exit_failed", minutes_ago, Some("no bids"), None)
                .await
                .unwrap();
        }
        event(
            "arb",
            "closed_via_resolution",
            1,
            None,
            Some(Decimal::new(42, 2)),
        )
        .await
        .unwrap();

        let feed = load_activity(&pool, 50, 0).await.unwrap();
        // Requests are not activity; newest first.
        let types: Vec<&str> = feed.iter().map(|a| a.activity_type.as_str()).collect();
        assert_eq!(
            types,
            [
                "ARB_POSITION_CLOSED",
                "TRADE_FAILED",
                "TRADE_EXECUTED",
                "ARB_POSITION_OPENED"
            ]
        );
        // A new reason is a new failure to show.
        event("flow", "exit_failed", 5, Some("no token ids"), None)
            .await
            .unwrap();
        let reasons: Vec<Option<String>> = load_activity(&pool, 50, 0)
            .await
            .unwrap()
            .into_iter()
            .filter(|a| a.activity_type == "TRADE_FAILED")
            .map(|a| a.error_message)
            .collect();
        assert_eq!(
            reasons,
            [
                Some("no token ids".to_string()),
                Some("no bids".to_string())
            ]
        );
        sqlx::query("DELETE FROM trade_events WHERE reason = 'no token ids'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(feed[0].pnl, Some(Decimal::new(42, 2)));
        assert_eq!(feed[0].details.as_ref().unwrap()["source"], "arbitrage");
        assert_eq!(feed[1].error_message.as_deref(), Some("no bids"));
        assert!(feed[1].message.contains("Flow (live) exit failed"));
        assert!(feed[2].message.contains("bought $4.50"));
        // At the price's own precision, not the column's ten places.
        assert!(feed[2].message.ends_with("@ 0.45"), "{}", feed[2].message);
        // Paging, and out-of-range values no longer fail the request.
        assert_eq!(load_activity(&pool, 2, 1).await.unwrap().len(), 2);
        assert_eq!(load_activity(&pool, -5, -3).await.unwrap().len(), 1);
    }
}
