//! Vault handlers for wallet key management and on-chain wallet withdrawals.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{info, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use auth::jwt::Claims;
use auth::{AuditAction, AuditEvent};

use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::workspace_scope::resolve_canonical_workspace_membership;
use polymarket_core::api::PolygonClient;

const POLYGONSCAN_TX_BASE_URL: &str = "https://polygonscan.com/tx/";

async fn resolve_primary_wallet_address(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let primary: Option<(String,)> = sqlx::query_as(
        "SELECT address FROM user_wallets WHERE user_id = $1 AND is_primary = true LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(primary.map(|(address,)| address))
}

/// Resolve a [`PolygonClient`] for the given user.
///
/// Fallback chain:
///   1. `state.polygon_client` (env-var-initialized at startup)
///   2. Canonical workspace `polygon_rpc_url`
///   3. Canonical workspace `alchemy_api_key` (decrypted)
///   4. 503 ServiceUnavailable
async fn resolve_polygon_client_for_user(
    state: &AppState,
    user_id: Uuid,
) -> Result<PolygonClient, ApiError> {
    // Fast path: env-var-based client is already present.
    if let Some(client) = state.polygon_client.clone() {
        return Ok(client);
    }

    // Slow path: look up the canonical workspace RPC settings from DB.
    warn!(
        user_id = %user_id,
        "polygon_client not in AppState; falling back to canonical workspace RPC config"
    );

    let workspace_id = resolve_canonical_workspace_membership(&state.pool, user_id)
        .await?
        .map(|workspace| workspace.id);

    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT w.polygon_rpc_url, w.alchemy_api_key
        FROM workspaces w
        WHERE w.id = $1
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await?;

    let (rpc_url_opt, alchemy_key_opt) = row.unwrap_or((None, None));

    // Priority: explicit RPC URL > Alchemy key.
    if let Some(rpc_url) = rpc_url_opt.filter(|s| !s.is_empty()) {
        return Ok(PolygonClient::new(rpc_url));
    }

    if let Some(encrypted_key) = alchemy_key_opt.filter(|s| !s.is_empty()) {
        // Decrypt the stored Alchemy key (AES-256-GCM).
        // Fall back to treating value as plaintext for backward compat with pre-encryption rows.
        let plaintext =
            crypto::decrypt_field(&encrypted_key, &state.encryption_key).unwrap_or(encrypted_key);
        return Ok(PolygonClient::with_alchemy(&plaintext));
    }

    Err(ApiError::ServiceUnavailable(
        "Polygon RPC not configured. Set polygon_rpc_url or alchemy_api_key in workspace settings."
            .into(),
    ))
}

async fn require_canonical_workspace_member(
    pool: &PgPool,
    user_id: Uuid,
) -> ApiResult<(Uuid, String)> {
    resolve_canonical_workspace_membership(pool, user_id)
        .await?
        .map(|workspace| (workspace.id, workspace.role))
        .ok_or_else(|| {
            ApiError::Forbidden("You do not have access to the trading workspace".into())
        })
}

fn validate_wallet_address(address: &str) -> ApiResult<String> {
    let normalized = address.trim().to_lowercase();
    let is_valid = normalized.len() == 42
        && normalized.starts_with("0x")
        && normalized
            .bytes()
            .skip(2)
            .all(|byte| byte.is_ascii_hexdigit());

    if !is_valid {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    Ok(normalized)
}

fn decimal_to_usdc_units(amount: Decimal) -> ApiResult<u128> {
    if amount <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "Withdrawal amount must be greater than zero".into(),
        ));
    }
    if amount.scale() > 6 {
        return Err(ApiError::BadRequest(
            "USDC withdrawals support at most 6 decimal places".into(),
        ));
    }

    let scaled = amount * Decimal::from(1_000_000u64);
    scaled
        .to_u128()
        .ok_or_else(|| ApiError::BadRequest("Withdrawal amount is too large or invalid".into()))
}

fn explorer_url_for_tx(tx_hash: &str) -> String {
    format!("{}{}", POLYGONSCAN_TX_BASE_URL, tx_hash)
}

/// Request to store a wallet.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StoreWalletRequest {
    /// Ethereum wallet address (0x...).
    pub address: String,
    /// Wallet private key (will be encrypted).
    pub private_key: String,
    /// Optional label for the wallet.
    #[serde(default)]
    pub label: Option<String>,
}

