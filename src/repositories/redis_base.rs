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
        match error.kind() {
            redis::ErrorKind::IoError => {
                error!("Redis IO error in {}: {}", context, error);
                RepositoryError::ConnectionError(format!("Redis connection failed: {}", error))
            }
            redis::ErrorKind::AuthenticationFailed => {
                error!("Redis authentication failed in {}: {}", context, error);
                RepositoryError::PermissionDenied(format!("Redis authentication failed: {}", error))
            }
            redis::ErrorKind::TypeError => {
                error!("Redis type error in {}: {}", context, error);
                RepositoryError::InvalidData(format!("Redis data type error: {}", error))
            }
            redis::ErrorKind::ExecAbortError => {
                warn!("Redis transaction aborted in {}: {}", context, error);
                RepositoryError::TransactionFailure(format!("Redis transaction aborted: {}", error))
            }
            redis::ErrorKind::BusyLoadingError => {
                warn!("Redis busy loading in {}: {}", context, error);
                RepositoryError::ConnectionError(format!("Redis is loading: {}", error))
            }
            redis::ErrorKind::NoScriptError => {
                error!("Redis script error in {}: {}", context, error);
                RepositoryError::Other(format!("Redis script error: {}", error))
            }
            _ => {
                error!("Unexpected Redis error in {}: {}", context, error);
                RepositoryError::Other(format!("Redis error in {}: {}", context, error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::io;

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
    fn test_deserialize_entity_wrong_structure() {
        let repo = TestRedisRepository::new();
        let json = r#"{"wrong":"field"}"#;

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
    fn test_deserialize_entity_empty_json() {
        let repo = TestRedisRepository::new();
        let json = "";

        let result: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(json, "test-id", "TestEntity");

        assert!(result.is_err());
        match result.unwrap_err() {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Failed to deserialize TestEntity test-id"));
                assert!(msg.contains("JSON length: 0"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_deserialize_entity_simple_struct() {
        let repo = TestRedisRepository::new();
        let json = r#"{"id":"simple-id"}"#;

        let result: Result<SimpleEntity, RepositoryError> =
            repo.deserialize_entity(json, "simple-id", "SimpleEntity");

        assert!(result.is_ok());
        let entity = result.unwrap();
        assert_eq!(entity.id, "simple-id");
    }

    #[test]
    fn test_map_redis_error_io_error() {
        let repo = TestRedisRepository::new();
        let io_error = io::Error::new(io::ErrorKind::ConnectionRefused, "Connection refused");
        let redis_error = RedisError::from(io_error);

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert!(msg.contains("Redis connection failed"));
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_redis_error_authentication_failed() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::AuthenticationFailed, "Auth failed"));

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::PermissionDenied(msg) => {
                assert!(msg.contains("Redis authentication failed"));
            }
            _ => panic!("Expected PermissionDenied error"),
        }
    }

    #[test]
    fn test_map_redis_error_type_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::TypeError, "Type error"));

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::InvalidData(msg) => {
                assert!(msg.contains("Redis data type error"));
            }
            _ => panic!("Expected InvalidData error"),
        }
    }

    #[test]
    fn test_map_redis_error_exec_abort_error() {
        let repo = TestRedisRepository::new();
        let redis_error =
            RedisError::from((redis::ErrorKind::ExecAbortError, "Transaction aborted"));

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::TransactionFailure(msg) => {
                assert!(msg.contains("Redis transaction aborted"));
            }
            _ => panic!("Expected TransactionFailure error"),
        }
    }

    #[test]
    fn test_map_redis_error_busy_loading_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::BusyLoadingError, "Loading"));

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::ConnectionError(msg) => {
                assert!(msg.contains("Redis is loading"));
            }
            _ => panic!("Expected ConnectionError"),
        }
    }

    #[test]
    fn test_map_redis_error_no_script_error() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::NoScriptError, "Script not found"));

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::Other(msg) => {
                assert!(msg.contains("Redis script error"));
            }
            _ => panic!("Expected Other error"),
        }
    }

    #[test]
    fn test_map_redis_error_cluster_down() {
        let repo = TestRedisRepository::new();
        let redis_error = RedisError::from((redis::ErrorKind::ClusterDown, "Cluster down"));

        let result = repo.map_redis_error(redis_error, "test_context");

        match result {
            RepositoryError::Other(msg) => {
                assert!(msg.contains("Redis error in test_context"));
            }
            _ => panic!("Expected Other error"),
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

    #[test]
    fn test_deserialize_entity_with_optional_fields() {
        let repo = TestRedisRepository::new();

        #[derive(Deserialize, Debug, PartialEq)]
        struct OptionalEntity {
            id: String,
            optional_field: Option<String>,
        }

        // Test with optional field present
        let json_with_optional = r#"{"id":"test-id","optional_field":"present"}"#;
        let result: Result<OptionalEntity, RepositoryError> =
            repo.deserialize_entity(json_with_optional, "test-id", "OptionalEntity");

        assert!(result.is_ok());
        let entity = result.unwrap();
        assert_eq!(entity.id, "test-id");
        assert_eq!(entity.optional_field, Some("present".to_string()));

        // Test with optional field missing
        let json_without_optional = r#"{"id":"test-id"}"#;
        let result: Result<OptionalEntity, RepositoryError> =
            repo.deserialize_entity(json_without_optional, "test-id", "OptionalEntity");

        assert!(result.is_ok());
        let entity = result.unwrap();
        assert_eq!(entity.id, "test-id");
        assert_eq!(entity.optional_field, None);
    }

    #[test]
    fn test_error_propagation_with_different_entity_types() {
        let repo = TestRedisRepository::new();

        // Test with different entity types to ensure error messages are correct
        let invalid_json = r#"{"invalid": "json"}"#;

        let result1: Result<TestEntity, RepositoryError> =
            repo.deserialize_entity(invalid_json, "id1", "TestEntity");
        let result2: Result<SimpleEntity, RepositoryError> =
            repo.deserialize_entity(invalid_json, "id2", "SimpleEntity");

        assert!(result1.is_err());
        assert!(result2.is_err());

        let error1 = result1.unwrap_err();
        let error2 = result2.unwrap_err();

        match (error1, error2) {
            (RepositoryError::InvalidData(msg1), RepositoryError::InvalidData(msg2)) => {
                assert!(msg1.contains("TestEntity id1"));
                assert!(msg2.contains("SimpleEntity id2"));
            }
            _ => panic!("Expected InvalidData errors"),
        }
    }
}
