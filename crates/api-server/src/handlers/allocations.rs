//! Workspace wallet allocation handlers.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::{AuditAction, AuditEvent, Claims};
use trading_engine::copy_trader::TrackedWallet;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Wallet allocation response.
#[derive(Debug, Serialize, ToSchema)]
pub struct AllocationResponse {
    pub id: String,
    pub wallet_address: String,
    pub allocation_pct: Decimal,
    pub max_position_size: Option<Decimal>,
    pub tier: String,
    pub auto_assigned: bool,
    pub auto_assigned_reason: Option<String>,
    pub backtest_roi: Option<Decimal>,
    pub backtest_sharpe: Option<Decimal>,
    pub backtest_win_rate: Option<Decimal>,
    pub copy_behavior: String,
    pub arb_threshold_pct: Option<Decimal>,
    pub added_by: Option<String>,
    pub added_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Denormalized wallet info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet_success_score: Option<Decimal>,
}

/// Add wallet to roster request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddAllocationRequest {
    /// Ignored — address comes from the URL path parameter.
    #[serde(default)]
    pub wallet_address: String,
    #[serde(default = "default_allocation")]
    pub allocation_pct: Decimal,
    pub max_position_size: Option<Decimal>,
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(default = "default_copy_behavior")]
    pub copy_behavior: String,
    pub arb_threshold_pct: Option<Decimal>,
}

fn default_allocation() -> Decimal {
    Decimal::new(20, 0)
}

fn default_tier() -> String {
    "bench".to_string()
}

fn default_copy_behavior() -> String {
    "copy_all".to_string()
}

/// Update allocation request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAllocationRequest {
    pub allocation_pct: Option<Decimal>,
    pub max_position_size: Option<Decimal>,
    pub copy_behavior: Option<String>,
    pub arb_threshold_pct: Option<Decimal>,
}

/// Query params for listing allocations.
#[derive(Debug, Deserialize)]
pub struct ListAllocationsQuery {
    pub tier: Option<String>,
}

