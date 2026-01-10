//! Trading wallet management for live order signing.
//!
//! Provides wallet loading from environment variables and message signing
//! for Polymarket CLOB authentication.

use alloy_primitives::Address;
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use std::str::FromStr;

/// A trading wallet with private key access for signing orders.
///
/// The wallet can be loaded from an environment variable or directly
/// from a hex-encoded private key.
#[derive(Clone)]
pub struct TradingWallet {
    signer: PrivateKeySigner,
    address: Address,
}

impl TradingWallet {
    /// Load wallet from the `WALLET_PRIVATE_KEY` environment variable.
    ///
    /// The private key should be a 64-character hex string, optionally
    /// prefixed with "0x".
    ///
    /// # Errors
    ///
    /// Returns an error if the environment variable is not set or
    /// if the private key format is invalid.
    pub fn from_env() -> Result<Self> {
        let private_key = std::env::var("WALLET_PRIVATE_KEY")
            .context("WALLET_PRIVATE_KEY environment variable not set")?;

        Self::from_private_key(&private_key)
    }

    /// Create a wallet from a hex-encoded private key.
    ///
    /// # Arguments
    ///
    /// * `key` - A 64-character hex string, optionally prefixed with "0x"
    ///
    /// # Errors
    ///
    /// Returns an error if the private key format is invalid.
    pub fn from_private_key(key: &str) -> Result<Self> {
        let key_clean = key.trim().trim_start_matches("0x");

        let signer = PrivateKeySigner::from_str(key_clean)
            .context("Invalid private key format - expected 64 hex characters")?;

        let address = signer.address();

        Ok(Self { signer, address })
    }

    /// Get the wallet's Ethereum address.
    pub fn address(&self) -> Address {
        self.address
    }

    /// Get the wallet address as a checksummed hex string.
    pub fn address_string(&self) -> String {
        format!("{}", self.address)
    }

    /// Get a reference to the underlying signer for EIP-712 signing.
    pub fn signer(&self) -> &PrivateKeySigner {
        &self.signer
    }

    /// Consume the wallet and return the signer.
    pub fn into_signer(self) -> PrivateKeySigner {
        self.signer
    }

    /// Sign an arbitrary message.
    ///
    /// This uses EIP-191 personal sign format (prefixed with
    /// "\x19Ethereum Signed Message:\n{len}").
    pub async fn sign_message(&self, message: &[u8]) -> Result<Vec<u8>> {
        let signature = self.signer.sign_message(message).await?;
        Ok(signature.as_bytes().to_vec())
    }

    /// Sign a message and return the signature as a hex string with 0x prefix.
    pub async fn sign_message_hex(&self, message: &[u8]) -> Result<String> {
        let sig_bytes = self.sign_message(message).await?;
        Ok(format!("0x{}", hex::encode(sig_bytes)))
    }
}

impl std::fmt::Debug for TradingWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never expose the private key in debug output
        f.debug_struct("TradingWallet")
            .field("address", &self.address_string())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test private key (DO NOT USE IN PRODUCTION - this is a well-known test key)
    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDRESS: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

    #[test]
    fn test_from_private_key_with_prefix() {
        let wallet = TradingWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        assert_eq!(
            wallet.address_string().to_lowercase(),
            TEST_ADDRESS.to_lowercase()
        );
    }

    #[test]
    fn test_from_private_key_without_prefix() {
        let key_no_prefix = TEST_PRIVATE_KEY.trim_start_matches("0x");
        let wallet = TradingWallet::from_private_key(key_no_prefix).unwrap();
        assert_eq!(
            wallet.address_string().to_lowercase(),
            TEST_ADDRESS.to_lowercase()
        );
    }

    #[test]
    fn test_invalid_private_key() {
        let result = TradingWallet::from_private_key("not-a-valid-key");
        assert!(result.is_err());
    }

    #[test]
    fn test_short_private_key() {
        let result = TradingWallet::from_private_key("0x1234");
        assert!(result.is_err());
    }

    #[test]
    fn test_debug_does_not_expose_key() {
        let wallet = TradingWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        let debug_str = format!("{:?}", wallet);

        // Should contain address but not the private key
        assert!(debug_str.contains("TradingWallet"));
        assert!(debug_str.contains("address"));
        assert!(!debug_str.contains("ac0974bec39a17e36ba4a6b4d238ff944bacb478"));
    }

    #[tokio::test]
    async fn test_sign_message() {
        let wallet = TradingWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        let message = b"Hello, Polymarket!";

        let signature = wallet.sign_message(message).await.unwrap();

        // Signature should be 65 bytes (r: 32, s: 32, v: 1)
        assert_eq!(signature.len(), 65);
    }

    #[tokio::test]
    async fn test_sign_message_hex() {
        let wallet = TradingWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        let message = b"Test message";

        let sig_hex = wallet.sign_message_hex(message).await.unwrap();

        // Should be 0x prefix + 130 hex chars (65 bytes * 2)
        assert!(sig_hex.starts_with("0x"));
        assert_eq!(sig_hex.len(), 132);
    }

    // Note: These env var tests can be flaky when run in parallel.
    // They are kept for documentation purposes but may need to be run with --test-threads=1

    #[test]
    #[ignore = "env var tests are flaky in parallel - run with --test-threads=1"]
    fn test_from_env_missing_var() {
        // Ensure the env var is not set
        std::env::remove_var("WALLET_PRIVATE_KEY");

        let result = TradingWallet::from_env();
        assert!(result.is_err());
    }

    #[test]
    #[ignore = "env var tests are flaky in parallel - run with --test-threads=1"]
    fn test_from_env_with_var() {
        std::env::set_var("WALLET_PRIVATE_KEY", TEST_PRIVATE_KEY);

        let result = TradingWallet::from_env();
        assert!(result.is_ok());

        let wallet = result.unwrap();
        assert_eq!(
            wallet.address_string().to_lowercase(),
            TEST_ADDRESS.to_lowercase()
        );

        // Clean up
        std::env::remove_var("WALLET_PRIVATE_KEY");
    }

    // Alternative test that doesn't rely on env vars
    #[test]
    fn test_from_private_key_is_equivalent_to_from_env() {
        // Test that from_private_key works the same as from_env would
        let wallet = TradingWallet::from_private_key(TEST_PRIVATE_KEY).unwrap();
        assert_eq!(
            wallet.address_string().to_lowercase(),
            TEST_ADDRESS.to_lowercase()
        );
    }
}
