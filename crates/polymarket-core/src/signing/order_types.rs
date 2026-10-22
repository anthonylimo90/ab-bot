//! Order types for Polymarket CLOB signing.
//!
//! Defines the order data structures used for EIP-712 signing and
//! submission to the Polymarket CLOB API.

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolValue;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::domain::{OrderSide, SignatureType};

/// The EIP-712 type of a CTF Exchange V2 order.
pub const ORDER_TYPE: &str = "Order(uint256 salt,address maker,address signer,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint8 side,uint8 signatureType,uint256 timestamp,bytes32 metadata,bytes32 builder)";

/// Raw order data for EIP-712 signing.
///
/// The signed fields match the CTF Exchange V2 `Order` struct; `expiration`
/// travels with the order but is not signed.
#[derive(Debug, Clone)]
pub struct OrderData {
    /// Random salt for uniqueness.
    pub salt: U256,
    /// Maker address (your wallet).
    pub maker: Address,
    /// Signer address (usually same as maker).
    pub signer: Address,
    /// Token ID of the outcome being traded.
    pub token_id: U256,
    /// Maker amount in base units.
    pub maker_amount: U256,
    /// Taker amount in base units.
    pub taker_amount: U256,
    /// Order side (0 = buy, 1 = sell).
    pub side: u8,
    /// Signature type.
    pub signature_type: u8,
    /// Creation time in unix milliseconds.
    pub timestamp: U256,
    /// Metadata attached to the order (zero for none).
    pub metadata: B256,
    /// Builder code the order is attributed to (zero for none).
    pub builder: B256,
    /// Unix seconds after which a GTD order lapses, zero for none. Not signed.
    pub expiration: U256,
}

impl OrderData {
    /// Create a new order with generated salt, created now.
    pub fn new(
        maker: Address,
        token_id: U256,
        side: OrderSide,
        maker_amount: U256,
        taker_amount: U256,
        expiration_secs: u64,
    ) -> Self {
        Self {
            salt: U256::from(rand_salt()),
            maker,
            signer: maker,
            token_id,
            maker_amount,
            taker_amount,
            side: side.as_u8(),
            signature_type: SignatureType::Eoa.as_u8(),
            timestamp: U256::from(now_millis()),
            metadata: B256::ZERO,
            builder: B256::ZERO,
            expiration: U256::from(expiration_secs),
        }
    }

    /// Compute the EIP-712 struct hash for this order.
    pub fn struct_hash(&self) -> B256 {
        let order_type_hash = alloy_primitives::keccak256(ORDER_TYPE.as_bytes());

        // EIP-712 encodeData: all values must be padded to 32 bytes.
        // Addresses are left-padded from 20 bytes to 32 bytes.
        let maker_padded = B256::left_padding_from(self.maker.as_slice());
        let signer_padded = B256::left_padding_from(self.signer.as_slice());

        let encoded = (
            order_type_hash,
            self.salt,
            maker_padded,
            signer_padded,
            self.token_id,
            self.maker_amount,
            self.taker_amount,
            U256::from(self.side),
            U256::from(self.signature_type),
            self.timestamp,
            self.metadata,
            self.builder,
        )
            .abi_encode_packed();

        alloy_primitives::keccak256(&encoded)
    }
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
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

/// A signed order ready for submission, in the exchange's wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedOrder {
    /// Local reconciliation identity; excluded from the exchange payload.
    #[serde(skip)]
    pub order_hash: Option<String>,
    /// Order salt (must be a JSON number).
    pub salt: u64,
    /// Maker address as hex string.
    pub maker: String,
    /// Signer address as hex string.
    pub signer: String,
    /// Token ID as string.
    #[serde(rename = "tokenId")]
    pub token_id: String,
    /// Maker amount as string.
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    /// Taker amount as string.
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    /// Side ("BUY" or "SELL").
    pub side: String,
    /// Expiration timestamp as string (not signed).
    pub expiration: String,
    /// Signature type.
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
    /// Creation time in unix milliseconds, as string.
    pub timestamp: String,
    /// Metadata as 0x-prefixed bytes32 hex.
    pub metadata: String,
    /// Builder code as 0x-prefixed bytes32 hex.
    pub builder: String,
    /// EIP-712 signature as hex string.
    pub signature: String,
}

