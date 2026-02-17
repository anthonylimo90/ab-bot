//! Order types for Polymarket CLOB signing.
//!
//! Defines the order data structures used for EIP-712 signing and
//! submission to the Polymarket CLOB API.

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolValue;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::domain::{OrderSide, SignatureType};

/// Raw order data for EIP-712 signing.
///
/// This matches the struct used by the CTF Exchange contract.
#[derive(Debug, Clone)]
pub struct OrderData {
    /// Random salt for uniqueness.
    pub salt: U256,
    /// Maker address (your wallet).
    pub maker: Address,
    /// Signer address (usually same as maker).
    pub signer: Address,
    /// Taker address (zero for any taker).
    pub taker: Address,
    /// Token ID of the outcome being traded.
    pub token_id: U256,
    /// Maker amount in base units.
    pub maker_amount: U256,
    /// Taker amount in base units.
    pub taker_amount: U256,
    /// Order expiration timestamp (unix seconds).
    pub expiration: U256,
    /// Nonce for order management.
    pub nonce: U256,
    /// Fee rate in basis points.
    pub fee_rate_bps: U256,
    /// Order side (0 = buy, 1 = sell).
    pub side: u8,
    /// Signature type.
    pub signature_type: u8,
}

impl OrderData {
    /// Create a new order with generated salt.
    pub fn new(
        maker: Address,
        token_id: U256,
        side: OrderSide,
        maker_amount: U256,
        taker_amount: U256,
        expiration_secs: u64,
    ) -> Self {
        // Generate random salt
        let salt = U256::from(rand_salt() as u128);

        Self {
            salt,
            maker,
            signer: maker,
            taker: Address::ZERO,
            token_id,
            maker_amount,
            taker_amount,
            expiration: U256::from(expiration_secs),
            nonce: U256::ZERO,
            fee_rate_bps: U256::ZERO,
            side: side.as_u8(),
            signature_type: SignatureType::Eoa.as_u8(),
        }
    }

    /// Compute the EIP-712 struct hash for this order.
    pub fn struct_hash(&self) -> B256 {
        let order_type_hash = alloy_primitives::keccak256(
            b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)",
        );

        // EIP-712 encodeData: all values must be padded to 32 bytes.
        // Addresses are left-padded from 20 bytes to 32 bytes.
        let maker_padded = B256::left_padding_from(self.maker.as_slice());
        let signer_padded = B256::left_padding_from(self.signer.as_slice());
        let taker_padded = B256::left_padding_from(self.taker.as_slice());

        let encoded = (
            order_type_hash,
            self.salt,
            maker_padded,
            signer_padded,
            taker_padded,
            self.token_id,
            self.maker_amount,
            self.taker_amount,
            self.expiration,
            self.nonce,
            self.fee_rate_bps,
            U256::from(self.side),
            U256::from(self.signature_type),
        )
            .abi_encode_packed();

        alloy_primitives::keccak256(&encoded)
    }
}

/// Generate a random salt for order uniqueness.
/// Masked to 2^53-1 (IEEE 754 safe integer range) as required by the CLOB API.
fn rand_salt() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Mix timestamp with process id, then mask to IEEE 754 safe integer range
    let raw = (nanos ^ ((std::process::id() as u128) << 32)) as u64;
    raw & ((1u64 << 53) - 1)
}

/// A signed order ready for submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedOrder {
    /// Order salt (must be a JSON number).
    pub salt: u64,
    /// Maker address as hex string.
    pub maker: String,
    /// Signer address as hex string.
    pub signer: String,
    /// Taker address as hex string.
    pub taker: String,
    /// Token ID as string.
    #[serde(rename = "tokenId")]
    pub token_id: String,
    /// Maker amount as string.
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    /// Taker amount as string.
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    /// Expiration timestamp as string.
    pub expiration: String,
    /// Nonce as string.
    pub nonce: String,
    /// Fee rate in basis points.
    #[serde(rename = "feeRateBps")]
    pub fee_rate_bps: String,
    /// Side ("BUY" or "SELL").
    pub side: String,
    /// Signature type.
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
    /// EIP-712 signature as hex string.
    pub signature: String,
}