/// Response for wallet info (no private key).
#[derive(Debug, Serialize, ToSchema)]
pub struct WalletInfo {
    /// Wallet ID.
    pub id: String,
    /// Ethereum wallet address.
    pub address: String,
    /// Optional label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether this is the primary wallet.
    pub is_primary: bool,
    /// When the wallet was added.
    pub created_at: DateTime<Utc>,
}

/// Database row for user wallet.
#[derive(Debug, sqlx::FromRow)]
struct UserWalletRow {
    id: Uuid,
    address: String,
    label: Option<String>,
    is_primary: bool,
    created_at: DateTime<Utc>,
}

impl From<UserWalletRow> for WalletInfo {
    fn from(row: UserWalletRow) -> Self {
        Self {
            id: row.id.to_string(),
            address: row.address,
            label: row.label,
            is_primary: row.is_primary,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWithdrawalRequest {
    /// Source wallet address or alias ("primary" / "active"). Defaults to the active wallet.
    #[serde(default)]
    pub source_address: Option<String>,
    /// Destination Polygon wallet address.
    pub destination_address: String,
    /// Amount of USDC.e to transfer.
    pub amount: Decimal,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct WalletWithdrawalListQuery {
    /// Maximum number of recent withdrawals to return.
    #[serde(default = "default_withdrawal_limit")]
    pub limit: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WalletWithdrawalResponse {
    pub id: String,
    pub wallet_address: String,
    pub destination_address: String,
    pub asset: String,
    pub amount: Decimal,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explorer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub requested_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, sqlx::FromRow)]
struct WalletWithdrawalRow {
    id: Uuid,
    wallet_address: String,
    destination_address: String,
    asset: String,
    amount: Decimal,
    status: String,
    tx_hash: Option<String>,
    error: Option<String>,
    requested_at: DateTime<Utc>,
    confirmed_at: Option<DateTime<Utc>>,
}

impl From<WalletWithdrawalRow> for WalletWithdrawalResponse {
    fn from(row: WalletWithdrawalRow) -> Self {
        Self {
            id: row.id.to_string(),
            wallet_address: row.wallet_address,
            destination_address: row.destination_address,
            asset: row.asset,
            amount: row.amount,
            status: row.status,
            explorer_url: row.tx_hash.as_ref().map(|hash| explorer_url_for_tx(hash)),
            tx_hash: row.tx_hash,
            error: row.error,
            requested_at: row.requested_at,
            confirmed_at: row.confirmed_at,
        }
    }
}

fn default_withdrawal_limit() -> i64 {
    10
}

async fn resolve_user_wallet_address(
    state: &AppState,
    user_id: Uuid,
    requested_address: Option<&str>,
) -> ApiResult<String> {
    let requested = requested_address
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("active")
        .to_lowercase();

    let address = if requested == "primary" {
        resolve_primary_wallet_address(&state.pool, user_id)
            .await?
            .ok_or_else(|| ApiError::NotFound("No primary wallet connected".into()))?
    } else if requested == "active" {
        match state.order_executor.wallet_address().await {
            Some(address) => address.to_lowercase(),
            None => resolve_primary_wallet_address(&state.pool, user_id)
                .await?
                .ok_or_else(|| {
                    ApiError::NotFound("No active or primary wallet connected".into())
                })?,
        }
    } else {
        validate_wallet_address(&requested)?
    };

    let wallet_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM user_wallets WHERE user_id = $1 AND address = $2")
            .bind(user_id)
            .bind(&address)
            .fetch_optional(&state.pool)
            .await?;

    if wallet_exists.is_none() {
        return Err(ApiError::Forbidden(
            "The selected wallet is not connected under your account".into(),
        ));
    }

    Ok(address)
}

/// Store a wallet's private key in the vault.
#[utoipa::path(
    post,
    path = "/api/v1/vault/wallets",
    request_body = StoreWalletRequest,
    responses(
        (status = 201, description = "Wallet stored successfully", body = WalletInfo),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Wallet already exists"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "vault"
)]
pub async fn store_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<StoreWalletRequest>,
) -> ApiResult<(StatusCode, Json<WalletInfo>)> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    // Validate address format (basic check)
    let address = validate_wallet_address(&req.address)?;

    // Validate private key format
    let private_key = req.private_key.trim();
    let key_bytes = if let Some(stripped) = private_key.strip_prefix("0x") {
        hex::decode(stripped)
            .map_err(|_| ApiError::BadRequest("Invalid private key format".into()))?
    } else {
        hex::decode(private_key)
            .map_err(|_| ApiError::BadRequest("Invalid private key format".into()))?
    };

    if key_bytes.len() != 32 {
        return Err(ApiError::BadRequest("Private key must be 32 bytes".into()));
    }

    // Check if wallet already exists for this user
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM user_wallets WHERE user_id = $1 AND address = $2")
            .bind(user_id)
            .bind(&address)
            .fetch_optional(&state.pool)
            .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict("Wallet already connected".into()));
    }

    // Store the private key in the KeyVault
    state
        .key_vault
        .store_wallet_key(&address, &key_bytes)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store key: {}", e)))?;

    // Check if this is the first wallet (will be primary)
    let wallet_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM user_wallets WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&state.pool)
            .await?;
    let is_primary = wallet_count.0 == 0;

    // Save wallet metadata to database
    let wallet_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO user_wallets (id, user_id, address, label, is_primary, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $6)
        "#,
    )
    .bind(wallet_id)
    .bind(user_id)
    .bind(&address)
    .bind(&req.label)
    .bind(is_primary)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Seamless live mode: first/primary connected wallet becomes active trading wallet immediately.
    if is_primary && state.order_executor.is_live() {
        match state.activate_trading_wallet(&address).await {
            Ok(loaded) => info!(wallet = %loaded, "Activated primary wallet for live trading"),
            Err(e) => {
                warn!(wallet = %address, error = %e, "Failed to activate primary wallet for live trading")
            }
        }
    }

    Ok((
        StatusCode::CREATED,
        Json(WalletInfo {
            id: wallet_id.to_string(),
            address,
            label: req.label,
            is_primary,
            created_at: now,
        }),
    ))
}

