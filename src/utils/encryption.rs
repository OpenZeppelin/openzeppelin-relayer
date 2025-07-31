//! Field-level encryption utilities for sensitive data protection
//!
//! This module provides secure encryption and decryption of sensitive fields using AES-256-GCM.
//! It's designed to be used transparently in the repository layer to protect data at rest.

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use serde::{Deserialize, Serialize};
use std::env;
use thiserror::Error;
use zeroize::Zeroize;

use crate::{
    models::SecretString,
    utils::{base64_decode, base64_encode},
};

#[derive(Error, Debug, Clone)]
pub enum EncryptionError {
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("Key derivation failed: {0}")]
    KeyDerivationFailed(String),
    #[error("Invalid encrypted data format: {0}")]
    InvalidFormat(String),
    #[error("Missing encryption key environment variable: {0}")]
    MissingKey(String),
    #[error("Invalid key length: expected 32 bytes, got {0}")]
    InvalidKeyLength(usize),
}

/// Encrypted data container that holds the nonce and ciphertext
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    /// Base64-encoded nonce (12 bytes for GCM)
    pub nonce: String,
    /// Base64-encoded ciphertext with authentication tag
    pub ciphertext: String,
    /// Version for future compatibility
    pub version: u8,
}

/// Main encryption service for field-level encryption
#[derive(Clone)]
pub struct FieldEncryption {
    cipher: Aes256Gcm,
}

impl FieldEncryption {
    /// Creates a new FieldEncryption instance using a key from environment variables
    ///
    /// # Environment Variables
    /// - `STORAGE_ENCRYPTION_KEY`: Base64-encoded 32-byte encryption key
    /// ```
    pub fn new() -> Result<Self, EncryptionError> {
        let key = Self::load_key_from_env()?;
        let cipher = Aes256Gcm::new(&key);
        Ok(Self { cipher })
    }

    /// Creates a new FieldEncryption instance with a provided key (for testing)
    pub fn new_with_key(key: &[u8; 32]) -> Result<Self, EncryptionError> {
        let key = Key::<Aes256Gcm>::from_slice(key);
        let cipher = Aes256Gcm::new(key);
        Ok(Self { cipher })
    }

    /// Loads encryption key from environment variables
    fn load_key_from_env() -> Result<Key<Aes256Gcm>, EncryptionError> {
        let key = env::var("STORAGE_ENCRYPTION_KEY")
            .map(|v| SecretString::new(&v))
            .map_err(|_| {
                EncryptionError::MissingKey("STORAGE_ENCRYPTION_KEY must be set".to_string())
            })?;

        key.as_str(|key_b64| {
            let mut key_bytes = base64_decode(key_b64)
                .map_err(|e| EncryptionError::KeyDerivationFailed(e.to_string()))?;
            if key_bytes.len() != 32 {
                key_bytes.zeroize(); // Explicit cleanup on error path
                return Err(EncryptionError::InvalidKeyLength(key_bytes.len()));
            }

            Ok(*Key::<Aes256Gcm>::from_slice(&key_bytes))
        })
    }

    /// Encrypts plaintext data and returns an EncryptedData structure
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedData, EncryptionError> {
        // Generate random 12-byte nonce for GCM
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt the data
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| EncryptionError::EncryptionFailed(e.to_string()))?;

