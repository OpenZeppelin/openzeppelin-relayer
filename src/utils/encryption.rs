//! Field-level encryption utilities for sensitive data protection
//!
//! This module provides secure encryption and decryption of sensitive fields using AES-256-GCM.
//! It's designed to be used transparently in the repository layer to protect data at rest.

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload},
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
    #[error("Missing AAD for v2 decryption")]
    MissingAAD,
    #[error("Unsupported encryption version: {0}")]
    UnsupportedVersion(u8),
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
        let key = Key::<Aes256Gcm>::from(*key);
        let cipher = Aes256Gcm::new(&key);
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

            let key_array: [u8; 32] = key_bytes
                .as_slice()
                .try_into()
                .map_err(|_| EncryptionError::InvalidKeyLength(key_bytes.len()))?;
            Ok(Key::<Aes256Gcm>::from(key_array))
        })
    }

    /// Encrypts plaintext data and returns an EncryptedData structure
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedData, EncryptionError> {
        // Generate random 12-byte nonce for GCM
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = &Nonce::from(nonce_bytes);

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

    /// Decrypts an EncryptedData structure and returns the plaintext (v1, no AAD)
    pub fn decrypt(&self, encrypted_data: &EncryptedData) -> Result<Vec<u8>, EncryptionError> {
        if encrypted_data.version != 1 {
            return Err(EncryptionError::InvalidFormat(format!(
                "Unsupported encryption version: {}",
                encrypted_data.version
            )));
        }

        // Decode nonce and ciphertext
        let nonce_bytes = base64_decode(&encrypted_data.nonce)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid nonce: {e}")))?;

        let ciphertext_bytes = base64_decode(&encrypted_data.ciphertext)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid ciphertext: {e}")))?;

        if nonce_bytes.len() != 12 {
            return Err(EncryptionError::InvalidFormat(format!(
                "Invalid nonce length: expected 12, got {}",
                nonce_bytes.len()
            )));
        }

        let nonce_array: [u8; 12] = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| EncryptionError::InvalidFormat("Invalid nonce length".to_string()))?;
        let nonce = &Nonce::from(nonce_array);

        // Decrypt the data
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext_bytes.as_ref())
            .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))?;

        Ok(plaintext)
    }

    /// Encrypts plaintext data with AAD and returns an EncryptedData structure (version 2)
    ///
    /// AAD (Additional Authenticated Data) binds the ciphertext to a specific context,
    /// preventing ciphertext swap attacks where encrypted data could be moved between
    /// different storage locations.
    pub fn encrypt_with_aad(
        &self,
        plaintext: &[u8],
        aad: &[u8],
    ) -> Result<EncryptedData, EncryptionError> {
        // Generate random 12-byte nonce for GCM
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);

        // Encrypt the data with AAD
        let ciphertext = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|e| EncryptionError::EncryptionFailed(e.to_string()))?;

        Ok(EncryptedData {
            nonce: base64_encode(&nonce_bytes),
            ciphertext: base64_encode(&ciphertext),
            version: 2, // Version 2 indicates AAD was used
        })
    }

    /// Decrypts an EncryptedData structure with AAD and returns the plaintext (version 2)
    ///
    /// The AAD must match what was used during encryption, otherwise decryption will fail.
    /// This prevents ciphertext swap attacks.
    pub fn decrypt_with_aad(
        &self,
        encrypted_data: &EncryptedData,
        aad: &[u8],
    ) -> Result<Vec<u8>, EncryptionError> {
        if encrypted_data.version != 2 {
            return Err(EncryptionError::InvalidFormat(format!(
                "Expected version 2 for AAD decryption, got {}",
                encrypted_data.version
            )));
        }

        // Decode nonce and ciphertext
        let nonce_bytes = base64_decode(&encrypted_data.nonce)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid nonce: {e}")))?;

        let ciphertext_bytes = base64_decode(&encrypted_data.ciphertext)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid ciphertext: {e}")))?;

        if nonce_bytes.len() != 12 {
            return Err(EncryptionError::InvalidFormat(format!(
                "Invalid nonce length: expected 12, got {}",
                nonce_bytes.len()
            )));
        }

        let nonce_array: [u8; 12] = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| EncryptionError::InvalidFormat("Invalid nonce length".to_string()))?;
        let nonce = Nonce::from(nonce_array);

        // Decrypt the data with AAD
        let plaintext = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext_bytes,
                    aad,
                },
            )
            .map_err(|e| EncryptionError::DecryptionFailed(e.to_string()))?;

        Ok(plaintext)
    }

    /// Auto-detect version and decrypt accordingly
    ///
    /// - Version 1: Decrypts without AAD (legacy)
    /// - Version 2: Decrypts with AAD (requires aad parameter)
    ///
    /// This enables backwards compatibility with existing v1 encrypted data
    /// while supporting the new v2 format with AAD.
    pub fn decrypt_auto(
        &self,
        encrypted_data: &EncryptedData,
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>, EncryptionError> {
        match encrypted_data.version {
            1 => self.decrypt(encrypted_data),
            2 => {
                let aad = aad.ok_or(EncryptionError::MissingAAD)?;
                self.decrypt_with_aad(encrypted_data, aad)
            }
            v => Err(EncryptionError::UnsupportedVersion(v)),
        }
    }

    /// Encrypts a string and returns base64-encoded encrypted data (opaque format)
    pub fn encrypt_string(&self, plaintext: &str) -> Result<String, EncryptionError> {
        let encrypted_data = self.encrypt(plaintext.as_bytes())?;
        let json_data = serde_json::to_string(&encrypted_data)
            .map_err(|e| EncryptionError::EncryptionFailed(format!("Serialization failed: {e}")))?;

        // Base64 encode the entire JSON to make it opaque
        Ok(base64_encode(json_data.as_bytes()))
    }

    /// Decrypts a base64-encoded encrypted string
    pub fn decrypt_string(&self, encrypted_base64: &str) -> Result<String, EncryptionError> {
        // Decode from base64 to get the JSON
        let json_bytes = base64_decode(encrypted_base64)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid base64: {e}")))?;

        let encrypted_json = String::from_utf8(json_bytes).map_err(|e| {
            EncryptionError::InvalidFormat(format!("Invalid UTF-8 in decoded data: {e}"))
        })?;

        let encrypted_data: EncryptedData = serde_json::from_str(&encrypted_json)
            .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid JSON structure: {e}")))?;

        let plaintext_bytes = self.decrypt(&encrypted_data)?;
        String::from_utf8(plaintext_bytes).map_err(|e| {
            EncryptionError::DecryptionFailed(format!("Invalid UTF-8 in plaintext: {e}"))
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
        let json_data = serde_json::to_string(data)
            .map_err(|e| EncryptionError::EncryptionFailed(format!("JSON encoding failed: {e}")))?;
        Ok(base64_encode(json_data.as_bytes()))
    }
}

/// Decrypts sensitive data from base64 format
pub fn decrypt_sensitive_field(data: &str) -> Result<String, EncryptionError> {
    // Always try to decode base64 first
    let json_bytes = base64_decode(data)
        .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid base64: {e}")))?;

    let json_str = String::from_utf8(json_bytes)
        .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid UTF-8: {e}")))?;

    // Try to parse as encrypted data first (if encryption is configured)
    if FieldEncryption::is_configured() {
        if let Ok(encryption) = get_encryption() {
            // Check if this looks like encrypted data by trying to parse as EncryptedData
            if let Ok(encrypted_data) = serde_json::from_str::<EncryptedData>(&json_str) {
                // This is encrypted data, decrypt it
                let plaintext_bytes = encryption.decrypt(&encrypted_data)?;
                return String::from_utf8(plaintext_bytes).map_err(|e| {
                    EncryptionError::DecryptionFailed(format!("Invalid UTF-8 in plaintext: {e}"))
                });
            }
        }
    }

    // If we get here, either encryption is not configured, or this is fallback data
    // Try to parse as JSON string (fallback format)
    serde_json::from_str(&json_str)
        .map_err(|e| EncryptionError::DecryptionFailed(format!("Invalid JSON string: {e}")))
}

/// Encrypts sensitive data with AAD (Additional Authenticated Data)
///
/// AAD binds the ciphertext to a specific context (e.g., Redis key),
/// preventing ciphertext swap attacks.
pub fn encrypt_sensitive_field_with_aad(data: &str, aad: &str) -> Result<String, EncryptionError> {
    if FieldEncryption::is_configured() {
        match get_encryption() {
            Ok(encryption) => {
                let encrypted_data =
                    encryption.encrypt_with_aad(data.as_bytes(), aad.as_bytes())?;
                let json_data = serde_json::to_string(&encrypted_data).map_err(|e| {
                    EncryptionError::EncryptionFailed(format!("Serialization failed: {e}"))
                })?;
                Ok(base64_encode(json_data.as_bytes()))
            }
            Err(e) => Err(e.clone()),
        }
    } else {
        // For development/testing when encryption is not configured,
        // base64-encode the JSON string for consistency
        let json_data = serde_json::to_string(data)
            .map_err(|e| EncryptionError::EncryptionFailed(format!("JSON encoding failed: {e}")))?;
        Ok(base64_encode(json_data.as_bytes()))
    }
}

/// Decrypts sensitive data with automatic version detection
///
/// - Version 1: Decrypts without AAD (legacy, backwards compatible)
/// - Version 2: Decrypts with AAD (new format)
pub fn decrypt_sensitive_field_auto(
    data: &str,
    aad: Option<&str>,
) -> Result<String, EncryptionError> {
    // Always try to decode base64 first
    let json_bytes = base64_decode(data)
        .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid base64: {e}")))?;

    let json_str = String::from_utf8(json_bytes)
        .map_err(|e| EncryptionError::InvalidFormat(format!("Invalid UTF-8: {e}")))?;

    // Try to parse as encrypted data first (if encryption is configured)
    if FieldEncryption::is_configured() {
        if let Ok(encryption) = get_encryption() {
            // Check if this looks like encrypted data by trying to parse as EncryptedData
            if let Ok(encrypted_data) = serde_json::from_str::<EncryptedData>(&json_str) {
                // Use auto-detect to handle both v1 and v2
                let aad_bytes = aad.map(|s| s.as_bytes());
                let plaintext_bytes = encryption.decrypt_auto(&encrypted_data, aad_bytes)?;
                return String::from_utf8(plaintext_bytes).map_err(|e| {
                    EncryptionError::DecryptionFailed(format!("Invalid UTF-8 in plaintext: {e}"))
                });
            }
        }
    }

    // If we get here, either encryption is not configured, or this is fallback data
    // Try to parse as JSON string (fallback format)
    serde_json::from_str(&json_str)
        .map_err(|e| EncryptionError::DecryptionFailed(format!("Invalid JSON string: {e}")))
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
        // Use direct FieldEncryption instance to avoid global OnceLock issues
        // that cause test flakiness when tests run in different orders
        let key = [8u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = "sensitive data";

        // Test that encrypt_string/decrypt_string work together
        let encoded = encryption.encrypt_string(plaintext).unwrap();
        let decoded = encryption.decrypt_string(&encoded).unwrap();
        assert_eq!(plaintext, decoded);

        // All outputs should be base64-encoded
        assert!(base64_decode(&encoded).is_ok());

        // Test raw encrypt/decrypt as well
        let encrypted_data = encryption.encrypt(plaintext.as_bytes()).unwrap();
        let decrypted_bytes = encryption.decrypt(&encrypted_data).unwrap();
        assert_eq!(plaintext.as_bytes(), decrypted_bytes.as_slice());
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

    // ========== AAD (Additional Authenticated Data) Tests ==========

    #[test]
    fn test_encrypt_decrypt_with_aad() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";
        let aad = b"oz-relayer:signer:test-id";

        let encrypted = encryption.encrypt_with_aad(plaintext, aad).unwrap();
        assert_eq!(encrypted.version, 2);

        let decrypted = encryption.decrypt_with_aad(&encrypted, aad).unwrap();
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_wrong_aad_fails() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";

        let encrypted = encryption.encrypt_with_aad(plaintext, b"key-A").unwrap();
        let result = encryption.decrypt_with_aad(&encrypted, b"key-B");

        assert!(result.is_err()); // AAD mismatch → decryption fails
        if let Err(EncryptionError::DecryptionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected DecryptionFailed error for AAD mismatch");
        }
    }

    #[test]
    fn test_v1_backwards_compatibility() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";

        // Encrypt with v1 (no AAD)
        let encrypted = encryption.encrypt(plaintext).unwrap();
        assert_eq!(encrypted.version, 1);

        // Decrypt with auto-detect (no AAD needed for v1)
        let decrypted = encryption.decrypt_auto(&encrypted, None).unwrap();
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_v2_requires_aad() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";
        let aad = b"storage-key";

        // Encrypt with v2 (with AAD)
        let encrypted = encryption.encrypt_with_aad(plaintext, aad).unwrap();
        assert_eq!(encrypted.version, 2);

        // Decrypt with auto-detect but no AAD → should fail
        let result = encryption.decrypt_auto(&encrypted, None);
        assert!(matches!(result, Err(EncryptionError::MissingAAD)));

        // Decrypt with correct AAD → should succeed
        let decrypted = encryption.decrypt_auto(&encrypted, Some(aad)).unwrap();
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_decrypt_auto_unsupported_version() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let invalid_data = EncryptedData {
            nonce: base64_encode(&[0u8; 12]),
            ciphertext: base64_encode(b"fake"),
            version: 99, // Unsupported version
        };

        let result = encryption.decrypt_auto(&invalid_data, None);
        assert!(matches!(
            result,
            Err(EncryptionError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn test_encrypt_sensitive_field_with_aad() {
        // Use direct FieldEncryption instance to avoid global OnceLock issues
        let key = [11u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = b"sensitive-api-key";
        let aad = b"oz-relayer:signer:my-signer-id";

        let encrypted = encryption.encrypt_with_aad(plaintext, aad).unwrap();
        assert_eq!(encrypted.version, 2);

        let decrypted = encryption.decrypt_auto(&encrypted, Some(aad)).unwrap();
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_decrypt_sensitive_field_auto_v1_compat() {
        // Use direct FieldEncryption instance to avoid global OnceLock issues
        let key = [12u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = b"legacy-secret";

        // Encrypt with v1 (no AAD)
        let encrypted = encryption.encrypt(plaintext).unwrap();
        assert_eq!(encrypted.version, 1);

        // Decrypt with auto (should work without AAD for v1)
        let decrypted = encryption.decrypt_auto(&encrypted, None).unwrap();
        assert_eq!(plaintext, decrypted.as_slice());

        // Decrypt with auto (should also work with AAD provided for v1)
        let decrypted_with_aad = encryption
            .decrypt_auto(&encrypted, Some(b"ignored"))
            .unwrap();
        assert_eq!(plaintext, decrypted_with_aad.as_slice());
    }

    #[test]
    fn test_ciphertext_swap_prevention() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let secret_a = b"secret-for-signer-a";
        let secret_b = b"secret-for-signer-b";
        let aad_a = b"oz-relayer:signer:signer-a";
        let aad_b = b"oz-relayer:signer:signer-b";

        // Encrypt secrets with their respective AADs
        let encrypted_a = encryption.encrypt_with_aad(secret_a, aad_a).unwrap();
        let encrypted_b = encryption.encrypt_with_aad(secret_b, aad_b).unwrap();

        // Attempting to decrypt encrypted_a with aad_b should fail (ciphertext swap attack)
        let swap_result = encryption.decrypt_with_aad(&encrypted_a, aad_b);
        assert!(swap_result.is_err());

        // Correct decryption should work
        let correct_a = encryption.decrypt_with_aad(&encrypted_a, aad_a).unwrap();
        let correct_b = encryption.decrypt_with_aad(&encrypted_b, aad_b).unwrap();

        assert_eq!(secret_a, correct_a.as_slice());
        assert_eq!(secret_b, correct_b.as_slice());
    }

    #[test]
    fn test_decrypt_with_aad_version_mismatch() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        // Encrypt with v1 (no AAD)
        let encrypted_v1 = encryption.encrypt(b"secret").unwrap();
        assert_eq!(encrypted_v1.version, 1);

        // Try to decrypt v1 data with decrypt_with_aad (expects v2)
        let result = encryption.decrypt_with_aad(&encrypted_v1, b"some-aad");
        assert!(result.is_err());
        if let Err(EncryptionError::InvalidFormat(msg)) = result {
            assert!(msg.contains("Expected version 2"));
            assert!(msg.contains("got 1"));
        } else {
            panic!("Expected InvalidFormat error for version mismatch");
        }
    }

    #[test]
    fn test_decrypt_with_aad_invalid_nonce_base64() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let invalid_data = EncryptedData {
            nonce: "not-valid-base64!!!".to_string(),
            ciphertext: base64_encode(b"fake"),
            version: 2,
        };

        let result = encryption.decrypt_with_aad(&invalid_data, b"aad");
        assert!(result.is_err());
        if let Err(EncryptionError::InvalidFormat(msg)) = result {
            assert!(msg.contains("Invalid nonce"));
        } else {
            panic!("Expected InvalidFormat error for invalid nonce base64");
        }
    }

    #[test]
    fn test_decrypt_with_aad_invalid_ciphertext_base64() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let invalid_data = EncryptedData {
            nonce: base64_encode(&[0u8; 12]),
            ciphertext: "not-valid-base64!!!".to_string(),
            version: 2,
        };

        let result = encryption.decrypt_with_aad(&invalid_data, b"aad");
        assert!(result.is_err());
        if let Err(EncryptionError::InvalidFormat(msg)) = result {
            assert!(msg.contains("Invalid ciphertext"));
        } else {
            panic!("Expected InvalidFormat error for invalid ciphertext base64");
        }
    }

    #[test]
    fn test_decrypt_with_aad_invalid_nonce_length() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        // Nonce should be 12 bytes, use 8 bytes instead
        let invalid_data = EncryptedData {
            nonce: base64_encode(&[0u8; 8]),
            ciphertext: base64_encode(b"fake-ciphertext"),
            version: 2,
        };

        let result = encryption.decrypt_with_aad(&invalid_data, b"aad");
        assert!(result.is_err());
        if let Err(EncryptionError::InvalidFormat(msg)) = result {
            assert!(msg.contains("Invalid nonce length"));
        } else {
            panic!("Expected InvalidFormat error for invalid nonce length");
        }
    }

    #[test]
    fn test_decrypt_auto_v2_wrong_aad() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";
        let correct_aad = b"correct-aad";
        let wrong_aad = b"wrong-aad";

        let encrypted = encryption.encrypt_with_aad(plaintext, correct_aad).unwrap();

        // decrypt_auto with wrong AAD should fail
        let result = encryption.decrypt_auto(&encrypted, Some(wrong_aad));
        assert!(result.is_err());
        if let Err(EncryptionError::DecryptionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected DecryptionFailed error for wrong AAD");
        }
    }

    #[test]
    fn test_encrypt_with_aad_empty_plaintext() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let aad = b"context";

        // Empty plaintext should work
        let encrypted = encryption.encrypt_with_aad(b"", aad).unwrap();
        assert_eq!(encrypted.version, 2);

        let decrypted = encryption.decrypt_with_aad(&encrypted, aad).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_encrypt_with_aad_empty_aad() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";

        // Empty AAD should work
        let encrypted = encryption.encrypt_with_aad(plaintext, b"").unwrap();
        assert_eq!(encrypted.version, 2);

        let decrypted = encryption.decrypt_with_aad(&encrypted, b"").unwrap();
        assert_eq!(plaintext, decrypted.as_slice());

        // Empty AAD != non-empty AAD
        let result = encryption.decrypt_with_aad(&encrypted, b"some-aad");
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_with_aad_large_data() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let large_plaintext = vec![0xABu8; 10_000];
        let aad = b"large-data-context";

        let encrypted = encryption.encrypt_with_aad(&large_plaintext, aad).unwrap();
        assert_eq!(encrypted.version, 2);

        let decrypted = encryption.decrypt_with_aad(&encrypted, aad).unwrap();
        assert_eq!(large_plaintext, decrypted);
    }

    #[test]
    fn test_encrypt_with_aad_nonce_uniqueness() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"same message";
        let aad = b"same-aad";

        let encrypted1 = encryption.encrypt_with_aad(plaintext, aad).unwrap();
        let encrypted2 = encryption.encrypt_with_aad(plaintext, aad).unwrap();

        // Same plaintext and AAD should produce different ciphertext due to random nonces
        assert_ne!(encrypted1.nonce, encrypted2.nonce);
        assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);

        // Both should decrypt correctly
        assert_eq!(
            encryption.decrypt_with_aad(&encrypted1, aad).unwrap(),
            plaintext
        );
        assert_eq!(
            encryption.decrypt_with_aad(&encrypted2, aad).unwrap(),
            plaintext
        );
    }

    #[test]
    fn test_encrypt_sensitive_field_with_aad_fallback() {
        // Temporarily clear encryption key to test fallback
        let old_key = env::var("STORAGE_ENCRYPTION_KEY").ok();
        env::remove_var("STORAGE_ENCRYPTION_KEY");

        let plaintext = "fallback-secret";
        let aad = "context-aad";

        // Should use fallback mode (base64-encoded JSON)
        let encoded = encrypt_sensitive_field_with_aad(plaintext, aad).unwrap();
        let decoded = decrypt_sensitive_field_auto(&encoded, Some(aad)).unwrap();
        assert_eq!(plaintext, decoded);

        // Should be base64-encoded JSON (fallback ignores AAD)
        let expected_json = serde_json::to_string(plaintext).unwrap();
        let expected_b64 = base64_encode(expected_json.as_bytes());
        assert_eq!(encoded, expected_b64);

        // Restore original environment
        if let Some(key) = old_key {
            env::set_var("STORAGE_ENCRYPTION_KEY", key);
        }
    }

    #[test]
    fn test_encrypt_sensitive_field_with_aad_wrong_aad_on_decrypt() {
        // Use direct FieldEncryption instance to avoid global OnceLock issues
        let key = [10u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();

        let plaintext = b"sensitive-data";
        let correct_aad = b"correct-context";
        let wrong_aad = b"wrong-context";

        let encrypted = encryption.encrypt_with_aad(plaintext, correct_aad).unwrap();
        assert_eq!(encrypted.version, 2);

        // Decrypting with wrong AAD should fail
        let result = encryption.decrypt_auto(&encrypted, Some(wrong_aad));
        assert!(result.is_err());
        if let Err(EncryptionError::DecryptionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected DecryptionFailed error for wrong AAD");
        }

        // Decrypting with correct AAD should succeed
        let decrypted = encryption
            .decrypt_auto(&encrypted, Some(correct_aad))
            .unwrap();
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_decrypt_auto_v1_ignores_aad() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";

        // Encrypt with v1 (no AAD)
        let encrypted = encryption.encrypt(plaintext).unwrap();
        assert_eq!(encrypted.version, 1);

        // Decrypt with AAD provided - should be ignored for v1
        let decrypted = encryption
            .decrypt_auto(&encrypted, Some(b"any-aad"))
            .unwrap();
        assert_eq!(plaintext, decrypted.as_slice());

        // Decrypt with different AAD - still works (ignored for v1)
        let decrypted2 = encryption
            .decrypt_auto(&encrypted, Some(b"different-aad"))
            .unwrap();
        assert_eq!(plaintext, decrypted2.as_slice());
    }

    #[test]
    fn test_decrypt_with_aad_tampered_ciphertext() {
        let key = [0u8; 32];
        let encryption = FieldEncryption::new_with_key(&key).unwrap();
        let plaintext = b"secret";
        let aad = b"context";

        let mut encrypted = encryption.encrypt_with_aad(plaintext, aad).unwrap();

        // Tamper with the ciphertext
        let mut ciphertext_bytes = base64_decode(&encrypted.ciphertext).unwrap();
        if !ciphertext_bytes.is_empty() {
            ciphertext_bytes[0] ^= 0xFF; // Flip bits
        }
        encrypted.ciphertext = base64_encode(&ciphertext_bytes);

        // Decryption should fail due to authentication failure
        let result = encryption.decrypt_with_aad(&encrypted, aad);
        assert!(result.is_err());
        if let Err(EncryptionError::DecryptionFailed(_)) = result {
            // Expected - GCM authentication failed
        } else {
            panic!("Expected DecryptionFailed error for tampered ciphertext");
        }
    }
}
