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
use tracing::warn;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use auth::jwt::Claims;
use auth::{AuditAction, AuditEvent};

use crate::crypto;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::workspace_scope::{
    resolve_canonical_workspace_id, resolve_canonical_workspace_membership,
};
use polymarket_core::api::polygon::{
    is_protocol_contract, BroadcastOutcome, WithdrawalSource, CALL_WOULD_FAIL,
};
use polymarket_core::api::PolygonClient;

use crate::withdrawals::{
    chain_error_message, fail_unsent, final_receipt, finalize_mined, hash_recorded,
    withdrawal_plan, Receipt, SpendLock, WithdrawalHolds,
};

#[cfg(test)]
#[path = "vault/ownership_tests.rs"]
mod wallet_ownership_tests;

#[cfg(test)]
#[path = "vault/withdrawal_tests.rs"]
mod withdrawal_tests;

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
pub(crate) async fn resolve_polygon_client_for_user(
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
    workspace_polygon_client(state, workspace_id).await
}

/// The [`PolygonClient`] for work done for the canonical workspace rather
/// than one user (reconciling withdrawals, feeding the circuit breaker): the
/// same chain as [`resolve_polygon_client_for_user`], whoever is a member.
pub(crate) async fn canonical_polygon_client(state: &AppState) -> Result<PolygonClient, ApiError> {
    if let Some(client) = state.polygon_client.clone() {
        return Ok(client);
    }
    let workspace_id = resolve_canonical_workspace_id(&state.pool).await?;
    workspace_polygon_client(state, workspace_id).await
}

async fn workspace_polygon_client(
    state: &AppState,
    workspace_id: Option<Uuid>,
) -> Result<PolygonClient, ApiError> {
    match workspace_rpc_url(&state.pool, &state.encryption_key, workspace_id).await? {
        Some(rpc_url) => Ok(PolygonClient::new(rpc_url)),
        None => Err(ApiError::ServiceUnavailable(
            "Polygon RPC not configured. Set polygon_rpc_url or alchemy_api_key in workspace settings."
                .into(),
        )),
    }
}

/// The Polygon RPC endpoint a workspace's settings name: its explicit RPC
/// URL, else its (decrypted) Alchemy key's endpoint. `None` when it names
/// neither.
pub(crate) async fn workspace_rpc_url(
    pool: &PgPool,
    encryption_key: &str,
    workspace_id: Option<Uuid>,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT w.polygon_rpc_url, w.alchemy_api_key
        FROM workspaces w
        WHERE w.id = $1
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;

    let (rpc_url_opt, alchemy_key_opt) = row.unwrap_or((None, None));

    // Priority: explicit RPC URL > Alchemy key.
    if let Some(rpc_url) = rpc_url_opt.filter(|s| !s.is_empty()) {
        return Ok(Some(rpc_url));
    }

    if let Some(encrypted_key) = alchemy_key_opt.filter(|s| !s.is_empty()) {
        // Decrypt the stored Alchemy key (AES-256-GCM).
        // Fall back to treating value as plaintext for backward compat with pre-encryption rows.
        let plaintext =
            crypto::decrypt_field(&encrypted_key, encryption_key).unwrap_or(encrypted_key);
        return Ok(Some(format!(
            "https://polygon-mainnet.g.alchemy.com/v2/{plaintext}"
        )));
    }

    Ok(None)
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
    let trimmed = address.trim();
    let normalized = trimmed.to_lowercase();
    let is_valid = normalized.len() == 42
        && normalized.starts_with("0x")
        && normalized
            .bytes()
            .skip(2)
            .all(|byte| byte.is_ascii_hexdigit());

    if !is_valid {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }
    // Mixed case carries an EIP-55 checksum: a mistyped character in it
    // would otherwise name an address nobody controls, and funds sent there
    // are lost. Single-case addresses carry none to check.
    let hex = &trimmed[2..];
    let mixed_case =
        hex.bytes().any(|b| b.is_ascii_uppercase()) && hex.bytes().any(|b| b.is_ascii_lowercase());
    if mixed_case && alloy_primitives::Address::parse_checksummed(format!("0x{hex}"), None).is_err()
    {
        return Err(ApiError::BadRequest(
            "Wallet address checksum does not match; check the address".into(),
        ));
    }

    Ok(normalized)
}

fn validate_wallet_key(address: &str, private_key: &str) -> ApiResult<Vec<u8>> {
    let normalized = validate_wallet_address(address)?;
    let private_key = private_key.trim();
    let wallet = auth::TradingWallet::from_private_key(private_key)
        .map_err(|_| ApiError::BadRequest("Invalid wallet private key".into()))?;
    if wallet.address_string().to_lowercase() != normalized {
        return Err(ApiError::BadRequest(
            "Private key does not match the wallet address".into(),
        ));
    }
    hex::decode(private_key.strip_prefix("0x").unwrap_or(private_key))
        .map_err(|_| ApiError::BadRequest("Invalid wallet private key".into()))
}

