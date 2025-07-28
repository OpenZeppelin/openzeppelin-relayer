//! Helper functions to serialize and deserialize secrets as encrypted base64 for storage

use secrets::SecretVec;
use serde::{Deserialize, Deserializer, Serializer};

use crate::{
    models::SecretString,
    utils::{base64_decode, base64_encode, decrypt_sensitive_field, encrypt_sensitive_field},
};

/// Helper function to serialize secrets as encrypted base64 for storage
pub fn serialize_secret_vec<S>(secret: &SecretVec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    // First encode the raw secret as base64
    let base64 = base64_encode(secret.borrow().as_ref());

    // Then encrypt the base64 string for secure storage
    let encrypted = encrypt_sensitive_field(&base64)
        .map_err(|e| serde::ser::Error::custom(format!("Encryption failed: {}", e)))?;

    serializer.serialize_str(&encrypted)
}

/// Helper function to deserialize secrets from encrypted base64 storage
pub fn deserialize_secret_vec<'de, D>(deserializer: D) -> Result<SecretVec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let encrypted_str = String::deserialize(deserializer)?;

    // First decrypt the encrypted string to get the base64 string
    let base64_str = decrypt_sensitive_field(&encrypted_str)
        .map_err(|e| serde::de::Error::custom(format!("Decryption failed: {}", e)))?;

    // Then decode the base64 to get the raw secret bytes
    let decoded = base64_decode(&base64_str)
        .map_err(|e| serde::de::Error::custom(format!("Invalid base64: {}", e)))?;

    Ok(SecretVec::new(decoded.len(), |v| {
        v.copy_from_slice(&decoded)
    }))
}

/// Helper function to serialize secrets as encrypted base64 for storage
pub fn serialize_secret_string<S>(secret: &SecretString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let secret_content = secret.to_str();
    let encrypted = encrypt_sensitive_field(&secret_content)
        .map_err(|e| serde::ser::Error::custom(format!("Encryption failed: {}", e)))?;

    let encoded = base64_encode(encrypted.as_bytes());

    serializer.serialize_str(&encoded)
}

/// Helper function to deserialize secrets from encrypted base64 storage
pub fn deserialize_secret_string<'de, D>(deserializer: D) -> Result<SecretString, D::Error>
where
    D: Deserializer<'de>,
{
    let base64_str = String::deserialize(deserializer)?;

    // First decode the base64 to get the encrypted bytes
    let encrypted_bytes = base64_decode(&base64_str)
        .map_err(|e| serde::de::Error::custom(format!("Invalid base64: {}", e)))?;

    // Convert encrypted bytes back to string
    let encrypted_str = String::from_utf8(encrypted_bytes)
        .map_err(|e| serde::de::Error::custom(format!("Invalid UTF-8: {}", e)))?;

    // Then decrypt the encrypted string to get the original content
    let decrypted = decrypt_sensitive_field(&encrypted_str)
        .map_err(|e| serde::de::Error::custom(format!("Decryption failed: {}", e)))?;

    Ok(SecretString::new(&decrypted))
}

/// Helper function to serialize optional secrets as encrypted base64 for storage
pub fn serialize_option_secret_string<S>(
    secret: &Option<SecretString>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match secret {
        Some(secret_string) => {
            let secret_content = secret_string.to_str();
            let encrypted = encrypt_sensitive_field(&secret_content)
                .map_err(|e| serde::ser::Error::custom(format!("Encryption failed: {}", e)))?;

            let encoded = base64_encode(encrypted.as_bytes());
            serializer.serialize_some(&encoded)
        }
        None => serializer.serialize_none(),
    }
}

