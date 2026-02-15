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

/// Get the workspace's configured total_budget (falls back to 0 if not set).
async fn get_workspace_budget(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
) -> Result<Decimal, sqlx::Error> {
    let row: Option<(Decimal,)> =
        sqlx::query_as("SELECT total_budget FROM workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(b,)| b).unwrap_or(Decimal::ZERO))
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
    if req.quantity <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Quantity must be greater than 0".into(),
        ));
    }
    if req.entry_price <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Entry price must be greater than 0".into(),
        ));
    }

    let position_id = Uuid::new_v4();
    let now = Utc::now();
    let current_price = req.current_price.unwrap_or(req.entry_price);
    let position_cost = req.quantity * req.entry_price;

    let mut tx = state.pool.begin().await?;

    // Ensure balance row exists, then lock it for atomic debit.
    let budget = get_workspace_budget(&state.pool, workspace_id).await
        .map_err(|e| ApiError::Internal(format!("Failed to get workspace budget: {e}")))?;
    sqlx::query(
        r#"
        INSERT INTO demo_balances (workspace_id, balance, initial_balance, updated_at)
        VALUES ($1, $2, $2, $3)
        ON CONFLICT (workspace_id) DO NOTHING
        "#,
    )
    .bind(workspace_id)
    .bind(budget)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    let (current_balance,): (Decimal,) =
        sqlx::query_as("SELECT balance FROM demo_balances WHERE workspace_id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await?;

    if current_balance < position_cost {
        return Err(ApiError::BadRequest(format!(
            "Insufficient demo balance: required {}, available {}",
            position_cost, current_balance
        )));
    }

    sqlx::query(
        "UPDATE demo_balances SET balance = balance - $2, updated_at = $3 WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .bind(position_cost)
    .bind(now)
    .execute(&mut *tx)
    .await?;

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
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

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

    let now = Utc::now();
    let mut tx = state.pool.begin().await?;

    // Lock position row for atomic close + balance credit semantics.
    let position: Option<(Decimal, Decimal, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT quantity, entry_price, closed_at
        FROM demo_positions
        WHERE id = $1 AND workspace_id = $2
        FOR UPDATE
        "#,
    )
    .bind(position_uuid)
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (quantity, entry_price, existing_closed_at) =
        position.ok_or_else(|| ApiError::NotFound("Position not found".into()))?;

    let is_close_request =
        req.closed_at.is_some() || req.exit_price.is_some() || req.realized_pnl.is_some();

    let row: DemoPositionRow = if is_close_request {
        if existing_closed_at.is_some() {
            return Err(ApiError::BadRequest("Position is already closed".into()));
        }

        let exit_price = req.exit_price.or(req.current_price).ok_or_else(|| {
            ApiError::BadRequest("Exit price is required when closing a position".into())
        })?;
        if exit_price <= Decimal::ZERO {
            return Err(ApiError::BadRequest(
                "Exit price must be greater than 0".into(),
            ));
        }
        let closed_at = req.closed_at.unwrap_or(now);
        let realized_pnl = req
            .realized_pnl
            .unwrap_or((exit_price - entry_price) * quantity);
        let exit_value = quantity * exit_price;

        let budget = get_workspace_budget(&state.pool, workspace_id).await
            .map_err(|e| ApiError::Internal(format!("Failed to get workspace budget: {e}")))?;
        sqlx::query(
            r#"
            INSERT INTO demo_balances (workspace_id, balance, initial_balance, updated_at)
            VALUES ($1, $2, $2, $3)
            ON CONFLICT (workspace_id) DO NOTHING
            "#,
        )
        .bind(workspace_id)
        .bind(budget)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE demo_balances SET balance = balance + $2, updated_at = $3 WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .bind(exit_value)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query_as(
            r#"
            UPDATE demo_positions
            SET current_price = COALESCE($1, current_price),
                closed_at = $2,
                exit_price = $3,
                realized_pnl = $4,
                updated_at = $5
            WHERE id = $6 AND workspace_id = $7
            RETURNING id, workspace_id, created_by, wallet_address, wallet_label,
                      market_id, market_question, outcome, quantity, entry_price,
                      current_price, opened_at, closed_at, exit_price, realized_pnl,
                      created_at, updated_at
            "#,
        )
        .bind(Some(req.current_price.unwrap_or(exit_price)))
        .bind(Some(closed_at))
        .bind(Some(exit_price))
        .bind(Some(realized_pnl))
        .bind(now)
        .bind(position_uuid)
        .bind(workspace_id)
        .fetch_one(&mut *tx)
        .await?
    } else {
        sqlx::query_as(
            r#"
            UPDATE demo_positions
            SET current_price = COALESCE($1, current_price),
                updated_at = $2
            WHERE id = $3 AND workspace_id = $4
            RETURNING id, workspace_id, created_by, wallet_address, wallet_label,
                      market_id, market_question, outcome, quantity, entry_price,
                      current_price, opened_at, closed_at, exit_price, realized_pnl,
                      created_at, updated_at
            "#,
        )
        .bind(req.current_price)
        .bind(now)
        .bind(position_uuid)
        .bind(workspace_id)
        .fetch_one(&mut *tx)
        .await?
    };

    tx.commit().await?;

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

    let mut tx = state.pool.begin().await?;

    // Fetch the position (lock row) to check if it's open and get cost info
    let position: Option<(Decimal, Decimal, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT quantity, entry_price, closed_at
        FROM demo_positions
        WHERE id = $1 AND workspace_id = $2
        FOR UPDATE
        "#,
    )
    .bind(position_uuid)
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (quantity, entry_price, closed_at) =
        position.ok_or_else(|| ApiError::NotFound("Position not found".into()))?;

    // If the position is still open, refund the entry cost to the demo balance
    if closed_at.is_none() {
        let refund = quantity * entry_price;
        sqlx::query(
            "UPDATE demo_balances SET balance = balance + $2, updated_at = NOW() WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .bind(refund)
        .execute(&mut *tx)
        .await?;
    }

    let result = sqlx::query("DELETE FROM demo_positions WHERE id = $1 AND workspace_id = $2")
        .bind(position_uuid)
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Position not found".into()));
    }

    tx.commit().await?;

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
            // Create default balance from workspace's configured budget
            let now = Utc::now();
            let default_balance = get_workspace_budget(&state.pool, workspace_id).await
                .map_err(|e| ApiError::Internal(format!("Failed to get workspace budget: {e}")))?;
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

    if req.balance < Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Demo balance cannot be negative".into(),
        ));
    }

    let now = Utc::now();

    // Upsert balance
    let budget = get_workspace_budget(&state.pool, workspace_id).await
        .map_err(|e| ApiError::Internal(format!("Failed to get workspace budget: {e}")))?;
    let row: DemoBalanceRow = sqlx::query_as(
        r#"
        INSERT INTO demo_balances (workspace_id, balance, initial_balance, updated_at)
        VALUES ($1, $2, $4, $3)
        ON CONFLICT (workspace_id)
        DO UPDATE SET balance = $2, updated_at = $3
        RETURNING workspace_id, balance, initial_balance, updated_at
        "#,
    )
    .bind(workspace_id)
    .bind(req.balance)
    .bind(now)
    .bind(budget)
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
    let default_balance = get_workspace_budget(&state.pool, workspace_id).await
        .map_err(|e| ApiError::Internal(format!("Failed to get workspace budget: {e}")))?;
    let mut tx = state.pool.begin().await?;

    // Delete all positions
    sqlx::query("DELETE FROM demo_positions WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
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
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

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

    #[test]
    fn test_demo_balance_response_serialization() {
        let now = Utc::now();
        let response = DemoBalanceResponse {
            workspace_id: "ws-123".to_string(),
            balance: Decimal::new(9500, 0),
            initial_balance: Decimal::new(5000, 0),
            updated_at: now,
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["workspace_id"], "ws-123");
        assert_eq!(parsed["balance"], "9500");
        assert_eq!(parsed["initial_balance"], "5000");
        assert!(parsed["updated_at"].is_string());
    }

    #[test]
    fn test_create_demo_position_request_deserialization() {
        // Valid request
        let json = r#"{
            "wallet_address": "0xabc",
            "market_id": "market-1",
            "outcome": "yes",
            "quantity": "10",
            "entry_price": "0.50",
            "opened_at": "2025-01-01T00:00:00Z"
        }"#;
        let req: CreateDemoPositionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.wallet_address, "0xabc");
        assert_eq!(req.quantity, Decimal::new(10, 0));
        assert_eq!(req.entry_price, Decimal::new(50, 2));
        assert!(req.wallet_label.is_none());
        assert!(req.market_question.is_none());
        assert!(req.current_price.is_none());

        // Invalid outcome value (validated at handler level, not deserialization)
        let json_bad = r#"{
            "wallet_address": "0xabc",
            "market_id": "market-1",
            "outcome": "maybe",
            "quantity": "10",
            "entry_price": "0.50",
            "opened_at": "2025-01-01T00:00:00Z"
        }"#;
        let bad_req: CreateDemoPositionRequest = serde_json::from_str(json_bad).unwrap();
        assert_eq!(bad_req.outcome, "maybe"); // Deserializes fine, handler rejects
    }

    #[test]
    fn test_demo_position_closed_fields_serialized() {
        let now = Utc::now();
        let response = DemoPositionResponse {
            id: "pos-1".to_string(),
            workspace_id: "ws-1".to_string(),
            created_by: "user-1".to_string(),
            wallet_address: "0x123".to_string(),
            wallet_label: None,
            market_id: "market-1".to_string(),
            market_question: None,
            outcome: "yes".to_string(),
            quantity: Decimal::new(50, 0),
            entry_price: Decimal::new(40, 2),
            current_price: Some(Decimal::new(60, 2)),
            opened_at: now - chrono::Duration::days(1),
            closed_at: Some(now),
            exit_price: Some(Decimal::new(60, 2)),
            realized_pnl: Some(Decimal::new(10, 0)),
            created_at: now - chrono::Duration::days(1),
            updated_at: now,
        };

        let json = serde_json::to_string(&response).unwrap();
        // Closed fields should all be present
        assert!(json.contains("closed_at"), "closed_at should be serialized");
        assert!(
            json.contains("exit_price"),
            "exit_price should be serialized"
        );
        assert!(
            json.contains("realized_pnl"),
            "realized_pnl should be serialized"
        );
        // Optional None fields should be absent
        assert!(
            !json.contains("wallet_label"),
            "wallet_label should be skipped when None"
        );
        assert!(
            !json.contains("market_question"),
            "market_question should be skipped when None"
        );
    }
}