/// Serialize metadata and vault mutations across accounts and API replicas.
/// Lock the user first so concurrent first-wallet requests cannot both become primary.
/// The final lock also protects file-provider read/modify/write across addresses.
async fn lock_wallet_mutation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    address: &str,
) -> ApiResult<()> {
    for key in [
        format!("wallet-user:{user_id}"),
        format!("wallet-address:{address}"),
        "wallet-vault-file".to_string(),
    ] {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
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

    amount
        .checked_mul(Decimal::from(1_000_000u64))
        .and_then(|scaled| scaled.to_u128())
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
    /// Client-generated idempotency key. If a withdrawal with this key already exists for the
    /// source wallet, it is returned as-is without creating a duplicate.
    #[serde(default)]
    pub client_request_id: Option<Uuid>,
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

    let wallet = register_wallet(&state.pool, &state.key_vault, user_id, &req).await?;
    Ok((StatusCode::CREATED, Json(wallet)))
}

async fn register_wallet(
    pool: &PgPool,
    vault: &auth::KeyVault,
    user_id: Uuid,
    req: &StoreWalletRequest,
) -> ApiResult<WalletInfo> {
    // Validate address format (basic check)
    let address = validate_wallet_address(&req.address)?;

    // Prove control before touching metadata or the address-indexed vault.
    let key_bytes = validate_wallet_key(&address, &req.private_key)?;
    let mut tx = pool.begin().await?;
    lock_wallet_mutation(&mut tx, user_id, &address).await?;

    // Vault keys are global by address, so ownership checks must be global too.
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM user_wallets WHERE LOWER(address) = $1 LIMIT 1")
            .bind(&address)
            .fetch_optional(&mut *tx)
            .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "Wallet address is already connected".into(),
        ));
    }

    // Existing keys can come from startup configuration or a prior failed DB commit.
    // Reuse identical material; never replace a key when connecting metadata.
    let existing_key = vault
        .get_wallet_key(&address)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check wallet key: {}", e)))?;
    if existing_key.as_ref().is_some_and(|key| key != &key_bytes) {
        return Err(ApiError::Conflict(
            "Wallet key does not match the stored signer".into(),
        ));
    }

    // Check if this is the first wallet (will be primary)
    let wallet_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM user_wallets WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
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
    .execute(&mut *tx)
    .await?;

    if existing_key.is_none() {
        vault
            .store_wallet_key(&address, &key_bytes)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to store wallet key");
                ApiError::ServiceUnavailable("The key vault could not store the wallet key".into())
            })?;
    }
    tx.commit().await?;

    Ok(WalletInfo {
        id: wallet_id.to_string(),
        address,
        label: req.label.clone(),
        is_primary,
        created_at: now,
    })
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

    let active_signer = state.order_executor.wallet_address().await;
    disconnect_wallet(
        &state.pool,
        &state.key_vault,
        active_signer.as_deref(),
        user_id,
        &address,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect_wallet(
    pool: &PgPool,
    vault: &auth::KeyVault,
    active_signer: Option<&str>,
    user_id: Uuid,
    address: &str,
) -> ApiResult<()> {
    let address = validate_wallet_address(address)?;
    // The executor signs with the key it loaded; erasing it from the vault
    // would not stop that, only strand the next start.
    if active_signer.is_some_and(|signer| signer.eq_ignore_ascii_case(&address)) {
        return Err(ApiError::Conflict(
            "This wallet is the live trading signer the server is using; configure another \
             signer and restart before disconnecting it"
                .into(),
        ));
    }

    let mut tx = pool.begin().await?;
    lock_wallet_mutation(&mut tx, user_id, &address).await?;

    // Check if wallet exists
    let wallet: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, is_primary FROM user_wallets WHERE user_id = $1 AND LOWER(address) = $2",
    )
    .bind(user_id)
    .bind(&address)
    .fetch_optional(&mut *tx)
    .await?;

    let (wallet_id, was_primary) = match wallet {
        Some(w) => w,
        None => return Err(ApiError::NotFound("Wallet not connected".into())),
    };

    // The pinned trading signer must keep its key while anything it traded is
    // still open: exits, reconciliation and withdrawals all need it.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('position-exposure'))")
        .execute(&mut *tx)
        .await?;
    let pinned: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE LOWER(trading_wallet_address) = $1 \
         AND id = ({}))",
        crate::workspace_scope::CANONICAL_WORKSPACE_SQL
    ))
    .bind(&address)
    .fetch_one(&mut *tx)
    .await?;
    if pinned {
        let exposed: bool = sqlx::query_scalar(crate::state::STARTUP_EXPOSURE_SQL)
            .fetch_one(&mut *tx)
            .await?;
        if exposed {
            return Err(ApiError::Conflict(
                "This wallet signs live trading and positions or orders are still open; \
                 close them before disconnecting it"
                    .into(),
            ));
        }
    }

    // Legacy data may contain multiple registrations, including mixed-case rows.
    // Disconnecting one row must not remove signer material still referenced elsewhere.
    let shared: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM user_wallets WHERE LOWER(address) = $1 AND id <> $2)",
    )
    .bind(&address)
    .bind(wallet_id)
    .fetch_one(&mut *tx)
    .await?;
    if !shared {
        // Its key is about to go: the next start must not look for it.
        sqlx::query(
            "UPDATE workspaces SET trading_wallet_address = NULL \
             WHERE LOWER(trading_wallet_address) = $1",
        )
        .bind(&address)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM user_wallets WHERE id = $1")
        .bind(wallet_id)
        .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    if shared {
        return Ok(());
    }

    // The key goes only after the disconnect is committed, so a failure
    // leaves an unused key (which reconnecting reuses), never a connected
    // wallet without one. Recheck under the locks: the address may have been
    // connected again meanwhile.
    let mut tx = pool.begin().await?;
    lock_wallet_mutation(&mut tx, user_id, &address).await?;
    let reconnected: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM user_wallets WHERE LOWER(address) = $1)")
            .bind(&address)
            .fetch_one(&mut *tx)
            .await?;
    if !reconnected {
        vault.remove_wallet_key(&address).await.map_err(|e| {
            tracing::error!(error = %e, wallet = %address, "Disconnected a wallet but could not erase its key");
            ApiError::ServiceUnavailable(
                "The wallet was disconnected, but its key could not be erased from the vault; \
                 an operator must remove it"
                    .into(),
            )
        })?;
    }
    tx.commit().await?;
    Ok(())
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

    Ok(Json(
        set_personal_primary_wallet(&state.pool, user_id, &address).await?,
    ))
}