/// Database row for allocation.
#[derive(Debug, sqlx::FromRow)]
struct AllocationRow {
    id: Uuid,
    #[allow(dead_code)]
    workspace_id: Uuid,
    wallet_address: String,
    allocation_pct: Decimal,
    max_position_size: Option<Decimal>,
    tier: String,
    auto_assigned: bool,
    auto_assigned_reason: Option<String>,
    backtest_roi: Option<Decimal>,
    backtest_sharpe: Option<Decimal>,
    backtest_win_rate: Option<Decimal>,
    copy_behavior: String,
    arb_threshold_pct: Option<Decimal>,
    added_by: Option<Uuid>,
    added_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    // Joined from tracked_wallets
    wallet_label: Option<String>,
    wallet_success_score: Option<Decimal>,
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

/// Get user's role in a workspace.
async fn get_user_role(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let role: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(role.map(|(r,)| r))
}

/// List wallet allocations for current workspace.
#[utoipa::path(
    get,
    path = "/api/v1/allocations",
    params(
        ("tier" = Option<String>, Query, description = "Filter by tier (active/bench)"),
    ),
    responses(
        (status = 200, description = "List of wallet allocations", body = Vec<AllocationResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn list_allocations(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ListAllocationsQuery>,
) -> ApiResult<Json<Vec<AllocationResponse>>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    let tier_filter = query.tier.as_deref();

    let allocations: Vec<AllocationRow> = if let Some(tier) = tier_filter {
        sqlx::query_as(
            r#"
            SELECT
                wwa.id, wwa.workspace_id, wwa.wallet_address, wwa.allocation_pct,
                wwa.max_position_size, wwa.tier, wwa.auto_assigned, wwa.auto_assigned_reason,
                wwa.backtest_roi, wwa.backtest_sharpe, wwa.backtest_win_rate,
                wwa.copy_behavior, wwa.arb_threshold_pct, wwa.added_by,
                wwa.added_at, wwa.updated_at,
                tw.label as wallet_label, tw.success_score as wallet_success_score
            FROM workspace_wallet_allocations wwa
            LEFT JOIN tracked_wallets tw ON wwa.wallet_address = tw.address
            WHERE wwa.workspace_id = $1 AND wwa.tier = $2
            ORDER BY wwa.allocation_pct DESC, wwa.added_at
            "#,
        )
        .bind(workspace_id)
        .bind(tier)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT
                wwa.id, wwa.workspace_id, wwa.wallet_address, wwa.allocation_pct,
                wwa.max_position_size, wwa.tier, wwa.auto_assigned, wwa.auto_assigned_reason,
                wwa.backtest_roi, wwa.backtest_sharpe, wwa.backtest_win_rate,
                wwa.copy_behavior, wwa.arb_threshold_pct, wwa.added_by,
                wwa.added_at, wwa.updated_at,
                tw.label as wallet_label, tw.success_score as wallet_success_score
            FROM workspace_wallet_allocations wwa
            LEFT JOIN tracked_wallets tw ON wwa.wallet_address = tw.address
            WHERE wwa.workspace_id = $1
            ORDER BY
                CASE wwa.tier WHEN 'active' THEN 0 ELSE 1 END,
                wwa.allocation_pct DESC,
                wwa.added_at
            "#,
        )
        .bind(workspace_id)
        .fetch_all(&state.pool)
        .await?
    };

    let response: Vec<AllocationResponse> = allocations
        .into_iter()
        .map(|a| AllocationResponse {
            id: a.id.to_string(),
            wallet_address: a.wallet_address,
            allocation_pct: a.allocation_pct,
            max_position_size: a.max_position_size,
            tier: a.tier,
            auto_assigned: a.auto_assigned,
            auto_assigned_reason: a.auto_assigned_reason,
            backtest_roi: a.backtest_roi,
            backtest_sharpe: a.backtest_sharpe,
            backtest_win_rate: a.backtest_win_rate,
            copy_behavior: a.copy_behavior,
            arb_threshold_pct: a.arb_threshold_pct,
            added_by: a.added_by.map(|id| id.to_string()),
            added_at: a.added_at,
            updated_at: a.updated_at,
            wallet_label: a.wallet_label,
            wallet_success_score: a.wallet_success_score,
        })
        .collect();

    Ok(Json(response))
}

/// Add a wallet to the workspace roster.
#[utoipa::path(
    post,
    path = "/api/v1/allocations/{address}",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    request_body = AddAllocationRequest,
    responses(
        (status = 201, description = "Wallet added to roster", body = AllocationResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 409, description = "Wallet already in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn add_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
    Json(req): Json<AddAllocationRequest>,
) -> ApiResult<(StatusCode, Json<AllocationResponse>)> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can modify roster".into(),
        ));
    }

    // Validate tier
    let tier = req.tier.to_lowercase();
    if !["active", "bench"].contains(&tier.as_str()) {
        return Err(ApiError::BadRequest(
            "Tier must be 'active' or 'bench'".into(),
        ));
    }

    // Validate copy behavior
    let copy_behavior = req.copy_behavior.to_lowercase();
    if !["copy_all", "events_only", "arb_threshold"].contains(&copy_behavior.as_str()) {
        return Err(ApiError::BadRequest(
            "Copy behavior must be 'copy_all', 'events_only', or 'arb_threshold'".into(),
        ));
    }

    // Check if already in roster
    let existing: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict("Wallet already in roster".into()));
    }

    // Check active limit if adding to active tier
    if tier == "active" {
        let active_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND tier = 'active'",
        )
        .bind(workspace_id)
        .fetch_one(&state.pool)
        .await?;

        if active_count.0 >= 5 {
            return Err(ApiError::BadRequest(
                "Cannot have more than 5 active wallets. Demote one first.".into(),
            ));
        }
    }

    let allocation_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO workspace_wallet_allocations (
            id, workspace_id, wallet_address, allocation_pct, max_position_size,
            tier, copy_behavior, arb_threshold_pct, added_by, added_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
        "#,
    )
    .bind(allocation_id)
    .bind(workspace_id)
    .bind(&address)
    .bind(req.allocation_pct)
    .bind(req.max_position_size)
    .bind(&tier)
    .bind(&copy_behavior)
    .bind(req.arb_threshold_pct)
    .bind(user_id)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Sync copy_enabled on tracked_wallets
    if tier == "active" {
        sqlx::query(
            r#"
            INSERT INTO tracked_wallets (address, label, copy_enabled, allocation_pct, copy_delay_ms)
            VALUES ($1, $2, TRUE, $3, 500)
            ON CONFLICT (address) DO UPDATE SET copy_enabled = TRUE, allocation_pct = $3
            "#,
        )
        .bind(&address)
        .bind(&address)
        .bind(req.allocation_pct)
        .execute(&state.pool)
        .await?;

        // Sync to in-memory trade monitor + copy trader
        if let Some(monitor) = &state.trade_monitor {
            monitor.add_wallet(&address).await;
        }
        if let Some(trader) = &state.copy_trader {
            let trader = trader.read().await;
            let wallet = TrackedWallet::new(address.clone(), req.allocation_pct);
            trader.add_tracked_wallet(wallet);
        }
    }

    // Audit log
    let event = AuditEvent::builder(
        AuditAction::Custom("roster_wallet_added".to_string()),
        format!("allocation/{}", allocation_id),
    )
    .user(claims.sub.clone())
    .details(serde_json::json!({
        "workspace_id": workspace_id.to_string(),
        "wallet_address": &address,
        "tier": &tier
    }))
    .build();
    state.audit_logger.log(event);

    // Fetch wallet info
    let wallet: Option<(Option<String>, Option<Decimal>)> =
        sqlx::query_as("SELECT label, success_score FROM tracked_wallets WHERE address = $1")
            .bind(&address)
            .fetch_optional(&state.pool)
            .await?;

    let (wallet_label, wallet_success_score) = wallet.unwrap_or((None, None));

    Ok((
        StatusCode::CREATED,
        Json(AllocationResponse {
            id: allocation_id.to_string(),
            wallet_address: address,
            allocation_pct: req.allocation_pct,
            max_position_size: req.max_position_size,
            tier,
            auto_assigned: false,
            auto_assigned_reason: None,
            backtest_roi: None,
            backtest_sharpe: None,
            backtest_win_rate: None,
            copy_behavior,
            arb_threshold_pct: req.arb_threshold_pct,
            added_by: Some(user_id.to_string()),
            added_at: now,
            updated_at: now,
            wallet_label,
            wallet_success_score,
        }),
    ))
}

