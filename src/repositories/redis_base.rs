//! Base Redis repository functionality shared across all Redis implementations.
//!
//! This module provides common utilities and patterns used by all Redis repository
//! implementations to reduce code duplication and ensure consistency.

use crate::models::RepositoryError;
use log::{error, warn};
use redis::RedisError;
use serde::{Deserialize, Serialize};

/// Base trait for Redis repositories providing common functionality
pub trait RedisRepository {
    fn serialize_entity<T, F>(
        &self,
        entity: &T,
        id_extractor: F,
        entity_type: &str,
    ) -> Result<String, RepositoryError>
    where
        T: Serialize,
        F: Fn(&T) -> &str,
    {
        serde_json::to_string(entity).map_err(|e| {
            let id = id_extractor(entity);
            error!("Serialization failed for {} {}: {}", entity_type, id, e);
            RepositoryError::InvalidData(format!(
                "Failed to serialize {} {}: {}",
                entity_type, id, e
            ))
        })
    }

    /// Deserialize entity with detailed error context
    /// Default implementation that works for any Deserialize type
    fn deserialize_entity<T>(
        &self,
        json: &str,
        entity_id: &str,
        entity_type: &str,
    ) -> Result<T, RepositoryError>
    where
        T: for<'de> Deserialize<'de>,
    {
        serde_json::from_str(json).map_err(|e| {
            error!(
                "Deserialization failed for {} {}: {}",
                entity_type, entity_id, e
            );
            RepositoryError::InvalidData(format!(
                "Failed to deserialize {} {}: {} (JSON length: {})",
                entity_type,
                entity_id,
                e,
                json.len()
            ))
        })
    }

