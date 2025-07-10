//! Redis-backed implementation of the PluginRepository.

use crate::models::{PluginModel, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::PluginRepositoryTrait;
use async_trait::async_trait;
use log::{debug, error};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const PLUGIN_PREFIX: &str = "plugin";
const PLUGIN_LIST_KEY: &str = "plugin_list";

#[derive(Clone)]
pub struct RedisPluginRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisPluginRepository {}

impl RedisPluginRepository {
    pub fn new(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        if key_prefix.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Redis key prefix cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            client: connection_manager,
            key_prefix,
        })
    }

    /// Generate key for plugin data: plugin:{plugin_id}
    fn plugin_key(&self, plugin_id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, PLUGIN_PREFIX, plugin_id)
    }

    /// Generate key for plugin list: plugin_list (set of all plugin IDs)
    fn plugin_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, PLUGIN_LIST_KEY)
    }
}

impl fmt::Debug for RedisPluginRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisPluginRepository")
            .field("client", &"<ConnectionManager>")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[async_trait]
impl PluginRepositoryTrait for RedisPluginRepository {
    async fn get_by_id(&self, id: &str) -> Result<Option<PluginModel>, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Plugin ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let key = self.plugin_key(id);

        debug!("Fetching plugin data for ID: {}", id);

        let json: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, &format!("get_plugin_by_id_{}", id)))?;

        match json {
            Some(json) => {
                debug!("Found plugin data for ID: {}", id);
                let plugin = self.deserialize_entity::<PluginModel>(&json, id, "plugin")?;
                Ok(Some(plugin))
            }
            None => {
                debug!("No plugin found for ID: {}", id);
                Ok(None)
            }
        }
    }

    async fn add(&self, plugin: PluginModel) -> Result<(), RepositoryError> {
        if plugin.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Plugin ID cannot be empty".to_string(),
            ));
        }

        if plugin.path.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Plugin path cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let key = self.plugin_key(&plugin.id);
        let list_key = self.plugin_list_key();

        debug!("Adding plugin with ID: {}", plugin.id);

        // Check if plugin already exists
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|e| self.map_redis_error(e, &format!("check_plugin_exists_{}", plugin.id)))?;

        if exists {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Plugin with ID {} already exists",
                plugin.id
            )));
        }

        // Serialize plugin
        let json = self.serialize_entity(&plugin, |p| &p.id, "plugin")?;

        // Use a pipeline to ensure atomicity
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&key, &json);
        pipe.sadd(&list_key, &plugin.id);

        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!("Failed to add plugin {}: {}", plugin.id, e);
            self.map_redis_error(e, &format!("add_plugin_{}", plugin.id))
        })?;

        debug!("Successfully added plugin with ID: {}", plugin.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_PLUGIN_TIMEOUT_SECONDS;
    use crate::models::PluginModel;
    use std::{sync::Arc, time::Duration};

    fn create_test_plugin(id: &str, path: &str) -> PluginModel {
        PluginModel {
            id: id.to_string(),
            path: path.to_string(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECONDS),
        }
    }

    async fn setup_test_repo() -> RedisPluginRepository {
        let client =
            redis::Client::open("redis://127.0.0.1:6379/").expect("Failed to create Redis client");
        let connection_manager = redis::aio::ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        RedisPluginRepository::new(Arc::new(connection_manager), "test_plugin".to_string())
            .expect("Failed to create Redis plugin repository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert_eq!(repo.key_prefix, "test_plugin");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_empty_prefix_fails() {
        let client =
            redis::Client::open("redis://127.0.0.1:6379/").expect("Failed to create Redis client");
        let connection_manager = redis::aio::ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        let result = RedisPluginRepository::new(Arc::new(connection_manager), "".to_string());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("key prefix cannot be empty"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_key_generation() {
        let repo = setup_test_repo().await;

        let plugin_key = repo.plugin_key("test-plugin");
        assert_eq!(plugin_key, "test_plugin:plugin:test-plugin");

        let list_key = repo.plugin_list_key();
        assert_eq!(list_key, "test_plugin:plugin_list");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_serialize_deserialize_plugin() {
        let repo = setup_test_repo().await;
        let plugin = create_test_plugin("test-plugin", "/path/to/plugin");

        let json = repo.serialize_entity(&plugin, |p| &p.id, "plugin").unwrap();
        let deserialized: PluginModel = repo
            .deserialize_entity(&json, &plugin.id, "plugin")
            .unwrap();

        assert_eq!(plugin.id, deserialized.id);
        assert_eq!(plugin.path, deserialized.path);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_add_plugin() {
        let repo = setup_test_repo().await;
        let plugin_name = uuid::Uuid::new_v4().to_string();
        let plugin = create_test_plugin(&plugin_name, "/path/to/plugin");

        let result = repo.add(plugin).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_plugin() {
        let repo = setup_test_repo().await;
        let plugin_name = uuid::Uuid::new_v4().to_string();
        let plugin = create_test_plugin(&plugin_name, "/path/to/plugin");

        // Add the plugin first
        repo.add(plugin.clone()).await.unwrap();

        // Get the plugin
        let retrieved = repo.get_by_id(&plugin_name).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, plugin.id);
        assert_eq!(retrieved.path, plugin.path);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_plugin() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("nonexistent-plugin").await;
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_duplicate_plugin_addition() {
        let repo = setup_test_repo().await;
        let plugin_name = uuid::Uuid::new_v4().to_string();
        let plugin = create_test_plugin(&plugin_name, "/path/to/plugin");

        // Add the plugin first time
        repo.add(plugin.clone()).await.unwrap();

        // Try to add the same plugin again
        let result = repo.add(plugin).await;
        assert!(result.is_err());

        if let Err(RepositoryError::ConstraintViolation(msg)) = result {
            assert!(msg.contains("already exists"));
        } else {
            panic!("Expected ConstraintViolation error");
        }
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_debug_implementation() {
        let repo = setup_test_repo().await;
        let debug_str = format!("{:?}", repo);
        assert!(debug_str.contains("RedisPluginRepository"));
        assert!(debug_str.contains("test_plugin"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_error_handling_empty_id() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ID cannot be empty"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_add_plugin_with_empty_id() {
        let repo = setup_test_repo().await;
        let plugin = create_test_plugin("", "/path/to/plugin");

        let result = repo.add(plugin).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ID cannot be empty"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_add_plugin_with_empty_path() {
        let repo = setup_test_repo().await;
        let plugin = create_test_plugin("test-plugin", "");

        let result = repo.add(plugin).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("path cannot be empty"));
    }
}