/// Helper function to deserialize optional secrets from encrypted base64 storage
pub fn deserialize_option_secret_string<'de, D>(
    deserializer: D,
) -> Result<Option<SecretString>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt_base64_str: Option<String> = Option::deserialize(deserializer)?;

    match opt_base64_str {
        Some(base64_str) => {
            // First decode the base64 to get the encrypted bytes
            let encrypted_bytes = base64_decode(&base64_str)
                .map_err(|e| serde::de::Error::custom(format!("Invalid base64: {}", e)))?;

            // Convert encrypted bytes back to string
            let encrypted_str = String::from_utf8(encrypted_bytes)
                .map_err(|e| serde::de::Error::custom(format!("Invalid UTF-8: {}", e)))?;

            // Then decrypt the encrypted string to get the original content
            let decrypted = decrypt_sensitive_field(&encrypted_str)
                .map_err(|e| serde::de::Error::custom(format!("Decryption failed: {}", e)))?;

            Ok(Some(SecretString::new(&decrypted)))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrets::SecretVec;
    use serde_json;

    #[test]
    fn test_serialize_deserialize_secret_string() {
        let secret = SecretString::new("test-secret-content");

        // Create a test struct that uses the secret string serialization
        #[derive(serde::Serialize, serde::Deserialize)]
        struct TestStruct {
            #[serde(
                serialize_with = "serialize_secret_string",
                deserialize_with = "deserialize_secret_string"
            )]
            secret: SecretString,
        }

        let test_struct = TestStruct {
            secret: secret.clone(),
        };

        // Test serialization
        let serialized = serde_json::to_string(&test_struct).unwrap();

        // Test deserialization
        let deserialized: TestStruct = serde_json::from_str(&serialized).unwrap();

        // Verify content matches
        assert_eq!(secret.to_str(), deserialized.secret.to_str());
    }

    #[test]
    fn test_serialize_deserialize_secret_vec() {
        let original_data = vec![1, 2, 3, 4, 5];
        let secret = SecretVec::new(original_data.len(), |v| v.copy_from_slice(&original_data));

        // Create a test struct that uses the secret vec serialization
        #[derive(serde::Serialize, serde::Deserialize)]
        struct TestStruct {
            #[serde(
                serialize_with = "serialize_secret_vec",
                deserialize_with = "deserialize_secret_vec"
            )]
            secret_data: SecretVec<u8>,
        }

        let test_struct = TestStruct {
            secret_data: secret,
        };

        // Test serialization
        let serialized = serde_json::to_string(&test_struct).unwrap();

        // Test deserialization
        let deserialized: TestStruct = serde_json::from_str(&serialized).unwrap();

        // Verify content matches
        let original_borrowed = original_data;
        let deserialized_borrowed = deserialized.secret_data.borrow();
        assert_eq!(original_borrowed, *deserialized_borrowed);
    }

    #[test]
    fn test_serialize_deserialize_option_secret_string_some() {
        let secret = SecretString::new("test-optional-secret");

        // Create a test struct that uses the option secret string serialization
        #[derive(serde::Serialize, serde::Deserialize)]
        struct TestStruct {
            #[serde(
                serialize_with = "serialize_option_secret_string",
                deserialize_with = "deserialize_option_secret_string"
            )]
            optional_secret: Option<SecretString>,
        }

        let test_struct = TestStruct {
            optional_secret: Some(secret.clone()),
        };

        // Test serialization
        let serialized = serde_json::to_string(&test_struct).unwrap();

        // Test deserialization
        let deserialized: TestStruct = serde_json::from_str(&serialized).unwrap();

        // Verify content matches
        assert!(deserialized.optional_secret.is_some());
        assert_eq!(
            secret.to_str(),
            deserialized.optional_secret.unwrap().to_str()
        );
    }

    #[test]
    fn test_serialize_deserialize_option_secret_string_none() {
        let secret: Option<SecretString> = None;

        // Create a test struct that uses the option secret string serialization
        #[derive(serde::Serialize, serde::Deserialize)]
        struct TestStruct {
            #[serde(
                serialize_with = "serialize_option_secret_string",
                deserialize_with = "deserialize_option_secret_string"
            )]
            optional_secret: Option<SecretString>,
        }

        let test_struct = TestStruct {
            optional_secret: secret,
        };

        // Test serialization
        let serialized = serde_json::to_string(&test_struct).unwrap();

        // Test deserialization
        let deserialized: TestStruct = serde_json::from_str(&serialized).unwrap();

        // Verify it's None
        assert!(deserialized.optional_secret.is_none());
    }

    #[test]
    fn test_round_trip_secret_string() {
        let original = SecretString::new("complex-secret-with-special-chars-!@#$%^&*()");

        #[derive(serde::Serialize, serde::Deserialize)]
        struct TestStruct {
            #[serde(
                serialize_with = "serialize_secret_string",
                deserialize_with = "deserialize_secret_string"
            )]
            secret: SecretString,
        }

        let test_struct = TestStruct {
            secret: original.clone(),
        };

        // Serialize to JSON
        let json = serde_json::to_string(&test_struct).unwrap();

        // Deserialize back
        let deserialized: TestStruct = serde_json::from_str(&json).unwrap();

        // Verify the content is identical
        assert_eq!(original.to_str(), deserialized.secret.to_str());
    }

    #[test]
    fn test_round_trip_option_secret_string_with_multiple_values() {
        let test_cases = vec![
            Some(SecretString::new("test1")),
            None,
            Some(SecretString::new("")),
            Some(SecretString::new("test-with-unicode-🔐")),
            Some(SecretString::new(&"very-long-secret-".repeat(100))),
        ];

        #[derive(serde::Serialize, serde::Deserialize)]
        struct TestStruct {
            #[serde(
                serialize_with = "serialize_option_secret_string",
                deserialize_with = "deserialize_option_secret_string"
            )]
            optional_secret: Option<SecretString>,
        }

        for test_case in test_cases {
            let test_struct = TestStruct {
                optional_secret: test_case.clone(),
            };

            // Serialize to JSON
            let json = serde_json::to_string(&test_struct).unwrap();

            // Deserialize back
            let deserialized: TestStruct = serde_json::from_str(&json).unwrap();

            // Verify the content matches
            match (test_case, deserialized.optional_secret) {
                (Some(original), Some(deserialized_secret)) => {
                    assert_eq!(original.to_str(), deserialized_secret.to_str());
                }
                (None, None) => {
                    // Both are None, this is correct
                }
                _ => panic!("Mismatch between original and deserialized optional secret"),
            }
        }
    }

    #[test]
    fn test_serialized_content_is_encrypted() {
        let secret = SecretString::new("plaintext-secret");

        #[derive(serde::Serialize)]
        struct TestStruct {
            #[serde(serialize_with = "serialize_secret_string")]
            secret: SecretString,
        }

        let test_struct = TestStruct { secret };
        let json = serde_json::to_string(&test_struct).unwrap();

        // The serialized JSON should not contain the plaintext secret
        assert!(!json.contains("plaintext-secret"));

        // It should be base64 encoded (contains only valid base64 characters)
        let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let serialized_secret = json_value["secret"].as_str().unwrap();

        // Verify it's valid base64 by attempting to decode it
        assert!(base64_decode(serialized_secret).is_ok());
    }

    #[test]
    fn test_serialized_option_content_when_some() {
        let secret = Some(SecretString::new("plaintext-secret"));

        #[derive(serde::Serialize)]
        struct TestStruct {
            #[serde(serialize_with = "serialize_option_secret_string")]
            optional_secret: Option<SecretString>,
        }

        let test_struct = TestStruct {
            optional_secret: secret,
        };
        let json = serde_json::to_string(&test_struct).unwrap();

        // The serialized JSON should not contain the plaintext secret
        assert!(!json.contains("plaintext-secret"));

        // Parse the JSON to verify structure
        let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json_value["optional_secret"].is_string());

        let serialized_secret = json_value["optional_secret"].as_str().unwrap();
        // Verify it's valid base64
        assert!(base64_decode(serialized_secret).is_ok());
    }

    #[test]
    fn test_serialized_option_content_when_none() {
        let secret: Option<SecretString> = None;

        #[derive(serde::Serialize)]
        struct TestStruct {
            #[serde(serialize_with = "serialize_option_secret_string")]
            optional_secret: Option<SecretString>,
        }

        let test_struct = TestStruct {
            optional_secret: secret,
        };
        let json = serde_json::to_string(&test_struct).unwrap();

        // Parse the JSON to verify structure
        let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(json_value["optional_secret"].is_null());
    }
}
