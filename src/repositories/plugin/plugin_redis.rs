//! Redis-backed implementation of the PluginRepository.

use crate::models::{PaginationQuery, PluginModel, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{BatchRetrievalResult, PaginatedResult, PluginRepositoryTrait};
use async_trait::async_trait;
use log::{debug, error, warn};
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

    /// Generate key for plugin list: plugin_list (paginated list of plugin IDs)
    fn plugin_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, PLUGIN_LIST_KEY)
    }

    /// Get plugin by ID using an existing connection.
    /// This method is useful to prevent creating new connections for
    /// getting individual plugins on list operations.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the plugin to get.
    /// * `conn` - The connection to use.
    async fn get_by_id_with_connection(
        &self,
        id: &str,
        conn: &mut ConnectionManager,
    ) -> Result<Option<PluginModel>, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Plugin ID cannot be empty".to_string(),
            ));
        }
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

    async fn get_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<PluginModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No plugin IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.plugin_key(id)).collect();

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_plugins"))?;

        let mut plugins = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => match self.deserialize_entity(&json, &ids[i], "plugin") {
                    Ok(plugin) => plugins.push(plugin),
                    Err(e) => {
                        failed_count += 1;
                        error!("Failed to deserialize plugin {}: {}", ids[i], e);
                        failed_ids.push(ids[i].clone());
                    }
                },
                None => {
                    warn!("Plugin {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} plugins in batch",
                failed_count,
                ids.len()
            );
        }

        Ok(BatchRetrievalResult {
            results: plugins,
            failed_ids,
        })
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
        let mut conn = self.client.as_ref().clone();
        self.get_by_id_with_connection(id, &mut conn).await
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

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<PluginModel>, RepositoryError> {
        if query.page == 0 {
            return Err(RepositoryError::InvalidData(
                "Page number must be greater than 0".to_string(),
            ));
        }

        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "Per page count must be greater than 0".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let plugin_list_key = self.plugin_list_key();

        // Get total count
        let total: u64 = conn
            .scard(&plugin_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_count"))?;

        if total == 0 {
            return Ok(PaginatedResult {
                items: vec![],
                total: 0,
                page: query.page,
                per_page: query.per_page,
            });
        }

        // Get all IDs and paginate in memory
        let all_ids: Vec<String> = conn
            .smembers(&plugin_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_members"))?;

        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_ids.len());

        let ids_to_query = &all_ids[start..end];
        let items = self.get_by_ids(ids_to_query).await?;

        Ok(PaginatedResult {
            items: items.results.clone(),
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let plugin_list_key = self.plugin_list_key();

        let count: u64 = conn
            .scard(&plugin_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_plugins"))?;

        Ok(count as usize)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let plugin_list_key = self.plugin_list_key();

        debug!("Checking if plugin entries exist");

        let exists: bool = conn
            .exists(&plugin_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "has_entries_check"))?;

        debug!("Plugin entries exist: {}", exists);
        Ok(exists)
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let plugin_list_key = self.plugin_list_key();

        debug!("Dropping all plugin entries");

        // Get all plugin IDs first
        let plugin_ids: Vec<String> = conn
            .smembers(&plugin_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_ids"))?;

        if plugin_ids.is_empty() {
            debug!("No plugin entries to drop");
            return Ok(());
        }

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all individual plugin entries
        for plugin_id in &plugin_ids {
            let plugin_key = self.plugin_key(plugin_id);
            pipe.del(&plugin_key);
        }

        // Delete the plugin list key
        pipe.del(&plugin_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!("Dropped {} plugin entries", plugin_ids.len());
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
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let mut connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        // Clear the plugin lists
        connection_manager
            .del::<&str, ()>("test_plugin:plugin_list")
            .await
            .unwrap();

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

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_ids_plugins() {
        let repo = setup_test_repo().await;
        let plugin_name1 = uuid::Uuid::new_v4().to_string();
        let plugin_name2 = uuid::Uuid::new_v4().to_string();
        let plugin1 = create_test_plugin(&plugin_name1, "/path/to/plugin1");
        let plugin2 = create_test_plugin(&plugin_name2, "/path/to/plugin2");

        repo.add(plugin1.clone()).await.unwrap();
        repo.add(plugin2.clone()).await.unwrap();

        let retrieved = repo
            .get_by_ids(&[plugin1.id.clone(), plugin2.id.clone()])
            .await
            .unwrap();
        assert!(retrieved.results.len() == 2);
        assert_eq!(retrieved.results[0].id, plugin2.id);
        assert_eq!(retrieved.results[1].id, plugin1.id);
        assert_eq!(retrieved.failed_ids.len(), 0);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_paginated_plugins() {
        let repo = setup_test_repo().await;

        let plugin_id1 = uuid::Uuid::new_v4().to_string();
        let plugin_id2 = uuid::Uuid::new_v4().to_string();
        let plugin_id3 = uuid::Uuid::new_v4().to_string();
        let plugin1 = create_test_plugin(&plugin_id1, "/path/to/plugin1");
        let plugin2 = create_test_plugin(&plugin_id2, "/path/to/plugin2");
        let plugin3 = create_test_plugin(&plugin_id3, "/path/to/plugin3");

        repo.add(plugin1.clone()).await.unwrap();
        repo.add(plugin2.clone()).await.unwrap();
        repo.add(plugin3.clone()).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };

        let result = repo.list_paginated(query).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.items.len() == 2);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries() {
        let repo = setup_test_repo().await;
        assert!(!repo.has_entries().await.unwrap());
        repo.add(create_test_plugin("test-plugin", "/path/to/plugin"))
            .await
            .unwrap();
        assert!(repo.has_entries().await.unwrap());
        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries() {
        let repo = setup_test_repo().await;
        repo.add(create_test_plugin("test-plugin", "/path/to/plugin"))
            .await
            .unwrap();
        assert!(repo.has_entries().await.unwrap());
        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}
