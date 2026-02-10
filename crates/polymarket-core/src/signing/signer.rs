//! Order signing for Polymarket CLOB.
//!
//! Provides EIP-712 typed data signing for orders and L1 authentication
//! messages required by the Polymarket CLOB API.

use alloy_primitives::{Address, B256, U256};
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::SolValue;
use anyhow::{Context, Result};

use super::domain::{ClobAuthDomain, Eip712Domain};
use super::order_types::{OrderBuilder, OrderData, SignedOrder};

/// Order signer for Polymarket CLOB.
///
/// Handles EIP-712 signing of orders and authentication messages.
#[derive(Clone)]
pub struct OrderSigner {
    signer: PrivateKeySigner,
    domain: Eip712Domain,
}

impl OrderSigner {
    /// Create a new order signer with the default CTF Exchange domain.
    pub fn new(signer: PrivateKeySigner) -> Self {
        Self {
            signer,
            domain: Eip712Domain::ctf_exchange(),
        }
    }

    /// Create a new order signer for neg-risk markets.
    pub fn new_neg_risk(signer: PrivateKeySigner) -> Self {
        Self {
            signer,
            domain: Eip712Domain::neg_risk_ctf_exchange(),
        }
    }

    /// Create a new order signer with a custom domain.
    pub fn with_domain(signer: PrivateKeySigner, domain: Eip712Domain) -> Self {
        Self { signer, domain }
    }

    /// Get the signer's address.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Get an order builder pre-configured with the maker address.
    pub fn order_builder(&self) -> OrderBuilder {
        OrderBuilder::new().maker(self.address())
    }

    /// Sign an order and return the signed order ready for submission.
    pub async fn sign_order(&self, order: &OrderData) -> Result<SignedOrder> {
        let signature = self.sign_typed_data(order).await?;
        Ok(SignedOrder::from_order_data(order, signature))
    }

    /// Sign order data using EIP-712 typed data signing.
    async fn sign_typed_data(&self, order: &OrderData) -> Result<String> {
        // Compute the EIP-712 hash: keccak256("\x19\x01" ++ domainSeparator ++ structHash)
        let domain_separator = self.domain.separator();
        let struct_hash = order.struct_hash();

        let digest = compute_typed_data_hash(domain_separator, struct_hash);

        // Sign the digest
        let signature = self
            .signer
            .sign_hash(&digest)
            .await
            .context("Failed to sign order")?;

        Ok(format!("0x{}", hex::encode(signature.as_bytes())))
    }

    /// Sign a CLOB auth message using EIP-712 typed data (L1 authentication).
    ///
    /// This matches Polymarket's ClobAuth EIP-712 struct:
    /// ClobAuth(address address, string timestamp, uint256 nonce, string message)
    pub async fn sign_clob_auth_message(&self, timestamp: u64, nonce: u64) -> Result<String> {
        let auth_domain = ClobAuthDomain::polygon();
        let domain_separator = auth_domain.separator();

        let struct_hash = clob_auth_struct_hash(self.address(), timestamp, nonce);

        let digest = compute_typed_data_hash(domain_separator, struct_hash);

        let signature = self
            .signer
            .sign_hash(&digest)
            .await
            .context("Failed to sign CLOB auth message")?;

        Ok(format!("0x{}", hex::encode(signature.as_bytes())))
    }

    /// Sign a message for L1 authentication (API key derivation).
    ///
    /// The message format is: "I am signing this message to authenticate with Polymarket"
    pub async fn sign_auth_message(&self) -> Result<String> {
        let message = "I am signing this message to authenticate with Polymarket";
        self.sign_personal_message(message.as_bytes()).await
    }

    /// Sign a message with a timestamp for L1 authentication.
    ///
    /// The message format is: "I am signing this message to authenticate with Polymarket\nTimestamp: {timestamp}"
    pub async fn sign_auth_message_with_timestamp(&self, timestamp: u64) -> Result<String> {
        let message = format!(
            "I am signing this message to authenticate with Polymarket\nTimestamp: {}",
            timestamp
        );
        self.sign_personal_message(message.as_bytes()).await
    }

    /// Sign a personal message (EIP-191).
    pub async fn sign_personal_message(&self, message: &[u8]) -> Result<String> {
        let signature = self
            .signer
            .sign_message(message)
            .await
            .context("Failed to sign message")?;

        Ok(format!("0x{}", hex::encode(signature.as_bytes())))
    }
}

/// Compute the EIP-712 typed data hash.
fn compute_typed_data_hash(domain_separator: B256, struct_hash: B256) -> B256 {
    let prefix = [0x19, 0x01];
    let data = (prefix, domain_separator, struct_hash).abi_encode_packed();
    alloy_primitives::keccak256(&data)
}