impl SignedOrder {
    /// Create from order data and signature.
    pub fn from_order_data(order: &OrderData, signature: String) -> Self {
        let side = if order.side == 0 { "BUY" } else { "SELL" };

        Self {
            salt: order.salt.to::<u64>(),
            maker: format!("{:?}", order.maker),
            signer: format!("{:?}", order.signer),
            taker: format!("{:?}", order.taker),
            token_id: format!("{}", order.token_id),
            maker_amount: format!("{}", order.maker_amount),
            taker_amount: format!("{}", order.taker_amount),
            expiration: format!("{}", order.expiration),
            nonce: format!("{}", order.nonce),
            fee_rate_bps: format!("{}", order.fee_rate_bps),
            side: side.to_string(),
            signature_type: order.signature_type,
            signature,
        }
    }
}

/// Order builder for creating orders with a fluent API.
#[derive(Debug, Clone)]
pub struct OrderBuilder {
    maker: Option<Address>,
    token_id: Option<U256>,
    side: OrderSide,
    price: Option<Decimal>,
    size: Option<Decimal>,
    expiration_secs: Option<u64>,
    nonce: U256,
    fee_rate_bps: U256,
}

impl OrderBuilder {
    /// Create a new order builder.
    pub fn new() -> Self {
        Self {
            maker: None,
            token_id: None,
            side: OrderSide::Buy,
            price: None,
            size: None,
            expiration_secs: None,
            nonce: U256::ZERO,
            fee_rate_bps: U256::ZERO,
        }
    }

    /// Set the maker address.
    pub fn maker(mut self, maker: Address) -> Self {
        self.maker = Some(maker);
        self
    }

    /// Set the token ID.
    pub fn token_id(mut self, token_id: U256) -> Self {
        self.token_id = Some(token_id);
        self
    }

    /// Set the token ID from a string.
    pub fn token_id_str(mut self, token_id: &str) -> Self {
        self.token_id = Some(U256::from_str_radix(token_id, 10).unwrap_or_default());
        self
    }

    /// Set the order side.
    pub fn side(mut self, side: OrderSide) -> Self {
        self.side = side;
        self
    }

    /// Set the price (0.0 to 1.0).
    pub fn price(mut self, price: Decimal) -> Self {
        self.price = Some(price);
        self
    }

    /// Set the size in USDC.
    pub fn size(mut self, size: Decimal) -> Self {
        self.size = Some(size);
        self
    }

    /// Set expiration in seconds from now.
    pub fn expires_in(mut self, seconds: u64) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.expiration_secs = Some(now + seconds);
        self
    }

    /// Set absolute expiration timestamp.
    pub fn expires_at(mut self, timestamp: u64) -> Self {
        self.expiration_secs = Some(timestamp);
        self
    }

    /// Set the nonce.
    pub fn nonce(mut self, nonce: U256) -> Self {
        self.nonce = nonce;
        self
    }

    /// Set the fee rate in basis points.
    pub fn fee_rate_bps(mut self, fee_rate: u64) -> Self {
        self.fee_rate_bps = U256::from(fee_rate);
        self
    }

    /// Build the order data.
    ///
    /// Returns None if required fields are missing.
    pub fn build(self) -> Option<OrderData> {
        let maker = self.maker?;
        let token_id = self.token_id?;
        let price = self.price?;
        let size = self.size?;
        let expiration = self.expiration_secs?;

        // Convert price and size to maker/taker amounts
        // For a BUY: maker pays USDC, receives tokens
        // For a SELL: maker provides tokens, receives USDC
        let (maker_amount, taker_amount) = calculate_amounts(self.side, price, size);

        let mut order = OrderData::new(
            maker,
            token_id,
            self.side,
            maker_amount,
            taker_amount,
            expiration,
        );
        order.nonce = self.nonce;
        order.fee_rate_bps = self.fee_rate_bps;

        Some(order)
    }
}

