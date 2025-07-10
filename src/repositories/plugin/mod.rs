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
    models::{PluginModel, RepositoryError},
    repositories::ConversionError,
};

#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait PluginRepositoryTrait {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError>;
    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError>;
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
}