/// Update a wallet allocation.
#[utoipa::path(
    put,
    path = "/api/v1/allocations/{address}",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    request_body = UpdateAllocationRequest,
    responses(
        (status = 200, description = "Allocation updated", body = AllocationResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 404, description = "Wallet not in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn update_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
    Json(req): Json<UpdateAllocationRequest>,
) -> ApiResult<Json<AllocationResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can modify roster".into(),
        ));
    }

    // Check exists
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    if existing.is_none() {
        return Err(ApiError::NotFound("Wallet not in roster".into()));
    }

    let now = Utc::now();
    let mut updates = vec!["updated_at = $3".to_string()];
    let mut param_idx = 4;

    if req.allocation_pct.is_some() {
        updates.push(format!("allocation_pct = ${}", param_idx));
        param_idx += 1;
    }
    if req.max_position_size.is_some() {
        updates.push(format!("max_position_size = ${}", param_idx));
        param_idx += 1;
    }
    if req.copy_behavior.is_some() {
        updates.push(format!("copy_behavior = ${}", param_idx));
        param_idx += 1;
    }
    if req.arb_threshold_pct.is_some() {
        updates.push(format!("arb_threshold_pct = ${}", param_idx));
    }

    let query = format!(
        "UPDATE workspace_wallet_allocations SET {} WHERE workspace_id = $1 AND wallet_address = $2",
        updates.join(", ")
    );

    let mut q = sqlx::query(&query)
        .bind(workspace_id)
        .bind(&address)
        .bind(now);

    if let Some(allocation_pct) = req.allocation_pct {
        q = q.bind(allocation_pct);
    }
    if let Some(max_position_size) = req.max_position_size {
        q = q.bind(max_position_size);
    }
    if let Some(ref copy_behavior) = req.copy_behavior {
        q = q.bind(copy_behavior.to_lowercase());
    }
    if let Some(arb_threshold_pct) = req.arb_threshold_pct {
        q = q.bind(arb_threshold_pct);
    }

    q.execute(&state.pool).await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_allocation_updated".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    // Fetch updated allocation
    let allocation: AllocationRow = sqlx::query_as(
        r#"
        SELECT
            wwa.id, wwa.workspace_id, wwa.wallet_address, wwa.allocation_pct,
            wwa.max_position_size, wwa.tier, wwa.auto_assigned, wwa.auto_assigned_reason,
            wwa.backtest_roi, wwa.backtest_sharpe, wwa.backtest_win_rate,
            wwa.copy_behavior, wwa.arb_threshold_pct, wwa.added_by,
            wwa.added_at, wwa.updated_at,
            tw.label as wallet_label, tw.success_score as wallet_success_score
        FROM workspace_wallet_allocations wwa
        LEFT JOIN tracked_wallets tw ON wwa.wallet_address = tw.address
        WHERE wwa.workspace_id = $1 AND wwa.wallet_address = $2
        "#,
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(AllocationResponse {
        id: allocation.id.to_string(),
        wallet_address: allocation.wallet_address,
        allocation_pct: allocation.allocation_pct,
        max_position_size: allocation.max_position_size,
        tier: allocation.tier,
        auto_assigned: allocation.auto_assigned,
        auto_assigned_reason: allocation.auto_assigned_reason,
        backtest_roi: allocation.backtest_roi,
        backtest_sharpe: allocation.backtest_sharpe,
        backtest_win_rate: allocation.backtest_win_rate,
        copy_behavior: allocation.copy_behavior,
        arb_threshold_pct: allocation.arb_threshold_pct,
        added_by: allocation.added_by.map(|id| id.to_string()),
        added_at: allocation.added_at,
        updated_at: allocation.updated_at,
        wallet_label: allocation.wallet_label,
        wallet_success_score: allocation.wallet_success_score,
    }))
}