impl SignedOrder {
    /// Create from order data and signature.
    pub fn from_order_data(order: &OrderData, signature: String) -> Self {
        let side = if order.side == 0 { "BUY" } else { "SELL" };

        Self {
            order_hash: None,
            salt: order.salt.to::<u64>(),
            maker: format!("{:?}", order.maker),
            signer: format!("{:?}", order.signer),
            token_id: format!("{}", order.token_id),
            maker_amount: format!("{}", order.maker_amount),
            taker_amount: format!("{}", order.taker_amount),
            side: side.to_string(),
            expiration: format!("{}", order.expiration),
            signature_type: order.signature_type,
            timestamp: format!("{}", order.timestamp),
            metadata: format!("{:#x}", order.metadata),
            builder: format!("{:#x}", order.builder),
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

        Some(OrderData::new(
            maker,
            token_id,
            self.side,
            maker_amount,
            taker_amount,
            expiration,
        ))
    }
}

impl Default for OrderBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// USDC has 6 decimal places.
const USDC_DECIMALS: u32 = 6;

/// Lot size scale — token sizes have max 2 decimal places.
const LOT_SIZE_SCALE: u32 = 2;

/// Convert a decimal to fixed-point u128 with the given decimal scale.
/// The value is first truncated to `scale` decimal places, then scaled up
/// to USDC_DECIMALS (6) for the on-chain representation.
fn to_fixed_u128(d: Decimal, scale: u32) -> u128 {
    d.trunc_with_scale(scale)
        .trunc_with_scale(USDC_DECIMALS)
        .mantissa()
        .unsigned_abs()
}

/// Calculate maker and taker amounts from price and size.
///
/// Price is in the range 0.0 to 1.0 (probability).
/// Size is the number of outcome tokens to trade.
///
/// Follows the Polymarket convention:
/// - BUY:  taker_amount = size, maker_amount = size * price
/// - SELL: maker_amount = size, taker_amount = size * price
///
/// Token amounts (size) are truncated to LOT_SIZE_SCALE (2) decimals.
/// USDC amounts (cost) are truncated to LOT_SIZE_SCALE + tick_decimals decimals.
/// Default tick_decimals = 2 (for tick_size 0.01), giving 4 decimal USDC precision.
fn calculate_amounts(side: OrderSide, price: Decimal, size: Decimal) -> (U256, U256) {
    let raw_cost = size * price;

    // Token amounts: max 2 decimal places (lot size)
    let token_scale = LOT_SIZE_SCALE;
    // USDC amounts: lot_size + tick_size decimals (default 2+2=4)
    let cost_scale = LOT_SIZE_SCALE + LOT_SIZE_SCALE;

    match side {
        OrderSide::Buy => {
            // BUY: maker pays USDC (size * price), taker provides tokens (size)
            let maker_amount = U256::from(to_fixed_u128(raw_cost, cost_scale));
            let taker_amount = U256::from(to_fixed_u128(size, token_scale));
            (maker_amount, taker_amount)
        }
        OrderSide::Sell => {
            // SELL: maker provides tokens (size), taker pays USDC (size * price)
            let maker_amount = U256::from(to_fixed_u128(size, token_scale));
            let taker_amount = U256::from(to_fixed_u128(raw_cost, cost_scale));
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
        assert_eq!(order.token_id, token_id);
        assert_eq!(order.side, 0);
        assert_eq!(order.metadata, B256::ZERO);
        assert_eq!(order.builder, B256::ZERO);
        // Milliseconds, not seconds.
        assert!(order.timestamp > U256::from(1_700_000_000_000u64));
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

        // The V2 wire format: what the official clients send, and no more.
        let json = serde_json::to_value(&signed).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "builder",
                "expiration",
                "maker",
                "makerAmount",
                "metadata",
                "salt",
                "side",
                "signature",
                "signatureType",
                "signer",
                "takerAmount",
                "timestamp",
                "tokenId"
            ]
        );
        assert!(json["salt"].is_u64());
        assert_eq!(json["expiration"], "1700000000");
        assert_eq!(json["metadata"], format!("0x{}", "0".repeat(64)));
        assert_eq!(json["builder"], format!("0x{}", "0".repeat(64)));
        assert_eq!(json["timestamp"], order.timestamp.to_string());
    }

