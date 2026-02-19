//! Vault handlers for wallet key management.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{info, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use auth::jwt::Claims;

use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use polymarket_core::api::PolygonClient;

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
///   2. User's default workspace `polygon_rpc_url`
///   3. User's default workspace `alchemy_api_key` (decrypted)
///   4. 503 ServiceUnavailable
async fn resolve_polygon_client_for_user(
    state: &AppState,
    user_id: Uuid,
) -> Result<PolygonClient, ApiError> {
    // Fast path: env-var-based client is already present.
    if let Some(client) = state.polygon_client.clone() {
        return Ok(client);
    }

    // Slow path: look up the user's workspace RPC settings from DB.
    warn!(
        user_id = %user_id,
        "polygon_client not in AppState; falling back to workspace-level RPC config"
    );

    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT w.polygon_rpc_url, w.alchemy_api_key
        FROM workspaces w
        INNER JOIN user_settings us ON us.default_workspace_id = w.id
        WHERE us.user_id = $1
        LIMIT 1
        "#,
    )
    .bind(user_id)
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
    let address = req.address.to_lowercase();
    if !address.starts_with("0x") || address.len() != 42 {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

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
    axum::extract::Path(address): axum::extract::Path<String>,
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