/// Remove a wallet from the roster.
#[utoipa::path(
    delete,
    path = "/api/v1/allocations/{address}",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 204, description = "Wallet removed from roster"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 404, description = "Wallet not in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn remove_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<StatusCode> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can modify roster".into(),
        ));
    }

    let result = sqlx::query(
        "DELETE FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Wallet not in roster".into()));
    }

    // Sync copy_enabled on tracked_wallets
    sqlx::query("UPDATE tracked_wallets SET copy_enabled = FALSE WHERE address = $1")
        .bind(&address)
        .execute(&state.pool)
        .await?;

    // Sync to in-memory trade monitor + copy trader
    if let Some(monitor) = &state.trade_monitor {
        monitor.remove_wallet(&address).await;
    }
    if let Some(trader) = &state.copy_trader {
        let trader = trader.read().await;
        trader.remove_tracked_wallet(&address);
    }

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_removed".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Promote a wallet from bench to active.
#[utoipa::path(
    post,
    path = "/api/v1/allocations/{address}/promote",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet promoted to active", body = AllocationResponse),
        (status = 400, description = "Already active or limit reached"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 404, description = "Wallet not in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn promote_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<Json<AllocationResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can modify roster".into(),
        ));
    }

    // Check current tier
    let current: Option<(String,)> = sqlx::query_as(
        "SELECT tier FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    let current_tier = current
        .ok_or_else(|| ApiError::NotFound("Wallet not in roster".into()))?
        .0;

    if current_tier == "active" {
        return Err(ApiError::BadRequest("Wallet is already active".into()));
    }

    // Check active limit (the DB trigger will also enforce this, but better UX to check first)
    let active_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND tier = 'active'",
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    if active_count.0 >= 5 {
        return Err(ApiError::BadRequest(
            "Cannot have more than 5 active wallets. Demote one first.".into(),
        ));
    }

    // Promote
    let now = Utc::now();
    sqlx::query(
        "UPDATE workspace_wallet_allocations SET tier = 'active', updated_at = $1 WHERE workspace_id = $2 AND wallet_address = $3",
    )
    .bind(now)
    .bind(workspace_id)
    .bind(&address)
    .execute(&state.pool)
    .await?;

    // Fetch allocation_pct for upsert + in-memory sync
    let alloc_pct: Decimal = sqlx::query_scalar(
        "SELECT allocation_pct FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(Decimal::new(20, 0));

    // Sync copy_enabled on tracked_wallets (upsert — row may not exist if added as bench/watchlist)
    sqlx::query(
        r#"
        INSERT INTO tracked_wallets (address, label, copy_enabled, allocation_pct, copy_delay_ms)
        VALUES ($1, $1, TRUE, $2, 500)
        ON CONFLICT (address) DO UPDATE SET copy_enabled = TRUE, allocation_pct = $2
        "#,
    )
    .bind(&address)
    .bind(alloc_pct)
    .execute(&state.pool)
    .await?;

    // Sync to in-memory trade monitor + copy trader
    if let Some(monitor) = &state.trade_monitor {
        monitor.add_wallet(&address).await;
    }
    if let Some(trader) = &state.copy_trader {
        let trader = trader.read().await;
        let wallet = TrackedWallet::new(address.clone(), alloc_pct);
        trader.add_tracked_wallet(wallet);
    }

    // Log rotation history
    sqlx::query(
        r#"
        INSERT INTO auto_rotation_history (workspace_id, action, wallet_in, reason, triggered_by, created_at)
        VALUES ($1, 'promote', $2, 'Manual promotion by user', $3, $4)
        "#,
    )
    .bind(workspace_id)
    .bind(&address)
    .bind(user_id)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_promoted".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    // Fetch updated allocation
    let allocation: AllocationRow = sqlx::query_as(
        r#"
        SELECT
            wwa.id, wwa.workspace_id, wwa.wallet_address, wwa.allocation_pct,
            wwa.max_position_size, wwa.tier, wwa.auto_assigned, wwa.auto_assigned_reason,
            wwa.backtest_roi, wwa.backtest_sharpe, wwa.backtest_win_rate,
            wwa.copy_behavior, wwa.arb_threshold_pct, wwa.added_by,
            wwa.added_at, wwa.updated_at,
            tw.label as wallet_label, tw.success_score as wallet_success_score
        FROM workspace_wallet_allocations wwa
        LEFT JOIN tracked_wallets tw ON wwa.wallet_address = tw.address
        WHERE wwa.workspace_id = $1 AND wwa.wallet_address = $2
        "#,
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(AllocationResponse {
        id: allocation.id.to_string(),
        wallet_address: allocation.wallet_address,
        allocation_pct: allocation.allocation_pct,
        max_position_size: allocation.max_position_size,
        tier: allocation.tier,
        auto_assigned: allocation.auto_assigned,
        auto_assigned_reason: allocation.auto_assigned_reason,
        backtest_roi: allocation.backtest_roi,
        backtest_sharpe: allocation.backtest_sharpe,
        backtest_win_rate: allocation.backtest_win_rate,
        copy_behavior: allocation.copy_behavior,
        arb_threshold_pct: allocation.arb_threshold_pct,
        added_by: allocation.added_by.map(|id| id.to_string()),
        added_at: allocation.added_at,
        updated_at: allocation.updated_at,
        wallet_label: allocation.wallet_label,
        wallet_success_score: allocation.wallet_success_score,
    }))
}

/// Demote a wallet from active to bench.
#[utoipa::path(
    post,
    path = "/api/v1/allocations/{address}/demote",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet demoted to bench", body = AllocationResponse),
        (status = 400, description = "Already on bench"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 404, description = "Wallet not in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn demote_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<Json<AllocationResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can modify roster".into(),
        ));
    }

    // Check current tier
    let current: Option<(String,)> = sqlx::query_as(
        "SELECT tier FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    let current_tier = current
        .ok_or_else(|| ApiError::NotFound("Wallet not in roster".into()))?
        .0;

    if current_tier == "bench" {
        return Err(ApiError::BadRequest("Wallet is already on bench".into()));
    }

    // Demote
    let now = Utc::now();
    sqlx::query(
        "UPDATE workspace_wallet_allocations SET tier = 'bench', updated_at = $1 WHERE workspace_id = $2 AND wallet_address = $3",
    )
    .bind(now)
    .bind(workspace_id)
    .bind(&address)
    .execute(&state.pool)
    .await?;

    // Sync copy_enabled on tracked_wallets
    sqlx::query("UPDATE tracked_wallets SET copy_enabled = FALSE WHERE address = $1")
        .bind(&address)
        .execute(&state.pool)
        .await?;

    // Sync to in-memory trade monitor + copy trader
    if let Some(monitor) = &state.trade_monitor {
        monitor.remove_wallet(&address).await;
    }
    if let Some(trader) = &state.copy_trader {
        let trader = trader.read().await;
        trader.remove_tracked_wallet(&address);
    }

    // Log rotation history
    sqlx::query(
        r#"
        INSERT INTO auto_rotation_history (workspace_id, action, wallet_out, reason, triggered_by, created_at)
        VALUES ($1, 'demote', $2, 'Manual demotion by user', $3, $4)
        "#,
    )
    .bind(workspace_id)
    .bind(&address)
    .bind(user_id)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_demoted".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    // Fetch updated allocation
    let allocation: AllocationRow = sqlx::query_as(
        r#"
        SELECT
            wwa.id, wwa.workspace_id, wwa.wallet_address, wwa.allocation_pct,
            wwa.max_position_size, wwa.tier, wwa.auto_assigned, wwa.auto_assigned_reason,
            wwa.backtest_roi, wwa.backtest_sharpe, wwa.backtest_win_rate,
            wwa.copy_behavior, wwa.arb_threshold_pct, wwa.added_by,
            wwa.added_at, wwa.updated_at,
            tw.label as wallet_label, tw.success_score as wallet_success_score
        FROM workspace_wallet_allocations wwa
        LEFT JOIN tracked_wallets tw ON wwa.wallet_address = tw.address
        WHERE wwa.workspace_id = $1 AND wwa.wallet_address = $2
        "#,
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(AllocationResponse {
        id: allocation.id.to_string(),
        wallet_address: allocation.wallet_address,
        allocation_pct: allocation.allocation_pct,
        max_position_size: allocation.max_position_size,
        tier: allocation.tier,
        auto_assigned: allocation.auto_assigned,
        auto_assigned_reason: allocation.auto_assigned_reason,
        backtest_roi: allocation.backtest_roi,
        backtest_sharpe: allocation.backtest_sharpe,
        backtest_win_rate: allocation.backtest_win_rate,
        copy_behavior: allocation.copy_behavior,
        arb_threshold_pct: allocation.arb_threshold_pct,
        added_by: allocation.added_by.map(|id| id.to_string()),
        added_at: allocation.added_at,
        updated_at: allocation.updated_at,
        wallet_label: allocation.wallet_label,
        wallet_success_score: allocation.wallet_success_score,
    }))
}

