//! Order signing handlers for MetaMask/wallet-based trade execution.
//!
//! This module provides endpoints for preparing EIP-712 typed data
//! that users can sign with their wallet, and then submitting
//! the signed orders to the Polymarket CLOB.

use alloy_primitives::{keccak256, Address, B256};
use alloy_sol_types::SolValue;
use axum::extract::State;
use axum::Extension;
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;
use uuid::Uuid;

use auth::Claims;
use polymarket_core::signing::{
    SignedOrder, CTF_EXCHANGE_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS, POLYGON_CHAIN_ID,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// EIP-712 domain data for the typed data structure.
#[derive(Debug, Serialize, ToSchema)]
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
    #[serde(rename = "chainId")]
    pub chain_id: u64,
    #[serde(rename = "verifyingContract")]
    pub verifying_contract: String,
}

/// EIP-712 Order type for Polymarket CTF Exchange.
#[derive(Debug, Serialize, ToSchema)]
pub struct Eip712Order {
    pub salt: String,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    #[serde(rename = "feeRateBps")]
    pub fee_rate_bps: String,
    pub side: u8,
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
}

/// Full EIP-712 typed data structure for signing.
#[derive(Debug, Serialize, ToSchema)]
pub struct Eip712TypedData {
    pub types: Eip712Types,
    #[serde(rename = "primaryType")]
    pub primary_type: String,
    pub domain: Eip712Domain,
    pub message: Eip712Order,
}

/// EIP-712 type definitions.
#[derive(Debug, Serialize, ToSchema)]
pub struct Eip712Types {
    #[serde(rename = "EIP712Domain")]
    pub eip712_domain: Vec<TypeDefinition>,
    #[serde(rename = "Order")]
    pub order: Vec<TypeDefinition>,
}

/// Single type definition field.
#[derive(Debug, Serialize, ToSchema)]
pub struct TypeDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

/// Request to prepare an order for signing.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PrepareOrderRequest {
    /// Market condition/token ID to trade.
    pub token_id: String,
    /// Order side: "BUY" or "SELL".
    pub side: String,
    /// Price (0.0 to 1.0 probability).
    pub price: Decimal,
    /// Number of outcome shares (both BUY and SELL).
    pub size: Decimal,
    /// Maker/signer wallet address.
    pub maker_address: String,
    /// Whether this is a neg-risk market (uses different exchange contract).
    #[serde(default)]
    pub neg_risk: bool,
    /// Optional expiration in seconds from now (defaults to 1 hour).
    pub expires_in_secs: Option<u64>,
}

/// Response with prepared order and typed data for signing.
#[derive(Debug, Serialize, ToSchema)]
pub struct PrepareOrderResponse {
    /// Unique ID for this pending order.
    pub pending_order_id: String,
    /// Full EIP-712 typed data structure for signTypedData_v4.
    pub typed_data: Eip712TypedData,
    /// Order expiration timestamp.
    pub expires_at: DateTime<Utc>,
    /// Human-readable order summary.
    pub summary: OrderSummary,
}

/// Human-readable order summary.
#[derive(Debug, Serialize, ToSchema)]
pub struct OrderSummary {
    pub side: String,
    pub outcome: String,
    pub price: String,
    pub size: String,
    pub total_cost: String,
    pub potential_payout: String,
}

/// Request to submit a signed order.
#[derive(Debug, Deserialize, ToSchema)]
pub struct WalletAuthProof {
    pub timestamp: u64,
    pub nonce: u64,
    pub signature: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SubmitOrderRequest {
    /// Fresh wallet-signed ClobAuth proof for this order's maker.
    pub auth: WalletAuthProof,
    /// The pending order ID from prepare_order.
    pub pending_order_id: String,
    /// The EIP-712 signature from MetaMask.
    pub signature: String,
}

/// Response after submitting a signed order.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubmitOrderResponse {
    /// Whether the order was successfully submitted.
    pub success: bool,
    /// Order ID from the CLOB (if successful).
    pub order_id: Option<String>,
    /// Status message.
    pub message: String,
    /// Transaction hash (if available).
    pub tx_hash: Option<String>,
}

