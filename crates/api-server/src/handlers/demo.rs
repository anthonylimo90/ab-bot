//! Demo position management handlers.
//!
//! Demo positions are shared across workspace members and persisted in the database.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use auth::Claims;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Demo position response.
#[derive(Debug, Serialize, ToSchema)]
pub struct DemoPositionResponse {
    pub id: String,
    pub workspace_id: String,
    pub created_by: String,
    pub wallet_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_label: Option<String>,
    pub market_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_question: Option<String>,
    pub outcome: String,
    pub quantity: Decimal,
    pub entry_price: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_price: Option<Decimal>,
    pub opened_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_price: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realized_pnl: Option<Decimal>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Create demo position request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateDemoPositionRequest {
    pub wallet_address: String,
    #[serde(default)]
    pub wallet_label: Option<String>,
    pub market_id: String,
    #[serde(default)]
    pub market_question: Option<String>,
    pub outcome: String,
    pub quantity: Decimal,
    pub entry_price: Decimal,
    #[serde(default)]
    pub current_price: Option<Decimal>,
    pub opened_at: DateTime<Utc>,
}

/// Update demo position request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDemoPositionRequest {
    #[serde(default)]
    pub current_price: Option<Decimal>,
    #[serde(default)]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub exit_price: Option<Decimal>,
    #[serde(default)]
    pub realized_pnl: Option<Decimal>,
}

/// Query params for listing demo positions.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListDemoPositionsQuery {
    /// Filter by status: open, closed, or all (default: all)
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "all".to_string()
}

/// Demo balance response.
#[derive(Debug, Serialize, ToSchema)]
pub struct DemoBalanceResponse {
    pub workspace_id: String,
    pub balance: Decimal,
    pub initial_balance: Decimal,
    pub updated_at: DateTime<Utc>,
}

/// Update demo balance request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDemoBalanceRequest {
    pub balance: Decimal,
}