/// Pin response.
#[derive(Debug, Serialize, ToSchema)]
pub struct PinResponse {
    pub success: bool,
    pub pinned: bool,
    pub message: String,
}

/// Ban wallet request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BanWalletRequest {
    pub wallet_address: String,
    pub reason: Option<String>,
    /// Optional expiration (ISO 8601 format)
    pub expires_at: Option<String>,
}

/// Ban response.
#[derive(Debug, Serialize, ToSchema)]
pub struct BanResponse {
    pub id: String,
    pub wallet_address: String,
    pub reason: Option<String>,
    pub banned_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// List of bans response.
#[derive(Debug, Serialize, ToSchema)]
pub struct BanListResponse {
    pub bans: Vec<BanResponse>,
}

/// Pin a wallet to prevent auto-demotion.
#[utoipa::path(
    put,
    path = "/api/v1/allocations/{address}/pin",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet pinned", body = PinResponse),
        (status = 400, description = "Invalid request or max pins reached"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 404, description = "Wallet not in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn pin_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<Json<PinResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can pin wallets".into(),
        ));
    }

    // Check if wallet is active
    let existing: Option<(String, bool)> = sqlx::query_as(
        "SELECT tier, COALESCE(pinned, false) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    let (tier, already_pinned) =
        existing.ok_or_else(|| ApiError::NotFound("Wallet not in roster".into()))?;

    if tier != "active" {
        return Err(ApiError::BadRequest(
            "Only active wallets can be pinned".into(),
        ));
    }

    if already_pinned {
        return Ok(Json(PinResponse {
            success: true,
            pinned: true,
            message: "Wallet is already pinned".to_string(),
        }));
    }

    // Check max pinned limit
    let (max_pins,): (i32,) =
        sqlx::query_as("SELECT COALESCE(max_pinned_wallets, 3) FROM workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&state.pool)
            .await?;

    let (current_pins,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM workspace_wallet_allocations WHERE workspace_id = $1 AND COALESCE(pinned, false) = true",
    )
    .bind(workspace_id)
    .fetch_one(&state.pool)
    .await?;

    if current_pins >= max_pins as i64 {
        return Err(ApiError::BadRequest(format!(
            "Maximum {} pinned wallets reached. Unpin one first.",
            max_pins
        )));
    }

    // Pin the wallet
    let now = Utc::now();
    sqlx::query(
        r#"
        UPDATE workspace_wallet_allocations
        SET pinned = true, pinned_at = $1, pinned_by = $2, updated_at = $1
        WHERE workspace_id = $3 AND wallet_address = $4
        "#,
    )
    .bind(now)
    .bind(user_id)
    .bind(workspace_id)
    .bind(&address)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_pinned".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    Ok(Json(PinResponse {
        success: true,
        pinned: true,
        message: "Wallet pinned successfully".to_string(),
    }))
}