/// Database row for pending orders.
#[derive(Debug, sqlx::FromRow)]
struct PendingOrderRow {
    #[allow(dead_code)]
    id: Uuid,
    #[allow(dead_code)]
    user_id: Uuid,
    maker_address: String,
    token_id: String,
    side: i16,
    maker_amount: String,
    taker_amount: String,
    salt: String,
    expiration: i64,
    nonce: String,
    fee_rate_bps: i32,
    signature_type: i16,
    neg_risk: bool,
    expires_at: DateTime<Utc>,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
}

/// Which outcome `token_id` is, under the catalog's YES/NO mapping, for the
/// summary a user confirms before signing; "UNKNOWN" rather than a guess
/// when the token is not in the catalog.
async fn outcome_label(state: &AppState, token_id: &str) -> &'static str {
    let condition: Option<String> =
        sqlx::query_scalar("SELECT condition_id FROM token_condition_cache WHERE token_id = $1")
            .bind(token_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
    let Some(condition) = condition else {
        return "UNKNOWN";
    };
    match state.market_catalog.get(&condition).await {
        Some((yes, _)) if yes == token_id => "YES",
        Some((_, no)) if no == token_id => "NO",
        _ => "UNKNOWN",
    }
}

/// Generate a random salt for order uniqueness.
fn generate_salt() -> String {
    ((Uuid::new_v4().as_u128() as u64) & ((1u64 << 53) - 1)).to_string()
}

/// Calculate maker and taker amounts from price and size.
fn calculate_amounts(side: &str, price: Decimal, shares: Decimal) -> (String, String) {
    let scale = Decimal::from(1_000_000u64);
    let tokens = (shares * scale).trunc().normalize().to_string();
    let usdc = (shares * price * scale).trunc().normalize().to_string();
    match side.to_ascii_uppercase().as_str() {
        "BUY" => (usdc, tokens),
        "SELL" => (tokens, usdc),
        _ => ("0".into(), "0".into()),
    }
}

/// Build EIP-712 type definitions.
fn build_eip712_types() -> Eip712Types {
    Eip712Types {
        eip712_domain: vec![
            TypeDefinition {
                name: "name".to_string(),
                type_name: "string".to_string(),
            },
            TypeDefinition {
                name: "version".to_string(),
                type_name: "string".to_string(),
            },
            TypeDefinition {
                name: "chainId".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "verifyingContract".to_string(),
                type_name: "address".to_string(),
            },
        ],
        order: vec![
            TypeDefinition {
                name: "salt".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "maker".to_string(),
                type_name: "address".to_string(),
            },
            TypeDefinition {
                name: "signer".to_string(),
                type_name: "address".to_string(),
            },
            TypeDefinition {
                name: "taker".to_string(),
                type_name: "address".to_string(),
            },
            TypeDefinition {
                name: "tokenId".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "makerAmount".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "takerAmount".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "expiration".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "nonce".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "feeRateBps".to_string(),
                type_name: "uint256".to_string(),
            },
            TypeDefinition {
                name: "side".to_string(),
                type_name: "uint8".to_string(),
            },
            TypeDefinition {
                name: "signatureType".to_string(),
                type_name: "uint8".to_string(),
            },
        ],
    }
}