impl Default for OrderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate maker and taker amounts from price and size.
///
/// Price is in the range 0.0 to 1.0 (probability).
/// Size is the USDC amount for the trade.
fn calculate_amounts(side: OrderSide, price: Decimal, size: Decimal) -> (U256, U256) {
    // USDC has 6 decimals
    let usdc_decimals = Decimal::from(1_000_000u64);

    // Convert to base units
    let size_base = (size * usdc_decimals).round();

    match side {
        OrderSide::Buy => {
            // Buying tokens: pay size USDC, receive (size / price) tokens
            let maker_amount = U256::from(size_base.to_string().parse::<u128>().unwrap_or(0));
            let token_amount = if price > Decimal::ZERO {
                size / price
            } else {
                Decimal::ZERO
            };
            let taker_amount = U256::from(
                (token_amount * usdc_decimals)
                    .round()
                    .to_string()
                    .parse::<u128>()
                    .unwrap_or(0),
            );
            (maker_amount, taker_amount)
        }
        OrderSide::Sell => {
            // Selling tokens: provide (size / price) tokens, receive size USDC
            let token_amount = if price > Decimal::ZERO {
                size / price
            } else {
                Decimal::ZERO
            };
            let maker_amount = U256::from(
                (token_amount * usdc_decimals)
                    .round()
                    .to_string()
                    .parse::<u128>()
                    .unwrap_or(0),
            );
            let taker_amount = U256::from(size_base.to_string().parse::<u128>().unwrap_or(0));
            (maker_amount, taker_amount)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_data_creation() {
        let maker = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse::<Address>()
            .unwrap();
        let token_id = U256::from(12345u64);
        let expiration = 1700000000u64;

        let order = OrderData::new(
            maker,
            token_id,
            OrderSide::Buy,
            U256::from(100_000_000u64), // 100 USDC
            U256::from(200_000_000u64), // 200 tokens at 0.5
            expiration,
        );

        assert_eq!(order.maker, maker);
        assert_eq!(order.signer, maker);
        assert_eq!(order.taker, Address::ZERO);
        assert_eq!(order.token_id, token_id);
        assert_eq!(order.side, 0);
    }

    #[test]
    fn test_order_struct_hash() {
        let maker = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse::<Address>()
            .unwrap();

        let mut order = OrderData::new(
            maker,
            U256::from(123u64),
            OrderSide::Buy,
            U256::from(100u64),
            U256::from(200u64),
            1700000000u64,
        );
        order.salt = U256::from(999u64); // Fixed salt for deterministic test

        let hash = order.struct_hash();
        assert_ne!(hash, B256::ZERO);
    }

    #[test]
    fn test_signed_order_serialization() {
        let maker = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse::<Address>()
            .unwrap();

        let order = OrderData::new(
            maker,
            U256::from(123u64),
            OrderSide::Buy,
            U256::from(100u64),
            U256::from(200u64),
            1700000000u64,
        );

        let signed = SignedOrder::from_order_data(&order, "0xsignature".to_string());

        assert_eq!(signed.side, "BUY");
        assert_eq!(signed.signature, "0xsignature");

        // Should serialize to JSON
        let json = serde_json::to_string(&signed).unwrap();
        assert!(json.contains("makerAmount"));
        assert!(json.contains("tokenId"));
    }

    #[test]
    fn test_order_builder() {
        let maker = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse::<Address>()
            .unwrap();

        let order = OrderBuilder::new()
            .maker(maker)
            .token_id(U256::from(123u64))
            .side(OrderSide::Buy)
            .price(Decimal::new(50, 2)) // 0.50
            .size(Decimal::from(100u64)) // 100 USDC
            .expires_in(3600) // 1 hour
            .build();

        assert!(order.is_some());
        let order = order.unwrap();
        assert_eq!(order.maker, maker);
        assert_eq!(order.side, 0);
    }

    #[test]
    fn test_calculate_amounts_buy() {
        let price = Decimal::new(50, 2); // 0.50
        let size = Decimal::from(100u64); // 100 USDC

        let (maker_amount, taker_amount) = calculate_amounts(OrderSide::Buy, price, size);

        // Maker pays 100 USDC (100 * 1_000_000 = 100_000_000)
        assert_eq!(maker_amount, U256::from(100_000_000u64));
        // Taker provides 200 tokens (100 / 0.5 * 1_000_000 = 200_000_000)
        assert_eq!(taker_amount, U256::from(200_000_000u64));
    }

    #[test]
    fn test_calculate_amounts_sell() {
        let price = Decimal::new(50, 2); // 0.50
        let size = Decimal::from(100u64); // 100 USDC worth

        let (maker_amount, taker_amount) = calculate_amounts(OrderSide::Sell, price, size);

        // Maker provides 200 tokens
        assert_eq!(maker_amount, U256::from(200_000_000u64));
        // Taker pays 100 USDC
        assert_eq!(taker_amount, U256::from(100_000_000u64));
    }
}