/// List all connected wallets for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/vault/wallets",
    responses(
        (status = 200, description = "List of connected wallets", body = Vec<WalletInfo>),
        (status = 401, description = "Unauthorized"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "vault"
)]
pub async fn list_wallets(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<Vec<WalletInfo>>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let wallets: Vec<UserWalletRow> = sqlx::query_as(
        r#"
        SELECT id, address, label, is_primary, created_at
        FROM user_wallets
        WHERE user_id = $1
        ORDER BY is_primary DESC, created_at ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let wallet_infos: Vec<WalletInfo> = wallets.into_iter().map(Into::into).collect();
    Ok(Json(wallet_infos))
}

/// Check if a specific wallet is connected.
#[utoipa::path(
    get,
    path = "/api/v1/vault/wallets/{address}",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet info", body = WalletInfo),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Wallet not found"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "vault"
)]
pub async fn get_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> ApiResult<Json<WalletInfo>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let address = address.to_lowercase();

    let wallet: Option<UserWalletRow> = sqlx::query_as(
        r#"
        SELECT id, address, label, is_primary, created_at
        FROM user_wallets
        WHERE user_id = $1 AND address = $2
        "#,
    )
    .bind(user_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    match wallet {
        Some(w) => Ok(Json(w.into())),
        None => Err(ApiError::NotFound("Wallet not connected".into())),
    }
}

