//! Plugin Repository Module
//!
//! This module provides the plugin repository layer for the OpenZeppelin Relayer service.
//! It implements a specialized repository pattern for managing plugin configurations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **Plugin Management**: Store and retrieve plugin configurations
//! - **Path Resolution**: Manage plugin script paths for execution
//! - **Duplicate Prevention**: Ensure unique plugin IDs
//! - **Configuration Loading**: Convert from file configurations to repository models
//! - **Compiled Code Caching**: Cache pre-compiled JavaScript code for performance
//!
//! ## Repository Implementations
//!
//! - [`InMemoryPluginRepository`]: Fast in-memory storage for testing/development
//! - [`RedisPluginRepository`]: Redis-backed storage for production environments
//!
//! ## Plugin System
//!
//! The plugin system allows extending the relayer functionality through external scripts.
//! Each plugin is identified by a unique ID and contains a path to the executable script.
//!

pub mod plugin_in_memory;
pub mod plugin_redis;

pub use plugin_in_memory::*;
pub use plugin_redis::*;

use crate::utils::RedisConnections;
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

#[cfg(test)]
use mockall::automock;

use crate::{
    config::PluginFileConfig,
    constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS,
    models::{PaginationQuery, PluginModel, RepositoryError},
    repositories::{ConversionError, PaginatedResult},
};

#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait PluginRepositoryTrait {
    // Plugin CRUD operations
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError>;
    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError>;
    /// Update an existing plugin. Returns the updated plugin if found.
    async fn update(&self, plugin: PluginModel) -> Result<PluginModel, RepositoryError>;
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<PluginModel>, RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;
    async fn has_entries(&self) -> Result<bool, RepositoryError>;
    async fn drop_all_entries(&self) -> Result<(), RepositoryError>;

    // Compiled code cache operations
    /// Get compiled JavaScript code for a plugin
    async fn get_compiled_code(&self, plugin_id: &str) -> Result<Option<String>, RepositoryError>;
    /// Store compiled JavaScript code for a plugin
    async fn store_compiled_code(
        &self,
        plugin_id: &str,
        compiled_code: &str,
        source_hash: Option<&str>,
    ) -> Result<(), RepositoryError>;
    /// Invalidate cached code for a plugin
    async fn invalidate_compiled_code(&self, plugin_id: &str) -> Result<(), RepositoryError>;
    /// Invalidate all cached plugin code
    async fn invalidate_all_compiled_code(&self) -> Result<(), RepositoryError>;
    /// Check if a plugin has cached compiled code
    async fn has_compiled_code(&self, plugin_id: &str) -> Result<bool, RepositoryError>;
    /// Get the source hash for cache validation
    async fn get_source_hash(&self, plugin_id: &str) -> Result<Option<String>, RepositoryError>;
}

/// Enum wrapper for different plugin repository implementations
#[derive(Debug, Clone)]
pub enum PluginRepositoryStorage {
    InMemory(InMemoryPluginRepository),
    Redis(RedisPluginRepository),
}

impl PluginRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryPluginRepository::new())
    }

    pub fn new_redis(
        connections: Arc<RedisConnections>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        let redis_repo = RedisPluginRepository::new(connections, key_prefix)?;
        Ok(Self::Redis(redis_repo))
    }
}

