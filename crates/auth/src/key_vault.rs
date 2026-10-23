//! Secure key vault for wallet keys and secrets.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

    /// Derive a 256-bit key from the master key and salt using SHA-256. This
    /// is not a password hash: the master key must itself be a high-entropy
    /// secret (see [`KeyVault::from_env`]).
    fn derive_key(key: &[u8], salt: &[u8]) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(key);
        hasher.update(salt);
        hasher.finalize().to_vec()
    }
}

/// Provider for key storage backend.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum KeyVaultProvider {
    /// Read keys from `WALLET_KEY_<ADDRESS>` environment variables. Read-only:
    /// storing a key fails, because it would not survive a restart.
    Environment,
    /// Store keys in encrypted file.
    EncryptedFile { path: PathBuf },
    /// Store keys in memory (testing only).
    #[default]
    Memory,
    /// AWS Secrets Manager (production).
    #[serde(rename = "aws")]
    AwsSecretsManager { region: String },
    /// HashiCorp Vault.
    HashicorpVault { address: String },
}

/// Secure vault for storing wallet keys and secrets.
pub struct KeyVault {
    provider: KeyVaultProvider,
    /// Master encryption key (from environment or secure source).
    master_key: Vec<u8>,
    /// In-memory cache of loaded keys.
    cache: Arc<RwLock<HashMap<String, WalletKey>>>,
    /// Serializes changes to the stored keys in this process: the vault
    /// file's read-modify-write cycles together with the cache updates that
    /// go with them, so a load cannot cache a key a removal just erased.
    file_lock: tokio::sync::Mutex<()>,
}

