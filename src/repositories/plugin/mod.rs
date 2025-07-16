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

use async_trait::async_trait;
use redis::aio::ConnectionManager;
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
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError>;
    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError>;
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<PluginModel>, RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;
    async fn has_entries(&self) -> Result<bool, RepositoryError>;
    async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
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
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        let redis_repo = RedisPluginRepository::new(connection_manager, key_prefix)?;
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
}

impl TryFrom<PluginFileConfig> for PluginModel {
    type Error = ConversionError;

    fn try_from(config: PluginFileConfig) -> Result<Self, Self::Error> {
        let timeout = Duration::from_secs(config.timeout.unwrap_or(DEFAULT_PLUGIN_TIMEOUT_SECONDS));

        Ok(PluginModel {
            id: config.id.clone(),
            path: config.path.clone(),
            timeout,
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

    #[tokio::test]
    async fn test_try_from() {
        let plugin = PluginFileConfig {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: None,
        };
        let result = PluginModel::try_from(plugin);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            PluginModel {
                id: "test-plugin".to_string(),
                path: "test-path".to_string(),
                timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            }
        );
    }

    // Helper function to create a test plugin
    fn create_test_plugin(id: &str, path: &str) -> PluginModel {
        PluginModel {
            id: id.to_string(),
            path: path.to_string(),
            timeout: Duration::from_secs(30),
        }
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_get_by_id_existing() {
        let storage = PluginRepositoryStorage::new_in_memory();
        let plugin = create_test_plugin("test-plugin", "/path/to/script.js");

        // Add the plugin first
        storage.add(plugin.clone()).await.unwrap();

        // Get the plugin
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

        // Add the plugin first time
        storage.add(plugin.clone()).await.unwrap();

        // Try to add the same plugin again - should succeed (overwrite)
        let result = storage.add(plugin).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_count_empty() {
        let storage = PluginRepositoryStorage::new_in_memory();

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_plugin_repository_storage_count_with_plugins() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Add multiple plugins
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

        // Add multiple plugins
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
    async fn test_plugin_repository_storage_list_paginated_with_plugins() {
        let storage = PluginRepositoryStorage::new_in_memory();

        // Add multiple plugins
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

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total, 3);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 2);
    }

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
}
