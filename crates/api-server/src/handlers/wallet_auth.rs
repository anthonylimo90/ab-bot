//! Wallet authentication handlers using SIWE (Sign-In with Ethereum).

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::Extension;
use axum::Json;
use chrono::{Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use siwe::{Message, VerificationOpts};
use std::sync::Arc;
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::jwt::Claims;
use auth::{AuditAction, AuditEvent, UserRole};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Challenge request for wallet authentication.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChallengeRequest {
    /// Wallet address (0x prefixed, 42 characters).
    pub address: String,
}

/// Challenge response with SIWE message.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChallengeResponse {
    /// The SIWE message to sign.
    pub message: String,
    /// Nonce for the challenge.
    pub nonce: String,
    /// Challenge expiration timestamp.
    pub expires_at: String,
}

/// Verify signature request.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VerifyRequest {
    /// The signed SIWE message.
    pub message: String,
    /// The signature from the wallet.
    pub signature: String,
}

/// Verify response with auth token.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyResponse {
    /// JWT access token.
    pub token: String,
    /// User information.
    pub user: WalletUserInfo,
    /// Whether this is a new user.
    pub is_new_user: bool,
}

/// User information for wallet auth.
#[derive(Debug, Serialize, ToSchema)]
pub struct WalletUserInfo {
    /// User ID.
    pub id: String,
    /// Wallet address.
    pub wallet_address: String,
    /// Email (if linked).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// User role.
    pub role: String,
    /// Account creation timestamp.
    pub created_at: String,
}

/// Link wallet request (for existing users).
#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkWalletRequest {
    /// The signed SIWE message.
    pub message: String,
    /// The signature from the wallet.
    pub signature: String,
}

/// Link wallet response.
#[derive(Debug, Serialize, ToSchema)]
pub struct LinkWalletResponse {
    /// Success message.
    pub message: String,
    /// The linked wallet address.
    pub wallet_address: String,
}

/// Database row for user with wallet.
#[derive(Debug, sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    email: Option<String>,
    #[allow(dead_code)]
    wallet_address: Option<String>,
    role: i16,
    name: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

impl UserRow {
    fn role_string(&self) -> String {
        match self.role {
            0 => "Viewer".to_string(),
            1 => "Trader".to_string(),
            2 => "PlatformAdmin".to_string(),
            _ => "Viewer".to_string(),
        }
    }

    fn user_role(&self) -> UserRole {
        match self.role {
            0 => UserRole::Viewer,
            1 => UserRole::Trader,
            2 => UserRole::PlatformAdmin,
            _ => UserRole::Viewer,
        }
    }
}

/// Chain id (Polygon mainnet) that every sign-in message must be bound to.
const SIWE_CHAIN_ID: u64 = 137;

/// What a sign-in message must say to be accepted by this deployment: the site
/// that asked for the signature and the chain it is bound to. Without this a
/// message (and signature) produced for any other site, or for another chain,
/// would log in here as long as it carried a nonce we issued.
struct SiteBinding {
    /// `APP_DOMAIN`: the authority the message must name.
    domain: String,
    /// `APP_URI`: the URI the message must name.
    uri: String,
}

impl SiteBinding {
    fn from_env() -> Self {
        Self {
            domain: std::env::var("APP_DOMAIN").unwrap_or_else(|_| "localhost".to_string()),
            uri: std::env::var("APP_URI").unwrap_or_else(|_| "http://localhost:3002".to_string()),
        }
    }

    /// Reject messages that were not issued for this site and chain.
    fn ensure_bound(&self, message: &Message) -> ApiResult<()> {
        let domain_ok = message
            .domain
            .as_str()
            .eq_ignore_ascii_case(self.domain.trim());
        let uri_ok = normalize_uri(message.uri.as_str()) == normalize_uri(&self.uri);
        let chain_ok = message.chain_id == SIWE_CHAIN_ID;
        if domain_ok && uri_ok && chain_ok {
            return Ok(());
        }
        tracing::warn!(
            domain_ok,
            uri_ok,
            chain_ok,
            message_domain = %message.domain,
            message_uri = %message.uri,
            message_chain_id = message.chain_id,
            "Rejected SIWE message not issued for this site"
        );
        Err(ApiError::Unauthorized(
            "Sign-in message was not issued for this site".into(),
        ))
    }

    /// Verify the signature, the message's validity window and (again) its domain.
    async fn verify_signature(
        &self,
        message: &Message,
        signature: &[u8],
    ) -> Result<(), siwe::VerificationError> {
        let options = VerificationOpts {
            domain: self.domain.trim().parse().ok(),
            nonce: Some(message.nonce.clone()),
            timestamp: Some(OffsetDateTime::now_utc()),
        };
        message.verify(signature, &options).await
    }
}

