//! Base Redis repository functionality shared across all Redis implementations.
//!
//! This module provides common utilities and patterns used by all Redis repository
//! implementations to reduce code duplication and ensure consistency.

use crate::models::RepositoryError;
use log::{error, warn};
use redis::{aio::ConnectionManager, RedisError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