impl KeyVault {
    /// Create a new key vault.
    pub fn new(provider: KeyVaultProvider, master_key: Vec<u8>) -> Self {
        Self {
            provider,
            master_key,
            cache: Arc::new(RwLock::new(HashMap::new())),
            file_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Create a key vault from environment configuration.
    pub fn from_env() -> Result<Self> {
        // A blank key (`VAULT_MASTER_KEY=` passed through empty) is no key:
        // keys encrypted under it would be readable by anyone with the file.
        let master_key = std::env::var("VAULT_MASTER_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .context("VAULT_MASTER_KEY environment variable not set")?;
        if master_key.len() < 32 {
            warn!(
                "VAULT_MASTER_KEY is shorter than 32 characters; use a random secret \
                 (e.g. openssl rand -base64 32)"
            );
        }

        let provider = match std::env::var("VAULT_PROVIDER").as_deref() {
            Ok("aws") => {
                let region =
                    std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
                KeyVaultProvider::AwsSecretsManager { region }
            }
            Ok("file") => {
                let path = std::env::var("VAULT_FILE_PATH")
                    .ok()
                    .filter(|path| !path.trim().is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("./vault.enc"));
                KeyVaultProvider::EncryptedFile { path }
            }
            _ => KeyVaultProvider::Environment,
        };

        Ok(Self::new(provider, master_key.into_bytes()))
    }

    /// Store a wallet key.
    pub async fn store_wallet_key(&self, address: &str, private_key: &[u8]) -> Result<()> {
        let wallet_key = WalletKey::new(address.to_string(), private_key, &self.master_key)?;

        let _guard = self.file_lock.lock().await;
        match &self.provider {
            KeyVaultProvider::Environment => {
                return Err(anyhow!(
                    "The environment key vault is read-only (keys come from WALLET_KEY_<ADDRESS> \
                     variables); set VAULT_PROVIDER=file and VAULT_MASTER_KEY to store keys"
                ));
            }
            KeyVaultProvider::EncryptedFile { path } => {
                self.store_to_file(path, &address.to_lowercase(), &wallet_key)
                    .await?;
            }
            KeyVaultProvider::Memory => {
                debug!("Storing key in memory");
            }
            KeyVaultProvider::AwsSecretsManager { region } => {
                return Err(anyhow!(
                    "AWS Secrets Manager storage not yet implemented (region: {}). \
                     Use 'file' or 'memory' provider instead.",
                    region
                ));
            }
            KeyVaultProvider::HashicorpVault { address } => {
                return Err(anyhow!(
                    "HashiCorp Vault storage not yet implemented (address: {}). \
                     Use 'file' or 'memory' provider instead.",
                    address
                ));
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

        // Load from provider, under the lock removals hold, and check the
        // cache again: another caller may have loaded it meanwhile.
        let _guard = self.file_lock.lock().await;
        if let Some(key) = self.cache.read().await.get(&address_lower) {
            return Ok(Some(key.decrypt(&self.master_key)?));
        }
        let wallet_key = match &self.provider {
            KeyVaultProvider::Environment => self.load_from_env(&address_lower).await?,
            KeyVaultProvider::EncryptedFile { path } => {
                self.load_from_file(path, &address_lower).await?
            }
            KeyVaultProvider::Memory => None,
            KeyVaultProvider::AwsSecretsManager { region } => {
                return Err(anyhow!(
                    "AWS Secrets Manager retrieval not yet implemented (region: {}). \
                     Use 'file' or 'memory' provider instead.",
                    region
                ));
            }
            KeyVaultProvider::HashicorpVault { address } => {
                return Err(anyhow!(
                    "HashiCorp Vault retrieval not yet implemented (address: {}). \
                     Use 'file' or 'memory' provider instead.",
                    address
                ));
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

    /// Check if a wallet key exists, in the cache or in persistent storage.
    pub async fn has_wallet_key(&self, address: &str) -> bool {
        matches!(self.get_wallet_key(address).await, Ok(Some(_)))
    }

    /// Remove a wallet key from persistent storage and the cache.
    pub async fn remove_wallet_key(&self, address: &str) -> Result<bool> {
        let address_lower = address.to_lowercase();

        let _guard = self.file_lock.lock().await;
        // Persistent storage first: a key that stays on disk would come back
        // on the next restart.
        let removed_from_file = match &self.provider {
            KeyVaultProvider::EncryptedFile { path } => {
                self.remove_from_file(path, &address_lower).await?
            }
            _ => false,
        };

        let mut cache = self.cache.write().await;
        let removed = cache.remove(&address_lower).is_some() || removed_from_file;

        if removed {
            info!(address = %address, "Wallet key removed");
        }

        Ok(removed)
    }

    /// List the wallet addresses whose keys are currently cached.
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

    async fn load_from_file(&self, path: &Path, address: &str) -> Result<Option<WalletKey>> {
        let keys = read_vault_file(path).await?;

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

    /// Called with `file_lock` held.
    async fn store_to_file(&self, path: &Path, address: &str, key: &WalletKey) -> Result<()> {
        use base64::Engine;

        // An unreadable vault is an error, never an empty one: rewriting it
        // would destroy every other key it holds.
        let mut keys = read_vault_file(path).await?;
        keys.insert(
            address.to_string(),
            StoredKey {
                encrypted_key: base64::engine::general_purpose::STANDARD.encode(&key.encrypted_key),
                salt: base64::engine::general_purpose::STANDARD.encode(&key.salt),
            },
        );
        write_vault_file(path, &keys).await
    }

    /// Called with `file_lock` held.
    async fn remove_from_file(&self, path: &Path, address: &str) -> Result<bool> {
        let mut keys = read_vault_file(path).await?;
        if keys.remove(address).is_none() {
            return Ok(false);
        }
        write_vault_file(path, &keys).await?;
        Ok(true)
    }
}

/// Read the vault file; empty when it does not exist yet.
async fn read_vault_file(path: &Path) -> Result<HashMap<String, StoredKey>> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read vault file {path:?}"))
        }
    };
    serde_json::from_str(&content)
        .with_context(|| format!("Vault file {path:?} is not a valid key store"))
}

/// Replace the vault file atomically: the new content is written to a
/// sibling file readable only by its owner, flushed, and renamed over the old
/// one, so a crash leaves either the old or the new vault, never a torn one.
async fn write_vault_file(path: &Path, keys: &HashMap<String, StoredKey>) -> Result<()> {
    let content = serde_json::to_vec_pretty(keys)?;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        use rand::Rng;
        use std::io::Write;

        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let name = path
            .file_name()
            .ok_or_else(|| anyhow!("Vault path {path:?} has no file name"))?
            .to_string_lossy()
            .into_owned();
        let tmp = dir.join(format!(
            ".{name}.{:016x}.tmp",
            rand::thread_rng().gen::<u64>()
        ));

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = (|| -> std::io::Result<()> {
            let mut file = options.open(&tmp)?;
            file.write_all(&content)?;
            file.sync_all()?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(error) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(error).with_context(|| format!("Failed to write vault file {path:?}"));
        }
        // The new vault is in place; flushing the directory only makes the
        // rename survive a power loss. Failing it must not report the write
        // as failed (callers would act as if the old vault were current).
        #[cfg(unix)]
        if let Err(error) = std::fs::File::open(&dir).and_then(|dir| dir.sync_all()) {
            warn!(%error, path = ?path, "Could not flush the vault directory after a write");
        }
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("Vault write task failed: {e}"))?
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

    fn temp_vault_path(name: &str) -> PathBuf {
        use rand::Rng;
        let dir = std::env::temp_dir().join(format!(
            "key-vault-test-{name}-{:016x}",
            rand::thread_rng().gen::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("vault.enc")
    }

    fn file_vault(path: &Path) -> KeyVault {
        KeyVault::new(
            KeyVaultProvider::EncryptedFile {
                path: path.to_path_buf(),
            },
            b"0123456789abcdef0123456789abcdef".to_vec(),
        )
    }

    #[tokio::test]
    async fn file_vault_persists_and_removes_keys_across_restarts() {
        let path = temp_vault_path("persist");
        let vault = file_vault(&path);
        vault.store_wallet_key("0xAAA", b"key-a").await.unwrap();
        vault.store_wallet_key("0xbbb", b"key-b").await.unwrap();

        // A fresh process (empty cache) sees both keys.
        let restarted = file_vault(&path);
        assert!(restarted.has_wallet_key("0xaaa").await);
        assert_eq!(
            restarted.get_wallet_key("0xBBB").await.unwrap(),
            Some(b"key-b".to_vec())
        );

        // Removal reaches the file, not just the cache, and spares other keys.
        assert!(restarted.remove_wallet_key("0xAAA").await.unwrap());
        let again = file_vault(&path);
        assert_eq!(again.get_wallet_key("0xaaa").await.unwrap(), None);
        assert!(!again.has_wallet_key("0xaaa").await);
        assert_eq!(
            again.get_wallet_key("0xbbb").await.unwrap(),
            Some(b"key-b".to_vec())
        );
        assert!(!again.remove_wallet_key("0xaaa").await.unwrap());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "vault file must be owner-only");
        }
        // No temporary files are left behind.
        let entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_load_racing_a_removal_never_brings_the_key_back() {
        let path = temp_vault_path("race");
        let vault = Arc::new(file_vault(&path));
        for _ in 0..50 {
            vault.store_wallet_key("0xaaa", b"key-a").await.unwrap();
            vault.clear_cache().await;
            let loader = vault.clone();
            let load = tokio::spawn(async move { loader.get_wallet_key("0xaaa").await });
            vault.remove_wallet_key("0xaaa").await.unwrap();
            load.await.unwrap().unwrap();
            // Whichever ran first, the removal leaves no copy behind.
            assert!(vault.list_wallet_addresses().await.is_empty());
            assert_eq!(vault.get_wallet_key("0xaaa").await.unwrap(), None);
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_vault_file_is_never_overwritten() {
        let path = temp_vault_path("corrupt");
        std::fs::write(&path, "{\"0xaaa\": {\"encrypted_key\": \"trunc").unwrap();
        let vault = file_vault(&path);

        assert!(vault.store_wallet_key("0xbbb", b"key-b").await.is_err());
        assert!(vault.remove_wallet_key("0xaaa").await.is_err());
        assert!(vault.get_wallet_key("0xaaa").await.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"0xaaa\": {\"encrypted_key\": \"trunc"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn the_environment_vault_refuses_keys_it_cannot_keep() {
        let vault = KeyVault::new(KeyVaultProvider::Environment, b"test-master-key".to_vec());
        assert!(vault.store_wallet_key("0x1234", b"key").await.is_err());
        assert!(!vault.has_wallet_key("0x1234").await);
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