    /// Convert Redis errors to appropriate RepositoryError types
    fn map_redis_error(&self, error: RedisError, context: &str) -> RepositoryError {
        warn!("Redis operation failed in context '{}': {}", context, error);

        match error.kind() {
            redis::ErrorKind::TypeError => RepositoryError::InvalidData(format!(
                "Redis data type error in operation '{}': {}",
                context, error
            )),
            redis::ErrorKind::AuthenticationFailed => {
                RepositoryError::InvalidData("Redis authentication failed".to_string())
            }
            redis::ErrorKind::NoScriptError => RepositoryError::InvalidData(format!(
                "Redis script error in operation '{}': {}",
                context, error
            )),
            redis::ErrorKind::ReadOnly => RepositoryError::InvalidData(format!(
                "Redis is read-only in operation '{}': {}",
                context, error
            )),
            redis::ErrorKind::ExecAbortError => RepositoryError::InvalidData(format!(
                "Redis transaction aborted in operation '{}': {}",
                context, error
            )),
            redis::ErrorKind::BusyLoadingError => RepositoryError::InvalidData(format!(
                "Redis is busy in operation '{}': {}",
                context, error
            )),
            redis::ErrorKind::ExtensionError => RepositoryError::InvalidData(format!(
                "Redis extension error in operation '{}': {}",
                context, error
            )),
            // Default to Other for connection errors and other issues
            _ => RepositoryError::Other(format!("Redis operation '{}' failed: {}", context, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    // Test structs for serialization/deserialization
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestEntity {
        id: String,
        name: String,
        value: i32,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct SimpleEntity {
        id: String,
    }

    // Test implementation of RedisRepository trait
    struct TestRedisRepository;

    impl RedisRepository for TestRedisRepository {}

    impl TestRedisRepository {
        fn new() -> Self {
            TestRedisRepository
        }
    }

    #[test]
    fn test_serialize_entity_success() {
        let repo = TestRedisRepository::new();
        let entity = TestEntity {
            id: "test-id".to_string(),
            name: "test-name".to_string(),
            value: 42,
        };

        let result = repo.serialize_entity(&entity, |e| &e.id, "TestEntity");

        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("test-id"));
        assert!(json.contains("test-name"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_serialize_entity_with_different_id_extractor() {
        let repo = TestRedisRepository::new();
        let entity = TestEntity {
            id: "test-id".to_string(),
            name: "test-name".to_string(),
            value: 42,
        };

        // Use name as ID extractor
        let result = repo.serialize_entity(&entity, |e| &e.name, "TestEntity");

        assert!(result.is_ok());
        let json = result.unwrap();

        // Should still serialize the entire entity
        assert!(json.contains("test-id"));
        assert!(json.contains("test-name"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_serialize_entity_simple_struct() {
        let repo = TestRedisRepository::new();
        let entity = SimpleEntity {
            id: "simple-id".to_string(),
        };

        let result = repo.serialize_entity(&entity, |e| &e.id, "SimpleEntity");

        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("simple-id"));
    }

    #[test]
    fn test_deserialize_entity_success() {
        let repo = TestRedisRepository::new();
        let json = r#"{"id":"test-id","name":"test-name","value":42}"#;

        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(json, "test-id", "TestEntity");

        assert!(result.is_ok());
        let entity = result.unwrap();
        assert_eq!(entity.id, "test-id");
        assert_eq!(entity.name, "test-name");
        assert_eq!(entity.value, 42);
    }

    #[test]
    fn test_deserialize_entity_invalid_json() {
        let repo = TestRedisRepository::new();
        let invalid_json = r#"{"id":"test-id","name":"test-name","value":}"#; // Missing value

        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(invalid_json, "test-id", "TestEntity");

        assert!(result.is_err());
        match result.unwrap_err() {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Failed to deserialize TestEntity test-id"));
                assert!(msg.contains("JSON length:"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_deserialize_entity_invalid_structure() {
        let repo = TestRedisRepository::new();
        let json = r#"{"wrongfield":"test-id"}"#;

        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(json, "test-id", "TestEntity");

        assert!(result.is_err());
        match result.unwrap_err() {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Failed to deserialize TestEntity test-id"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_type_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::TypeError, "Type error"));

        let result = repo.map_redis_error(redis_error, "test_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis data type error"));
                assert!(msg.contains("test_operation"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_authentication_failed() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::AuthenticationFailed, "Auth failed"));

        let result = repo.map_redis_error(redis_error, "auth_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis authentication failed"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_connection_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::IoError, "Connection failed"));

        let result = repo.map_redis_error(redis_error, "connection_operation");

        match result {
            RepositoryError::Other(msg) => {
                assert!(msg.contains("Redis operation"));
                assert!(msg.contains("connection_operation"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_map_redis_error_no_script_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::NoScriptError, "Script not found"));

        let result = repo.map_redis_error(redis_error, "script_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis script error"));
                assert!(msg.contains("script_operation"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_read_only() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::ReadOnly, "Read only"));

        let result = repo.map_redis_error(redis_error, "write_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis is read-only"));
                assert!(msg.contains("write_operation"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_exec_abort_error() {
        let repo = TestRedisRepository::new();
        let redis_error =
            RedisError::from((redis::ErrorKind::ExecAbortError, "Transaction aborted"));

        let result = repo.map_redis_error(redis_error, "transaction_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis transaction aborted"));
                assert!(msg.contains("transaction_operation"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_busy_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::BusyLoadingError, "Server busy"));

        let result = repo.map_redis_error(redis_error, "busy_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis is busy"));
                assert!(msg.contains("busy_operation"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_extension_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::ExtensionError, "Extension error"));

        let result = repo.map_redis_error(redis_error, "extension_operation");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis extension error"));
                assert!(msg.contains("extension_operation"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_context_propagation() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::TypeError, "Type error"));
        let context = "user_repository_get_operation";

        let result = repo.map_redis_error(redis_error, context);

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis data type error"));
                // Context should be used in logging but not necessarily in the error message
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let repo = TestRedisRepository::new();
        let original = TestEntity {
            id: "roundtrip-id".to_string(),
            name: "roundtrip-name".to_string(),
            value: 123,
        };

        // Serialize
        let json = repo
            .serialize_entity(&original, |e| &e.id, "TestEntity")
            .unwrap();

        // Deserialize
        let deserialized: TestEntity = repo
            .deserialize_entity(&json, "roundtrip-id", "TestEntity")
            .unwrap();

        // Should be identical
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_serialize_deserialize_unicode_content() {
        let repo = TestRedisRepository::new();
        let original = TestEntity {
            id: "unicode-id".to_string(),
            name: "测试名称 🚀".to_string(),
            value: 456,
        };

        // Serialize
        let json = repo
            .serialize_entity(&original, |e| &e.id, "TestEntity")
            .unwrap();

        // Deserialize
        let deserialized: TestEntity = repo
            .deserialize_entity(&json, "unicode-id", "TestEntity")
            .unwrap();

        // Should handle unicode correctly
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_serialize_entity_with_complex_data() {
        let repo = TestRedisRepository::new();

        #[derive(Serialize)]
        struct ComplexEntity {
            id: String,
            nested: NestedData,
            list: Vec<i32>,
        }

        #[derive(Serialize)]
        struct NestedData {
            field1: String,
            field2: bool,
        }

        let complex_entity = ComplexEntity {
            id: "complex-id".to_string(),
            nested: NestedData {
                field1: "nested-value".to_string(),
                field2: true,
            },
            list: vec![1, 2, 3],
        };

        let result = repo.serialize_entity(&complex_entity, |e| &e.id, "ComplexEntity");

        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("complex-id"));
        assert!(json.contains("nested-value"));
        assert!(json.contains("true"));
        assert!(json.contains("[1,2,3]"));
    }

    // Test specifically for u128 serialization/deserialization with large values
    #[test]
    fn test_serialize_deserialize_u128_large_values() {
        use crate::utils::{deserialize_optional_u128, serialize_optional_u128};

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct TestU128Entity {
            id: String,
            #[serde(
                serialize_with = "serialize_optional_u128",
                deserialize_with = "deserialize_optional_u128",
                default
            )]
            gas_price: Option<u128>,
            #[serde(
                serialize_with = "serialize_optional_u128",
                deserialize_with = "deserialize_optional_u128",
                default
            )]
            max_fee_per_gas: Option<u128>,
        }

        let repo = TestRedisRepository::new();

        // Test with very large u128 values that would overflow JSON numbers
        let original = TestU128Entity {
            id: "u128-test".to_string(),
            gas_price: Some(u128::MAX), // 340282366920938463463374607431768211455
            max_fee_per_gas: Some(999999999999999999999999999999999u128),
        };

        // Serialize
        let json = repo
            .serialize_entity(&original, |e| &e.id, "TestU128Entity")
            .unwrap();

        // Verify it contains string representations, not numbers
        assert!(json.contains("\"340282366920938463463374607431768211455\""));
        assert!(json.contains("\"999999999999999999999999999999999\""));
        // Make sure they're not stored as numbers (which would cause overflow)
        assert!(!json.contains("3.4028236692093846e+38"));

        // Deserialize
        let deserialized: TestU128Entity = repo
            .deserialize_entity(&json, "u128-test", "TestU128Entity")
            .unwrap();

        // Should be identical
        assert_eq!(original, deserialized);
        assert_eq!(deserialized.gas_price, Some(u128::MAX));
        assert_eq!(
            deserialized.max_fee_per_gas,
            Some(999999999999999999999999999999999u128)
        );
    }

    #[test]
    fn test_serialize_deserialize_u128_none_values() {
        use crate::utils::{deserialize_optional_u128, serialize_optional_u128};

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct TestU128Entity {
            id: String,
            #[serde(
                serialize_with = "serialize_optional_u128",
                deserialize_with = "deserialize_optional_u128",
                default
            )]
            gas_price: Option<u128>,
        }

        let repo = TestRedisRepository::new();

        // Test with None values
        let original = TestU128Entity {
            id: "u128-none-test".to_string(),
            gas_price: None,
        };

        // Serialize
        let json = repo
            .serialize_entity(&original, |e| &e.id, "TestU128Entity")
            .unwrap();

        // Should contain null
        assert!(json.contains("null"));

        // Deserialize
        let deserialized: TestU128Entity = repo
            .deserialize_entity(&json, "u128-none-test", "TestU128Entity")
            .unwrap();

        // Should be identical
        assert_eq!(original, deserialized);
        assert_eq!(deserialized.gas_price, None);
    }
}
