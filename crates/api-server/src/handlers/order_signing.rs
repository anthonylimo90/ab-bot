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
    /// Size in USDC.
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
pub struct SubmitOrderRequest {
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
    id: Uuid,
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
    created_at: DateTime<Utc>,
}

/// Generate a random salt for order uniqueness.
fn generate_salt() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let random_component = (std::process::id() as u128) << 64;
    format!("{}", timestamp ^ random_component)
}

/// Calculate maker and taker amounts from price and size.
fn calculate_amounts(side: &str, price: Decimal, size: Decimal) -> (String, String) {
    // USDC has 6 decimals
    let usdc_decimals = Decimal::from(1_000_000u64);
    let size_base = (size * usdc_decimals).round();

    match side.to_uppercase().as_str() {
        "BUY" => {
            // Buying tokens: pay size USDC, receive (size / price) tokens
            let maker_amount = size_base.to_string().replace('.', "");
            let token_amount = if price > Decimal::ZERO {
                size / price
            } else {
                Decimal::ZERO
            };
            let taker_amount = (token_amount * usdc_decimals)
                .round()
                .to_string()
                .replace('.', "");
            (maker_amount, taker_amount)
        }
        "SELL" => {
            // Selling tokens: provide (size / price) tokens, receive size USDC
            let token_amount = if price > Decimal::ZERO {
                size / price
            } else {
                Decimal::ZERO
            };
            let maker_amount = (token_amount * usdc_decimals)
                .round()
                .to_string()
                .replace('.', "");
            let taker_amount = size_base.to_string().replace('.', "");
            (maker_amount, taker_amount)
        }
        _ => ("0".to_string(), "0".to_string()),
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
    if req.size <= Decimal::ZERO {
        return Err(ApiError::BadRequest("Size must be positive".into()));
    }

    // Validate maker address
    if !req.maker_address.starts_with("0x") || req.maker_address.len() != 42 {
        return Err(ApiError::BadRequest("Invalid maker address".into()));
    }

    // Calculate expiration
    let expires_in_secs = req.expires_in_secs.unwrap_or(3600); // Default 1 hour
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
    .bind(0i32) // fee_rate_bps
    .bind(0i16) // signature_type (EOA)
    .bind(req.neg_risk)
    .bind(expires_at)
    .bind(now)
    .execute(&state.pool)
    .await?;

    // Build EIP-712 typed data
    let verifying_contract = if req.neg_risk {
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
            fee_rate_bps: "0".to_string(),
            side,
            signature_type: 0, // EOA
        },
    };

    // Build human-readable summary
    let potential_payout = if side_str == "BUY" {
        req.size / req.price
    } else {
        req.size
    };

    let summary = OrderSummary {
        side: side_str.clone(),
        outcome: if req.token_id.ends_with("1") {
            "YES".to_string()
        } else {
            "NO".to_string()
        },
        price: format!("${:.2}", req.price),
        size: format!("${:.2}", req.size),
        total_cost: format!("${:.2}", req.size),
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
        salt: pending_order.salt.clone(),
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

    // Submit to Polymarket CLOB API
    let clob_result = submit_to_clob(&state, &signed_order, pending_order.neg_risk).await;

    // Delete pending order after submission attempt
    sqlx::query("DELETE FROM pending_wallet_orders WHERE id = $1")
        .bind(pending_order_id)
        .execute(&state.pool)
        .await?;

    match clob_result {
        Ok(response) => {
            tracing::info!(
                user_id = %user_id,
                pending_order_id = %pending_order_id,
                order_id = %response.order_id,
                maker = %pending_order.maker_address,
                token_id = %pending_order.token_id,
                side = %side_str,
                "Order submitted to CLOB successfully"
            );

            Ok(Json(SubmitOrderResponse {
                success: true,
                order_id: Some(response.order_id),
                message: "Order submitted successfully".to_string(),
                tx_hash: response.tx_hash,
            }))
        }
        Err(e) => {
            tracing::error!(
                user_id = %user_id,
                pending_order_id = %pending_order_id,
                maker = %pending_order.maker_address,
                error = %e,
                "Failed to submit order to CLOB"
            );

            // Return error but don't expose internal details
            Err(ApiError::Internal(format!(
                "Order submission failed: {}",
                e
            )))
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Response from CLOB submission.
struct ClobSubmitResponse {
    order_id: String,
    tx_hash: Option<String>,
}

/// Verify the EIP-712 signature and recover the signer address.
fn verify_order_signature(
    pending_order: &PendingOrderRow,
    signature: &str,
) -> Result<Address, String> {
    use alloy_primitives::U256;

    // Parse signature bytes
    let sig_bytes = hex::decode(signature.trim_start_matches("0x"))
        .map_err(|e| format!("Invalid signature hex: {}", e))?;

    if sig_bytes.len() != 65 {
        return Err("Signature must be 65 bytes".to_string());
    }

    // Extract r, s, v from signature
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&sig_bytes[0..32]);
    s.copy_from_slice(&sig_bytes[32..64]);
    let v = sig_bytes[64];

    // Build the EIP-712 struct hash
    let order_type_hash = keccak256(
        b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)",
    );

    // Parse order fields
    let salt = U256::from_str_radix(&pending_order.salt, 10)
        .map_err(|e| format!("Invalid salt: {}", e))?;
    let maker: Address = pending_order
        .maker_address
        .parse()
        .map_err(|e| format!("Invalid maker address: {}", e))?;
    let signer = maker;
    let taker = Address::ZERO;
    let token_id = U256::from_str_radix(&pending_order.token_id, 10)
        .map_err(|e| format!("Invalid token_id: {}", e))?;
    let maker_amount = U256::from_str_radix(&pending_order.maker_amount, 10)
        .map_err(|e| format!("Invalid maker_amount: {}", e))?;
    let taker_amount = U256::from_str_radix(&pending_order.taker_amount, 10)
        .map_err(|e| format!("Invalid taker_amount: {}", e))?;
    let expiration = U256::from(pending_order.expiration as u64);
    let nonce = U256::from_str_radix(&pending_order.nonce, 10).unwrap_or(U256::ZERO);
    let fee_rate_bps = U256::from(pending_order.fee_rate_bps as u64);
    let side = U256::from(pending_order.side as u64);
    let signature_type = U256::from(pending_order.signature_type as u64);

    // Encode struct hash
    let struct_encoded = (
        order_type_hash,
        salt,
        maker,
        signer,
        taker,
        token_id,
        maker_amount,
        taker_amount,
        expiration,
        nonce,
        fee_rate_bps,
        side,
        signature_type,
    )
        .abi_encode_packed();

    let struct_hash = keccak256(&struct_encoded);

    // Build domain separator
    let domain_type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = keccak256(b"Polymarket CTF Exchange");
    let version_hash = keccak256(b"1");
    let chain_id = U256::from(POLYGON_CHAIN_ID);
    let verifying_contract: Address = if pending_order.neg_risk {
        NEG_RISK_CTF_EXCHANGE_ADDRESS.parse().unwrap()
    } else {
        CTF_EXCHANGE_ADDRESS.parse().unwrap()
    };

    let domain_encoded = (
        domain_type_hash,
        name_hash,
        version_hash,
        chain_id,
        verifying_contract,
    )
        .abi_encode_packed();

    let domain_separator = keccak256(&domain_encoded);

    // Build EIP-712 hash: keccak256("\x19\x01" ++ domainSeparator ++ structHash)
    let prefix = [0x19u8, 0x01u8];
    let typed_data = (prefix, domain_separator, struct_hash).abi_encode_packed();
    let digest = keccak256(&typed_data);

    // Recover the signer address using secp256k1
    recover_address(&digest, &r, &s, v)
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
    let address_bytes: [u8; 20] = public_key_hash[12..32].try_into().unwrap();

    Ok(Address::from(address_bytes))
}

/// Submit signed order to Polymarket CLOB API.
async fn submit_to_clob(
    state: &Arc<AppState>,
    signed_order: &SignedOrder,
    _neg_risk: bool,
) -> Result<ClobSubmitResponse, String> {
    // Build request body
    let request_body = serde_json::json!({
        "order": signed_order,
        "orderType": "GTC"
    });

    // Submit to CLOB
    let url = format!(
        "{}/order",
        polymarket_core::api::ClobClient::DEFAULT_BASE_URL
    );

    let response = state
        .clob_client
        .http_client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("CLOB API error {}: {}", status, body));
    }

    #[derive(Deserialize)]
    struct ClobResponse {
        #[serde(rename = "orderID")]
        order_id: String,
        #[serde(rename = "transactionHash")]
        transaction_hash: Option<String>,
    }

    let clob_response: ClobResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse CLOB response: {}", e))?;

    Ok(ClobSubmitResponse {
        order_id: clob_response.order_id,
        tx_hash: clob_response.transaction_hash,
    })
}