        Ok(EncryptedData {
            nonce: base64_encode(&nonce_bytes),
            ciphertext: base64_encode(&ciphertext),
            version: 1,
        })
    }

    /// Decrypts an EncryptedData structure and returns the plaintext
    pub fn decrypt(&self, encrypted_data: &EncryptedData) -> Result<Vec<u8>, EncryptionError> {
        if encrypted_data.version != 1 {
            return Err(EncryptionError::InvalidFormat(format!(
                "Unsupported encryption version: {}",
                encrypted_data.version
            )));
        }

        // Decode nonce and ciphertext
        let nonce_bytes = base64_decode(&encrypted_data.nonce)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid nonce: {}", e)))?;

        let ciphertext_bytes = base64_decode(&encrypted_data.ciphertext)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid ciphertext: {}", e)))?;

        if nonce_bytes.len() != 12 {
            return Err(EncryptionError::InvalidFormat(format!(
                "Invalid nonce length: expected 12, got {}",
                nonce_bytes.len()
            )));
        }

        let nonce = Nonce::from_slice(&nonce_bytes);

        // Decrypt the data
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext_bytes.as_ref())
            .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))?;

        Ok(plaintext)
    }

    /// Encrypts a string and returns base64-encoded encrypted data (opaque format)
    pub fn encrypt_string(&self, plaintext: &str) -> Result<String, EncryptionError> {
        let encrypted_data = self.encrypt(plaintext.as_bytes())?;
        let json_data = serde_json::to_string(&encrypted_data).map_err(|e| {
            EncryptionError::EncryptionFailed(format!("Serialization failed: {}", e))
        })?;

        // Base64 encode the entire JSON to make it opaque
        Ok(base64_encode(json_data.as_bytes()))
    }

    /// Decrypts a base64-encoded encrypted string
    pub fn decrypt_string(&self, encrypted_base64: &str) -> Result<String, EncryptionError> {
        // Decode from base64 to get the JSON
        let json_bytes = base64_decode(encrypted_base64)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid base64: {}", e)))?;

        let encrypted_json = String::from_utf8(json_bytes).map_err(|e| {
            EncryptionError::InvalidFormat(format!("Invalid UTF-8 in decoded data: {}", e))
        })?;

        let encrypted_data: EncryptedData = serde_json::from_str(&encrypted_json).map_err(|e| {
            EncryptionError::InvalidFormat(format!("Invalid JSON structure: {}", e))
        })?;

        let plaintext_bytes = self.decrypt(&encrypted_data)?;
        String::from_utf8(plaintext_bytes).map_err(|e| {
            EncryptionError::DecryptionFailed(format!("Invalid UTF-8 in plaintext: {}", e))
        })
    }

    /// Utility function to generate a new encryption key for setup
    pub fn generate_key() -> String {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        let key_b64 = base64_encode(&key);

        // Zero out the key from memory
        let mut key_zeroize = key;
        key_zeroize.zeroize();

        key_b64
    }

    /// Checks if encryption is properly configured
    pub fn is_configured() -> bool {
        env::var("STORAGE_ENCRYPTION_KEY").is_ok()
    }
}

/// Global encryption instance (lazy-initialized)
static ENCRYPTION_INSTANCE: std::sync::OnceLock<Result<FieldEncryption, EncryptionError>> =
    std::sync::OnceLock::new();

/// Gets the global encryption instance
pub fn get_encryption() -> Result<&'static FieldEncryption, &'static EncryptionError> {
    ENCRYPTION_INSTANCE
        .get_or_init(FieldEncryption::new)
        .as_ref()
}

/// Encrypts sensitive data if encryption is configured, otherwise returns base64-encoded plaintext
pub fn encrypt_sensitive_field(data: &str) -> Result<String, EncryptionError> {
    if FieldEncryption::is_configured() {
        match get_encryption() {
            Ok(encryption) => encryption.encrypt_string(data),
            Err(e) => Err(e.clone()),
        }
    } else {
        // For development/testing when encryption is not configured,
        // base64-encode the JSON string for consistency
        let json_data = serde_json::to_string(data).map_err(|e| {
            EncryptionError::EncryptionFailed(format!("JSON encoding failed: {}", e))
        })?;
        Ok(base64_encode(json_data.as_bytes()))
    }
}