/// Prepare an order for signing with MetaMask.
///
/// Returns EIP-712 typed data that can be signed with signTypedData_v4.
#[utoipa::path(
    post,
    path = "/api/v1/orders/prepare",
    request_body = PrepareOrderRequest,
    responses(
        (status = 200, description = "Order prepared for signing", body = PrepareOrderResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "order_signing"
)]
pub async fn prepare_order(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PrepareOrderRequest>,
) -> ApiResult<Json<PrepareOrderResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    // Validate side
    let side_str = req.side.to_uppercase();
    if side_str != "BUY" && side_str != "SELL" {
        return Err(ApiError::BadRequest("Side must be 'BUY' or 'SELL'".into()));
    }
    let side: u8 = if side_str == "BUY" { 0 } else { 1 };

    // Validate price
    if req.price <= Decimal::ZERO || req.price >= Decimal::ONE {
        return Err(ApiError::BadRequest(
            "Price must be between 0 and 1 (exclusive)".into(),
        ));
    }

    // Validate size
    if req.size <= Decimal::ZERO
        || req.size > Decimal::from(1_000_000)
        || req.size.normalize().scale() > 2
    {
        return Err(ApiError::BadRequest(
            "Share size must be positive, at most 1,000,000, with at most two decimals".into(),
        ));
    }

    // Validate maker address
    req.maker_address
        .parse::<Address>()
        .map_err(|_| ApiError::BadRequest("Invalid maker address".into()))?;
    req.token_id
        .parse::<alloy_primitives::U256>()
        .map_err(|_| ApiError::BadRequest("Invalid token ID".into()))?;
    let (neg_risk, fee_rate) = state
        .clob_client
        .get_order_metadata(&req.token_id)
        .await
        .map_err(|_| {
            ApiError::ServiceUnavailable("Exchange contract/fee metadata unavailable".into())
        })?;

    // Calculate expiration
    let expires_in_secs = req.expires_in_secs.unwrap_or(3600); // Default 1 hour
    if !(60..=86400).contains(&expires_in_secs) {
        return Err(ApiError::BadRequest(
            "Order expiry must be between 60 seconds and one day".into(),
        ));
    }
    let now = Utc::now();
    let expires_at = now + Duration::seconds(expires_in_secs as i64);
    let expiration_timestamp = expires_at.timestamp() as u64;

    // Generate salt and calculate amounts
    let salt = generate_salt();
    let (maker_amount, taker_amount) = calculate_amounts(&side_str, req.price, req.size);

    // Create pending order ID
    let pending_order_id = Uuid::new_v4();

    // Store pending order in database
    sqlx::query(
        r#"
        INSERT INTO pending_wallet_orders (
            id, user_id, maker_address, token_id, side,
            maker_amount, taker_amount, salt, expiration, nonce,
            fee_rate_bps, signature_type, neg_risk, expires_at, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        "#,
    )
    .bind(pending_order_id)
    .bind(user_id)
    .bind(&req.maker_address)
    .bind(&req.token_id)
    .bind(side as i16)
    .bind(&maker_amount)
    .bind(&taker_amount)
    .bind(&salt)
    .bind(expiration_timestamp as i64)
    .bind("0") // nonce
    .bind(
        i32::try_from(fee_rate)
            .map_err(|_| ApiError::ServiceUnavailable("Invalid fee rate".into()))?,
    ) // fee_rate_bps
    .bind(0i16) // signature_type (EOA)
    .bind(neg_risk)
    .bind(expires_at)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Build EIP-712 typed data
    let verifying_contract = if neg_risk {
        NEG_RISK_CTF_EXCHANGE_ADDRESS
    } else {
        CTF_EXCHANGE_ADDRESS
    };

    let typed_data = Eip712TypedData {
        types: build_eip712_types(),
        primary_type: "Order".to_string(),
        domain: Eip712Domain {
            name: "Polymarket CTF Exchange".to_string(),
            version: "1".to_string(),
            chain_id: POLYGON_CHAIN_ID,
            verifying_contract: verifying_contract.to_string(),
        },
        message: Eip712Order {
            salt: salt.clone(),
            maker: req.maker_address.clone(),
            signer: req.maker_address.clone(),
            taker: "0x0000000000000000000000000000000000000000".to_string(),
            token_id: req.token_id.clone(),
            maker_amount: maker_amount.clone(),
            taker_amount: taker_amount.clone(),
            expiration: expiration_timestamp.to_string(),
            nonce: "0".to_string(),
            fee_rate_bps: fee_rate.to_string(),
            side,
            signature_type: 0, // EOA
        },
    };

    // Build human-readable summary
    let potential_payout = if side_str == "BUY" {
        req.size
    } else {
        req.size * req.price
    };

    let summary = OrderSummary {
        side: side_str.clone(),
        outcome: outcome_label(&state, &req.token_id).await.to_string(),
        price: format!("${:.2}", req.price),
        size: format!("{} shares", req.size),
        total_cost: format!("${:.2}", req.size * req.price),
        potential_payout: format!("${:.2}", potential_payout),
    };

    Ok(Json(PrepareOrderResponse {
        pending_order_id: pending_order_id.to_string(),
        typed_data,
        expires_at,
        summary,
    }))
}

