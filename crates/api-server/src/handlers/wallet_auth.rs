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

/// Normalize address to lowercase with checksum.
fn normalize_address(address: &str) -> String {
    address.to_lowercase()
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
    let domain = std::env::var("APP_DOMAIN").unwrap_or_else(|_| "localhost".to_string());
    let uri = std::env::var("APP_URI").unwrap_or_else(|_| "http://localhost:3002".to_string());

    let message = format!(
        "{domain} wants you to sign in with your Ethereum account:\n\
        {address}\n\n\
        Sign in to AB-Bot Trading Platform\n\n\
        URI: {uri}\n\
        Version: 1\n\
        Chain ID: 137\n\
        Nonce: {nonce}\n\
        Issued At: {issued_at}\n\
        Expiration Time: {expires_at}",
        domain = domain,
        address = address,
        uri = uri,
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
    let nonce = message.nonce.clone();

    // Check that the nonce exists and is not expired/used
    #[derive(sqlx::FromRow)]
    struct ChallengeRow {
        id: Uuid,
        expires_at: chrono::DateTime<Utc>,
        used_at: Option<chrono::DateTime<Utc>>,
    }

    let challenge: Option<ChallengeRow> = sqlx::query_as(
        r#"
        SELECT id, expires_at, used_at
        FROM auth_challenges
        WHERE wallet_address = $1 AND nonce = $2
        "#,
    )
    .bind(&address)
    .bind(&nonce)
    .fetch_optional(&state.pool)
    .await?;

    let challenge = match challenge {
        Some(c) => c,
        None => {
            tracing::warn!(address = %address, "No matching challenge found");
            return Err(ApiError::Unauthorized(
                "Invalid or expired challenge".into(),
            ));
        }
    };

    if challenge.used_at.is_some() {
        tracing::warn!(address = %address, "Challenge already used");
        return Err(ApiError::Unauthorized("Challenge already used".into()));
    }

    if challenge.expires_at < Utc::now() {
        tracing::warn!(address = %address, "Challenge expired");
        return Err(ApiError::Unauthorized("Challenge expired".into()));
    }

    // Verify the signature
    let signature_bytes = hex::decode(req.signature.trim_start_matches("0x"))
        .map_err(|_| ApiError::BadRequest("Invalid signature format".into()))?;

    let verification_opts = VerificationOpts {
        domain: None, // Skip domain check (already validated in message)
        nonce: Some(nonce.clone()),
        timestamp: Some(OffsetDateTime::now_utc()),
    };

    message
        .verify(&signature_bytes, &verification_opts)
        .await
        .map_err(|e| {
            tracing::warn!(address = %address, error = ?e, "Signature verification failed");
            ApiError::Unauthorized("Signature verification failed".into())
        })?;

    // Mark challenge as used
    sqlx::query("UPDATE auth_challenges SET used_at = $1 WHERE id = $2")
        .bind(Utc::now())
        .bind(challenge.id)
        .execute(&state.pool)
        .await?;

    // Find or create user
    let existing_user: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT id, email, wallet_address, role, name, created_at
        FROM users
        WHERE wallet_address = $1
        "#,
    )
    .bind(&address)
    .fetch_optional(&state.pool)
    .await?;

    let (user, is_new_user) = match existing_user {
        Some(u) => (u, false),
        None => {
            // Create new user with wallet
            let user_id = Uuid::new_v4();
            let role: i16 = 1; // Trader by default
            let now = Utc::now();

            sqlx::query(
                r#"
                INSERT INTO users (id, wallet_address, wallet_linked_at, role, created_at, updated_at)
                VALUES ($1, $2, $3, $4, $5, $5)
                "#,
            )
            .bind(user_id)
            .bind(&address)
            .bind(now)
            .bind(role)
            .bind(now)
            .execute(&state.pool)
            .await?;

            // Log user creation
            let audit_event =
                AuditEvent::builder(AuditAction::UserCreated, format!("user/{}", user_id))
                    .user(user_id.to_string())
                    .details(serde_json::json!({
                        "wallet_address": &address,
                        "role": "Trader",
                        "source": "wallet_auth"
                    }))
                    .build();
            state.audit_logger.log(audit_event);

            (
                UserRow {
                    id: user_id,
                    email: None,
                    wallet_address: Some(address.clone()),
                    role,
                    name: None,
                    created_at: now,
                },
                true,
            )
        }
    };

    // Update last login
    let _ = sqlx::query("UPDATE users SET last_login = $1 WHERE id = $2")
        .bind(Utc::now())
        .bind(user.id)
        .execute(&state.pool)
        .await;

    // Log successful login
    state
        .audit_logger
        .log_login(&user.id.to_string(), None, true);

    // Generate JWT token with wallet address
    let role = user.role_string();
    let user_role = user.user_role();

    let claims = Claims::new(user.id.to_string(), user_role, 24).with_wallet_address(&address);
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
    let nonce = message.nonce.clone();

    // Check that the nonce exists and is not expired/used
    #[derive(sqlx::FromRow)]
    struct ChallengeRow {
        id: Uuid,
        expires_at: chrono::DateTime<Utc>,
        used_at: Option<chrono::DateTime<Utc>>,
    }

    let challenge: Option<ChallengeRow> = sqlx::query_as(
        r#"
        SELECT id, expires_at, used_at
        FROM auth_challenges
        WHERE wallet_address = $1 AND nonce = $2
        "#,
    )
    .bind(&address)
    .bind(&nonce)
    .fetch_optional(&state.pool)
    .await?;

    let challenge = match challenge {
        Some(c) => c,
        None => {
            return Err(ApiError::Unauthorized(
                "Invalid or expired challenge".into(),
            ));
        }
    };

    if challenge.used_at.is_some() {
        return Err(ApiError::Unauthorized("Challenge already used".into()));
    }

    if challenge.expires_at < Utc::now() {
        return Err(ApiError::Unauthorized("Challenge expired".into()));
    }

    // Verify the signature
    let signature_bytes = hex::decode(req.signature.trim_start_matches("0x"))
        .map_err(|_| ApiError::BadRequest("Invalid signature format".into()))?;

    let verification_opts = VerificationOpts {
        domain: None,
        nonce: Some(nonce),
        timestamp: Some(OffsetDateTime::now_utc()),
    };

    message
        .verify(&signature_bytes, &verification_opts)
        .await
        .map_err(|_| ApiError::Unauthorized("Signature verification failed".into()))?;

    // Mark challenge as used
    sqlx::query("UPDATE auth_challenges SET used_at = $1 WHERE id = $2")
        .bind(Utc::now())
        .bind(challenge.id)
        .execute(&state.pool)
        .await?;

    // Check if wallet is already linked to another user
    let wallet_exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE wallet_address = $1 AND id != $2")
            .bind(&address)
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;

    if wallet_exists.is_some() {
        return Err(ApiError::Conflict(
            "This wallet is already linked to another account".into(),
        ));
    }

    // Link wallet to user
    sqlx::query("UPDATE users SET wallet_address = $1, wallet_linked_at = $2 WHERE id = $3")
        .bind(&address)
        .bind(Utc::now())
        .bind(user_id)
        .execute(&state.pool)
        .await?;

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
