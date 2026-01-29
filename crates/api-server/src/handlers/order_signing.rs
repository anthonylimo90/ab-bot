//! Order signing handlers for MetaMask/wallet-based trade execution.
//!
//! This module provides endpoints for preparing EIP-712 typed data
//! that users can sign with their wallet, and then submitting
//! the signed orders to the Polymarket CLOB.

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
    CTF_EXCHANGE_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS, POLYGON_CHAIN_ID,
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

    // Build the signed order for CLOB submission
    let side_str = if pending_order.side == 0 {
        "BUY"
    } else {
        "SELL"
    };

    let signed_order = serde_json::json!({
        "salt": pending_order.salt,
        "maker": pending_order.maker_address,
        "signer": pending_order.maker_address,
        "taker": "0x0000000000000000000000000000000000000000",
        "tokenId": pending_order.token_id,
        "makerAmount": pending_order.maker_amount,
        "takerAmount": pending_order.taker_amount,
        "expiration": pending_order.expiration.to_string(),
        "nonce": pending_order.nonce,
        "feeRateBps": pending_order.fee_rate_bps.to_string(),
        "side": side_str,
        "signatureType": pending_order.signature_type,
        "signature": req.signature
    });

    // TODO: Submit to actual Polymarket CLOB API
    // For now, we simulate a successful submission
    //
    // In production, this would:
    // 1. Call the Polymarket CLOB API with the signed order
    // 2. Handle the response (success/failure)
    // 3. Store the order in our database for tracking

    // Delete pending order after submission
    sqlx::query("DELETE FROM pending_wallet_orders WHERE id = $1")
        .bind(pending_order_id)
        .execute(&state.pool)
        .await?;

    // Generate a mock order ID for now
    let order_id = Uuid::new_v4().to_string();

    tracing::info!(
        user_id = %user_id,
        pending_order_id = %pending_order_id,
        maker = %pending_order.maker_address,
        token_id = %pending_order.token_id,
        side = %side_str,
        "Signed order submitted"
    );

    Ok(Json(SubmitOrderResponse {
        success: true,
        order_id: Some(order_id),
        message: "Order submitted successfully".to_string(),
        tx_hash: None, // Would be populated from CLOB response
    }))
}
