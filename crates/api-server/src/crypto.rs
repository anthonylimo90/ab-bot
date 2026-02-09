//! Application-level field encryption for sensitive data stored in the database.
//!
//! Uses AES-256-GCM with a 12-byte random nonce prepended to the ciphertext.
//! The combined (nonce || ciphertext) is base64-encoded for storage in VARCHAR columns.
//!
//! Key is derived by SHA-256 hashing the `ENCRYPTION_KEY` environment variable
//! (or falling back to `JWT_SECRET`).

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
pub fn encrypt_field(plaintext: &str, encryption_key: &str) -> Option<String> {
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
pub fn decrypt_field(encoded: &str, encryption_key: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