    #[test]
    fn the_order_type_hash_is_the_v2_exchanges() {
        // ORDER_TYPEHASH in the CTF Exchange V2's Structs.sol.
        assert_eq!(
            format!("{:#x}", alloy_primitives::keccak256(ORDER_TYPE.as_bytes())),
            "0xbb86318a2138f5fa8ae32fbe8e659f8fcf13cc6ae4014a707893055433818589"
        );
    }

    #[test]
    fn struct_hash_matches_an_independent_eip712_implementation() {
        alloy_sol_types::sol! {
            struct Order {
                uint256 salt;
                address maker;
                address signer;
                uint256 tokenId;
                uint256 makerAmount;
                uint256 takerAmount;
                uint8 side;
                uint8 signatureType;
                uint256 timestamp;
                bytes32 metadata;
                bytes32 builder;
            }
        }
        use alloy_sol_types::SolStruct;

        let maker = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
            .parse::<Address>()
            .unwrap();
        let mut order = OrderData::new(
            maker,
            U256::from_str_radix(
                "71321045679252212594626385532706912750332728571942532289631379312455583992563",
                10,
            )
            .unwrap(),
            OrderSide::Sell,
            U256::from(25_000_000u64),
            U256::from(12_250_000u64),
            0,
        );
        order.salt = U256::from(479_249_096_354u64);
        order.timestamp = U256::from(1_761_000_000_123u64);
        order.metadata = B256::repeat_byte(0xab);
        order.builder = B256::repeat_byte(0x01);

        let reference = Order {
            salt: order.salt,
            maker,
            signer: maker,
            tokenId: order.token_id,
            makerAmount: order.maker_amount,
            takerAmount: order.taker_amount,
            side: 1,
            signatureType: 0,
            timestamp: order.timestamp,
            metadata: order.metadata,
            builder: order.builder,
        };
        assert_eq!(Order::eip712_encode_type(), ORDER_TYPE);
        assert_eq!(order.struct_hash(), reference.eip712_hash_struct());
        // The expiration is not part of what is signed.
        let mut lapsing = order.clone();
        lapsing.expiration = U256::from(1_761_000_600u64);
        assert_eq!(lapsing.struct_hash(), order.struct_hash());
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
        let size = Decimal::from(100u64); // 100 tokens

        let (maker_amount, taker_amount) = calculate_amounts(OrderSide::Buy, price, size);

        // BUY: maker pays USDC (size * price = 100 * 0.50 = 50 USDC = 50_000_000)
        assert_eq!(maker_amount, U256::from(50_000_000u64));
        // BUY: taker provides tokens (size = 100 tokens = 100_000_000)
        assert_eq!(taker_amount, U256::from(100_000_000u64));
    }

    #[test]
    fn test_calculate_amounts_sell() {
        let price = Decimal::new(50, 2); // 0.50
        let size = Decimal::from(100u64); // 100 tokens

        let (maker_amount, taker_amount) = calculate_amounts(OrderSide::Sell, price, size);

        // SELL: maker provides tokens (size = 100 tokens = 100_000_000)
        assert_eq!(maker_amount, U256::from(100_000_000u64));
        // SELL: taker pays USDC (size * price = 100 * 0.50 = 50 USDC = 50_000_000)
        assert_eq!(taker_amount, U256::from(50_000_000u64));
    }
}
