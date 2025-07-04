pub mod plugin_in_memory;
pub mod plugin_redis;

pub use plugin_in_memory::*;
pub use plugin_redis::*;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;

#[cfg(test)]
use mockall::automock;

use crate::{
    config::{PluginFileConfig, ServerConfig},
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

/// Enum representing the type of plugin repository to use
pub enum PluginRepositoryType {
    InMemory,
    Redis,
}

/// Enum wrapper for different plugin repository implementations
#[derive(Debug, Clone)]
pub enum PluginRepositoryImpl {
    InMemory(InMemoryPluginRepository),
    Redis(RedisPluginRepository),
}

impl PluginRepositoryType {
    /// Creates a plugin repository based on the enum variant
    pub async fn create_repository(self, config: &ServerConfig) -> PluginRepositoryImpl {
        match self {
            PluginRepositoryType::InMemory => {
                PluginRepositoryImpl::InMemory(InMemoryPluginRepository::new())
            }
            PluginRepositoryType::Redis => {
                let client = redis::Client::open(config.redis_url.clone())
                    .expect("Failed to create Redis client");
                let connection_manager = redis::aio::ConnectionManager::new(client)
                    .await
                    .expect("Failed to create Redis connection manager");
                PluginRepositoryImpl::Redis(
                    RedisPluginRepository::new(
                        Arc::new(connection_manager),
                        config.redis_key_prefix.clone(),
                    )
                    .expect("Failed to create Redis plugin repository"),
                )
            }
        }
    }
}

impl PluginRepositoryImpl {
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
impl PluginRepositoryTrait for PluginRepositoryImpl {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        match self {
            PluginRepositoryImpl::InMemory(repo) => repo.get_by_id(id).await,
            PluginRepositoryImpl::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError> {
        match self {
            PluginRepositoryImpl::InMemory(repo) => repo.add(plugin).await,
            PluginRepositoryImpl::Redis(repo) => repo.add(plugin).await,
        }
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