/// Database row for demo position.
#[derive(Debug, sqlx::FromRow)]
struct DemoPositionRow {
    id: Uuid,
    workspace_id: Uuid,
    created_by: Uuid,
    wallet_address: String,
    wallet_label: Option<String>,
    market_id: String,
    market_question: Option<String>,
    outcome: String,
    quantity: Decimal,
    entry_price: Decimal,
    current_price: Option<Decimal>,
    opened_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
    exit_price: Option<Decimal>,
    realized_pnl: Option<Decimal>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<DemoPositionRow> for DemoPositionResponse {
    fn from(row: DemoPositionRow) -> Self {
        Self {
            id: row.id.to_string(),
            workspace_id: row.workspace_id.to_string(),
            created_by: row.created_by.to_string(),
            wallet_address: row.wallet_address,
            wallet_label: row.wallet_label,
            market_id: row.market_id,
            market_question: row.market_question,
            outcome: row.outcome,
            quantity: row.quantity,
            entry_price: row.entry_price,
            current_price: row.current_price,
            opened_at: row.opened_at,
            closed_at: row.closed_at,
            exit_price: row.exit_price,
            realized_pnl: row.realized_pnl,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Database row for demo balance.
#[derive(Debug, sqlx::FromRow)]
struct DemoBalanceRow {
    workspace_id: Uuid,
    balance: Decimal,
    initial_balance: Decimal,
    updated_at: DateTime<Utc>,
}

/// Get user's current workspace ID.
async fn get_current_workspace(
    pool: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let settings: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT default_workspace_id FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

    Ok(settings.and_then(|(id,)| id))
}

/// Check if user is a member of the workspace.
async fn is_workspace_member(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2")
            .bind(workspace_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

    Ok(exists.is_some())
}

/// List demo positions for current workspace.
#[utoipa::path(
    get,
    path = "/api/v1/demo/positions",
    params(ListDemoPositionsQuery),
    responses(
        (status = 200, description = "List of demo positions", body = Vec<DemoPositionResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn list_demo_positions(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListDemoPositionsQuery>,
) -> ApiResult<Json<Vec<DemoPositionResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let positions: Vec<DemoPositionRow> = match query.status.as_str() {
        "open" => {
            sqlx::query_as(
                r#"
                SELECT id, workspace_id, created_by, wallet_address, wallet_label,
                       market_id, market_question, outcome, quantity, entry_price,
                       current_price, opened_at, closed_at, exit_price, realized_pnl,
                       created_at, updated_at
                FROM demo_positions
                WHERE workspace_id = $1 AND closed_at IS NULL
                ORDER BY opened_at DESC
                "#,
            )
            .bind(workspace_id)
            .fetch_all(&state.pool)
            .await?
        }
        "closed" => {
            sqlx::query_as(
                r#"
                SELECT id, workspace_id, created_by, wallet_address, wallet_label,
                       market_id, market_question, outcome, quantity, entry_price,
                       current_price, opened_at, closed_at, exit_price, realized_pnl,
                       created_at, updated_at
                FROM demo_positions
                WHERE workspace_id = $1 AND closed_at IS NOT NULL
                ORDER BY closed_at DESC
                "#,
            )
            .bind(workspace_id)
            .fetch_all(&state.pool)
            .await?
        }
        _ => {
            sqlx::query_as(
                r#"
                SELECT id, workspace_id, created_by, wallet_address, wallet_label,
                       market_id, market_question, outcome, quantity, entry_price,
                       current_price, opened_at, closed_at, exit_price, realized_pnl,
                       created_at, updated_at
                FROM demo_positions
                WHERE workspace_id = $1
                ORDER BY opened_at DESC
                "#,
            )
            .bind(workspace_id)
            .fetch_all(&state.pool)
            .await?
        }
    };

    let response: Vec<DemoPositionResponse> = positions.into_iter().map(Into::into).collect();

    Ok(Json(response))
}

/// Create a demo position.
#[utoipa::path(
    post,
    path = "/api/v1/demo/positions",
    request_body = CreateDemoPositionRequest,
    responses(
        (status = 201, description = "Demo position created", body = DemoPositionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn create_demo_position(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateDemoPositionRequest>,
) -> ApiResult<(StatusCode, Json<DemoPositionResponse>)> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Validate outcome
    let outcome = req.outcome.to_lowercase();
    if !["yes", "no"].contains(&outcome.as_str()) {
        return Err(ApiError::BadRequest("Outcome must be 'yes' or 'no'".into()));
    }

    let position_id = Uuid::new_v4();
    let now = Utc::now();
    let current_price = req.current_price.unwrap_or(req.entry_price);

    let row: DemoPositionRow = sqlx::query_as(
        r#"
        INSERT INTO demo_positions (
            id, workspace_id, created_by, wallet_address, wallet_label,
            market_id, market_question, outcome, quantity, entry_price,
            current_price, opened_at, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
        RETURNING id, workspace_id, created_by, wallet_address, wallet_label,
                  market_id, market_question, outcome, quantity, entry_price,
                  current_price, opened_at, closed_at, exit_price, realized_pnl,
                  created_at, updated_at
        "#,
    )
    .bind(position_id)
    .bind(workspace_id)
    .bind(user_id)
    .bind(&req.wallet_address)
    .bind(&req.wallet_label)
    .bind(&req.market_id)
    .bind(&req.market_question)
    .bind(&outcome)
    .bind(req.quantity)
    .bind(req.entry_price)
    .bind(current_price)
    .bind(req.opened_at)
    .bind(now)
    .fetch_one(&state.pool)
    .await?;

    Ok((StatusCode::CREATED, Json(row.into())))
}

/// Update a demo position.
#[utoipa::path(
    put,
    path = "/api/v1/demo/positions/{position_id}",
    params(
        ("position_id" = String, Path, description = "Position ID")
    ),
    request_body = UpdateDemoPositionRequest,
    responses(
        (status = 200, description = "Demo position updated", body = DemoPositionResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Position not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn update_demo_position(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(position_id): Path<String>,
    Json(req): Json<UpdateDemoPositionRequest>,
) -> ApiResult<Json<DemoPositionResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let position_uuid = Uuid::parse_str(&position_id)
        .map_err(|_| ApiError::BadRequest("Invalid position ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Verify position belongs to workspace
    let exists: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM demo_positions WHERE id = $1 AND workspace_id = $2")
            .bind(position_uuid)
            .bind(workspace_id)
            .fetch_optional(&state.pool)
            .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound("Position not found".into()));
    }

    let now = Utc::now();

    // Build dynamic update query
    let row: DemoPositionRow = sqlx::query_as(
        r#"
        UPDATE demo_positions
        SET current_price = COALESCE($1, current_price),
            closed_at = COALESCE($2, closed_at),
            exit_price = COALESCE($3, exit_price),
            realized_pnl = COALESCE($4, realized_pnl),
            updated_at = $5
        WHERE id = $6 AND workspace_id = $7
        RETURNING id, workspace_id, created_by, wallet_address, wallet_label,
                  market_id, market_question, outcome, quantity, entry_price,
                  current_price, opened_at, closed_at, exit_price, realized_pnl,
                  created_at, updated_at
        "#,
    )
    .bind(req.current_price)
    .bind(req.closed_at)
    .bind(req.exit_price)
    .bind(req.realized_pnl)
    .bind(now)
    .bind(position_uuid)
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(row.into()))
}

/// Delete a demo position.
#[utoipa::path(
    delete,
    path = "/api/v1/demo/positions/{position_id}",
    params(
        ("position_id" = String, Path, description = "Position ID")
    ),
    responses(
        (status = 204, description = "Demo position deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Position not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn delete_demo_position(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(position_id): Path<String>,
) -> ApiResult<StatusCode> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let position_uuid = Uuid::parse_str(&position_id)
        .map_err(|_| ApiError::BadRequest("Invalid position ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let result = sqlx::query("DELETE FROM demo_positions WHERE id = $1 AND workspace_id = $2")
        .bind(position_uuid)
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Position not found".into()));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Get demo balance for current workspace.
#[utoipa::path(
    get,
    path = "/api/v1/demo/balance",
    responses(
        (status = 200, description = "Demo balance", body = DemoBalanceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn get_demo_balance(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<DemoBalanceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    // Get or create balance
    let balance: Option<DemoBalanceRow> = sqlx::query_as(
        "SELECT workspace_id, balance, initial_balance, updated_at FROM demo_balances WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;

    let response = match balance {
        Some(b) => DemoBalanceResponse {
            workspace_id: b.workspace_id.to_string(),
            balance: b.balance,
            initial_balance: b.initial_balance,
            updated_at: b.updated_at,
        },
        None => {
            // Create default balance
            let now = Utc::now();
            let default_balance = Decimal::new(10000, 0);
            sqlx::query(
                "INSERT INTO demo_balances (workspace_id, balance, initial_balance, updated_at) VALUES ($1, $2, $2, $3)",
            )
            .bind(workspace_id)
            .bind(default_balance)
            .bind(now)
            .execute(&state.pool)
            .await?;

            DemoBalanceResponse {
                workspace_id: workspace_id.to_string(),
                balance: default_balance,
                initial_balance: default_balance,
                updated_at: now,
            }
        }
    };

    Ok(Json(response))
}

/// Update demo balance for current workspace.
#[utoipa::path(
    put,
    path = "/api/v1/demo/balance",
    request_body = UpdateDemoBalanceRequest,
    responses(
        (status = 200, description = "Demo balance updated", body = DemoBalanceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn update_demo_balance(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<UpdateDemoBalanceRequest>,
) -> ApiResult<Json<DemoBalanceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let now = Utc::now();

    // Upsert balance
    let row: DemoBalanceRow = sqlx::query_as(
        r#"
        INSERT INTO demo_balances (workspace_id, balance, initial_balance, updated_at)
        VALUES ($1, $2, 10000, $3)
        ON CONFLICT (workspace_id)
        DO UPDATE SET balance = $2, updated_at = $3
        RETURNING workspace_id, balance, initial_balance, updated_at
        "#,
    )
    .bind(workspace_id)
    .bind(req.balance)
    .bind(now)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(DemoBalanceResponse {
        workspace_id: row.workspace_id.to_string(),
        balance: row.balance,
        initial_balance: row.initial_balance,
        updated_at: row.updated_at,
    }))
}

/// Reset demo portfolio for current workspace.
#[utoipa::path(
    post,
    path = "/api/v1/demo/reset",
    responses(
        (status = 200, description = "Demo portfolio reset", body = DemoBalanceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "demo"
)]
pub async fn reset_demo_portfolio(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<DemoBalanceResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    if !is_workspace_member(&state.pool, workspace_id, user_id).await? {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let now = Utc::now();
    let default_balance = Decimal::new(10000, 0);

    // Delete all positions
    sqlx::query("DELETE FROM demo_positions WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&state.pool)
        .await?;

    // Reset balance
    let row: DemoBalanceRow = sqlx::query_as(
        r#"
        INSERT INTO demo_balances (workspace_id, balance, initial_balance, updated_at)
        VALUES ($1, $2, $2, $3)
        ON CONFLICT (workspace_id)
        DO UPDATE SET balance = $2, initial_balance = $2, updated_at = $3
        RETURNING workspace_id, balance, initial_balance, updated_at
        "#,
    )
    .bind(workspace_id)
    .bind(default_balance)
    .bind(now)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(DemoBalanceResponse {
        workspace_id: row.workspace_id.to_string(),
        balance: row.balance,
        initial_balance: row.initial_balance,
        updated_at: row.updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demo_position_response_serialization() {
        let response = DemoPositionResponse {
            id: "test-id".to_string(),
            workspace_id: "ws-id".to_string(),
            created_by: "user-id".to_string(),
            wallet_address: "0x123".to_string(),
            wallet_label: Some("Test Wallet".to_string()),
            market_id: "market-1".to_string(),
            market_question: Some("Will it rain?".to_string()),
            outcome: "yes".to_string(),
            quantity: Decimal::new(100, 0),
            entry_price: Decimal::new(50, 2),
            current_price: Some(Decimal::new(55, 2)),
            opened_at: Utc::now(),
            closed_at: None,
            exit_price: None,
            realized_pnl: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("0x123"));
        assert!(!json.contains("closed_at")); // skipped when None
    }
}
