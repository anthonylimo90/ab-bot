//! EIP-712 domain separators for Polymarket CLOB.
//!
//! Polymarket uses EIP-712 typed data signing for order authentication.
//! This module defines the domain separators for the CTF Exchange contract.

use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolValue;

/// Chain ID for Polygon mainnet.
pub const POLYGON_CHAIN_ID: u64 = 137;

/// Chain ID for Polygon Amoy testnet.
pub const POLYGON_AMOY_CHAIN_ID: u64 = 80002;

/// CTF Exchange contract address on Polygon mainnet.
pub const CTF_EXCHANGE_ADDRESS: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

/// Neg Risk CTF Exchange contract address on Polygon mainnet.
pub const NEG_RISK_CTF_EXCHANGE_ADDRESS: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";

/// Neg Risk Adapter address on Polygon mainnet.
pub const NEG_RISK_ADAPTER_ADDRESS: &str = "0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296";

/// USDC contract address on Polygon mainnet.
pub const USDC_ADDRESS: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// EIP-712 domain separator for order signing.
#[derive(Debug, Clone)]
pub struct Eip712Domain {
    /// Domain name.
    pub name: String,
    /// Domain version.
    pub version: String,
    /// Chain ID.
    pub chain_id: U256,
    /// Verifying contract address.
    pub verifying_contract: Address,
}

/// EIP-712 domain separator for CLOB authentication (no verifyingContract).
#[derive(Debug, Clone)]
pub struct ClobAuthDomain {
    pub name: String,
    pub version: String,
    pub chain_id: U256,
}

impl ClobAuthDomain {
    /// Create the ClobAuthDomain for Polygon mainnet.
    pub fn polygon() -> Self {
        Self {
            name: "ClobAuthDomain".to_string(),
            version: "1".to_string(),
            chain_id: U256::from(POLYGON_CHAIN_ID),
        }
    }

    /// Compute the EIP-712 domain separator hash.
    pub fn separator(&self) -> B256 {
        let domain_type_hash = alloy_primitives::keccak256(
            b"EIP712Domain(string name,string version,uint256 chainId)",
        );

        let name_hash = alloy_primitives::keccak256(self.name.as_bytes());
        let version_hash = alloy_primitives::keccak256(self.version.as_bytes());

        let encoded =
            (domain_type_hash, name_hash, version_hash, self.chain_id).abi_encode_packed();

        alloy_primitives::keccak256(&encoded)
    }
}

impl Eip712Domain {
    /// Create domain for CTF Exchange on Polygon mainnet.
    pub fn ctf_exchange() -> Self {
        Self {
            name: "Polymarket CTF Exchange".to_string(),
            version: "1".to_string(),
            chain_id: U256::from(POLYGON_CHAIN_ID),
            verifying_contract: CTF_EXCHANGE_ADDRESS.parse().expect("Invalid CTF address"),
        }
    }

    /// Create domain for Neg Risk CTF Exchange on Polygon mainnet.
    pub fn neg_risk_ctf_exchange() -> Self {
        Self {
            name: "Polymarket CTF Exchange".to_string(),
            version: "1".to_string(),
            chain_id: U256::from(POLYGON_CHAIN_ID),
            verifying_contract: NEG_RISK_CTF_EXCHANGE_ADDRESS
                .parse()
                .expect("Invalid Neg Risk CTF address"),
        }
    }

    /// Create domain with custom parameters.
    pub fn custom(
        name: impl Into<String>,
        version: impl Into<String>,
        chain_id: u64,
        verifying_contract: Address,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            chain_id: U256::from(chain_id),
            verifying_contract,
        }
    }

    /// Compute the EIP-712 domain separator hash.
    pub fn separator(&self) -> B256 {
        let domain_type_hash = alloy_primitives::keccak256(
            b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
        );

        let name_hash = alloy_primitives::keccak256(self.name.as_bytes());
        let version_hash = alloy_primitives::keccak256(self.version.as_bytes());

        let encoded = (
            domain_type_hash,
            name_hash,
            version_hash,
            self.chain_id,
            self.verifying_contract,
        )
            .abi_encode_packed();

        alloy_primitives::keccak256(&encoded)
    }
}

/// Order side (buy/sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy = 0,
    Sell = 1,
}

impl OrderSide {
    /// Get the numeric value for signing.
    pub fn as_u8(&self) -> u8 {
        match self {
            OrderSide::Buy => 0,
            OrderSide::Sell => 1,
        }
    }
}

impl std::fmt::Display for OrderSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "BUY"),
            OrderSide::Sell => write!(f, "SELL"),
        }
    }
}

/// Signature type for orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SignatureType {
    /// EOA signature (most common).
    #[default]
    Eoa = 0,
    /// EIP-1271 contract signature.
    Poly = 1,
    /// Poly proxy signature.
    PolyProxy = 2,
}

impl SignatureType {
    /// Get the numeric value for signing.
    pub fn as_u8(&self) -> u8 {
        match self {
            SignatureType::Eoa => 0,
            SignatureType::Poly => 1,
            SignatureType::PolyProxy => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ctf_exchange_domain() {
        let domain = Eip712Domain::ctf_exchange();
        assert_eq!(domain.name, "Polymarket CTF Exchange");
        assert_eq!(domain.version, "1");
        assert_eq!(domain.chain_id, U256::from(137u64));
    }

    #[test]
    fn test_neg_risk_domain() {
        let domain = Eip712Domain::neg_risk_ctf_exchange();
        assert_eq!(
            domain.verifying_contract,
            NEG_RISK_CTF_EXCHANGE_ADDRESS.parse::<Address>().unwrap()
        );
    }

    #[test]
    fn test_domain_separator_deterministic() {
        let domain1 = Eip712Domain::ctf_exchange();
        let domain2 = Eip712Domain::ctf_exchange();
        assert_eq!(domain1.separator(), domain2.separator());
    }

    #[test]
    fn test_order_side() {
        assert_eq!(OrderSide::Buy.as_u8(), 0);
        assert_eq!(OrderSide::Sell.as_u8(), 1);
        assert_eq!(format!("{}", OrderSide::Buy), "BUY");
        assert_eq!(format!("{}", OrderSide::Sell), "SELL");
    }

    #[test]
    fn test_signature_type() {
        assert_eq!(SignatureType::Eoa.as_u8(), 0);
        assert_eq!(SignatureType::Poly.as_u8(), 1);
        assert_eq!(SignatureType::PolyProxy.as_u8(), 2);
    }
}
