//! This module provides an in-memory implementation of plugins.
//!
//! The `InMemoryPluginRepository` struct is used to store and retrieve plugins
//! script paths for further execution.
use crate::{
    models::{PaginationQuery, PluginModel},
    repositories::{PaginatedResult, PluginRepositoryTrait, RepositoryError},
};

use async_trait::async_trait;

use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryPluginRepository {
    store: Mutex<HashMap<String, PluginModel>>,
}

impl Clone for InMemoryPluginRepository {
    fn clone(&self) -> Self {
        // Try to get the current data, or use empty HashMap if lock fails
        let data = self
            .store
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| HashMap::new());

        Self {
            store: Mutex::new(data),
        }
    }
}

impl InMemoryPluginRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.get(id).cloned())
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemoryPluginRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PluginRepositoryTrait for InMemoryPluginRepository {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.get(id).cloned())
    }

    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.insert(plugin.id.clone(), plugin);
        Ok(())
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<PluginModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;

        let items = self
            .store
            .lock()
            .await
            .values()
            .skip(start)
            .take(query.per_page as usize)
            .cloned()
            .collect();

        Ok(PaginatedResult {
            items,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = self.store.lock().await;
        Ok(store.len())
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(!store.is_empty())
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{config::PluginFileConfig, constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS};

    use super::*;
    use std::{sync::Arc, time::Duration};

    #[tokio::test]
    async fn test_in_memory_plugin_repository() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        // Test add and get_by_id
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
        };
        plugin_repository.add(plugin.clone()).await.unwrap();
        assert_eq!(
            plugin_repository.get_by_id("test-plugin").await.unwrap(),
            Some(plugin)
        );
    }

    #[tokio::test]
    async fn test_get_nonexistent_plugin() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        let result = plugin_repository.get_by_id("test-plugin").await;
        assert!(matches!(result, Ok(None)));
    }

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

    #[tokio::test]
    async fn test_get_by_id() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
        };
        plugin_repository.add(plugin.clone()).await.unwrap();
        assert_eq!(
            plugin_repository.get_by_id("test-plugin").await.unwrap(),
            Some(plugin)
        );
    }

    #[tokio::test]
    async fn test_list_paginated() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        let plugin1 = PluginModel {
            id: "test-plugin1".to_string(),
            path: "test-path1".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
        };

        let plugin2 = PluginModel {
            id: "test-plugin2".to_string(),
            path: "test-path2".to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
        };

        plugin_repository.add(plugin1.clone()).await.unwrap();
        plugin_repository.add(plugin2.clone()).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };

        let result = plugin_repository.list_paginated(query).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.items.len(), 2);
    }

    #[tokio::test]
    async fn test_has_entries() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());
        assert!(!plugin_repository.has_entries().await.unwrap());
        plugin_repository
            .add(PluginModel {
                id: "test-plugin".to_string(),
                path: "test-path".to_string(),
                timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            })
            .await
            .unwrap();

        assert!(plugin_repository.has_entries().await.unwrap());
        plugin_repository.drop_all_entries().await.unwrap();
        assert!(!plugin_repository.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_drop_all_entries() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());
        plugin_repository
            .add(PluginModel {
                id: "test-plugin".to_string(),
                path: "test-path".to_string(),
                timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
            })
            .await
            .unwrap();

        assert!(plugin_repository.has_entries().await.unwrap());
        plugin_repository.drop_all_entries().await.unwrap();
        assert!(!plugin_repository.has_entries().await.unwrap());
    }
}
