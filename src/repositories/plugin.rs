//! This module provides an in-memory implementation of plugins.
//!
//! The `InMemoryPluginRepository` struct is used to store and retrieve plugins
//! script paths for further execution.
use crate::{
    config::PluginFileConfig,
    models::PluginModel,
    repositories::{ConversionError, RepositoryError},
};
use async_trait::async_trait;

#[cfg(test)]
use mockall::automock;

use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryPluginRepository {
    store: Mutex<HashMap<String, PluginModel>>,
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
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait PluginRepositoryTrait {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError>;
    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError>;
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
}

impl TryFrom<PluginFileConfig> for PluginModel {
    type Error = ConversionError;

    fn try_from(config: PluginFileConfig) -> Result<Self, Self::Error> {
        Ok(PluginModel {
            id: config.id.clone(),
            path: config.path.clone(),
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
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_in_memory_plugin_repository() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        // Test add and get_by_id
        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
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
        };
        let result = PluginModel::try_from(plugin);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            PluginModel {
                id: "test-plugin".to_string(),
                path: "test-path".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_get_by_id() {
        let plugin_repository = Arc::new(InMemoryPluginRepository::new());

        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
        };
        plugin_repository.add(plugin.clone()).await.unwrap();
        assert_eq!(
            plugin_repository.get_by_id("test-plugin").await.unwrap(),
            Some(plugin)
        );
    }
}