/// Unpin a wallet to allow auto-demotion.
#[utoipa::path(
    delete,
    path = "/api/v1/allocations/{address}/pin",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet unpinned", body = PinResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to modify roster"),
        (status = 404, description = "Wallet not in roster"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn unpin_allocation(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<Json<PinResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can unpin wallets".into(),
        ));
    }

    // Unpin the wallet
    let now = Utc::now();
    let result = sqlx::query(
        r#"
        UPDATE workspace_wallet_allocations
        SET pinned = false, pinned_at = NULL, pinned_by = NULL, updated_at = $1
        WHERE workspace_id = $2 AND wallet_address = $3
        "#,
    )
    .bind(now)
    .bind(workspace_id)
    .bind(&address)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Wallet not in roster".into()));
    }

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_unpinned".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    Ok(Json(PinResponse {
        success: true,
        pinned: false,
        message: "Wallet unpinned successfully".to_string(),
    }))
}

/// Ban a wallet from auto-promotion.
#[utoipa::path(
    post,
    path = "/api/v1/allocations/bans",
    request_body = BanWalletRequest,
    responses(
        (status = 201, description = "Wallet banned", body = BanResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to ban wallets"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn ban_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<BanWalletRequest>,
) -> ApiResult<(StatusCode, Json<BanResponse>)> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can ban wallets".into(),
        ));
    }

    // Parse optional expiration
    let expires_at = req
        .expires_at
        .as_ref()
        .map(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| ApiError::BadRequest("Invalid expires_at format".into()))
        })
        .transpose()?;

    let ban_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO workspace_wallet_bans
        (id, workspace_id, wallet_address, reason, banned_by, banned_at, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (workspace_id, wallet_address) DO UPDATE SET
            reason = EXCLUDED.reason,
            banned_by = EXCLUDED.banned_by,
            banned_at = EXCLUDED.banned_at,
            expires_at = EXCLUDED.expires_at
        "#,
    )
    .bind(ban_id)
    .bind(workspace_id)
    .bind(&req.wallet_address)
    .bind(&req.reason)
    .bind(user_id)
    .bind(now)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_banned".to_string()),
        &req.wallet_address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string(),
            "reason": &req.reason
        }),
    );

    Ok((
        StatusCode::CREATED,
        Json(BanResponse {
            id: ban_id.to_string(),
            wallet_address: req.wallet_address,
            reason: req.reason,
            banned_at: now,
            expires_at,
        }),
    ))
}