/// Submit a signed order to the Polymarket CLOB.
#[utoipa::path(
    post,
    path = "/api/v1/orders/submit",
    request_body = SubmitOrderRequest,
    responses(
        (status = 200, description = "Order submitted", body = SubmitOrderResponse),
        (status = 400, description = "Invalid request or signature"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Pending order not found"),
        (status = 410, description = "Order expired"),
    ),
    security(("bearer_auth" = [])),
    tag = "order_signing"
)]
pub async fn submit_order(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SubmitOrderRequest>,
) -> ApiResult<Json<SubmitOrderResponse>> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Internal("Invalid user ID".into()))?;

    let pending_order_id = Uuid::parse_str(&req.pending_order_id)
        .map_err(|_| ApiError::BadRequest("Invalid pending order ID".into()))?;

    // Fetch pending order
    let pending_order: Option<PendingOrderRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, maker_address, token_id, side,
               maker_amount, taker_amount, salt, expiration, nonce,
               fee_rate_bps, signature_type, neg_risk, expires_at, created_at
        FROM pending_wallet_orders
        WHERE id = $1 AND user_id = $2
        "#,
    )
    .bind(pending_order_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;

    let pending_order = pending_order.ok_or_else(|| {
        ApiError::NotFound("Pending order not found or does not belong to you".into())
    })?;

    let previous: Option<(String, Option<serde_json::Value>)> =
        sqlx::query_as("SELECT status,response FROM execution_intents WHERE id=$1")
            .bind(pending_order_id)
            .fetch_optional(&state.pool)
            .await?;
    if let Some((status, response)) = previous {
        if status == "acknowledged" {
            if let Some(response) = response {
                let response: polymarket_core::api::clob::PostOrderResponse =
                    serde_json::from_value(response)?;
                return Ok(Json(wallet_submission_response(response)));
            }
        }
        return Err(ApiError::Conflict(
            "This order was already submitted; reconcile its outcome before placing another order"
                .into(),
        ));
    }

    // Check if order has expired
    if Utc::now() > pending_order.expires_at {
        // Delete expired order
        sqlx::query("DELETE FROM pending_wallet_orders WHERE id = $1")
            .bind(pending_order_id)
            .execute(&state.pool)
            .await?;

        return Err(ApiError::Gone("Order has expired".into()));
    }

    // Validate signature format
    if !req.signature.starts_with("0x") || req.signature.len() != 132 {
        return Err(ApiError::BadRequest(
            "Invalid signature format (expected 0x + 130 hex chars)".into(),
        ));
    }

    // Verify signature matches the maker address
    let recovered_address =
        verify_order_signature(&pending_order, &req.signature).map_err(|e| {
            tracing::warn!(
                maker = %pending_order.maker_address,
                error = %e,
                "Signature verification failed"
            );
            ApiError::BadRequest(format!("Signature verification failed: {}", e))
        })?;

    // Ensure recovered address matches maker
    let maker_addr = pending_order.maker_address.to_lowercase();
    let recovered_addr = format!("{:?}", recovered_address).to_lowercase();
    if maker_addr != recovered_addr {
        tracing::warn!(
            maker = %maker_addr,
            recovered = %recovered_addr,
            "Signature signer mismatch"
        );
        return Err(ApiError::BadRequest(
            "Signature does not match maker address".into(),
        ));
    }

    // Build the signed order for CLOB submission
    let side_str = if pending_order.side == 0 {
        "BUY"
    } else {
        "SELL"
    };

    let signed_order = SignedOrder {
        order_hash: Some(format!(
            "{:#x}",
            order_digest(&pending_order).map_err(ApiError::BadRequest)?
        )),
        salt: pending_order.salt.parse::<u64>().unwrap_or(0),
        maker: pending_order.maker_address.clone(),
        signer: pending_order.maker_address.clone(),
        taker: "0x0000000000000000000000000000000000000000".to_string(),
        token_id: pending_order.token_id.clone(),
        maker_amount: pending_order.maker_amount.clone(),
        taker_amount: pending_order.taker_amount.clone(),
        expiration: pending_order.expiration.to_string(),
        nonce: pending_order.nonce.clone(),
        fee_rate_bps: pending_order.fee_rate_bps.to_string(),
        side: side_str.to_string(),
        signature_type: pending_order.signature_type as u8,
        signature: req.signature.clone(),
    };

    verify_wallet_auth(
        &pending_order.maker_address,
        &req.auth,
        Utc::now().timestamp(),
    )?;
    let credentials = state
        .clob_client
        .derive_external_api_key(
            &pending_order.maker_address,
            req.auth.timestamp,
            req.auth.nonce,
            &req.auth.signature,
        )
        .await
        .map_err(|_| {
            ApiError::ServiceUnavailable("Wallet-specific exchange authentication failed".into())
        })?;
    let Some(operation) = state.order_executor.begin_operation() else {
        return Err(ApiError::ServiceUnavailable(
            crate::handlers::trading::SHUTTING_DOWN.into(),
        ));
    };
    // Runs to the end even if the client goes away, so the exchange's answer
    // is recorded rather than dropped mid-request.
    let submit_state = state.clone();
    let response = tokio::spawn(async move {
        let _operation = operation;
        submit_state
            .order_executor
            .submit_wallet_order(
                &submit_state.clob_client,
                pending_order_id,
                signed_order,
                &credentials,
            )
            .await
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Order task failed: {e}")))?
    .map_err(|_| {
        ApiError::ServiceUnavailable(
            "Order submission requires reconciliation; keep this pending order ID for retries"
                .into(),
        )
    })?;
    Ok(Json(wallet_submission_response(response)))
}

fn wallet_submission_response(
    response: polymarket_core::api::clob::PostOrderResponse,
) -> SubmitOrderResponse {
    SubmitOrderResponse {
        success: !response.is_unfilled(),
        order_id: Some(response.order_id),
        message: "Exchange response recorded; check order status for confirmed fills".into(),
        tx_hash: response.transaction_hash,
    }
}

fn verify_wallet_auth(maker: &str, proof: &WalletAuthProof, now: i64) -> ApiResult<()> {
    let timestamp = i64::try_from(proof.timestamp)
        .map_err(|_| ApiError::BadRequest("Invalid authentication timestamp".into()))?;
    if timestamp < now - 300 || timestamp > now + 30 || proof.nonce != 0 {
        return Err(ApiError::BadRequest(
            "Wallet authentication proof is stale or has an unsupported nonce".into(),
        ));
    }
    let maker = maker
        .parse::<Address>()
        .map_err(|_| ApiError::BadRequest("Invalid maker".into()))?;
    let digest =
        polymarket_core::signing::signer::clob_auth_digest(maker, proof.timestamp, proof.nonce);
    let recovered = recover_signature(&digest, &proof.signature).map_err(ApiError::BadRequest)?;
    if recovered != maker {
        return Err(ApiError::Forbidden(
            "Authentication proof does not match the order maker".into(),
        ));
    }
    Ok(())
}

// ============================================================================
// Helper Functions
// ============================================================================

fn recover_signature(digest: &B256, signature: &str) -> Result<Address, String> {
    let bytes =
        hex::decode(signature.trim_start_matches("0x")).map_err(|_| "Invalid signature hex")?;
    if bytes.len() != 65 {
        return Err("Signature must be 65 bytes".into());
    }
    let r: [u8; 32] = bytes[..32].try_into().unwrap();
    let s: [u8; 32] = bytes[32..64].try_into().unwrap();
    recover_address(digest, &r, &s, bytes[64])
}

fn order_digest(pending: &PendingOrderRow) -> Result<B256, String> {
    use alloy_primitives::U256;
    let number = |value: &str| {
        U256::from_str_radix(value, 10).map_err(|_| "Invalid order integer".to_string())
    };
    let maker = pending
        .maker_address
        .parse::<Address>()
        .map_err(|_| "Invalid maker")?;
    let order = polymarket_core::signing::OrderData {
        salt: number(&pending.salt)?,
        maker,
        signer: maker,
        taker: Address::ZERO,
        token_id: number(&pending.token_id)?,
        maker_amount: number(&pending.maker_amount)?,
        taker_amount: number(&pending.taker_amount)?,
        expiration: U256::from(pending.expiration as u64),
        nonce: number(&pending.nonce)?,
        fee_rate_bps: U256::from(pending.fee_rate_bps as u64),
        side: pending.side as u8,
        signature_type: pending.signature_type as u8,
    };
    let domain = if pending.neg_risk {
        polymarket_core::signing::Eip712Domain::neg_risk_ctf_exchange()
    } else {
        polymarket_core::signing::Eip712Domain::ctf_exchange()
    };
    Ok(keccak256(
        ([0x19u8, 0x01u8], domain.separator(), order.struct_hash()).abi_encode_packed(),
    ))
}

fn verify_order_signature(pending: &PendingOrderRow, signature: &str) -> Result<Address, String> {
    recover_signature(&order_digest(pending)?, signature)
}

/// Recover address from signature using secp256k1.
fn recover_address(digest: &B256, r: &[u8; 32], s: &[u8; 32], v: u8) -> Result<Address, String> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};

    // Normalize v value (handle both legacy 27/28 and EIP-155 formats)
    let recovery_id_val = match v {
        0 | 27 => 0,
        1 | 28 => 1,
        _ => return Err(format!("Invalid recovery id: {}", v)),
    };

    let recovery_id =
        RecoveryId::try_from(recovery_id_val).map_err(|e| format!("Invalid recovery id: {}", e))?;

    // Create signature from r and s
    let mut sig_bytes = [0u8; 64];
    sig_bytes[..32].copy_from_slice(r);
    sig_bytes[32..].copy_from_slice(s);

    let signature =
        Signature::from_slice(&sig_bytes).map_err(|e| format!("Invalid signature: {}", e))?;

    // Recover the verifying key
    let verifying_key =
        VerifyingKey::recover_from_prehash(digest.as_slice(), &signature, recovery_id)
            .map_err(|e| format!("Failed to recover key: {}", e))?;

    // Convert to address (keccak256 of public key, take last 20 bytes)
    let public_key_bytes = verifying_key.to_encoded_point(false);
    let public_key_hash = keccak256(&public_key_bytes.as_bytes()[1..]); // Skip the 0x04 prefix
    let address_bytes: [u8; 20] = public_key_hash[12..32]
        .try_into()
        .map_err(|_| "Failed to extract address bytes from public key hash".to_string())?;

    Ok(Address::from(address_bytes))
}

