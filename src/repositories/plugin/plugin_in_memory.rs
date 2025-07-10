//! This module provides an in-memory implementation of plugins.
//!
//! The `InMemoryPluginRepository` struct is used to store and retrieve plugins
//! script paths for further execution.
use crate::{
    models::PluginModel,
    repositories::{PluginRepositoryTrait, RepositoryError},
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

    async fn get_all(&self) -> Result<Vec<PluginModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.values().cloned().collect())
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
    async fn test_get_all_plugins() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        let plugin1 = PluginModel {
            id: "test-plugin1".to_string(),
            path: "test-path1".to_string(),
            timeout: Duration::from_secs(69),
        };

        let plugin2 = PluginModel {
            id: "test-plugin2".to_string(),
            path: "test-path2".to_string(),
            timeout: Duration::from_secs(69),
        };

        plugin_repository.add(plugin1.clone()).await.unwrap();
        plugin_repository.add(plugin2.clone()).await.unwrap();

        let retrieved = plugin_repository.get_all().await.unwrap();
        assert_eq!(retrieved.len(), 2);
        assert_eq!(retrieved[0].id, plugin1.id);
        assert_eq!(retrieved[0].path, plugin1.path);
        assert_eq!(retrieved[0].timeout, plugin1.timeout);
        assert_eq!(retrieved[1].id, plugin2.id);
        assert_eq!(retrieved[1].path, plugin2.path);
        assert_eq!(retrieved[1].timeout, plugin2.timeout);
    }
}
