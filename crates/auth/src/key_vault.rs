//! Secure key vault for wallet keys and secrets.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// AES-GCM nonce size (96 bits / 12 bytes as recommended).
const NONCE_SIZE: usize = 12;

/// A wallet private key (securely stored).
#[derive(Clone)]
pub struct WalletKey {
    /// Wallet address.
    pub address: String,
    /// Encrypted private key (includes nonce prefix).
    encrypted_key: Vec<u8>,
    /// Key derivation salt.
    salt: Vec<u8>,
}

impl WalletKey {
    /// Create a new wallet key (encrypts the private key using AES-256-GCM).
    pub fn new(address: String, private_key: &[u8], encryption_key: &[u8]) -> Result<Self> {
        use rand::Rng;

        // Generate random salt for key derivation
        let mut salt = [0u8; 32];
        rand::thread_rng().fill(&mut salt);

        // Generate random nonce for AES-GCM
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::thread_rng().fill(&mut nonce_bytes);

        let encrypted = Self::encrypt(private_key, encryption_key, &salt, &nonce_bytes)?;

        Ok(Self {
            address,
            encrypted_key: encrypted,
            salt: salt.to_vec(),
        })
    }

    /// Decrypt and get the private key.
    pub fn decrypt(&self, encryption_key: &[u8]) -> Result<Vec<u8>> {
        Self::decrypt_data(&self.encrypted_key, encryption_key, &self.salt)
    }

    /// Encrypt data using AES-256-GCM.
    /// The nonce is prepended to the ciphertext.
    fn encrypt(
        data: &[u8],
        key: &[u8],
        salt: &[u8],
        nonce_bytes: &[u8; NONCE_SIZE],
    ) -> Result<Vec<u8>> {
        let derived_key = Self::derive_key(key, salt);

        let cipher = Aes256Gcm::new_from_slice(&derived_key)
            .map_err(|e| anyhow!("Failed to create AES-GCM cipher: {}", e))?;

        let nonce = Nonce::from_slice(nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, data)
            .map_err(|e| anyhow!("AES-GCM encryption failed: {}", e))?;

        // Prepend nonce to ciphertext for storage
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// Decrypt data using AES-256-GCM.
    /// Expects nonce to be prepended to the ciphertext.
    fn decrypt_data(encrypted: &[u8], key: &[u8], salt: &[u8]) -> Result<Vec<u8>> {
        if encrypted.len() < NONCE_SIZE {
            return Err(anyhow!("Encrypted data too short"));
        }

        let derived_key = Self::derive_key(key, salt);

        let cipher = Aes256Gcm::new_from_slice(&derived_key)
            .map_err(|e| anyhow!("Failed to create AES-GCM cipher: {}", e))?;

        // Extract nonce from the beginning
        let nonce = Nonce::from_slice(&encrypted[..NONCE_SIZE]);
        let ciphertext = &encrypted[NONCE_SIZE..];

        cipher.decrypt(nonce, ciphertext).map_err(|e| {
            anyhow!(
                "AES-GCM decryption failed (wrong key or corrupted data): {}",
                e
            )
        })
    }

    /// Derive a 256-bit key from the master key and salt using SHA-256.
    fn derive_key(key: &[u8], salt: &[u8]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(key);
        hasher.update(salt);
        hasher.finalize().to_vec()
    }
}

/// Provider for key storage backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum KeyVaultProvider {
    /// Store keys in environment variables (development only).
    Environment,
    /// Store keys in encrypted file.
    EncryptedFile { path: PathBuf },
    /// Store keys in memory (testing only).
    Memory,
    /// AWS Secrets Manager (production).
    #[serde(rename = "aws")]
    AwsSecretsManager { region: String },
    /// HashiCorp Vault.
    HashicorpVault { address: String },
}

impl Default for KeyVaultProvider {
    fn default() -> Self {
        Self::Memory
    }
}

/// Secure vault for storing wallet keys and secrets.
pub struct KeyVault {
    provider: KeyVaultProvider,
    /// Master encryption key (from environment or secure source).
    master_key: Vec<u8>,
    /// In-memory cache of loaded keys.
    cache: Arc<RwLock<HashMap<String, WalletKey>>>,
}