#[async_trait]
impl PluginRepositoryTrait for PluginRepositoryStorage {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            PluginRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.add(plugin).await,
            PluginRepositoryStorage::Redis(repo) => repo.add(plugin).await,
        }
    }

    async fn update(&self, plugin: PluginModel) -> Result<PluginModel, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.update(plugin).await,
            PluginRepositoryStorage::Redis(repo) => repo.update(plugin).await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<PluginModel>, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            PluginRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.count().await,
            PluginRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            PluginRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            PluginRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }

    async fn get_compiled_code(&self, plugin_id: &str) -> Result<Option<String>, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.get_compiled_code(plugin_id).await,
            PluginRepositoryStorage::Redis(repo) => repo.get_compiled_code(plugin_id).await,
        }
    }

    async fn store_compiled_code(
        &self,
        plugin_id: &str,
        compiled_code: &str,
        source_hash: Option<&str>,
    ) -> Result<(), RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => {
                repo.store_compiled_code(plugin_id, compiled_code, source_hash)
                    .await
            }
            PluginRepositoryStorage::Redis(repo) => {
                repo.store_compiled_code(plugin_id, compiled_code, source_hash)
                    .await
            }
        }
    }

    async fn invalidate_compiled_code(&self, plugin_id: &str) -> Result<(), RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => {
                repo.invalidate_compiled_code(plugin_id).await
            }
            PluginRepositoryStorage::Redis(repo) => repo.invalidate_compiled_code(plugin_id).await,
        }
    }

    async fn invalidate_all_compiled_code(&self) -> Result<(), RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.invalidate_all_compiled_code().await,
            PluginRepositoryStorage::Redis(repo) => repo.invalidate_all_compiled_code().await,
        }
    }

    async fn has_compiled_code(&self, plugin_id: &str) -> Result<bool, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.has_compiled_code(plugin_id).await,
            PluginRepositoryStorage::Redis(repo) => repo.has_compiled_code(plugin_id).await,
        }
    }

    async fn get_source_hash(&self, plugin_id: &str) -> Result<Option<String>, RepositoryError> {
        match self {
            PluginRepositoryStorage::InMemory(repo) => repo.get_source_hash(plugin_id).await,
            PluginRepositoryStorage::Redis(repo) => repo.get_source_hash(plugin_id).await,
        }
    }
}

impl TryFrom<PluginFileConfig> for PluginModel {
    type Error = ConversionError;

    fn try_from(config: PluginFileConfig) -> Result<Self, Self::Error> {
        let timeout = Duration::from_secs(config.timeout.unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECONDS));

        Ok(PluginModel {
            id: config.id.clone(),
            path: config.path.clone(),
            timeout,
            emit_logs: config.emit_logs,
            emit_traces: config.emit_traces,
            raw_response: config.raw_response,
            allow_get_invocation: config.allow_get_invocation,
            config: config.config,
            forward_logs: config.forward_logs,
        })
    }
}

impl PartialEq for PluginModel {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.path == other.path
    }
}

#[cfg(test)]
mod tests {
    use crate::{config::PluginFileConfig, constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS};
    use std::time::Duration;

    use super::*;

    // ============================================
    // Helper functions
    // ============================================

    fn create_test_plugin(id: &str, path: &str) -> PluginModel {
        PluginModel {
            id: id.to_string(),
            path: path.to_string(),
            timeout: Duration::from_secs(30),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        }
    }

    fn create_test_plugin_with_options(
        id: &str,
        path: &str,
        emit_logs: bool,
        emit_traces: bool,
        raw_response: bool,
    ) -> PluginModel {
        PluginModel {
            id: id.to_string(),
            path: path.to_string(),
            timeout: Duration::from_secs(30),
            emit_logs,
            emit_traces,
            raw_response,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        }
    }

    // ============================================
    // PluginModel TryFrom tests
    // ============================================

