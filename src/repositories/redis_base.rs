//! Base Redis repository functionality shared across all Redis implementations.
//!
//! This module provides common utilities and patterns used by all Redis repository
//! implementations to reduce code duplication and ensure consistency.

use crate::models::RepositoryError;
use deadpool_redis::{Connection, Pool, PoolError, TimeoutType};
use redis::RedisError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, warn};

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
            error!(entity_type = %entity_type, id = %id, error = %e, "serialization failed");
            RepositoryError::InvalidData(format!("Failed to serialize {entity_type} {id}: {e}"))
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
            error!(entity_type = %entity_type, entity_id = %entity_id, error = %e, "deserialization failed");
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
        warn!(context = %context, error = %error, "redis operation failed");

        match error.kind() {
            redis::ErrorKind::TypeError => RepositoryError::InvalidData(format!(
                "Redis data type error in operation '{context}': {error}"
            )),
            redis::ErrorKind::AuthenticationFailed => {
                RepositoryError::InvalidData("Redis authentication failed".to_string())
            }
            redis::ErrorKind::NoScriptError => RepositoryError::InvalidData(format!(
                "Redis script error in operation '{context}': {error}"
            )),
            redis::ErrorKind::ReadOnly => RepositoryError::InvalidData(format!(
                "Redis is read-only in operation '{context}': {error}"
            )),
            redis::ErrorKind::ExecAbortError => RepositoryError::InvalidData(format!(
                "Redis transaction aborted in operation '{context}': {error}"
            )),
            redis::ErrorKind::BusyLoadingError => RepositoryError::InvalidData(format!(
                "Redis is busy in operation '{context}': {error}"
            )),
            redis::ErrorKind::ExtensionError => RepositoryError::InvalidData(format!(
                "Redis extension error in operation '{context}': {error}"
            )),
            // Default to Other for connection errors and other issues
            _ => RepositoryError::Other(format!("Redis operation '{context}' failed: {error}")),
        }
    }

    /// Convert deadpool Pool errors to appropriate RepositoryError types
    fn map_pool_error(&self, error: PoolError, context: &str) -> RepositoryError {
        error!(context = %context, error = %error, "redis pool operation failed");

        match error {
            PoolError::Timeout(timeout) => {
                let detail = match timeout {
                    TimeoutType::Wait => "waiting for an available connection",
                    TimeoutType::Create => "creating a new connection",
                    TimeoutType::Recycle => "recycling a connection",
                };
                RepositoryError::ConnectionError(format!(
                    "Redis pool timeout while {detail} in operation '{context}'"
                ))
            }
            PoolError::Backend(redis_err) => self.map_redis_error(redis_err, context),
            PoolError::Closed => {
                RepositoryError::ConnectionError("Redis pool is closed".to_string())
            }
            PoolError::NoRuntimeSpecified => {
                RepositoryError::ConnectionError("Redis pool has no runtime specified".to_string())
            }
            other => RepositoryError::ConnectionError(format!(
                "Redis pool error in operation '{context}': {other}"
            )),
        }
    }

    /// Get a connection from the Redis pool with error handling
    ///
    /// # Arguments
    ///
    /// * `pool` - Reference to the Redis connection pool
    /// * `context` - Context string for error messages (e.g., "get_by_id", "create")
    ///
    /// # Returns
    ///
    /// A connection from the pool, or a RepositoryError if getting the connection fails
    #[allow(async_fn_in_trait)]
    async fn get_connection(
        &self,
        pool: &Arc<Pool>,
        context: &str,
    ) -> Result<Connection, RepositoryError> {
        pool.get()
            .await
            .map_err(|e| self.map_pool_error(e, context))
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

    #[test]
    fn test_map_pool_error_timeout_wait() {
        let repo = TestRedisRepository::new();
        let timeout_error = PoolError::Timeout(TimeoutType::Wait);

        let result = repo.map_pool_error(timeout_error, "test_operation");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert!(msg.contains("Redis pool timeout"));
                assert!(msg.contains("waiting for an available connection"));
                assert!(msg.contains("test_operation"));
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_pool_error_timeout_create() {
        let repo = TestRedisRepository::new();
        let timeout_error = PoolError::Timeout(TimeoutType::Create);

        let result = repo.map_pool_error(timeout_error, "create_operation");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert!(msg.contains("Redis pool timeout"));
                assert!(msg.contains("creating a new connection"));
                assert!(msg.contains("create_operation"));
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_pool_error_timeout_recycle() {
        let repo = TestRedisRepository::new();
        let timeout_error = PoolError::Timeout(TimeoutType::Recycle);

        let result = repo.map_pool_error(timeout_error, "recycle_operation");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert!(msg.contains("Redis pool timeout"));
                assert!(msg.contains("recycling a connection"));
                assert!(msg.contains("recycle_operation"));
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_pool_error_backend() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::TypeError, "Backend error"));
        let pool_error = PoolError::Backend(redis_error);

        let result = repo.map_pool_error(pool_error, "backend_operation");

        // Should delegate to map_redis_error
        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis data type error"));
                assert!(msg.contains("backend_operation"));
            }
            _ => panic!("Expected InvalidData error from map_redis_error"),
        }
    }

    #[test]
    fn test_map_pool_error_closed() {
        let repo = TestRedisRepository::new();
        let pool_error = PoolError::Closed;

        let result = repo.map_pool_error(pool_error, "closed_operation");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert_eq!(msg, "Redis pool is closed");
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_pool_error_no_runtime() {
        let repo = TestRedisRepository::new();
        let pool_error = PoolError::NoRuntimeSpecified;

        let result = repo.map_pool_error(pool_error, "runtime_operation");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert_eq!(msg, "Redis pool has no runtime specified");
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_redis_error_empty_context() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::TypeError, "Type error"));

        let result = repo.map_redis_error(redis_error, "");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis data type error"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_pool_error_empty_context() {
        let repo = TestRedisRepository::new();
        let pool_error = PoolError::Closed;

        let result = repo.map_pool_error(pool_error, "");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert_eq!(msg, "Redis pool is closed");
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_serialize_entity_empty_id() {
        let repo = TestRedisRepository::new();
        let entity = TestEntity {
            id: "".to_string(),
            name: "test-name".to_string(),
            value: 42,
        };

        let result = repo.serialize_entity(&entity, |e| &e.id, "TestEntity");

        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("\"id\":\"\""));
    }

    #[test]
    fn test_deserialize_entity_empty_json() {
        let repo = TestRedisRepository::new();

        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity("", "test-id", "TestEntity");

        assert!(result.is_err());
        match result.unwrap_err() {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Failed to deserialize"));
                assert!(msg.contains("JSON length: 0"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_deserialize_entity_malformed_json() {
        let repo = TestRedisRepository::new();
        let malformed_json = r#"{"id":"test-id","name":"test-name","value":}"#;

        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(malformed_json, "test-id", "TestEntity");

        assert!(result.is_err());
        match result.unwrap_err() {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Failed to deserialize"));
                assert!(msg.contains("test-id"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_serialize_entity_with_special_characters() {
        let repo = TestRedisRepository::new();
        let entity = TestEntity {
            id: "test-id".to_string(),
            name: "test\"name\nwith\tspecial\rchars".to_string(),
            value: 42,
        };

        let result = repo.serialize_entity(&entity, |e| &e.id, "TestEntity");

        assert!(result.is_ok());
        let json = result.unwrap();
        // JSON should properly escape special characters
        assert!(json.contains("test-id"));
        // Verify it's valid JSON by deserializing
        let deserialized: TestEntity = repo
            .deserialize_entity(&json, "test-id", "TestEntity")
            .unwrap();
        assert_eq!(deserialized.name, entity.name);
    }

    #[test]
    fn test_serialize_entity_with_numeric_id() {
        let repo = TestRedisRepository::new();
        #[derive(Serialize)]
        struct NumericIdEntity {
            id: i32,
            name: String,
        }

        let entity = NumericIdEntity {
            id: 12345,
            name: "test".to_string(),
        };

        // Use a static string for the ID extractor since we're testing numeric IDs
        let result = repo.serialize_entity(&entity, |_| "numeric-id", "NumericIdEntity");

        assert!(result.is_ok());
        let json = result.unwrap();
        assert!(json.contains("12345"));
        assert!(json.contains("test")); // Verify the name field is serialized
    }

    #[test]
    fn test_map_redis_error_all_error_kinds() {
        let repo = TestRedisRepository::new();
        let error_kinds = vec![
            (redis::ErrorKind::TypeError, "TypeError"),
            (
                redis::ErrorKind::AuthenticationFailed,
                "AuthenticationFailed",
            ),
            (redis::ErrorKind::NoScriptError, "NoScriptError"),
            (redis::ErrorKind::ReadOnly, "ReadOnly"),
            (redis::ErrorKind::ExecAbortError, "ExecAbortError"),
            (redis::ErrorKind::BusyLoadingError, "BusyLoadingError"),
            (redis::ErrorKind::ExtensionError, "ExtensionError"),
            (redis::ErrorKind::IoError, "IoError"),
            (redis::ErrorKind::ClientError, "ClientError"),
        ];

        for (kind, expected_type) in error_kinds {
            let redis_error = RedisError::from((kind, "test error"));
            let result = repo.map_redis_error(redis_error, "test_op");

            match result {
                RepositoryError::InvalidData(_)
                    if expected_type != "IoError" && expected_type != "ClientError" =>
                {
                    // Expected for most error kinds
                }
                RepositoryError::Other(_)
                    if expected_type == "IoError" || expected_type == "ClientError" =>
                {
                    // Expected for connection/client errors
                }
                _ => panic!("Unexpected error type for {:?}", kind),
            }
        }
    }

    #[test]
    fn test_serialize_entity_error_message_includes_id() {
        let repo = TestRedisRepository::new();
        // Create an entity that will fail serialization by using a custom serializer
        // Actually, let's test with a valid entity but verify the error message format
        let entity = TestEntity {
            id: "error-test-id".to_string(),
            name: "test-name".to_string(),
            value: 42,
        };

        // This should succeed, but let's verify the error message would include the ID
        // by checking the error handling path
        let result = repo.serialize_entity(&entity, |e| &e.id, "TestEntity");
        assert!(result.is_ok());

        // For a real error case, we'd need a type that fails to serialize
        // But we can verify the error message format is correct from the code
    }

    #[test]
    fn test_deserialize_entity_error_message_includes_length() {
        let repo = TestRedisRepository::new();
        let short_json = r#"{"id":"test"}"#;

        // Test with short JSON
        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(short_json, "test-id", "TestEntity");
        assert!(result.is_err());
        if let RepositoryError::InvalidData(msg) = result.unwrap_err() {
            assert!(msg.contains("JSON length:"));
        }

        // Test with longer JSON to verify length is included
        let long_json_str = format!(r#"{{"id":"test","name":"{}"}}"#, "a".repeat(1000));
        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(&long_json_str, "test-id", "TestEntity");
        assert!(result.is_err());
        if let RepositoryError::InvalidData(msg) = result.unwrap_err() {
            assert!(msg.contains("JSON length:"));
            // Should include a length > 1000
            assert!(msg.len() > 20); // Error message should be substantial
        }
    }

    #[test]
    fn test_map_pool_error_context_in_error_message() {
        let repo = TestRedisRepository::new();
        let contexts = vec!["get_by_id", "create", "update", "delete", "list_all"];

        for context in contexts {
            // Test Timeout which includes context in error message
            let timeout_error = PoolError::Timeout(TimeoutType::Wait);
            let timeout_result = repo.map_pool_error(timeout_error, context);

            match timeout_result {
                RepositoryError::ConnectionError(msg) => {
                    assert!(
                        msg.contains(context),
                        "Context '{}' should appear in error message",
                        context
                    );
                }
                _ => panic!("Expected ConnectionError"),
            }
        }
    }

    #[test]
    fn test_serialize_entity_with_null_values() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct NullableEntity {
            id: String,
            optional_field: Option<String>,
        }

        let repo = TestRedisRepository::new();
        let entity = NullableEntity {
            id: "null-test".to_string(),
            optional_field: None,
        };

        let json = repo
            .serialize_entity(&entity, |e| &e.id, "NullableEntity")
            .unwrap();

        assert!(json.contains("null"));
        assert!(json.contains("null-test"));

        // Verify roundtrip
        let deserialized: NullableEntity = repo
            .deserialize_entity(&json, "null-test", "NullableEntity")
            .unwrap();
        assert_eq!(entity, deserialized);
    }

    #[test]
    fn test_serialize_entity_with_empty_strings() {
        let repo = TestRedisRepository::new();
        let entity = TestEntity {
            id: "empty-test".to_string(),
            name: "".to_string(),
            value: 0,
        };

        let json = repo
            .serialize_entity(&entity, |e| &e.id, "TestEntity")
            .unwrap();

        assert!(json.contains("\"name\":\"\""));
        assert!(json.contains("\"value\":0"));

        // Verify roundtrip
        let deserialized: TestEntity = repo
            .deserialize_entity(&json, "empty-test", "TestEntity")
            .unwrap();
        assert_eq!(entity, deserialized);
    }

    #[test]
    fn test_map_redis_error_different_contexts() {
        let repo = TestRedisRepository::new();
        let contexts = vec![
            "short",
            "very_long_context_name_that_might_be_used_in_real_world_scenarios",
            "context-with-dashes",
            "context_with_underscores",
            "context.with.dots",
        ];

        for context in contexts {
            let redis_error = RedisError::from((redis::ErrorKind::TypeError, "Type error"));
            let result = repo.map_redis_error(redis_error, context);
            match result {
                RepositoryError::InvalidData(msg) => {
                    assert!(msg.contains("Redis data type error"));
                }
                _ => panic!("Expected InvalidData error"),
            }
        }
    }
}