#[cfg(test)]
mod amount_tests {
    use super::*;

    #[sqlx::test(migrations = false)]
    async fn the_signing_summary_names_the_outcome_from_the_catalog(pool: sqlx::PgPool) {
        sqlx::raw_sql(include_str!(
            "../../../../migrations/037_token_condition_cache.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        // Token ids carry no YES/NO meaning: this YES token ends in 0.
        sqlx::query(
            "INSERT INTO token_condition_cache (token_id, condition_id) VALUES \
             ('7770', 'cond'), ('8881', 'cond'), ('9991', 'elsewhere')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let state = AppState::for_tests(pool);
        state
            .market_catalog
            .alias("cond", &("7770".into(), "8881".into()))
            .await;
        assert_eq!(outcome_label(&state, "7770").await, "YES");
        assert_eq!(outcome_label(&state, "8881").await, "NO");
        assert_eq!(outcome_label(&state, "9991").await, "UNKNOWN");
        assert_eq!(outcome_label(&state, "1231").await, "UNKNOWN");
    }
    #[tokio::test]
    async fn browser_order_and_auth_signatures_match_shared_signing_for_both_domains() {
        let wallet = auth::TradingWallet::from_private_key(&format!("{:064x}", 1)).unwrap();
        let now = Utc::now();
        let signer = polymarket_core::signing::OrderSigner::new(wallet.signer().clone());
        for neg in [false, true] {
            let signer = if neg {
                signer.to_neg_risk()
            } else {
                signer.clone()
            };
            let salt = generate_salt();
            assert!(salt.parse::<u64>().unwrap() < (1u64 << 53));
            let data = signer
                .order_builder()
                .token_id_str("1")
                .side(polymarket_core::signing::OrderSide::Buy)
                .price(Decimal::new(4, 1))
                .size(Decimal::TEN)
                .expires_at((now.timestamp() + 3600) as u64)
                .build()
                .unwrap();
            let signed = signer.sign_order(&data).await.unwrap();
            let mut pending = PendingOrderRow {
                id: Uuid::new_v4(),
                user_id: Uuid::new_v4(),
                maker_address: wallet.address_string(),
                token_id: "1".into(),
                side: 0,
                maker_amount: signed.maker_amount.clone(),
                taker_amount: signed.taker_amount.clone(),
                salt: signed.salt.to_string(),
                expiration: now.timestamp() + 3600,
                nonce: "0".into(),
                fee_rate_bps: 0,
                signature_type: 0,
                neg_risk: neg,
                expires_at: now + Duration::hours(1),
                created_at: now,
            };
            assert_eq!(
                verify_order_signature(&pending, &signed.signature).unwrap(),
                wallet.address()
            );
            assert_eq!(
                Some(format!("{:#x}", order_digest(&pending).unwrap())),
                signed.order_hash
            );
            pending.taker_amount = "11000000".into();
            assert_ne!(
                verify_order_signature(&pending, &signed.signature).unwrap(),
                wallet.address()
            );
        }
        let proof = WalletAuthProof {
            timestamp: now.timestamp() as u64,
            nonce: 0,
            signature: signer
                .sign_clob_auth_message(now.timestamp() as u64, 0)
                .await
                .unwrap(),
        };
        verify_wallet_auth(&wallet.address_string(), &proof, now.timestamp()).unwrap();
        assert!(
            verify_wallet_auth(&wallet.address_string(), &proof, now.timestamp() + 400).is_err()
        );
        assert!(verify_wallet_auth(
            "0x0000000000000000000000000000000000000001",
            &proof,
            now.timestamp()
        )
        .is_err());
    }

    #[test]
    fn share_quantity_and_notional_match_for_buys_and_sells() {
        assert_eq!(
            calculate_amounts("BUY", Decimal::new(4, 1), Decimal::TEN),
            ("4000000".into(), "10000000".into())
        );
        assert_eq!(
            calculate_amounts("SELL", Decimal::new(4, 1), Decimal::TEN),
            ("10000000".into(), "4000000".into())
        );
        assert_eq!(
            calculate_amounts("BUY", Decimal::new(23, 2), Decimal::new(1025, 2)),
            ("2357500".into(), "10250000".into())
        );
    }
}