/// Remove a wallet from the vault.
#[utoipa::path(
    delete,
    path = "/api/v1/vault/wallets/{address}",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 204, description = "Wallet removed"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Wallet not found"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "vault"
)]
pub async fn remove_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> ApiResult<StatusCode> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let address = address.to_lowercase();

    // Check if wallet exists
    let wallet: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, is_primary FROM user_wallets WHERE user_id = $1 AND address = $2",
    )
    .bind(user_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    let (wallet_id, was_primary) = match wallet {
        Some(w) => w,
        None => return Err(ApiError::NotFound("Wallet not connected".into())),
    };

    // Remove from KeyVault
    let _ = state.key_vault.remove_wallet_key(&address).await;

    // Remove from database
    sqlx::query("DELETE FROM user_wallets WHERE id = $1")
        .bind(wallet_id)
        .execute(&state.pool)
        .await?;

    // If this was the primary wallet, make another one primary
    if was_primary {
        sqlx::query(
            r#"
            UPDATE user_wallets
            SET is_primary = true, updated_at = NOW()
            WHERE id = (
                SELECT id FROM user_wallets
                WHERE user_id = $1
                ORDER BY created_at ASC
                LIMIT 1
            )
            "#,
        )
        .bind(user_id)
        .execute(&state.pool)
        .await?;

        if state.order_executor.is_live() {
            let next_primary: Option<(String,)> = sqlx::query_as(
                "SELECT address FROM user_wallets WHERE user_id = $1 AND is_primary = true LIMIT 1",
            )
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

            if let Some((next_address,)) = next_primary {
                if let Err(e) = state.activate_trading_wallet(&next_address).await {
                    warn!(wallet = %next_address, error = %e, "Failed to activate fallback primary wallet");
                }
            } else {
                warn!("No fallback primary wallet available after removal");
            }
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Set a wallet as the primary wallet.
#[utoipa::path(
    put,
    path = "/api/v1/vault/wallets/{address}/primary",
    params(
        ("address" = String, Path, description = "Wallet address")
    ),
    responses(
        (status = 200, description = "Wallet set as primary", body = WalletInfo),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Wallet not found"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "vault"
)]
pub async fn set_primary_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> ApiResult<Json<WalletInfo>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let address = address.to_lowercase();

    // Check if wallet exists
    let wallet: Option<UserWalletRow> = sqlx::query_as(
        r#"
        SELECT id, address, label, is_primary, created_at
        FROM user_wallets
        WHERE user_id = $1 AND address = $2
        "#,
    )
    .bind(user_id)
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    let wallet = match wallet {
        Some(w) => w,
        None => return Err(ApiError::NotFound("Wallet not connected".into())),
    };

    // When live, verify the vault key exists before committing the DB change
    if state.order_executor.is_live() {
        let key_exists = state
            .key_vault
            .get_wallet_key(&wallet.address)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to check vault key: {}", e)))?
            .is_some();

        if !key_exists {
            return Err(ApiError::BadRequest(
                "Cannot set as primary: wallet key not found in vault".into(),
            ));
        }
    }

    // Unset all other wallets as primary
    sqlx::query(
        "UPDATE user_wallets SET is_primary = false, updated_at = NOW() WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    // Set this wallet as primary
    sqlx::query("UPDATE user_wallets SET is_primary = true, updated_at = NOW() WHERE id = $1")
        .bind(wallet.id)
        .execute(&state.pool)
        .await?;

    if state.order_executor.is_live() {
        state
            .activate_trading_wallet(&wallet.address)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to activate primary wallet: {}", e)))?;
    }

    Ok(Json(WalletInfo {
        id: wallet.id.to_string(),
        address: wallet.address,
        label: wallet.label,
        is_primary: true,
        created_at: wallet.created_at,
    }))
}

/// Response for wallet USDC balance.
#[derive(Debug, Serialize, ToSchema)]
pub struct WalletBalanceResponse {
    /// Wallet address.
    pub address: String,
    /// USDC balance (human-readable, 6 decimal places on-chain).
    pub usdc_balance: f64,
}

/// Get the on-chain USDC balance for a connected wallet.
#[utoipa::path(
    get,
    path = "/api/v1/vault/wallets/{address}/balance",
    params(
        ("address" = String, Path, description = "Wallet address, 'primary', or 'active'")
    ),
    responses(
        (status = 200, description = "Wallet USDC balance", body = WalletBalanceResponse),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Wallet not found"),
        (status = 503, description = "Polygon RPC not configured"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "vault"
)]
pub async fn get_wallet_balance(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Path(address): Path<String>,
) -> ApiResult<Json<WalletBalanceResponse>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let requested = address.to_lowercase();

    // Support aliases:
    // - "primary": user's primary connected wallet
    // - "active": currently loaded live trading wallet (fallback to primary)
    let address = if requested == "primary" {
        match resolve_primary_wallet_address(&state.pool, user_id).await? {
            Some(primary_address) => primary_address,
            None => return Err(ApiError::NotFound("No primary wallet connected".into())),
        }
    } else if requested == "active" {
        match state.order_executor.wallet_address().await {
            Some(active_address) => active_address.to_lowercase(),
            None => match resolve_primary_wallet_address(&state.pool, user_id).await? {
                Some(primary_address) => primary_address,
                None => {
                    return Err(ApiError::NotFound(
                        "No active or primary wallet connected".into(),
                    ))
                }
            },
        }
    } else {
        requested
    };

    // Verify user owns this wallet
    let wallet_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM user_wallets WHERE user_id = $1 AND address = $2")
            .bind(user_id)
            .bind(&address)
            .fetch_optional(&state.pool)
            .await?;

    if wallet_exists.is_none() {
        return Err(ApiError::NotFound("Wallet not connected".into()));
    }

    let polygon_client = resolve_polygon_client_for_user(&state, user_id).await?;

    let usdc_balance = polygon_client
        .get_usdc_balance(&address)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch USDC balance: {}", e)))?;

    Ok(Json(WalletBalanceResponse {
        address,
        usdc_balance,
    }))
}

/// List recent on-chain withdrawals for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/vault/withdrawals",
    params(
        ("limit" = Option<i64>, Query, description = "Maximum number of recent withdrawals to return")
    ),
    responses(
        (status = 200, description = "Recent wallet withdrawals", body = Vec<WalletWithdrawalResponse>),
        (status = 401, description = "Unauthorized")
    ),
    security(("bearer_auth" = [])),
    tag = "vault"
)]
pub async fn list_withdrawals(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<WalletWithdrawalListQuery>,
) -> ApiResult<Json<Vec<WalletWithdrawalResponse>>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let limit = query.limit.clamp(1, 100);
    let rows: Vec<WalletWithdrawalRow> = sqlx::query_as(
        r#"
        SELECT id, wallet_address, destination_address, asset, amount, status, tx_hash, error,
               requested_at, confirmed_at
        FROM wallet_withdrawals
        WHERE user_id = $1
        ORDER BY requested_at DESC
        LIMIT $2
        "#,
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// Send USDC.e from a connected vault wallet to an external Polygon wallet.
#[utoipa::path(
    post,
    path = "/api/v1/vault/withdrawals",
    request_body = CreateWithdrawalRequest,
    responses(
        (status = 200, description = "Withdrawal confirmed", body = WalletWithdrawalResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 503, description = "Polygon RPC not configured")
    ),
    security(("bearer_auth" = [])),
    tag = "vault"
)]
pub async fn create_withdrawal(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateWithdrawalRequest>,
) -> ApiResult<Json<WalletWithdrawalResponse>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;
    let (workspace_id, role) = require_canonical_workspace_member(&state.pool, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "Only workspace owners and admins can initiate withdrawals".into(),
        ));
    }

    let source_address =
        resolve_user_wallet_address(state.as_ref(), user_id, req.source_address.as_deref()).await?;
    let destination_address = validate_wallet_address(&req.destination_address)?;
    if destination_address == source_address {
        return Err(ApiError::BadRequest(
            "Destination matches the source wallet. If this address is already in MetaMask, no withdrawal is needed.".into(),
        ));
    }

    let amount_units = decimal_to_usdc_units(req.amount)?;
    let polygon_client = resolve_polygon_client_for_user(state.as_ref(), user_id).await?;
    let available_units = polygon_client
        .get_usdc_balance_units(&source_address)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch source wallet balance: {}", e)))?;
    if amount_units > available_units {
        return Err(ApiError::BadRequest(format!(
            "Insufficient USDC balance. Requested {}, available {:.6}",
            req.amount,
            available_units as f64 / 1_000_000.0
        )));
    }

    let wallet = state
        .load_wallet_from_vault(&source_address)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load wallet signer: {}", e)))?;
    let signer = wallet.signer();

    let withdrawal_id = Uuid::new_v4();
    let requested_at = Utc::now();
    sqlx::query(
        r#"
        INSERT INTO wallet_withdrawals (
            id, user_id, workspace_id, wallet_address, destination_address, asset, amount,
            status, requested_at, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, 'USDC', $6, 'pending', $7, $7, $7)
        "#,
    )
    .bind(withdrawal_id)
    .bind(user_id)
    .bind(workspace_id)
    .bind(&source_address)
    .bind(&destination_address)
    .bind(req.amount)
    .bind(requested_at)
    .execute(&state.pool)
    .await?;

    let tx_hash = match polygon_client
        .submit_usdc_transfer(signer, &destination_address, amount_units)
        .await
    {
        Ok(tx_hash) => {
            sqlx::query(
                r#"
                UPDATE wallet_withdrawals
                SET status = 'submitted', tx_hash = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(withdrawal_id)
            .bind(&tx_hash)
            .execute(&state.pool)
            .await?;
            tx_hash
        }
        Err(error) => {
            let message = error.to_string();
            sqlx::query(
                r#"
                UPDATE wallet_withdrawals
                SET status = 'failed', error = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(withdrawal_id)
            .bind(&message)
            .execute(&state.pool)
            .await?;

            state.audit_logger.log(
                AuditEvent::builder(
                    AuditAction::Custom("wallet_withdrawal".to_string()),
                    format!("wallet/{}", source_address),
                )
                .user(user_id.to_string())
                .details(serde_json::json!({
                    "destination_address": destination_address,
                    "amount": req.amount.to_string(),
                    "status": "failed_pre_submission"
                }))
                .failure(message.clone())
                .build(),
            );

            return Err(ApiError::Internal(format!(
                "Failed to submit withdrawal transaction: {}",
                message
            )));
        }
    };

    let receipt = match polygon_client
        .wait_for_transaction(&tx_hash, 60, std::time::Duration::from_secs(2))
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => {
            let message = error.to_string();
            sqlx::query(
                r#"
                UPDATE wallet_withdrawals
                SET status = 'failed', error = $2, updated_at = NOW()
                WHERE id = $1
                "#,
            )
            .bind(withdrawal_id)
            .bind(&message)
            .execute(&state.pool)
            .await?;

            state.audit_logger.log(
                AuditEvent::builder(
                    AuditAction::Custom("wallet_withdrawal".to_string()),
                    format!("wallet/{}", source_address),
                )
                .user(user_id.to_string())
                .details(serde_json::json!({
                    "destination_address": destination_address,
                    "amount": req.amount.to_string(),
                    "status": "failed_after_submission",
                    "tx_hash": tx_hash
                }))
                .failure(message.clone())
                .build(),
            );

            return Err(ApiError::Internal(format!(
                "Withdrawal transaction failed after submission: {}",
                message
            )));
        }
    };

    let confirmed_at = Utc::now();
    sqlx::query(
        r#"
        UPDATE wallet_withdrawals
        SET status = 'confirmed', confirmed_at = $2, updated_at = $2
        WHERE id = $1
        "#,
    )
    .bind(withdrawal_id)
    .bind(confirmed_at)
    .execute(&state.pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO cash_flow_events (
            id, workspace_id, event_type, amount, currency, note, occurred_at, created_by
        )
        VALUES ($1, $2, 'withdrawal', $3, 'USDC', $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(workspace_id)
    .bind(-req.amount)
    .bind(format!(
        "On-chain wallet withdrawal to {} (tx: {})",
        destination_address, tx_hash
    ))
    .bind(confirmed_at)
    .bind(user_id)
    .execute(&state.pool)
    .await?;

    state.audit_logger.log(
        AuditEvent::builder(
            AuditAction::Custom("wallet_withdrawal".to_string()),
            format!("wallet/{}", source_address),
        )
        .user(user_id.to_string())
        .details(serde_json::json!({
            "destination_address": destination_address,
            "amount": req.amount.to_string(),
            "tx_hash": tx_hash,
            "block_number": receipt.block_number,
            "gas_used": receipt.gas_used
        }))
        .build(),
    );

    Ok(Json(WalletWithdrawalResponse {
        id: withdrawal_id.to_string(),
        wallet_address: source_address,
        destination_address,
        asset: "USDC".to_string(),
        amount: req.amount,
        status: "confirmed".to_string(),
        tx_hash: Some(receipt.tx_hash.clone()),
        explorer_url: Some(explorer_url_for_tx(&receipt.tx_hash)),
        error: None,
        requested_at,
        confirmed_at: Some(confirmed_at),
    }))
}