/// Compare URIs ignoring scheme/host case and a trailing slash.
fn normalize_uri(uri: &str) -> String {
    let uri = uri.trim();
    match url::Url::parse(uri) {
        Ok(parsed) => parsed.as_str().trim_end_matches('/').to_string(),
        Err(_) => uri.trim_end_matches('/').to_string(),
    }
}

async fn find_wallet_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    address: &str,
) -> ApiResult<Option<UserRow>> {
    Ok(sqlx::query_as(
        r#"
        SELECT id, email, wallet_address, role, name, created_at
        FROM users
        WHERE wallet_address = $1
        "#,
    )
    .bind(address)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Atomically consume a challenge nonce issued for `address`.
///
/// One statement checks and marks it, so of any number of concurrent attempts
/// exactly one succeeds. Unknown, already used and expired nonces all yield 401.
async fn consume_challenge(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    address: &str,
    nonce: &str,
) -> ApiResult<()> {
    let consumed: Option<(Uuid,)> = sqlx::query_as(
        r#"
        UPDATE auth_challenges
        SET used_at = NOW()
        WHERE wallet_address = $1 AND nonce = $2
          AND used_at IS NULL AND expires_at > NOW()
        RETURNING id
        "#,
    )
    .bind(address)
    .bind(nonce)
    .fetch_optional(&mut **tx)
    .await?;
    match consumed {
        Some(_) => Ok(()),
        None => {
            tracing::warn!(address = %address, "SIWE challenge unknown, used or expired");
            Err(ApiError::Unauthorized(
                "Invalid or expired challenge".into(),
            ))
        }
    }
}

/// Generate a random nonce for SIWE.
fn generate_nonce() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}

/// Validate Ethereum address format.
fn validate_address(address: &str) -> bool {
    if !address.starts_with("0x") || address.len() != 42 {
        return false;
    }
    address[2..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Normalize address to lowercase (the form stored in the database).
fn normalize_address(address: &str) -> String {
    address.to_lowercase()
}

/// EIP-55 checksummed form of a validated address. SIWE requires it in the
/// message body: a lowercase address makes `Message::from_str` fail, so a
/// challenge containing one could never be verified.
fn eip55_address(address: &str) -> String {
    match hex::decode(address.trim_start_matches("0x"))
        .ok()
        .and_then(|bytes| <[u8; 20]>::try_from(bytes).ok())
    {
        Some(bytes) => siwe::eip55(&bytes),
        None => address.to_string(),
    }
}

/// Generate a SIWE challenge for wallet authentication.
#[utoipa::path(
    post,
    path = "/api/v1/auth/wallet/challenge",
    request_body = ChallengeRequest,
    responses(
        (status = 200, description = "Challenge generated", body = ChallengeResponse),
        (status = 400, description = "Invalid request"),
        (status = 429, description = "Too many requests"),
    ),
    tag = "auth"
)]
pub async fn challenge(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<ChallengeRequest>, JsonRejection>,
) -> ApiResult<Json<ChallengeResponse>> {
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Wallet challenge request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    // Validate address format
    if !validate_address(&req.address) {
        return Err(ApiError::BadRequest("Invalid wallet address format".into()));
    }

    let address = normalize_address(&req.address);
    let nonce = generate_nonce();
    let now = Utc::now();
    let expires_at = now + Duration::minutes(5);

    // Store the challenge in database
    sqlx::query(
        r#"
        INSERT INTO auth_challenges (wallet_address, nonce, issued_at, expires_at)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(&address)
    .bind(&nonce)
    .bind(now)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;

    // Cleanup old challenges for this address
    let _ = sqlx::query(
        r#"
        DELETE FROM auth_challenges
        WHERE wallet_address = $1 AND (expires_at < NOW() OR used_at IS NOT NULL)
        "#,
    )
    .bind(&address)
    .execute(&state.pool)
    .await;

    // Build SIWE message
    let site = SiteBinding::from_env();

    let message = format!(
        "{domain} wants you to sign in with your Ethereum account:\n\
        {address}\n\n\
        Sign in to AB-Bot Trading Platform\n\n\
        URI: {uri}\n\
        Version: 1\n\
        Chain ID: {chain_id}\n\
        Nonce: {nonce}\n\
        Issued At: {issued_at}\n\
        Expiration Time: {expires_at}",
        domain = site.domain,
        address = eip55_address(&address),
        uri = site.uri,
        chain_id = SIWE_CHAIN_ID,
        nonce = nonce,
        issued_at = now.to_rfc3339(),
        expires_at = expires_at.to_rfc3339(),
    );

    Ok(Json(ChallengeResponse {
        message,
        nonce,
        expires_at: expires_at.to_rfc3339(),
    }))
}

/// Verify a signed SIWE message and authenticate the user.
#[utoipa::path(
    post,
    path = "/api/v1/auth/wallet/verify",
    request_body = VerifyRequest,
    responses(
        (status = 200, description = "Signature verified, user authenticated", body = VerifyResponse),
        (status = 400, description = "Invalid request or signature"),
        (status = 401, description = "Signature verification failed"),
    ),
    tag = "auth"
)]
pub async fn verify(
    State(state): State<Arc<AppState>>,
    payload: Result<Json<VerifyRequest>, JsonRejection>,
) -> ApiResult<Json<VerifyResponse>> {
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Wallet verify request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    // Parse the SIWE message
    let message: Message = req.message.parse().map_err(|e| {
        tracing::warn!(error = ?e, "Failed to parse SIWE message");
        ApiError::BadRequest("Invalid SIWE message format".into())
    })?;

    let address = normalize_address(&format!("0x{}", hex::encode(message.address.as_ref())));

    let signature_bytes = hex::decode(req.signature.trim_start_matches("0x"))
        .map_err(|_| ApiError::BadRequest("Invalid signature format".into()))?;

    // The signed message must have been issued for this site, not merely carry a
    // valid signature and a nonce we handed out.
    let site = SiteBinding::from_env();
    site.ensure_bound(&message)?;

    // Consume the nonce before verifying, in one atomic statement: of any number
    // of concurrent replays exactly one gets past this point.
    let mut tx = state.pool.begin().await?;
    consume_challenge(&mut tx, &address, &message.nonce).await?;

    if let Err(e) = site.verify_signature(&message, &signature_bytes).await {
        tracing::warn!(address = %address, error = ?e, "Signature verification failed");
        // A failed attempt still burns the nonce.
        tx.commit().await?;
        return Err(ApiError::Unauthorized(
            "Signature verification failed".into(),
        ));
    }

    // Find or create the user in the same transaction as the nonce consumption: if
    // creating the account fails, the nonce is not burned and the sign-in can be
    // retried.
    let existing_user: Option<UserRow> = find_wallet_user(&mut tx, &address).await?;
    let (user, is_new_user) = match existing_user {
        Some(u) => (u, false),
        None => {
            // Wallet-only account: no email or password (users.email and
            // users.password_hash are nullable for wallet accounts).
            let user_id = Uuid::new_v4();
            let role: i16 = 1; // Trader by default
            let now = Utc::now();

            // A concurrent first sign-in for the same wallet (with a different
            // nonce) may win the insert; then we simply sign in as that user.
            let inserted = sqlx::query(
                r#"
                INSERT INTO users (id, wallet_address, wallet_linked_at, role, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $5)
                ON CONFLICT (wallet_address) DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(&address)
            .bind(now)
            .bind(role)
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;

            let user = find_wallet_user(&mut tx, &address)
                .await?
                .ok_or_else(|| ApiError::Internal("Wallet user missing after insert".into()))?;
            (user, inserted)
        }
    };

    sqlx::query("UPDATE users SET last_login = $1 WHERE id = $2")
        .bind(Utc::now())
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    if is_new_user {
        let audit_event =
            AuditEvent::builder(AuditAction::UserCreated, format!("user/{}", user.id))
                .user(user.id.to_string())
                .details(serde_json::json!({
                    "wallet_address": &address,
                    "role": "Trader",
                    "source": "wallet_auth"
                }))
                .build();
        state.audit_logger.log(audit_event);
    }

    // Log successful login
    state
        .audit_logger
        .log_login(&user.id.to_string(), None, true);

    // Generate JWT token with wallet address
    let role = user.role_string();
    let user_role = user.user_role();

    let claims = crate::middleware::session_claims(&state.pool, user.id, user_role)
        .await?
        .with_wallet_address(&address);
    let claims = if let Some(ref email) = user.email {
        claims.with_email(email)
    } else {
        claims
    };

    let token = state
        .jwt_auth
        .create_token_with_claims(&claims)
        .map_err(|e| ApiError::Internal(format!("Token generation failed: {}", e)))?;

    let response = VerifyResponse {
        token,
        user: WalletUserInfo {
            id: user.id.to_string(),
            wallet_address: address,
            email: user.email,
            name: user.name,
            role,
            created_at: user.created_at.to_rfc3339(),
        },
        is_new_user,
    };

    Ok(Json(response))
}

/// Link a wallet to an existing authenticated user.
#[utoipa::path(
    post,
    path = "/api/v1/auth/wallet/link",
    request_body = LinkWalletRequest,
    responses(
        (status = 200, description = "Wallet linked successfully", body = LinkWalletResponse),
        (status = 400, description = "Invalid request or wallet already linked"),
        (status = 401, description = "Unauthorized or signature verification failed"),
        (status = 409, description = "Wallet already linked to another account"),
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "auth"
)]
pub async fn link_wallet(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    payload: Result<Json<LinkWalletRequest>, JsonRejection>,
) -> ApiResult<Json<LinkWalletResponse>> {
    let Json(req) = payload.map_err(|e| {
        tracing::warn!(error = %e, "Link wallet request JSON parsing failed");
        ApiError::BadRequest(format!("Invalid request body: {}", e.body_text()))
    })?;

    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Internal("Invalid user ID in token".into()))?;

    // Check if user already has a wallet linked
    let existing_wallet: Option<(Option<String>,)> =
        sqlx::query_as("SELECT wallet_address FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    if let Some((Some(_),)) = existing_wallet {
        return Err(ApiError::BadRequest(
            "A wallet is already linked to this account".into(),
        ));
    }

    // Parse and verify the SIWE message
    let message: Message = req.message.parse().map_err(|e| {
        tracing::warn!(error = ?e, "Failed to parse SIWE message");
        ApiError::BadRequest("Invalid SIWE message format".into())
    })?;

    let address = normalize_address(&format!("0x{}", hex::encode(message.address.as_ref())));

    let signature_bytes = hex::decode(req.signature.trim_start_matches("0x"))
        .map_err(|_| ApiError::BadRequest("Invalid signature format".into()))?;

    // Same site binding and atomic nonce consumption as `verify`.
    let site = SiteBinding::from_env();
    site.ensure_bound(&message)?;

    let mut tx = state.pool.begin().await?;
    consume_challenge(&mut tx, &address, &message.nonce).await?;

    if site
        .verify_signature(&message, &signature_bytes)
        .await
        .is_err()
    {
        // A failed attempt still burns the nonce.
        tx.commit().await?;
        return Err(ApiError::Unauthorized(
            "Signature verification failed".into(),
        ));
    }

    // Check if wallet is already linked to another user
    let wallet_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE wallet_address = $1 AND id != $2")
            .bind(&address)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;

    if wallet_exists.is_some() {
        return Err(ApiError::Conflict(
            "This wallet is already linked to another account".into(),
        ));
    }

    // Link the wallet in the same transaction as the nonce consumption, and only
    // while the account has none (a concurrent link must not overwrite it).
    let linked = sqlx::query(
        "UPDATE users SET wallet_address = $1, wallet_linked_at = $2 WHERE id = $3 AND wallet_address IS NULL",
    )
    .bind(&address)
    .bind(Utc::now())
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    if linked.rows_affected() != 1 {
        return Err(ApiError::BadRequest(
            "A wallet is already linked to this account".into(),
        ));
    }
    tx.commit().await?;

    // Log wallet linking
    let audit_event = AuditEvent::builder(
        AuditAction::Custom("wallet_linked".to_string()),
        format!("user/{}", user_id),
    )
    .user(user_id.to_string())
    .details(serde_json::json!({
        "wallet_address": &address,
    }))
    .build();
    state.audit_logger.log(audit_event);

    Ok(Json(LinkWalletResponse {
        message: "Wallet linked successfully".to_string(),
        wallet_address: address,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_address() {
        assert!(validate_address(
            "0x742d35Cc6634C0532925a3b844Bc9e7595f8bC7b"
        ));
        assert!(validate_address(
            "0x742d35cc6634c0532925a3b844bc9e7595f8bc7b"
        ));
        assert!(!validate_address(
            "742d35Cc6634C0532925a3b844Bc9e7595f8bC7b"
        )); // No 0x
        assert!(!validate_address("0x742d35Cc6634C0532925a3b844Bc9e759")); // Too short
        assert!(!validate_address(
            "0x742d35Cc6634C0532925a3b844Bc9e7595f8bC7bXX"
        )); // Too long
        assert!(!validate_address(
            "0x742d35Cc6634C0532925a3b844Bc9e7595f8bCZZ"
        )); // Invalid chars
    }

    #[test]
    fn test_normalize_address() {
        assert_eq!(
            normalize_address("0x742D35Cc6634C0532925a3b844Bc9e7595f8bC7B"),
            "0x742d35cc6634c0532925a3b844bc9e7595f8bc7b"
        );
    }

    #[test]
    fn test_generate_nonce() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_eq!(nonce1.len(), 32); // 16 bytes = 32 hex chars
        assert_ne!(nonce1, nonce2);
    }
}

#[cfg(test)]
#[path = "wallet_auth/siwe_tests.rs"]
mod siwe_tests;