async fn set_personal_primary_wallet(
    pool: &PgPool,
    user_id: Uuid,
    address: &str,
) -> ApiResult<WalletInfo> {
    let address = validate_wallet_address(address)?;
    let mut tx = pool.begin().await?;
    lock_wallet_mutation(&mut tx, user_id, &address).await?;

    // Check if wallet exists
    let wallet: Option<UserWalletRow> = sqlx::query_as(
        r#"
        SELECT id, address, label, is_primary, created_at
        FROM user_wallets
        WHERE user_id = $1 AND LOWER(address) = $2
        "#,
    )
    .bind(user_id)
    .bind(&address)
    .fetch_optional(&mut *tx)
    .await?;

    let wallet = match wallet {
        Some(w) => w,
        None => return Err(ApiError::NotFound("Wallet not connected".into())),
    };

    // Unset all other wallets as primary
    sqlx::query(
        "UPDATE user_wallets SET is_primary = false, updated_at = NOW() WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    // Set this wallet as primary
    sqlx::query("UPDATE user_wallets SET is_primary = true, updated_at = NOW() WHERE id = $1")
        .bind(wallet.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(WalletInfo {
        id: wallet.id.to_string(),
        address: wallet.address,
        label: wallet.label,
        is_primary: true,
        created_at: wallet.created_at,
    })
}

/// Response for wallet USDC balance.
#[derive(Debug, Serialize, ToSchema)]
pub struct WalletBalanceResponse {
    /// Wallet address.
    pub address: String,
    /// Cash balance in USDC: its USDC.e and its pUSD (the exchange's
    /// collateral, redeemable 1:1 for USDC.e).
    pub usdc_balance: f64,
}

/// Get the on-chain cash balance (USDC.e and pUSD) of a connected wallet.
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
        .get_cash_balances(&address)
        .await
        .map_err(|e| {
            warn!(error = %chain_error_message(&e), "Failed to fetch wallet balance");
            ApiError::ServiceUnavailable("Could not read the wallet's balance".into())
        })?
        .total()
        .to_f64()
        .unwrap_or_default();

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

async fn existing_withdrawal<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    user_id: Uuid,
    source: &str,
    destination: &str,
    amount: Decimal,
    request_id: Uuid,
) -> ApiResult<Option<WalletWithdrawalRow>> {
    let row: Option<WalletWithdrawalRow> = sqlx::query_as(
        "SELECT id, wallet_address, destination_address, asset, amount, status, tx_hash, error, \
         requested_at, confirmed_at FROM wallet_withdrawals \
         WHERE wallet_address = $1 AND client_request_id = $2 AND user_id = $3",
    )
    .bind(source)
    .bind(request_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    if let Some(row) = &row {
        if !row.destination_address.eq_ignore_ascii_case(destination) || row.amount != amount {
            return Err(ApiError::Conflict(
                "client_request_id was already used for a different withdrawal".into(),
            ));
        }
    }
    Ok(row)
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

    let request_id = req.client_request_id.ok_or_else(|| {
        ApiError::BadRequest("client_request_id is required for safe withdrawal retries".into())
    })?;
    let destination_address = validate_wallet_address(&req.destination_address)?;
    let amount_units = decimal_to_usdc_units(req.amount)?;
    if let Some(row) = existing_withdrawal(
        &state.pool,
        user_id,
        &source_address,
        &destination_address,
        req.amount,
        request_id,
    )
    .await?
    {
        return Ok(Json(row.into()));
    }

    if destination_address == source_address {
        return Err(ApiError::BadRequest(
            "Destination matches the source wallet. If this address is already in MetaMask, no withdrawal is needed.".into(),
        ));
    }
    if is_protocol_contract(&destination_address) {
        return Err(ApiError::BadRequest(
            "Destination is a token or Polymarket contract; funds sent there cannot be recovered"
                .into(),
        ));
    }

    let polygon_client = resolve_polygon_client_for_user(state.as_ref(), user_id).await?;
    let wallet = state
        .load_wallet_from_vault(&source_address)
        .await
        .map_err(|e| {
            warn!(error = %e, "Failed to load wallet signer for withdrawal");
            ApiError::ServiceUnavailable("The wallet's signing key is unavailable".into())
        })?;

    let request = WithdrawalRequest {
        user_id,
        workspace_id,
        source: source_address,
        destination: destination_address,
        amount: req.amount,
        amount_units,
        request_id,
    };
    // Once a transfer is signed its outcome must be recorded, so the work runs
    // to the end even if the client goes away.
    let row = tokio::spawn(submit_withdrawal(state, polygon_client, wallet, request))
        .await
        .map_err(|e| ApiError::Internal(format!("Withdrawal task failed: {e}")))??;
    Ok(Json(row.into()))
}

/// A validated withdrawal request.
struct WithdrawalRequest {
    user_id: Uuid,
    workspace_id: Uuid,
    source: String,
    destination: String,
    amount: Decimal,
    amount_units: u128,
    request_id: Uuid,
}

impl WithdrawalRequest {
    fn audit(&self, state: &AppState, withdrawal_id: Uuid, status: &str, failure: Option<String>) {
        let mut event = AuditEvent::builder(
            AuditAction::Custom("wallet_withdrawal".to_string()),
            format!("wallet/{}", self.source),
        )
        .user(self.user_id.to_string())
        .details(serde_json::json!({
            "withdrawal_id": withdrawal_id,
            "destination_address": self.destination,
            "amount": self.amount.to_string(),
            "status": status,
        }));
        if let Some(failure) = failure {
            event = event.failure(failure);
        }
        state.audit_logger.log(event.build());
    }
}

/// How long a withdrawal waits for another spend from the wallet (another
/// withdrawal, or a collateral wrap being mined) before it is refused.
const SPEND_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
/// How long the balance read may hold up entries, which wait on the lock.
const WITHDRAWAL_BALANCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// How many times, two seconds apart, a request looks for its transfer's
/// final receipt.
const WITHDRAWAL_RECEIPT_POLLS: u32 = if cfg!(test) { 2 } else { 30 };

/// A request's row: one it had already made, or the one it just reserved.
enum Reservation {
    Existing(WalletWithdrawalRow),
    Reserved {
        id: Uuid,
        requested_at: DateTime<Utc>,
        source: WithdrawalSource,
    },
}

/// Refuse while the wallet has a transaction waiting to be mined other than
/// its own withdrawals sent and not yet mined (a collateral wrap, an
/// approval, anything sent from elsewhere): a transfer signed behind it
/// would plan on balances it is about to change, or wait on a nonce nothing
/// fills should it be dropped. The spend lock keeps the sweeper's wraps out
/// while it holds; this also holds once it lets go, or should it be lost.
async fn unexplained_pending_transaction(
    lock: &mut SpendLock,
    client: &PolygonClient,
    source: &str,
) -> ApiResult<()> {
    let address = source
        .parse::<alloy_primitives::Address>()
        .map_err(|_| ApiError::BadRequest("Invalid source wallet address".into()))?;
    let (mined, pending) = match tokio::time::timeout(
        WITHDRAWAL_BALANCE_TIMEOUT,
        client.nonces(&address),
    )
    .await
    {
        Ok(Ok(nonces)) => nonces,
        Ok(Err(e)) => {
            warn!(error = %chain_error_message(&e), "Failed to read the source wallet's nonces");
            return Err(ApiError::ServiceUnavailable(
                "Could not read the wallet's pending transactions".into(),
            ));
        }
        Err(_) => {
            return Err(ApiError::ServiceUnavailable(
                "Reading the wallet's pending transactions timed out".into(),
            ))
        }
    };
    if pending <= mined {
        return Ok(());
    }
    // A nonce still waiting is explained only by one of the wallet's own
    // transfers signed at it that the node holds: one already mined, signed
    // above what the node holds, or dropped by it, explains none of them.
    let own: Vec<(i64, Option<String>, Vec<String>)> = sqlx::query_as(
        "SELECT nonce, tx_hash, replaced_tx_hashes FROM wallet_withdrawals \
         WHERE LOWER(wallet_address) = LOWER($1) AND status = 'submitted' \
           AND nonce >= $2 AND nonce < $3",
    )
    .bind(source)
    .bind(i64::try_from(mined).unwrap_or(i64::MAX))
    .bind(i64::try_from(pending).unwrap_or(i64::MAX))
    .fetch_all(lock.conn())
    .await?;
    let mut explained = std::collections::HashSet::new();
    for (nonce, tx_hash, replaced) in own {
        if explained.contains(&nonce) {
            continue;
        }
        for hash in tx_hash.into_iter().chain(replaced) {
            match tokio::time::timeout(WITHDRAWAL_BALANCE_TIMEOUT, client.transaction_known(&hash))
                .await
            {
                Ok(Ok(true)) => {
                    explained.insert(nonce);
                    break;
                }
                Ok(Ok(false)) => {}
                Ok(Err(e)) => {
                    warn!(error = %chain_error_message(&e), "Failed to look up a pending withdrawal");
                    return Err(ApiError::ServiceUnavailable(
                        "Could not read the wallet's pending transactions".into(),
                    ));
                }
                Err(_) => {
                    return Err(ApiError::ServiceUnavailable(
                        "Reading the wallet's pending transactions timed out".into(),
                    ))
                }
            }
        }
    }
    if pending - mined > explained.len() as u64 {
        return Err(ApiError::ServiceUnavailable(
            "A transaction from this wallet is still pending; nothing was sent, retry shortly"
                .into(),
        ));
    }
    Ok(())
}

/// Check capacity and write the `pending` row, under the wallet's spend lock
/// and, while deciding, the entries lock. Nothing is signed yet.
async fn reserve_withdrawal(
    lock: &mut SpendLock,
    client: &PolygonClient,
    request: &WithdrawalRequest,
) -> ApiResult<Reservation> {
    // A concurrent request with the same id may have held the lock first.
    if let Some(row) = existing_withdrawal(
        lock.conn(),
        request.user_id,
        &request.source,
        &request.destination,
        request.amount,
        request.request_id,
    )
    .await?
    {
        return Ok(Reservation::Existing(row));
    }

    // Read before entries wait on this decision: it is a chain read.
    unexplained_pending_transaction(lock, client, &request.source).await?;
    lock.lock_entries().await?;
    // Holds first, then the balance: a transfer settling in between can only
    // make the capacity look smaller.
    let holds = WithdrawalHolds::load(lock.conn(), &request.source).await?;
    let cash = match tokio::time::timeout(
        WITHDRAWAL_BALANCE_TIMEOUT,
        client.get_cash_balances(&request.source),
    )
    .await
    {
        Ok(Ok(cash)) => cash,
        Ok(Err(e)) => {
            warn!(error = %chain_error_message(&e), "Failed to fetch source wallet balance");
            return Err(ApiError::ServiceUnavailable(
                "Could not read the wallet's USDC balance".into(),
            ));
        }
        Err(_) => {
            return Err(ApiError::ServiceUnavailable(
                "Reading the wallet's USDC balance timed out".into(),
            ))
        }
    };
    let (capacity, source) = withdrawal_plan(&holds, &cash, request.amount);
    if let Some(reason) = capacity.blocking_reasons.first() {
        return Err(ApiError::Conflict(reason.clone()));
    }
    let Some(source) = source else {
        return Err(ApiError::BadRequest(exceeds_capacity(
            request.amount,
            &capacity,
        )));
    };

    let id = Uuid::new_v4();
    let requested_at = Utc::now();
    let inserted = sqlx::query(
        r#"
        INSERT INTO wallet_withdrawals (
            id, user_id, workspace_id, wallet_address, destination_address, asset, amount,
            status, requested_at, created_at, updated_at, client_request_id, journaled,
            trading_wallet
        )
        VALUES ($1, $2, $3, $4, $5, 'USDC', $6, 'pending', $7, $7, $7, $8, TRUE, $9)
        "#,
    )
    .bind(id)
    .bind(request.user_id)
    .bind(request.workspace_id)
    .bind(&request.source)
    .bind(&request.destination)
    .bind(request.amount)
    .bind(requested_at)
    .bind(request.request_id)
    .bind(holds.trading_wallet)
    .execute(lock.conn())
    .await;
    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.code().is_some_and(|c| c == "23505") => {
            // Unique constraint violation on (wallet_address, client_request_id).
            if let Some(row) = existing_withdrawal(
                lock.conn(),
                request.user_id,
                &request.source,
                &request.destination,
                request.amount,
                request.request_id,
            )
            .await?
            {
                return Ok(Reservation::Existing(row));
            }
            return Err(ApiError::Conflict(
                "A withdrawal with this client_request_id was processed concurrently. \
                 Retry with the same client_request_id after checking withdrawal history."
                    .into(),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    // The row now counts against capacity everywhere: entries may resume.
    lock.unlock_entries().await?;
    Ok(Reservation::Reserved {
        id,
        requested_at,
        source,
    })
}

/// Why `amount` may not leave a wallet with `capacity`.
fn exceeds_capacity(amount: Decimal, capacity: &crate::withdrawals::WithdrawalCapacity) -> String {
    format!(
        "Withdrawal of {} USDC exceeds the {} USDC that can leave this wallet in one \
         withdrawal (balance {} in USDC.e and pUSD, drawn on one of them; unconfirmed \
         withdrawals {}, open live buy orders {})",
        amount,
        capacity.max_withdrawable,
        capacity.available_usdc,
        capacity.in_flight_withdrawals,
        capacity.reserved_for_open_buys
    )
}

/// Reserve, sign, journal and send a withdrawal, then wait briefly for it to
/// settle. Errors are returned only while nothing has been sent; after the
/// broadcast every failure is logged and the reconciler takes over.
async fn submit_withdrawal(
    state: Arc<AppState>,
    client: PolygonClient,
    wallet: auth::TradingWallet,
    request: WithdrawalRequest,
) -> ApiResult<WalletWithdrawalRow> {
    let Some(mut lock) =
        SpendLock::acquire_within(&state.pool, &request.source, SPEND_LOCK_WAIT).await?
    else {
        return Err(ApiError::ServiceUnavailable(
            "Another transaction from this wallet is being sent; nothing was sent, retry shortly"
                .into(),
        ));
    };
    let (withdrawal_id, requested_at, source) =
        match reserve_withdrawal(&mut lock, &client, &request).await {
            Ok(Reservation::Reserved {
                id,
                requested_at,
                source,
            }) => (id, requested_at, source),
            Ok(Reservation::Existing(row)) => {
                lock.release().await;
                return Ok(row);
            }
            Err(error) => {
                lock.release().await;
                return Err(error);
            }
        };

    let signed = match sign_new_transfer(lock.conn(), &client, &wallet, &request, source).await {
        Ok(signed) => signed,
        Err(error) => {
            // Nothing was signed for the chain to see: a certain non-transfer.
            if let Err(e) = fail_unsent(lock.conn(), withdrawal_id, &error).await {
                // Still `pending` without a hash: the reconciler fails it.
                warn!(withdrawal_id = %withdrawal_id, error = %e, "Could not record an unsent withdrawal");
            }
            lock.release().await;
            request.audit(&state, withdrawal_id, "failed", Some(error.clone()));
            return Err(if error.contains("Insufficient POL") {
                ApiError::BadRequest(error)
            } else if error.contains(CALL_WOULD_FAIL) {
                ApiError::Conflict(format!(
                    "The transfer would fail on chain now, so nothing was sent ({error})"
                ))
            } else {
                ApiError::ServiceUnavailable(
                    "Could not prepare the transfer; nothing was sent".into(),
                )
            });
        }
    };

    // Journal the transfer before it can leave: from here on its hash and
    // nonce let the reconciler find out what happened whatever follows.
    let journaled = sqlx::query(
        "UPDATE wallet_withdrawals SET status = 'submitting', tx_hash = $2, nonce = $3, \
         raw_tx = $4, signed_at = NOW(), updated_at = NOW() WHERE id = $1 AND status = 'pending'",
    )
    .bind(withdrawal_id)
    .bind(&signed.tx_hash)
    .bind(signed.nonce as i64)
    .bind(&signed.raw)
    .execute(lock.conn())
    .await;
    if !matches!(&journaled, Ok(result) if result.rows_affected() == 1) {
        // This request sends nothing. If the journal write committed despite
        // the error, the reconciler sends the journaled transfer; otherwise a
        // `pending` row without a hash is failed by it.
        lock.release().await;
        if let Err(error) = journaled {
            warn!(withdrawal_id = %withdrawal_id, %error, "Could not journal a withdrawal; not sending it");
        }
        return Err(ApiError::ServiceUnavailable(
            "Could not record the transfer, so this request did not send it. Check this \
             withdrawal's status (retrying with the same client_request_id shows it) before \
             starting another"
                .into(),
        ));
    }

    let (mut status, mut error) = ("submitting", None::<String>);
    let note = match client.broadcast(&signed.raw, &signed.tx_hash).await {
        Ok(BroadcastOutcome::Accepted) => {
            status = "submitted";
            None
        }
        Ok(BroadcastOutcome::Rejected(reason)) => {
            // A refusal no held transaction can get, but make sure: a node
            // that knows the transfer, or a receipt, means it was sent.
            let reason: String = reason.chars().take(300).collect();
            let unsent = matches!(client.transaction_known(&signed.tx_hash).await, Ok(false))
                && matches!(client.transaction_receipt(&signed.tx_hash).await, Ok(None));
            if unsent {
                let message = format!("Rejected by the Polygon node: {reason}");
                match fail_unsent(lock.conn(), withdrawal_id, &message).await {
                    Ok(_) => {
                        status = "failed";
                        error = Some(message);
                        request.audit(&state, withdrawal_id, "failed", Some(reason));
                        None
                    }
                    Err(e) => {
                        warn!(withdrawal_id = %withdrawal_id, error = %e, "Could not record a refused withdrawal");
                        Some(UNRESOLVED_BROADCAST_NOTE)
                    }
                }
            } else {
                warn!(withdrawal_id = %withdrawal_id, %reason, "Node refused a withdrawal it may hold");
                Some(UNRESOLVED_BROADCAST_NOTE)
            }
        }
        Ok(BroadcastOutcome::Uncertain(reason)) => {
            warn!(withdrawal_id = %withdrawal_id, %reason, "Withdrawal broadcast outcome unknown");
            Some(UNRESOLVED_BROADCAST_NOTE)
        }
        Err(e) => {
            warn!(withdrawal_id = %withdrawal_id, error = %chain_error_message(&e), "Withdrawal broadcast outcome unknown");
            Some(UNRESOLVED_BROADCAST_NOTE)
        }
    };
    let recorded = match note {
        None if status == "submitted" => sqlx::query(
            "UPDATE wallet_withdrawals SET status = 'submitted', updated_at = NOW() \
             WHERE id = $1 AND status = 'submitting'",
        )
        .bind(withdrawal_id)
        .execute(lock.conn())
        .await
        .map(drop),
        None => Ok(()),
        Some(note) => {
            error = Some(note.to_string());
            request.audit(
                &state,
                withdrawal_id,
                "submission_requires_reconciliation",
                None,
            );
            sqlx::query(
                "UPDATE wallet_withdrawals SET error = $2, updated_at = NOW() \
                 WHERE id = $1 AND status = 'submitting'",
            )
            .bind(withdrawal_id)
            .bind(note)
            .execute(lock.conn())
            .await
            .map(drop)
        }
    };
    if let Err(e) = recorded {
        // The journal already holds the hash; the reconciler settles it.
        warn!(withdrawal_id = %withdrawal_id, error = %e, "Could not record a withdrawal's broadcast");
    }
    lock.release().await;

    // Wait briefly for the final receipt; the reconciler settles anything slower.
    let mut confirmed_at = None;
    if status != "failed" {
        for _ in 0..WITHDRAWAL_RECEIPT_POLLS {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            match final_receipt(&client, &signed.tx_hash).await {
                Ok(Receipt::Final(mined)) => {
                    let settled = match state.pool.acquire().await {
                        Ok(mut conn) => {
                            finalize_mined(&mut conn, withdrawal_id, &signed.tx_hash, &mined).await
                        }
                        Err(e) => Err(e),
                    };
                    match settled {
                        Ok(changed) => {
                            if changed {
                                request.audit(
                                    &state,
                                    withdrawal_id,
                                    if mined.success {
                                        "confirmed"
                                    } else {
                                        "reverted"
                                    },
                                    None,
                                );
                            }
                            if mined.success {
                                (status, error, confirmed_at) =
                                    ("confirmed", None, Some(Utc::now()));
                            }
                        }
                        Err(e) => {
                            warn!(withdrawal_id = %withdrawal_id, error = %e, "Could not settle a mined withdrawal; the reconciler will")
                        }
                    }
                    break;
                }
                Ok(Receipt::Unmined | Receipt::Unsettled) => continue,
                Err(_) => break,
            }
        }
    }

    let row: Result<WalletWithdrawalRow, sqlx::Error> = sqlx::query_as(
        "SELECT id, wallet_address, destination_address, asset, amount, status, tx_hash, error, \
         requested_at, confirmed_at FROM wallet_withdrawals WHERE id = $1",
    )
    .bind(withdrawal_id)
    .fetch_one(&state.pool)
    .await;
    Ok(row.unwrap_or_else(|e| {
        warn!(withdrawal_id = %withdrawal_id, error = %e, "Could not read back a sent withdrawal");
        WalletWithdrawalRow {
            id: withdrawal_id,
            wallet_address: request.source.clone(),
            destination_address: request.destination.clone(),
            asset: "USDC".into(),
            amount: request.amount,
            status: status.into(),
            tx_hash: Some(signed.tx_hash.clone()),
            error,
            requested_at,
            confirmed_at,
        }
    }))
}

/// Sign the request's transfer as a transaction no other withdrawal has
/// recorded. Signing is deterministic, so a retry of a transfer that was
/// refused at the same nonce would repeat the refused one's hash exactly; it
/// is signed again at a higher gas price, which also lets it replace that one
/// should a node hold it after all. Only one of the two can be mined. The
/// error is a client-safe reason.
async fn sign_new_transfer(
    conn: &mut sqlx::PgConnection,
    client: &PolygonClient,
    wallet: &auth::TradingWallet,
    request: &WithdrawalRequest,
    source: WithdrawalSource,
) -> Result<polymarket_core::api::polygon::SignedTransaction, String> {
    let mut min_gas_price = 0;
    for _ in 0..3 {
        let signed = client
            .prepare_withdrawal(
                wallet.signer(),
                &request.destination,
                request.amount_units,
                source,
                min_gas_price,
            )
            .await
            .map_err(|error| chain_error_message(&error))?;
        let recorded = hash_recorded(&mut *conn, &signed.tx_hash)
            .await
            .map_err(|error| {
                warn!(%error, "Could not check a signed withdrawal against the journal");
                "Could not check the signed transfer".to_string()
            })?;
        if !recorded {
            return Ok(signed);
        }
        // Nodes accept a replacement paying at least 10% more.
        min_gas_price = signed.gas_price + signed.gas_price / 10 + 1;
    }
    Err("Could not sign a transfer distinct from earlier ones".into())
}

/// What a transfer whose broadcast outcome is unknown tells its owner.
const UNRESOLVED_BROADCAST_NOTE: &str =
    "Broadcast outcome unknown; it is being resolved from the chain. Do not start another transfer.";

// ─── D3b: Withdrawal Preflight ────────────────────────────────────────────────

/// Query parameters for the withdrawal preflight check.
#[derive(Debug, Deserialize, IntoParams)]
pub struct WithdrawalPreflightQuery {
    /// Wallet address, "primary", or "active". Defaults to the active wallet.
    pub address: Option<String>,
    /// Proposed withdrawal amount in USDC.
    pub amount: Decimal,
}

/// Response from the withdrawal preflight check.
#[derive(Debug, Serialize, ToSchema)]
pub struct WithdrawalPreflightResponse {
    /// On-chain cash balance of the source wallet (USDC.e and pUSD, in USDC).
    pub available_usdc: Decimal,
    /// Withdrawals from this wallet not yet confirmed or rejected.
    pub in_flight_withdrawals: Decimal,
    /// USDC committed to the trading wallet's open live buy orders.
    pub reserved_for_open_buys: Decimal,
    /// Market value of orphaned (untracked) inventory tokens.
    pub orphan_exposure: Decimal,
    /// Maximum USDC that one withdrawal can take from the wallet now (what a
    /// withdrawal enforces: it draws on either its USDC.e or its pUSD).
    pub max_withdrawable: Decimal,
    /// Whether the wallet inventory data is considered fresh (< 10 minutes old).
    pub inventory_fresh: bool,
    /// Timestamp of the most recent wallet inventory sync, if any.
    pub inventory_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the requested amount can be safely withdrawn.
    pub can_withdraw: bool,
    /// Human-readable reasons that block the withdrawal, if any.
    pub blocking_reasons: Vec<String>,
}

/// Check whether a proposed withdrawal is safe before submitting it.
///
/// Applies the same capacity rules as a withdrawal (balance less unconfirmed
/// withdrawals and open live buys; blocked while live trades are in flight)
/// and reports orphan exposure. No funds are moved.
#[utoipa::path(
    get,
    path = "/api/v1/vault/withdrawals/preflight",
    params(WithdrawalPreflightQuery),
    responses(
        (status = 200, description = "Preflight result", body = WithdrawalPreflightResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 503, description = "Polygon RPC not configured"),
    ),
    security(("bearer_auth" = [])),
    tag = "vault"
)]
pub async fn withdrawal_preflight(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WithdrawalPreflightQuery>,
    Extension(claims): Extension<Claims>,
) -> ApiResult<Json<WithdrawalPreflightResponse>> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    let (_, role) = require_canonical_workspace_member(&state.pool, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "Only workspace owners and admins can use the withdrawal preflight".into(),
        ));
    }

    // Validate requested amount.
    if params.amount <= Decimal::ZERO {
        return Err(ApiError::BadRequest(
            "amount must be greater than zero".into(),
        ));
    }

    // Resolve the source wallet address.
    let source_address =
        resolve_user_wallet_address(state.as_ref(), user_id, params.address.as_deref()).await?;

    // Holds first, then the balance, as a withdrawal reads them.
    let polygon_client = resolve_polygon_client_for_user(state.as_ref(), user_id).await?;
    let holds = {
        let mut conn = state.pool.acquire().await?;
        WithdrawalHolds::load(&mut conn, &source_address).await?
    };
    let cash = polygon_client
        .get_cash_balances(&source_address)
        .await
        .map_err(|e| {
            warn!(error = %chain_error_message(&e), "Failed to fetch source wallet balance");
            ApiError::ServiceUnavailable("Could not read the wallet's USDC balance".into())
        })?;
    let (capacity, source) = withdrawal_plan(&holds, &cash, params.amount);

    // Query orphan exposure from wallet_inventory.
    let orphan_exposure: Decimal = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(quantity * current_price), 0)::numeric
        FROM wallet_inventory
        WHERE is_orphan = true AND wallet_address = $1
        "#,
    )
    .bind(&source_address)
    .fetch_one(&state.pool)
    .await?;

    // Inventory freshness: when it was last fully scanned (a wallet that is
    // not the trading wallet has no inventory and is never fresh).
    let inventory_synced_at =
        crate::wallet_inventory::last_completed_scan(&state.pool, &source_address)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to read inventory status: {e}")))?;

    let inventory_fresh = inventory_synced_at
        .map(|synced_at| {
            let age = chrono::Utc::now() - synced_at;
            age.num_seconds() < 600 // < 10 minutes
        })
        .unwrap_or(false);

    let mut blocking_reasons = capacity.blocking_reasons.clone();
    if source.is_none() {
        blocking_reasons.push(exceeds_capacity(params.amount, &capacity));
    }
    let can_withdraw = blocking_reasons.is_empty();

    Ok(Json(WithdrawalPreflightResponse {
        available_usdc: capacity.available_usdc,
        in_flight_withdrawals: capacity.in_flight_withdrawals,
        reserved_for_open_buys: capacity.reserved_for_open_buys,
        orphan_exposure,
        max_withdrawable: capacity.max_withdrawable,
        inventory_fresh,
        inventory_synced_at,
        can_withdraw,
        blocking_reasons,
    }))
}