    #[tokio::test]
    async fn test_try_from_default_timeout() {
        let config = PluginFileConfig {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: None,
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        };

        let result = PluginModel::try_from(config);
        assert!(result.is_ok());

        let plugin = result.unwrap();
        assert_eq!(plugin.id, "test-plugin");
        assert_eq!(plugin.path, "test-path");
        assert_eq!(
            plugin.timeout,
            Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS)
        );
    }

    #[tokio::test]
    async fn test_try_from_custom_timeout() {
        let config = PluginFileConfig {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Some(120),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        };

        let result = PluginModel::try_from(config);
        assert!(result.is_ok());

        let plugin = result.unwrap();
        assert_eq!(plugin.timeout, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn test_try_from_all_options_enabled() {
        let mut config_map = serde_json::Map::new();
        config_map.insert("key".to_string(), serde_json::json!("value"));

        let config = PluginFileConfig {
            id: "full-plugin".to_string(),
            path: "/scripts/full.js".to_string(),
            timeout: Some(60),
            emit_logs: true,
            emit_traces: true,
            raw_response: true,
            allow_get_invocation: true,
            config: Some(config_map),
            forward_logs: true,
        };

        let result = PluginModel::try_from(config);
        assert!(result.is_ok());

        let plugin = result.unwrap();
        assert_eq!(plugin.id, "full-plugin");
        assert!(plugin.emit_logs);
        assert!(plugin.emit_traces);
        assert!(plugin.raw_response);
        assert!(plugin.allow_get_invocation);
        assert!(plugin.config.is_some());
        assert!(plugin.forward_logs);
    }

    #[tokio::test]
    async fn test_try_from_zero_timeout() {
        let config = PluginFileConfig {
            id: "test".to_string(),
            path: "path".to_string(),
            timeout: Some(0),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        };

        let result = PluginModel::try_from(config);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().timeout, Duration::from_secs(0));
    }

    // ============================================
    // PluginModel PartialEq tests
    // ============================================

    #[test]
    fn test_plugin_model_equality_same_id_and_path() {
        let plugin1 = create_test_plugin("plugin-1", "/path/script.js");
        let plugin2 = create_test_plugin("plugin-1", "/path/script.js");

        assert_eq!(plugin1, plugin2);
    }

    #[test]
    fn test_plugin_model_equality_different_id() {
        let plugin1 = create_test_plugin("plugin-1", "/path/script.js");
        let plugin2 = create_test_plugin("plugin-2", "/path/script.js");

        assert_ne!(plugin1, plugin2);
    }

    #[test]
    fn test_plugin_model_equality_different_path() {
        let plugin1 = create_test_plugin("plugin-1", "/path/script1.js");
        let plugin2 = create_test_plugin("plugin-1", "/path/script2.js");

        assert_ne!(plugin1, plugin2);
    }

    #[test]
    fn test_plugin_model_equality_ignores_other_fields() {
        // Same id and path, different other fields
        let plugin1 =
            create_test_plugin_with_options("plugin-1", "/path/script.js", false, false, false);
        let plugin2 =
            create_test_plugin_with_options("plugin-1", "/path/script.js", true, true, true);

        // Should be equal because only id and path matter
        assert_eq!(plugin1, plugin2);
    }

    #[test]
    fn test_plugin_model_equality_different_timeout() {
        let mut plugin1 = create_test_plugin("plugin-1", "/path/script.js");
        plugin1.timeout = Duration::from_secs(30);

        let mut plugin2 = create_test_plugin("plugin-1", "/path/script.js");
        plugin2.timeout = Duration::from_secs(60);

        // Should be equal because timeout is not part of equality
        assert_eq!(plugin1, plugin2);
    }

    // ============================================
    // PluginRepositoryStorage constructor tests
    // ============================================

    #[tokio::test]
    async fn test_new_in_memory_creates_empty_storage() {
        let storage = PluginRepositoryStorage::new_in_memory();

        assert_eq!(storage.count().await.unwrap(), 0);
        assert!(!storage.has_entries().await.unwrap());
    }

    #[test]
    fn test_storage_enum_debug() {
        let storage = PluginRepositoryStorage::new_in_memory();
        let debug_str = format!("{storage:?}");
        assert!(debug_str.contains("InMemory"));
    }

    // ============================================
    // Basic CRUD tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_get_by_id_existing() {
        let storage = PluginRepositoryStorage::new_in_memory();
        let plugin = create_test_plugin("test-plugin", "/path/to/script.js");

        storage.add(plugin.clone()).await.unwrap();

        let result = storage.get_by_id("test-plugin").await.unwrap();
        assert_eq!(result, Some(plugin));
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_get_by_id_non_existing() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let result = storage.get_by_id("non-existent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_add_success() {
        let storage = PluginRepositoryStorage::new_in_memory();
        let plugin = create_test_plugin("test-plugin", "/path/to/script.js");

        let result = storage.add(plugin).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_add_duplicate() {
        let storage = PluginRepositoryStorage::new_in_memory();
        let plugin = create_test_plugin("test-plugin", "/path/to/script.js");

        storage.add(plugin.clone()).await.unwrap();

        // Try to add the same plugin again - should succeed (overwrite)
        let result = storage.add(plugin).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_add_multiple() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=10 {
            let plugin = create_test_plugin(&format!("plugin-{i}"), &format!("/path/{i}.js"));
            storage.add(plugin).await.unwrap();
        }

        assert_eq!(storage.count().await.unwrap(), 10);
    }

    // ============================================
    // Update tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_update_existing() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let plugin =
            create_test_plugin_with_options("test-plugin", "/path/script.js", false, false, false);
        storage.add(plugin).await.unwrap();

        let updated =
            create_test_plugin_with_options("test-plugin", "/path/script.js", true, true, true);
        let result = storage.update(updated.clone()).await;

        assert!(result.is_ok());
        let returned = result.unwrap();
        assert!(returned.emit_logs);
        assert!(returned.emit_traces);
        assert!(returned.raw_response);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_update_nonexistent() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let plugin = create_test_plugin("nonexistent", "/path/script.js");
        let result = storage.update(plugin).await;

        assert!(result.is_err());
        match result {
            Err(RepositoryError::NotFound(msg)) => {
                assert!(msg.contains("nonexistent"));
            }
            _ => panic!("Expected NotFound error"),
        }
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_update_persists_changes() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let plugin = create_test_plugin("test-plugin", "/path/script.js");
        storage.add(plugin).await.unwrap();

        let mut updated = create_test_plugin("test-plugin", "/path/updated.js");
        updated.emit_logs = true;
        storage.update(updated).await.unwrap();

        // Verify persisted changes
        let retrieved = storage.get_by_id("test-plugin").await.unwrap().unwrap();
        assert!(retrieved.emit_logs);
        assert_eq!(retrieved.path, "/path/updated.js");
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_update_does_not_affect_others() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .add(create_test_plugin("plugin-1", "/path/1.js"))
            .await
            .unwrap();
        storage
            .add(create_test_plugin("plugin-2", "/path/2.js"))
            .await
            .unwrap();
        storage
            .add(create_test_plugin("plugin-3", "/path/3.js"))
            .await
            .unwrap();

        let mut updated = create_test_plugin("plugin-2", "/path/updated.js");
        updated.emit_logs = true;
        storage.update(updated).await.unwrap();

        // Others unchanged
        let p1 = storage.get_by_id("plugin-1").await.unwrap().unwrap();
        assert_eq!(p1.path, "/path/1.js");
        assert!(!p1.emit_logs);

        let p3 = storage.get_by_id("plugin-3").await.unwrap().unwrap();
        assert_eq!(p3.path, "/path/3.js");
        assert!(!p3.emit_logs);
    }

    // ============================================
    // Count tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_count_empty() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_count_with_plugins() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .add(create_test_plugin("plugin1", "/path/1.js"))
            .await
            .unwrap();
        storage
            .add(create_test_plugin("plugin2", "/path/2.js"))
            .await
            .unwrap();
        storage
            .add(create_test_plugin("plugin3", "/path/3.js"))
            .await
            .unwrap();

        let count = storage.count().await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_count_after_drop() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=5 {
            storage
                .add(create_test_plugin(&format!("p{i}"), &format!("/{i}.js")))
                .await
                .unwrap();
        }

        assert_eq!(storage.count().await.unwrap(), 5);

        storage.drop_all_entries().await.unwrap();

        assert_eq!(storage.count().await.unwrap(), 0);
    }

    // ============================================
    // has_entries tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_has_entries_empty() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let has_entries = storage.has_entries().await.unwrap();
        assert!(!has_entries);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_has_entries_with_plugins() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .add(create_test_plugin("plugin1", "/path/1.js"))
            .await
            .unwrap();

        let has_entries = storage.has_entries().await.unwrap();
        assert!(has_entries);
    }

    // ============================================
    // drop_all_entries tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_drop_all_entries_empty() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let result = storage.drop_all_entries().await;
        assert!(result.is_ok());

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_drop_all_entries_with_plugins() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .add(create_test_plugin("plugin1", "/path/1.js"))
            .await
            .unwrap();
        storage
            .add(create_test_plugin("plugin2", "/path/2.js"))
            .await
            .unwrap();

        let result = storage.drop_all_entries().await;
        assert!(result.is_ok());

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);

        let has_entries = storage.has_entries().await.unwrap();
        assert!(!has_entries);
    }

    // ============================================
    // Pagination tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_list_paginated_empty() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 0);
        assert_eq!(result.total, 0);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 10);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_list_paginated_first_page() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=10 {
            storage
                .add(create_test_plugin(
                    &format!("plugin{i}"),
                    &format!("/{i}.js"),
                ))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 3);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_list_paginated_middle_page() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=10 {
            storage
                .add(create_test_plugin(
                    &format!("plugin{i}"),
                    &format!("/{i}.js"),
                ))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 2,
            per_page: 3,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 2);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_list_paginated_last_partial_page() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=10 {
            storage
                .add(create_test_plugin(
                    &format!("plugin{i}"),
                    &format!("/{i}.js"),
                ))
                .await
                .unwrap();
        }

        // 10 items, 3 per page, page 4 should have 1 item
        let query = PaginationQuery {
            page: 4,
            per_page: 3,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.total, 10);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_list_paginated_beyond_data() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=5 {
            storage
                .add(create_test_plugin(
                    &format!("plugin{i}"),
                    &format!("/{i}.js"),
                ))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 100,
            per_page: 10,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 0);
        assert_eq!(result.total, 5);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_list_paginated_large_per_page() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=5 {
            storage
                .add(create_test_plugin(
                    &format!("plugin{i}"),
                    &format!("/{i}.js"),
                ))
                .await
                .unwrap();
        }

        let query = PaginationQuery {
            page: 1,
            per_page: 100,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 5);
        assert_eq!(result.total, 5);
    }

    // ============================================
    // Compiled code cache tests
    // ============================================

    #[tokio::test]
    async fn test_store_and_get_compiled_code() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .store_compiled_code("plugin-1", "compiled code", None)
            .await
            .unwrap();

        let code = storage.get_compiled_code("plugin-1").await.unwrap();
        assert_eq!(code, Some("compiled code".to_string()));
    }

    #[tokio::test]
    async fn test_get_compiled_code_nonexistent() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let code = storage.get_compiled_code("nonexistent").await.unwrap();
        assert_eq!(code, None);
    }

    #[tokio::test]
    async fn test_store_compiled_code_with_source_hash() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .store_compiled_code("plugin-1", "code", Some("sha256:abc123"))
            .await
            .unwrap();

        let code = storage.get_compiled_code("plugin-1").await.unwrap();
        assert_eq!(code, Some("code".to_string()));

        let hash = storage.get_source_hash("plugin-1").await.unwrap();
        assert_eq!(hash, Some("sha256:abc123".to_string()));
    }

    #[tokio::test]
    async fn test_store_compiled_code_overwrites() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .store_compiled_code("plugin-1", "old code", Some("old-hash"))
            .await
            .unwrap();
        storage
            .store_compiled_code("plugin-1", "new code", Some("new-hash"))
            .await
            .unwrap();

        let code = storage.get_compiled_code("plugin-1").await.unwrap();
        assert_eq!(code, Some("new code".to_string()));

        let hash = storage.get_source_hash("plugin-1").await.unwrap();
        assert_eq!(hash, Some("new-hash".to_string()));
    }

    #[tokio::test]
    async fn test_has_compiled_code() {
        let storage = PluginRepositoryStorage::new_in_memory();

        assert!(!storage.has_compiled_code("plugin-1").await.unwrap());

        storage
            .store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();

        assert!(storage.has_compiled_code("plugin-1").await.unwrap());
        assert!(!storage.has_compiled_code("plugin-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_invalidate_compiled_code() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .store_compiled_code("plugin-1", "code1", None)
            .await
            .unwrap();
        storage
            .store_compiled_code("plugin-2", "code2", None)
            .await
            .unwrap();

        storage.invalidate_compiled_code("plugin-1").await.unwrap();

        assert!(!storage.has_compiled_code("plugin-1").await.unwrap());
        assert!(storage.has_compiled_code("plugin-2").await.unwrap());
    }

    #[tokio::test]
    async fn test_invalidate_compiled_code_nonexistent() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Should not fail
        let result = storage.invalidate_compiled_code("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_invalidate_all_compiled_code() {
        let storage = PluginRepositoryStorage::new_in_memory();

        for i in 1..=5 {
            storage
                .store_compiled_code(&format!("plugin-{i}"), &format!("code-{i}"), None)
                .await
                .unwrap();
        }

        storage.invalidate_all_compiled_code().await.unwrap();

        for i in 1..=5 {
            assert!(!storage
                .has_compiled_code(&format!("plugin-{i}"))
                .await
                .unwrap());
        }
    }

    #[tokio::test]
    async fn test_invalidate_all_compiled_code_empty() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Should not fail
        let result = storage.invalidate_all_compiled_code().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_source_hash() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // No hash
        storage
            .store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();
        let hash = storage.get_source_hash("plugin-1").await.unwrap();
        assert_eq!(hash, None);

        // With hash
        storage
            .store_compiled_code("plugin-2", "code", Some("hash123"))
            .await
            .unwrap();
        let hash = storage.get_source_hash("plugin-2").await.unwrap();
        assert_eq!(hash, Some("hash123".to_string()));
    }

    #[tokio::test]
    async fn test_get_source_hash_nonexistent() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let hash = storage.get_source_hash("nonexistent").await.unwrap();
        assert_eq!(hash, None);
    }

    // ============================================
    // Cache independence tests
    // ============================================

    #[tokio::test]
    async fn test_compiled_cache_independent_of_plugin_store() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Store compiled code without adding plugin
        storage
            .store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();

        // Plugin doesn't exist
        assert!(storage.get_by_id("plugin-1").await.unwrap().is_none());

        // But compiled code does
        assert!(storage.has_compiled_code("plugin-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_drop_all_does_not_clear_compiled_cache() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .add(create_test_plugin("plugin-1", "/path.js"))
            .await
            .unwrap();
        storage
            .store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();

        storage.drop_all_entries().await.unwrap();

        // Plugin gone
        assert!(storage.get_by_id("plugin-1").await.unwrap().is_none());

        // Compiled cache still has entry
        assert!(storage.has_compiled_code("plugin-1").await.unwrap());
    }

    #[tokio::test]
    async fn test_invalidate_all_compiled_does_not_clear_store() {
        let storage = PluginRepositoryStorage::new_in_memory();

        storage
            .add(create_test_plugin("plugin-1", "/path.js"))
            .await
            .unwrap();
        storage
            .store_compiled_code("plugin-1", "code", None)
            .await
            .unwrap();

        storage.invalidate_all_compiled_code().await.unwrap();

        // Compiled cache cleared
        assert!(!storage.has_compiled_code("plugin-1").await.unwrap());

        // Plugin still exists
        assert!(storage.get_by_id("plugin-1").await.unwrap().is_some());
    }

    // ============================================
    // Workflow/integration tests
    // ============================================

    #[tokio::test]
    async fn test_plugin_repository_storage_workflow() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Initially empty
        assert!(!storage.has_entries().await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 0);

        // Add plugins
        let plugin1 = create_test_plugin("auth-plugin", "/scripts/auth.js");
        let plugin2 = create_test_plugin("email-plugin", "/scripts/email.js");

        storage.add(plugin1.clone()).await.unwrap();
        storage.add(plugin2.clone()).await.unwrap();

        // Check state
        assert!(storage.has_entries().await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 2);

        // Retrieve specific plugin
        let retrieved = storage.get_by_id("auth-plugin").await.unwrap();
        assert_eq!(retrieved, Some(plugin1));

        // Update plugin
        let mut updated = create_test_plugin("auth-plugin", "/scripts/auth_v2.js");
        updated.emit_logs = true;
        storage.update(updated).await.unwrap();

        let after_update = storage.get_by_id("auth-plugin").await.unwrap().unwrap();
        assert_eq!(after_update.path, "/scripts/auth_v2.js");
        assert!(after_update.emit_logs);

        // List all plugins
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = storage.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total, 2);

        // Clear all plugins
        storage.drop_all_entries().await.unwrap();
        assert!(!storage.has_entries().await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_compiled_code_workflow() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Add plugin
        storage
            .add(create_test_plugin("my-plugin", "/scripts/plugin.js"))
            .await
            .unwrap();

        // Initially no compiled code
        assert!(!storage.has_compiled_code("my-plugin").await.unwrap());

        // Store compiled code
        storage
            .store_compiled_code("my-plugin", "compiled JS", Some("hash-v1"))
            .await
            .unwrap();

        // Verify
        assert!(storage.has_compiled_code("my-plugin").await.unwrap());
        assert_eq!(
            storage.get_compiled_code("my-plugin").await.unwrap(),
            Some("compiled JS".to_string())
        );
        assert_eq!(
            storage.get_source_hash("my-plugin").await.unwrap(),
            Some("hash-v1".to_string())
        );

        // Update compiled code
        storage
            .store_compiled_code("my-plugin", "updated JS", Some("hash-v2"))
            .await
            .unwrap();

        assert_eq!(
            storage.get_compiled_code("my-plugin").await.unwrap(),
            Some("updated JS".to_string())
        );
        assert_eq!(
            storage.get_source_hash("my-plugin").await.unwrap(),
            Some("hash-v2".to_string())
        );

        // Invalidate
        storage.invalidate_compiled_code("my-plugin").await.unwrap();

        assert!(!storage.has_compiled_code("my-plugin").await.unwrap());
        assert_eq!(storage.get_compiled_code("my-plugin").await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_multiple_plugins_compiled_code() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Store compiled code for multiple plugins
        for i in 1..=5 {
            storage
                .store_compiled_code(
                    &format!("plugin-{i}"),
                    &format!("code for plugin {i}"),
                    Some(&format!("hash-{i}")),
                )
                .await
                .unwrap();
        }

        // Verify all
        for i in 1..=5 {
            assert!(storage
                .has_compiled_code(&format!("plugin-{i}"))
                .await
                .unwrap());
            assert_eq!(
                storage
                    .get_compiled_code(&format!("plugin-{i}"))
                    .await
                    .unwrap(),
                Some(format!("code for plugin {i}"))
            );
            assert_eq!(
                storage
                    .get_source_hash(&format!("plugin-{i}"))
                    .await
                    .unwrap(),
                Some(format!("hash-{i}"))
            );
        }

        // Invalidate one
        storage.invalidate_compiled_code("plugin-3").await.unwrap();

        // Verify selective invalidation
        assert!(storage.has_compiled_code("plugin-1").await.unwrap());
        assert!(storage.has_compiled_code("plugin-2").await.unwrap());
        assert!(!storage.has_compiled_code("plugin-3").await.unwrap());
        assert!(storage.has_compiled_code("plugin-4").await.unwrap());
        assert!(storage.has_compiled_code("plugin-5").await.unwrap());
    }
}