/// Unban a wallet to allow auto-promotion.
#[utoipa::path(
    delete,
    path = "/api/v1/allocations/bans/{address}",
    params(
        ("address" = String, Path, description = "Wallet address to unban")
    ),
    responses(
        (status = 204, description = "Wallet unbanned"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Not allowed to unban wallets"),
        (status = 404, description = "Wallet not banned"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn unban_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<StatusCode> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Check role
    let role = get_user_role(&state.pool, workspace_id, user_id)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Not a member of this workspace".into()))?;

    if !["owner", "admin"].contains(&role.as_str()) {
        return Err(ApiError::Forbidden(
            "Only owner or admin can unban wallets".into(),
        ));
    }

    let result = sqlx::query(
        "DELETE FROM workspace_wallet_bans WHERE workspace_id = $1 AND wallet_address = $2",
    )
    .bind(workspace_id)
    .bind(&address)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Wallet not banned".into()));
    }

    // Audit log
    state.audit_logger.log_user_action(
        &claims.sub,
        AuditAction::Custom("roster_wallet_unbanned".to_string()),
        &address,
        serde_json::json!({
            "workspace_id": workspace_id.to_string()
        }),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// List banned wallets.
#[utoipa::path(
    get,
    path = "/api/v1/allocations/bans",
    responses(
        (status = 200, description = "List of banned wallets", body = BanListResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "No workspace set"),
    ),
    security(("bearer_auth" = [])),
    tag = "allocations"
)]
pub async fn list_bans(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<BanListResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let workspace_id = get_current_workspace(&state.pool, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("No workspace set".into()))?;

    // Verify membership
    let role = get_user_role(&state.pool, workspace_id, user_id).await?;
    if role.is_none() {
        return Err(ApiError::Forbidden("Not a member of this workspace".into()));
    }

    #[allow(clippy::type_complexity)]
    let bans: Vec<(
        Uuid,
        String,
        Option<String>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        r#"
        SELECT id, wallet_address, reason, banned_at, expires_at
        FROM workspace_wallet_bans
        WHERE workspace_id = $1
          AND (expires_at IS NULL OR expires_at > NOW())
        ORDER BY banned_at DESC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await?;

    let bans = bans
        .into_iter()
        .map(
            |(id, wallet_address, reason, banned_at, expires_at)| BanResponse {
                id: id.to_string(),
                wallet_address,
                reason,
                banned_at,
                expires_at,
            },
        )
        .collect();

    Ok(Json(BanListResponse { bans }))
}
