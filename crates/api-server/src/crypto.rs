//! Application-level field encryption for sensitive data stored in the database.
//!
//! Uses AES-256-GCM with a 12-byte random nonce prepended to the ciphertext.
//! The combined (nonce || ciphertext) is base64-encoded for storage in VARCHAR columns.
//!
//! Key is derived by SHA-256 hashing the `ENCRYPTION_KEY` environment variable
//! (or falling back to `JWT_SECRET`); see [`FieldKeys`].

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use sha2::{Digest, Sha256};

/// Derives a 256-bit AES key from the provided secret string.
fn derive_key(secret: &str) -> Key<Aes256Gcm> {
    let hash = Sha256::digest(secret.as_bytes());
    *Key::<Aes256Gcm>::from_slice(&hash)
}

/// Encrypts a plaintext string and returns a base64-encoded (nonce || ciphertext).
///
/// Returns `None` if encryption fails (should not happen with valid inputs).
fn encrypt_field(plaintext: &str, encryption_key: &str) -> Option<String> {
    let key = derive_key(encryption_key);
    let cipher = Aes256Gcm::new(&key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher.encrypt(&nonce, plaintext.as_bytes()).ok()?;

    // Prepend nonce (12 bytes) to ciphertext
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);

    Some(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &combined,
    ))
}

/// Decrypts a base64-encoded (nonce || ciphertext) back to plaintext.
///
/// Returns `None` if decryption fails (wrong key, corrupted data, or plaintext value).
fn decrypt_field(encoded: &str, encryption_key: &str) -> Option<String> {
    use base64::Engine;
    let combined = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;

    // Nonce is 12 bytes
    if combined.len() < 13 {
        return None;
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let key = derive_key(encryption_key);
    let cipher = Aes256Gcm::new(&key);

    let plaintext = cipher.decrypt(nonce, ciphertext).ok()?;
    String::from_utf8(plaintext).ok()
}

/// The keys stored secrets are encrypted and read with. Secrets saved before
/// `ENCRYPTION_KEY` was set were encrypted under `JWT_SECRET`; they are
/// still read with it, so setting the key later does not lose them.
#[derive(Clone)]
pub struct FieldKeys {
    current: String,
    previous: Option<String>,
}

impl FieldKeys {
    /// `ENCRYPTION_KEY` when set, else `JWT_SECRET`; with the first, the
    /// JWT secret is kept for reading older secrets.
    pub fn new(encryption_key: Option<String>, jwt_secret: &str) -> Self {
        match encryption_key {
            Some(key) if key != jwt_secret => Self {
                current: key,
                previous: Some(jwt_secret.to_string()),
            },
            _ => Self {
                current: jwt_secret.to_string(),
                previous: None,
            },
        }
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    pub fn encrypt(&self, plaintext: &str) -> Option<String> {
        encrypt_field(plaintext, &self.current)
    }

    /// The plaintext of a stored secret. A value saved before encryption is
    /// returned as it is; one that is encrypted but under neither key is
    /// `None`, never its ciphertext passed off as the secret. Each case is
    /// logged once per process: secrets are read on every snapshot.
    pub fn read_secret(&self, stored: &str) -> Option<String> {
        use std::sync::atomic::{AtomicBool, Ordering};
        static UNDER_PREVIOUS_KEY_REPORTED: AtomicBool = AtomicBool::new(false);
        static UNREADABLE_REPORTED: AtomicBool = AtomicBool::new(false);

        if let Some(plaintext) = decrypt_field(stored, &self.current) {
            return Some(plaintext);
        }
        if let Some(plaintext) = self
            .previous
            .as_deref()
            .and_then(|previous| decrypt_field(stored, previous))
        {
            if !UNDER_PREVIOUS_KEY_REPORTED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "A saved workspace secret is encrypted under JWT_SECRET, not ENCRYPTION_KEY; \
                     save it again to encrypt it under ENCRYPTION_KEY"
                );
            }
            return Some(plaintext);
        }
        if looks_encrypted(stored) {
            if !UNREADABLE_REPORTED.swap(true, Ordering::Relaxed) {
                tracing::error!(
                    "A saved workspace secret cannot be decrypted with ENCRYPTION_KEY or \
                     JWT_SECRET; enter it again"
                );
            }
            return None;
        }
        Some(stored.to_string())
    }
}

/// Whether a stored value has the shape `encrypt_field` writes: base64 of a
/// nonce, at least one byte and the tag. API keys saved in plaintext (32
/// characters at most, often with `-` or `_`) do not.
fn looks_encrypted(stored: &str) -> bool {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(stored)
        .is_ok_and(|bytes| bytes.len() > 12 + 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_saved_under_the_jwt_secret_are_read_after_a_key_is_set() {
        let before = FieldKeys::new(None, "jwt-secret");
        let saved = before.encrypt("alchemy-key").unwrap();

        let after = FieldKeys::new(Some("encryption-key".into()), "jwt-secret");
        assert_eq!(after.read_secret(&saved).as_deref(), Some("alchemy-key"));
        let resaved = after.encrypt("alchemy-key").unwrap();
        assert_eq!(after.read_secret(&resaved).as_deref(), Some("alchemy-key"));
    }

    #[test]
    fn a_secret_under_an_unknown_key_is_not_passed_off_as_plaintext() {
        let saved = encrypt_field("alchemy-key", "lost-key").unwrap();
        let keys = FieldKeys::new(Some("encryption-key".into()), "jwt-secret");
        assert_eq!(keys.read_secret(&saved), None);
        // A plaintext key saved before encryption is read as it is.
        for plaintext in ["abcdEFGHijklMNOPqrstUVWXyz012345", "abc-def_ghi"] {
            assert_eq!(keys.read_secret(plaintext).as_deref(), Some(plaintext));
        }
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = "test-encryption-key-at-least-32-chars";
        let plaintext = "sk_test_abc123xyz";

        let encrypted = encrypt_field(plaintext, key).unwrap();
        assert_ne!(encrypted, plaintext);

        let decrypted = decrypt_field(&encrypted, key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_wrong_key_fails() {
        let plaintext = "secret-api-key";
        let encrypted = encrypt_field(plaintext, "key1").unwrap();
        let result = decrypt_field(&encrypted, "key2");
        assert!(result.is_none());
    }

    #[test]
    fn test_plaintext_value_returns_none() {
        // A non-base64 string (e.g., an existing plaintext key) should return None
        let result = decrypt_field("not-encrypted-value", "some-key");
        assert!(result.is_none());
    }

    #[test]
    fn test_different_encryptions_produce_different_ciphertext() {
        let key = "test-key";
        let plaintext = "same-value";
        let enc1 = encrypt_field(plaintext, key).unwrap();
        let enc2 = encrypt_field(plaintext, key).unwrap();
        // Random nonce means different ciphertext each time
        assert_ne!(enc1, enc2);
        // But both decrypt to the same value
        assert_eq!(decrypt_field(&enc1, key).unwrap(), plaintext);
        assert_eq!(decrypt_field(&enc2, key).unwrap(), plaintext);
    }
}
