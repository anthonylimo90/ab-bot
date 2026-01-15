//! API key authentication for programmatic access.

use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::jwt::UserRole;

/// An API key for programmatic access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: Uuid,
    /// User this key belongs to.
    pub user_id: String,
    /// Display name for the key.
    pub name: String,
    /// Hash of the key (actual key is only shown once at creation).
    pub key_hash: String,
    /// Key prefix for identification (first 8 chars).
    pub key_prefix: String,
    /// Role/permissions for this key.
    pub role: UserRole,
    /// When the key was created.
    pub created_at: DateTime<Utc>,
    /// When the key expires (if any).
    pub expires_at: Option<DateTime<Utc>>,
    /// Last time the key was used.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Whether the key is active.
    pub active: bool,
}

impl ApiKey {
    /// Create a new API key and return the plain text key (shown once).
    pub fn new(user_id: String, name: String, role: UserRole) -> (Self, String) {
        let plain_key = Self::generate_key();
        let key_hash = Self::hash_key(&plain_key);
        let key_prefix = plain_key[..8].to_string();

        let api_key = Self {
            id: Uuid::new_v4(),
            user_id,
            name,
            key_hash,
            key_prefix,
            role,
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            active: true,
        };

        (api_key, plain_key)
    }

    /// Set expiration time.
    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Check if the key is valid (active and not expired).
    pub fn is_valid(&self) -> bool {
        if !self.active {
            return false;
        }

        if let Some(expires) = self.expires_at {
            if Utc::now() > expires {
                return false;
            }
        }

        true
    }

    /// Verify a plain text key against this API key.
    pub fn verify(&self, plain_key: &str) -> bool {
        let hash = Self::hash_key(plain_key);
        self.key_hash == hash
    }

    /// Update last used timestamp.
    pub fn touch(&mut self) {
        self.last_used_at = Some(Utc::now());
    }

    /// Deactivate the key.
    pub fn deactivate(&mut self) {
        self.active = false;
    }

    fn generate_key() -> String {
        use rand::distributions::Alphanumeric;
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(48)
            .map(char::from)
            .collect()
    }