/// Compute the EIP-712 struct hash for ClobAuth.
///
/// ClobAuth(address address, string timestamp, uint256 nonce, string message)
///
/// Uses standard ABI encoding (each field padded to 32 bytes) per EIP-712 spec.
fn clob_auth_struct_hash(address: Address, timestamp: u64, nonce: u64) -> B256 {
    const CLOB_AUTH_MSG: &str = "This message attests that I control the given wallet";

    let type_hash = alloy_primitives::keccak256(
        b"ClobAuth(address address,string timestamp,uint256 nonce,string message)",
    );

    let timestamp_hash = alloy_primitives::keccak256(timestamp.to_string().as_bytes());
    let message_hash = alloy_primitives::keccak256(CLOB_AUTH_MSG.as_bytes());

    // EIP-712 encodeData uses standard ABI encoding: address left-padded to 32 bytes
    let address_padded = B256::left_padding_from(address.as_slice());

    let encoded = (
        type_hash,
        address_padded,
        timestamp_hash,
        U256::from(nonce),
        message_hash,
    )
        .abi_encode_packed();

    alloy_primitives::keccak256(&encoded)
}

impl std::fmt::Debug for OrderSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrderSigner")
            .field("address", &format!("{:?}", self.address()))
            .field("domain", &self.domain.name)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    use super::super::domain::OrderSide;

    // Test private key (DO NOT USE IN PRODUCTION)
    const TEST_PRIVATE_KEY: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDRESS: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    fn test_signer() -> OrderSigner {
        let signer = PrivateKeySigner::from_str(TEST_PRIVATE_KEY).unwrap();
        OrderSigner::new(signer)
    }

    #[test]
    fn test_order_signer_creation() {
        let signer = test_signer();
        assert_eq!(
            signer.address().to_string().to_lowercase(),
            TEST_ADDRESS.to_lowercase()
        );
    }

    #[test]
    fn test_order_builder_from_signer() {
        let signer = test_signer();
        let order = signer
            .order_builder()
            .token_id(U256::from(123u64))
            .side(OrderSide::Buy)
            .price(Decimal::new(50, 2))
            .size(Decimal::from(100u64))
            .expires_in(3600)
            .build();

        assert!(order.is_some());
        let order = order.unwrap();
        assert_eq!(order.maker, signer.address());
    }

    #[tokio::test]
    async fn test_sign_order() {
        let signer = test_signer();

        let order = signer
            .order_builder()
            .token_id(U256::from(123u64))
            .side(OrderSide::Buy)
            .price(Decimal::new(50, 2))
            .size(Decimal::from(100u64))
            .expires_in(3600)
            .build()
            .unwrap();

        let signed = signer.sign_order(&order).await.unwrap();

        // Signature should be 0x + 130 hex chars (65 bytes)
        assert!(signed.signature.starts_with("0x"));
        assert_eq!(signed.signature.len(), 132);
        assert_eq!(signed.side, "BUY");
    }

    #[tokio::test]
    async fn test_sign_auth_message() {
        let signer = test_signer();
        let signature = signer.sign_auth_message().await.unwrap();

        assert!(signature.starts_with("0x"));
        assert_eq!(signature.len(), 132);
    }

    #[tokio::test]
    async fn test_sign_auth_message_with_timestamp() {
        let signer = test_signer();
        let timestamp = 1700000000u64;
        let signature = signer
            .sign_auth_message_with_timestamp(timestamp)
            .await
            .unwrap();

        assert!(signature.starts_with("0x"));
        assert_eq!(signature.len(), 132);
    }

    #[tokio::test]
    async fn test_signatures_are_deterministic() {
        let signer = test_signer();

        let mut order1 = OrderData::new(
            signer.address(),
            U256::from(123u64),
            OrderSide::Buy,
            U256::from(100u64),
            U256::from(200u64),
            1700000000u64,
        );
        order1.salt = U256::from(999u64); // Fixed salt

        let mut order2 = order1.clone();
        order2.salt = U256::from(999u64);

        let signed1 = signer.sign_order(&order1).await.unwrap();
        let signed2 = signer.sign_order(&order2).await.unwrap();

        // Same order data should produce same signature
        assert_eq!(signed1.signature, signed2.signature);
    }

    #[test]
    fn test_debug_does_not_expose_key() {
        let signer = test_signer();
        let debug_str = format!("{:?}", signer);

        assert!(debug_str.contains("OrderSigner"));
        assert!(debug_str.contains("address"));
        assert!(!debug_str.contains(TEST_PRIVATE_KEY));
    }

    #[test]
    fn test_neg_risk_signer() {
        let private_signer = PrivateKeySigner::from_str(TEST_PRIVATE_KEY).unwrap();
        let signer = OrderSigner::new_neg_risk(private_signer);

        assert_eq!(signer.domain.name, "Polymarket CTF Exchange");
    }
}