impl KeyVault {
    /// Create a new key vault.
    pub fn new(provider: KeyVaultProvider, master_key: Vec<u8>) -> Self {
        Self {
            provider,
            master_key,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a key vault from environment configuration.
    pub fn from_env() -> Result<Self> {
        let master_key = std::env::var("VAULT_MASTER_KEY")
            .context("VAULT_MASTER_KEY environment variable not set")?;

        let provider = match std::env::var("VAULT_PROVIDER").as_deref() {
            Ok("aws") => {
                let region =
                    std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
                KeyVaultProvider::AwsSecretsManager { region }
            }
            Ok("file") => {
                let path = std::env::var("VAULT_FILE_PATH")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("./vault.enc"));
                KeyVaultProvider::EncryptedFile { path }
            }
            _ => KeyVaultProvider::Environment,
        };

        Ok(Self::new(provider, master_key.into_bytes()))
    }

    /// Store a wallet key.
    pub async fn store_wallet_key(&self, address: &str, private_key: &[u8]) -> Result<()> {
        let wallet_key = WalletKey::new(address.to_string(), private_key, &self.master_key)?;

        match &self.provider {
            KeyVaultProvider::Environment => {
                warn!("Storing key in environment is not persistent");
            }
            KeyVaultProvider::EncryptedFile { path } => {
                self.store_to_file(path, address, &wallet_key).await?;
            }
            KeyVaultProvider::Memory => {
                debug!("Storing key in memory");
            }
            KeyVaultProvider::AwsSecretsManager { region: _ } => {
                // TODO: Implement AWS Secrets Manager storage
                warn!("AWS Secrets Manager not yet implemented, using memory");
            }
            KeyVaultProvider::HashicorpVault { address: _ } => {
                // TODO: Implement HashiCorp Vault storage
                warn!("HashiCorp Vault not yet implemented, using memory");
            }
        }

        // Cache the key
        let mut cache = self.cache.write().await;
        cache.insert(address.to_lowercase(), wallet_key);

        info!(address = %address, "Wallet key stored");
        Ok(())
    }

    /// Retrieve a wallet key.
    pub async fn get_wallet_key(&self, address: &str) -> Result<Option<Vec<u8>>> {
        let address_lower = address.to_lowercase();

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(key) = cache.get(&address_lower) {
                return Ok(Some(key.decrypt(&self.master_key)?));
            }
        }

        // Load from provider
        let wallet_key = match &self.provider {
            KeyVaultProvider::Environment => self.load_from_env(&address_lower).await?,
            KeyVaultProvider::EncryptedFile { path } => {
                self.load_from_file(path, &address_lower).await?
            }
            KeyVaultProvider::Memory => None,
            KeyVaultProvider::AwsSecretsManager { region: _ } => {
                // TODO: Implement AWS Secrets Manager retrieval
                None
            }
            KeyVaultProvider::HashicorpVault { address: _ } => {
                // TODO: Implement HashiCorp Vault retrieval
                None
            }
        };

        if let Some(key) = wallet_key {
            let decrypted = key.decrypt(&self.master_key)?;

            // Cache for future use
            let mut cache = self.cache.write().await;
            cache.insert(address_lower, key);

            Ok(Some(decrypted))
        } else {
            Ok(None)
        }
    }

    /// Check if a wallet key exists.
    pub async fn has_wallet_key(&self, address: &str) -> bool {
        let address_lower = address.to_lowercase();

        let cache = self.cache.read().await;
        cache.contains_key(&address_lower)
    }

    /// Remove a wallet key.
    pub async fn remove_wallet_key(&self, address: &str) -> Result<bool> {
        let address_lower = address.to_lowercase();

        let mut cache = self.cache.write().await;
        let removed = cache.remove(&address_lower).is_some();

        if removed {
            info!(address = %address, "Wallet key removed");
        }

        Ok(removed)
    }

    /// List all stored wallet addresses.
    pub async fn list_wallet_addresses(&self) -> Vec<String> {
        let cache = self.cache.read().await;
        cache.keys().cloned().collect()
    }