    fn hash_key(key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// API key authentication handler.
pub struct ApiKeyAuth {
    /// Keys indexed by hash for fast lookup.
    keys_by_hash: Arc<RwLock<HashMap<String, ApiKey>>>,
    /// Keys indexed by user for management.
    keys_by_user: Arc<RwLock<HashMap<String, Vec<Uuid>>>>,
}

impl ApiKeyAuth {
    /// Create a new API key authenticator.
    pub fn new() -> Self {
        Self {
            keys_by_hash: Arc::new(RwLock::new(HashMap::new())),
            keys_by_user: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a new API key for a user.
    pub async fn create_key(&self, user_id: &str, name: &str, role: UserRole) -> (ApiKey, String) {
        let (api_key, plain_key) = ApiKey::new(user_id.to_string(), name.to_string(), role);

        // Store by hash
        {
            let mut keys = self.keys_by_hash.write().await;
            keys.insert(api_key.key_hash.clone(), api_key.clone());
        }

        // Index by user
        {
            let mut user_keys = self.keys_by_user.write().await;
            user_keys
                .entry(user_id.to_string())
                .or_default()
                .push(api_key.id);
        }

        info!(
            user = %user_id,
            key_id = %api_key.id,
            prefix = %api_key.key_prefix,
            "Created new API key"
        );

        (api_key, plain_key)
    }

    /// Authenticate with an API key.
    pub async fn authenticate(&self, plain_key: &str) -> Option<ApiKey> {
        let hash = ApiKey::hash_key(plain_key);

        let mut keys = self.keys_by_hash.write().await;

        if let Some(key) = keys.get_mut(&hash) {
            if key.is_valid() && key.verify(plain_key) {
                key.touch();
                debug!(
                    key_prefix = %key.key_prefix,
                    user = %key.user_id,
                    "API key authenticated"
                );
                return Some(key.clone());
            }
        }

        warn!("API key authentication failed");
        None
    }

    /// Get all keys for a user.
    pub async fn get_user_keys(&self, user_id: &str) -> Vec<ApiKey> {
        let user_keys = self.keys_by_user.read().await;
        let keys = self.keys_by_hash.read().await;

        user_keys
            .get(user_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| keys.values().find(|k| &k.id == id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Revoke an API key.
    pub async fn revoke_key(&self, key_id: Uuid) -> bool {
        let mut keys = self.keys_by_hash.write().await;

        for key in keys.values_mut() {
            if key.id == key_id {
                key.deactivate();
                info!(key_id = %key_id, "API key revoked");
                return true;
            }
        }

        false
    }

    /// Delete an API key.
    pub async fn delete_key(&self, key_id: Uuid, user_id: &str) -> bool {
        // Remove from hash index
        let mut keys = self.keys_by_hash.write().await;
        let hash_to_remove = keys
            .iter()
            .find(|(_, k)| k.id == key_id)
            .map(|(h, _)| h.clone());

        if let Some(hash) = hash_to_remove {
            keys.remove(&hash);
        } else {
            return false;
        }

        drop(keys);

        // Remove from user index
        let mut user_keys = self.keys_by_user.write().await;
        if let Some(ids) = user_keys.get_mut(user_id) {
            ids.retain(|id| *id != key_id);
        }

        info!(key_id = %key_id, user = %user_id, "API key deleted");
        true
    }

    /// Count active keys for a user.
    pub async fn count_active_keys(&self, user_id: &str) -> usize {
        self.get_user_keys(user_id)
            .await
            .iter()
            .filter(|k| k.is_valid())
            .count()
    }
}

impl Default for ApiKeyAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_authenticate() {
        let auth = ApiKeyAuth::new();

        let (api_key, plain_key) = auth.create_key("user1", "Test Key", UserRole::Trader).await;

        assert_eq!(api_key.name, "Test Key");
        assert_eq!(api_key.role, UserRole::Trader);
        assert!(api_key.active);

        // Authenticate with the plain key
        let result = auth.authenticate(&plain_key).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, api_key.id);
    }

    #[tokio::test]
    async fn test_invalid_key_rejected() {
        let auth = ApiKeyAuth::new();

        auth.create_key("user1", "Test Key", UserRole::Trader).await;

        let result = auth.authenticate("invalid-key").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_revoked_key_rejected() {
        let auth = ApiKeyAuth::new();

        let (api_key, plain_key) = auth.create_key("user1", "Test Key", UserRole::Trader).await;

        // Revoke the key
        auth.revoke_key(api_key.id).await;

        // Should fail authentication
        let result = auth.authenticate(&plain_key).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_expired_key_rejected() {
        let auth = ApiKeyAuth::new();

        let (mut api_key, plain_key) = ApiKey::new(
            "user1".to_string(),
            "Test Key".to_string(),
            UserRole::Trader,
        );

        // Set expiry in the past
        api_key.expires_at = Some(Utc::now() - chrono::Duration::hours(1));

        // Manually store
        {
            let mut keys = auth.keys_by_hash.write().await;
            keys.insert(api_key.key_hash.clone(), api_key);
        }

        // Should fail authentication
        let result = auth.authenticate(&plain_key).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_user_keys() {
        let auth = ApiKeyAuth::new();

        auth.create_key("user1", "Key 1", UserRole::Viewer).await;
        auth.create_key("user1", "Key 2", UserRole::Trader).await;
        auth.create_key("user2", "Key 3", UserRole::Admin).await;

        let user1_keys = auth.get_user_keys("user1").await;
        assert_eq!(user1_keys.len(), 2);

        let user2_keys = auth.get_user_keys("user2").await;
        assert_eq!(user2_keys.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_key() {
        let auth = ApiKeyAuth::new();

        let (api_key, _) = auth.create_key("user1", "Test Key", UserRole::Trader).await;

        assert_eq!(auth.count_active_keys("user1").await, 1);

        auth.delete_key(api_key.id, "user1").await;

        assert_eq!(auth.count_active_keys("user1").await, 0);
    }
}
