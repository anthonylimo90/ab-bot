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

/// CTF Exchange (V2) on Polygon mainnet: the verifying contract of orders in
/// standard markets since the CLOB V2 cutover of 2026-04-28, after which
/// orders signed for the V1 exchange are no longer accepted.
pub const CTF_EXCHANGE_ADDRESS: &str = "0xE111180000d2663C0091e4f400237545B87B996B";

/// Neg Risk CTF Exchange (V2) on Polygon mainnet: the verifying contract of
/// orders in neg-risk markets.
pub const NEG_RISK_CTF_EXCHANGE_ADDRESS: &str = "0xe2222d279d744050d28e00520010520000310F59";

/// EIP-712 domain version of the V2 exchanges.
pub const EXCHANGE_DOMAIN_VERSION: &str = "2";

/// Neg Risk Adapter address on Polygon mainnet.
pub const NEG_RISK_ADAPTER_ADDRESS: &str = "0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296";

/// pUSD, the exchange's collateral token since V2 (6 decimals, backed 1:1
/// by USDC.e held in its vault).
pub const COLLATERAL_ADDRESS: &str = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB";

/// Wraps USDC.e into pUSD: `wrap(USDC.e, to, amount)` pulls the caller's
/// USDC.e, so the caller approves it first.
pub const COLLATERAL_ONRAMP_ADDRESS: &str = "0x93070a847efEf7F70739046A929D47a521F5B8ee";

/// Unwraps pUSD into USDC.e: `unwrap(USDC.e, to, amount)` pulls the
/// caller's pUSD and pays `to` in USDC.e, so the caller approves it first.
pub const COLLATERAL_OFFRAMP_ADDRESS: &str = "0x2957922Eb93258b93368531d39fAcCA3B4dC5854";

/// USDC.e contract address on Polygon mainnet (PoS bridged): what wallets are
/// funded with and withdrawals pay out, wrapped into pUSD to trade.
pub const USDC_ADDRESS: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// CTF (Conditional Token Framework) ERC-1155 contract on Polygon mainnet.
pub const CTF_ADDRESS: &str = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045";

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
            version: EXCHANGE_DOMAIN_VERSION.to_string(),
            chain_id: U256::from(POLYGON_CHAIN_ID),
            verifying_contract: CTF_EXCHANGE_ADDRESS.parse().expect("Invalid CTF address"),
        }
    }

    /// Create domain for Neg Risk CTF Exchange on Polygon mainnet.
    pub fn neg_risk_ctf_exchange() -> Self {
        Self {
            name: "Polymarket CTF Exchange".to_string(),
            version: EXCHANGE_DOMAIN_VERSION.to_string(),
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

        // EIP-712: address must be left-padded to 32 bytes
        let contract_padded = B256::left_padding_from(self.verifying_contract.as_slice());

        let encoded = (
            domain_type_hash,
            name_hash,
            version_hash,
            self.chain_id,
            contract_padded,
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
    /// ECDSA signature of an EOA that is also the order's maker.
    #[default]
    Eoa = 0,
    /// Signed by an EOA that owns a Polymarket proxy wallet.
    PolyProxy = 1,
    /// Signed by an EOA that owns a Polymarket Gnosis safe.
    PolyGnosisSafe = 2,
    /// EIP-1271 signature of a smart contract wallet.
    Poly1271 = 3,
}

impl SignatureType {
    /// Get the numeric value for signing.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ctf_exchange_domain() {
        let domain = Eip712Domain::ctf_exchange();
        assert_eq!(domain.name, "Polymarket CTF Exchange");
        assert_eq!(domain.version, "2");
        assert_eq!(domain.chain_id, U256::from(137u64));
        assert_eq!(
            domain.verifying_contract,
            CTF_EXCHANGE_ADDRESS.parse::<Address>().unwrap()
        );
    }

    #[test]
    fn domain_separators_match_an_independent_eip712_implementation() {
        for (domain, contract) in [
            (Eip712Domain::ctf_exchange(), CTF_EXCHANGE_ADDRESS),
            (
                Eip712Domain::neg_risk_ctf_exchange(),
                NEG_RISK_CTF_EXCHANGE_ADDRESS,
            ),
        ] {
            let reference = alloy_sol_types::eip712_domain! {
                name: "Polymarket CTF Exchange",
                version: "2",
                chain_id: 137,
                verifying_contract: contract.parse::<Address>().unwrap(),
            };
            assert_eq!(domain.separator(), reference.separator());
        }
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
        assert_eq!(SignatureType::PolyProxy.as_u8(), 1);
        assert_eq!(SignatureType::PolyGnosisSafe.as_u8(), 2);
        assert_eq!(SignatureType::Poly1271.as_u8(), 3);
    }
}