    /// Clear all cached keys (does not remove from persistent storage).
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
        info!("Key vault cache cleared");
    }

    // Private methods

    async fn load_from_env(&self, address: &str) -> Result<Option<WalletKey>> {
        let env_key = format!("WALLET_KEY_{}", address.replace("0x", "").to_uppercase());

        if let Ok(hex_key) = std::env::var(&env_key) {
            let key_bytes = hex::decode(hex_key.trim_start_matches("0x"))
                .context("Invalid hex in wallet key env var")?;
            let wallet_key = WalletKey::new(address.to_string(), &key_bytes, &self.master_key)?;
            Ok(Some(wallet_key))
        } else {
            Ok(None)
        }
    }

    async fn load_from_file(&self, path: &PathBuf, address: &str) -> Result<Option<WalletKey>> {
        if !path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(path).await?;
        let keys: HashMap<String, StoredKey> = serde_json::from_str(&content)?;

        if let Some(stored) = keys.get(address) {
            let wallet_key = WalletKey {
                address: address.to_string(),
                encrypted_key: base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &stored.encrypted_key,
                )?,
                salt: base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    &stored.salt,
                )?,
            };
            Ok(Some(wallet_key))
        } else {
            Ok(None)
        }
    }

    async fn store_to_file(&self, path: &PathBuf, address: &str, key: &WalletKey) -> Result<()> {
        use base64::Engine;

        let mut keys: HashMap<String, StoredKey> = if path.exists() {
            let content = tokio::fs::read_to_string(path).await?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        keys.insert(
            address.to_lowercase(),
            StoredKey {
                encrypted_key: base64::engine::general_purpose::STANDARD.encode(&key.encrypted_key),
                salt: base64::engine::general_purpose::STANDARD.encode(&key.salt),
            },
        );

        let content = serde_json::to_string_pretty(&keys)?;
        tokio::fs::write(path, content).await?;

        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct StoredKey {
    encrypted_key: String,
    salt: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_retrieve_key() {
        let vault = KeyVault::new(KeyVaultProvider::Memory, b"test-master-key".to_vec());

        let private_key = b"private-key-12345";
        vault
            .store_wallet_key("0x1234abcd", private_key)
            .await
            .unwrap();

        let retrieved = vault.get_wallet_key("0x1234abcd").await.unwrap();
        assert_eq!(retrieved, Some(private_key.to_vec()));
    }

    #[tokio::test]
    async fn test_case_insensitive_address() {
        let vault = KeyVault::new(KeyVaultProvider::Memory, b"test-master-key".to_vec());

        let private_key = b"private-key-12345";
        vault
            .store_wallet_key("0xABCD1234", private_key)
            .await
            .unwrap();

        // Should find with different case
        let retrieved = vault.get_wallet_key("0xabcd1234").await.unwrap();
        assert_eq!(retrieved, Some(private_key.to_vec()));
    }

    #[tokio::test]
    async fn test_remove_key() {
        let vault = KeyVault::new(KeyVaultProvider::Memory, b"test-master-key".to_vec());

        vault.store_wallet_key("0x1234", b"key").await.unwrap();
        assert!(vault.has_wallet_key("0x1234").await);

        vault.remove_wallet_key("0x1234").await.unwrap();
        assert!(!vault.has_wallet_key("0x1234").await);
    }

    #[tokio::test]
    async fn test_list_addresses() {
        let vault = KeyVault::new(KeyVaultProvider::Memory, b"test-master-key".to_vec());

        vault.store_wallet_key("0xAAA", b"key1").await.unwrap();
        vault.store_wallet_key("0xBBB", b"key2").await.unwrap();

        let addresses = vault.list_wallet_addresses().await;
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&"0xaaa".to_string()));
        assert!(addresses.contains(&"0xbbb".to_string()));
    }

    #[test]
    fn test_wallet_key_encryption() {
        let master_key = b"master-key-12345";
        let private_key = b"super-secret-private-key";

        let wallet_key = WalletKey::new("0x1234".to_string(), private_key, master_key).unwrap();

        // Encrypted key should be different from original (includes nonce + ciphertext + auth tag)
        assert_ne!(wallet_key.encrypted_key, private_key);

        // Encrypted data should be longer than original (nonce + auth tag overhead)
        assert!(wallet_key.encrypted_key.len() > private_key.len());

        // Decryption should return original
        let decrypted = wallet_key.decrypt(master_key).unwrap();
        assert_eq!(decrypted, private_key);

        // Wrong key should fail with authentication error (AES-GCM provides authenticated encryption)
        let wrong_result = wallet_key.decrypt(b"wrong-key");
        assert!(wrong_result.is_err());
        assert!(wrong_result
            .unwrap_err()
            .to_string()
            .contains("decryption failed"));
    }

    #[test]
    fn test_encryption_produces_different_ciphertext() {
        let master_key = b"master-key-12345";
        let private_key = b"same-private-key";

        // Encrypt the same key twice - should produce different ciphertext due to random nonce
        let wallet_key1 = WalletKey::new("0x1111".to_string(), private_key, master_key).unwrap();
        let wallet_key2 = WalletKey::new("0x2222".to_string(), private_key, master_key).unwrap();

        // Ciphertexts should be different (different nonces)
        assert_ne!(wallet_key1.encrypted_key, wallet_key2.encrypted_key);

        // But both should decrypt to the same plaintext
        let decrypted1 = wallet_key1.decrypt(master_key).unwrap();
        let decrypted2 = wallet_key2.decrypt(master_key).unwrap();
        assert_eq!(decrypted1, decrypted2);
        assert_eq!(decrypted1, private_key);
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let master_key = b"master-key-12345";
        let private_key = b"super-secret-private-key";

        let mut wallet_key = WalletKey::new("0x1234".to_string(), private_key, master_key).unwrap();

        // Tamper with the ciphertext
        if let Some(byte) = wallet_key.encrypted_key.last_mut() {
            *byte ^= 0xFF;
        }

        // Decryption should fail due to authentication tag mismatch
        let result = wallet_key.decrypt(master_key);
        assert!(result.is_err());
    }
}