/// Decrypts sensitive data from base64 format
pub fn decrypt_sensitive_field(data: &str) -> Result<String, EncryptionError> {
    // Always try to decode base64 first
    let json_bytes = base64_decode(data)
        .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid base64: {}", e)))?;

    let json_str = String::from_utf8(json_bytes)
        .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid UTF-8: {}", e)))?;

    // Try to parse as encrypted data first (if encryption is configured)
    if FieldEncryption::is_configured() {
        if let Ok(encryption) = get_encryption() {
            // Check if this looks like encrypted data by trying to parse as EncryptedData
            if let Ok(encrypted_data) = serde_json::from_str::<EncryptedData>(&json_str) {
                // This is encrypted data, decrypt it
                let plaintext_bytes = encryption.decrypt(&encrypted_data)?;
                return String::from_utf8(plaintext_bytes).map_err(|e| {
                    EncryptionError::DecryptionFailed(format!("Invalid UTF-8 in plaintext: {}", e))
                });
            }
        }
    }

    // If we get here, either encryption is not configured, or this is fallback data
    // Try to parse as JSON string (fallback format)
    serde_json::from_str(&json_str)
        .map_err(|e| EncryptionError::DecryptionFailed(format!("Invalid JSON string: {}", e)))
}

/// Utility function to generate a new encryption key
pub fn generate_encryption_key() -> String {
    FieldEncryption::generate_key()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_encrypt_decrypt_data() {
        let key = [0u8; 32]; // Test key
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = b"This is a secret message!";
        let encrypted = encryption.encrypt(plaintext).unwrap();
        let decrypted = encryption.decrypt(&encrypted).unwrap();

        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_encrypt_decrypt_string() {
        let key = [1u8; 32]; // Different test key
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = "Sensitive API key: sk-1234567890abcdef";
        let encrypted = encryption.encrypt_string(plaintext).unwrap();
        let decrypted = encryption.decrypt_string(&encrypted).unwrap();

        assert_eq!(plaintext, decrypted);
    }

    #[test]
    fn test_different_keys_produce_different_results() {
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let encryption1 = FieldEncryption::new_with_key(&key1).unwrap();
        let encryption2 = FieldEncryption::new_with_key(&key2).unwrap();

        let plaintext = "secret";
        let encrypted1 = encryption1.encrypt_string(plaintext).unwrap();
        let encrypted2 = encryption2.encrypt_string(plaintext).unwrap();

        assert_ne!(encrypted1, encrypted2);

        // Each should decrypt with their own key
        assert_eq!(encryption1.decrypt_string(&encrypted1).unwrap(), plaintext);
        assert_eq!(encryption2.decrypt_string(&encrypted2).unwrap(), plaintext);

        // But not with the other key
        assert!(encryption1.decrypt_string(&encrypted2).is_err());
        assert!(encryption2.decrypt_string(&encrypted1).is_err());
    }

    #[test]
    fn test_nonce_uniqueness() {
        let key = [3u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = "same message";
        let encrypted1 = encryption.encrypt_string(plaintext).unwrap();
        let encrypted2 = encryption.encrypt_string(plaintext).unwrap();

        // Same plaintext should produce different ciphertext due to random nonces
        assert_ne!(encrypted1, encrypted2);

        // Both should decrypt to the same plaintext
        assert_eq!(encryption.decrypt_string(&encrypted1).unwrap(), plaintext);
        assert_eq!(encryption.decrypt_string(&encrypted2).unwrap(), plaintext);
    }

    #[test]
    fn test_invalid_encrypted_data() {
        let key = [4u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        // Test with invalid base64
        assert!(encryption.decrypt_string("invalid base64!").is_err());

        // Test with valid base64 but invalid JSON inside
        assert!(encryption
            .decrypt_string(&base64_encode(b"not json"))
            .is_err());

        // Test with valid base64 but wrong JSON structure inside
        let invalid_json_b64 = base64_encode(b"{\"wrong\": \"structure\"}");
        assert!(encryption.decrypt_string(&invalid_json_b64).is_err());

        // Test with plain JSON (old format) - should fail since we only accept base64
        assert!(encryption
            .decrypt_string(&base64_encode(
                b"{\"nonce\":\"test\",\"ciphertext\":\"test\",\"version\":1}"
            ))
            .is_err());
    }

    #[test]
    fn test_generate_key() {
        let key1 = FieldEncryption::generate_key();
        let key2 = FieldEncryption::generate_key();

        // Keys should be different
        assert_ne!(key1, key2);

        // Keys should be valid base64
        assert!(base64_decode(&key1).is_ok());
        assert!(base64_decode(&key2).is_ok());

        // Decoded keys should be 32 bytes
        assert_eq!(base64_decode(&key1).unwrap().len(), 32);
        assert_eq!(base64_decode(&key2).unwrap().len(), 32);
    }

    #[test]
    fn test_env_key_loading() {
        // Test base64 key
        let test_key = FieldEncryption::generate_key();
        env::set_var("STORAGE_ENCRYPTION_KEY", &test_key);

        let encryption = FieldEncryption::new().unwrap();
        let plaintext = "test message";
        let encrypted = encryption.encrypt_string(plaintext).unwrap();
        let decrypted = encryption.decrypt_string(&encrypted).unwrap();
        assert_eq!(plaintext, decrypted);

        // Test missing key
        env::remove_var("STORAGE_ENCRYPTION_KEY");
        assert!(FieldEncryption::new().is_err());

        // Clean up
        env::set_var("STORAGE_ENCRYPTION_KEY", &test_key);
    }

    #[test]
    fn test_high_level_encryption_functions() {
        let plaintext = "sensitive data";

        // Test that the high-level encrypt/decrypt functions work together
        let encoded = encrypt_sensitive_field(plaintext).unwrap();
        let decoded = decrypt_sensitive_field(&encoded).unwrap();
        assert_eq!(plaintext, decoded);

        // All outputs should now be base64-encoded (whether encrypted or fallback)
        assert!(base64_decode(&encoded).is_ok());

        // Just verify it works - don't make assumptions about internal format
        // since global encryption state may vary between test runs
    }

    #[test]
    fn test_fallback_when_encryption_disabled() {
        // Temporarily clear encryption key to test fallback
        let old_key = env::var("STORAGE_ENCRYPTION_KEY").ok();

        env::remove_var("STORAGE_ENCRYPTION_KEY");

        let plaintext = "fallback test";

        // Should use fallback mode (base64-encoded JSON)
        let encoded = encrypt_sensitive_field(plaintext).unwrap();
        let decoded = decrypt_sensitive_field(&encoded).unwrap();
        assert_eq!(plaintext, decoded);

        // Should be base64-encoded JSON
        let expected_json = serde_json::to_string(plaintext).unwrap();
        let expected_b64 = base64_encode(expected_json.as_bytes());
        assert_eq!(encoded, expected_b64);

        // Restore original environment
        if let Some(key) = old_key {
            env::set_var("STORAGE_ENCRYPTION_KEY", key);
        }
    }

    #[test]
    fn test_core_encryption_methods() {
        let key = [9u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = "core encryption test";

        // Test core encryption methods directly
        let encrypted = encryption.encrypt_string(plaintext).unwrap();
        let decrypted = encryption.decrypt_string(&encrypted).unwrap();
        assert_eq!(plaintext, decrypted);

        // Should be base64-encoded
        assert!(base64_decode(&encrypted).is_ok());
        // Should not contain readable structure
        assert!(!encrypted.contains("nonce"));
        assert!(!encrypted.contains("ciphertext"));
        assert!(!encrypted.contains("{"));
    }

    #[test]
    fn test_base64_encoding_hides_structure() {
        let key = [7u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = "secret message";
        let encrypted = encryption.encrypt_string(plaintext).unwrap();

        // Should be valid base64
        assert!(base64_decode(&encrypted).is_ok());

        // Should not contain readable JSON structure
        assert!(!encrypted.contains("nonce"));
        assert!(!encrypted.contains("ciphertext"));
        assert!(!encrypted.contains("version"));
        assert!(!encrypted.contains("{"));
        assert!(!encrypted.contains("}"));

        // Should decrypt correctly
        let decrypted = encryption.decrypt_string(&encrypted).unwrap();
        assert_eq!(plaintext, decrypted);
    }
}
