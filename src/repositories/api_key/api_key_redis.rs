//! Redis-backed implementation of the ApiKeyRepository.

use crate::models::{ApiKeyRepoModel, PaginationQuery, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{ApiKeyRepositoryTrait, BatchRetrievalResult, PaginatedResult};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;
use tracing::{debug, error, warn};

const API_KEY_PREFIX: &str = "apikey";
const API_KEY_LIST_KEY: &str = "apikey_list";

#[derive(Clone)]
pub struct RedisApiKeyRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisApiKeyRepository {}

impl RedisApiKeyRepository {
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

    /// Generate key for api key data: apikey:{api_key_id}
    fn api_key_key(&self, api_key_id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, API_KEY_PREFIX, api_key_id)
    }

    /// Generate key for api key list: apikey_list (paginated list of api key IDs)
    fn api_key_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, API_KEY_LIST_KEY)
    }

    async fn get_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<ApiKeyRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No api key IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.api_key_key(id)).collect();

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_api_keys"))?;

        let mut apikeys = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => match self.deserialize_entity(&json, &ids[i], "apikey") {
                    Ok(apikey) => apikeys.push(apikey),
                    Err(e) => {
                        failed_count += 1;
                        error!("Failed to deserialize api key {}: {}", ids[i], e);
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
                "Failed to deserialize {} out of {} api keys in batch",
                failed_count,
                ids.len()
            );
        }

        Ok(BatchRetrievalResult {
            results: apikeys,
            failed_ids,
        })
    }
}

impl fmt::Debug for RedisApiKeyRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RedisApiKeyRepository {{ key_prefix: {} }}",
            self.key_prefix
        )
    }
}

#[async_trait]
impl ApiKeyRepositoryTrait for RedisApiKeyRepository {
    async fn create(&self, entity: ApiKeyRepoModel) -> Result<ApiKeyRepoModel, RepositoryError> {
        if entity.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key ID cannot be empty".to_string(),
            ));
        }

        let key = self.api_key_key(&entity.id);
        let list_key = self.api_key_list_key();
        let json = self.serialize_entity(&entity, |a| &a.id, "apikey")?;

        let mut conn = self.client.as_ref().clone();

        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "create_api_key_check"))?;

        if existing.is_some() {
            return Err(RepositoryError::ConstraintViolation(format!(
                "API Key with ID {} already exists",
                entity.id
            )));
        }

        // Use atomic pipeline for consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&key, json);
        pipe.sadd(&list_key, &entity.id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "create_api_key"))?;

        debug!("Successfully created API Key {}", entity.id);
        Ok(entity)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyRepoModel>, RepositoryError> {
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
        let api_key_list_key = self.api_key_list_key();

        // Get total count
        let total: u64 = conn
            .scard(&api_key_list_key)
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
            .smembers(&api_key_list_key)
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

    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyRepoModel>, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let api_key_key = self.api_key_key(id);

        debug!("Fetching api key with ID: {}", id);

        let json: Option<String> = conn
            .get(&api_key_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_api_key_by_id"))?;

        match json {
            Some(json) => {
                debug!("Found api key with ID: {}", id);
                self.deserialize_entity(&json, id, "apikey")
            }
            None => {
                debug!("Api key with ID {} not found", id);
                Ok(None)
            }
        }
    }

    async fn list_permissions(&self, api_key_id: &str) -> Result<Vec<String>, RepositoryError> {
        let api_key = self.get_by_id(api_key_id).await?;
        match api_key {
            Some(api_key) => Ok(api_key.permissions),
            None => Err(RepositoryError::NotFound(format!(
                "Api key with ID {} not found",
                api_key_id
            ))),
        }
    }

    async fn delete_by_id(&self, id: &str) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key ID cannot be empty".to_string(),
            ));
        }

        let key = self.api_key_key(id);
        let api_key_list_key = self.api_key_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Deleting api key with ID: {}", id);

        // Check if api key exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_api_key_check"))?;

        if existing.is_none() {
            return Err(RepositoryError::NotFound(format!(
                "Api key with ID {} not found",
                id
            )));
        }

        // Use atomic pipeline to ensure consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.srem(&api_key_list_key, id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_api_key"))?;

        debug!("Successfully deleted api key {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let api_key_list_key = self.api_key_list_key();

        let count: u64 = conn
            .scard(&api_key_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_api_keys"))?;

        Ok(count as usize)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let plugin_list_key = self.api_key_list_key();

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
        let plugin_list_key = self.api_key_list_key();

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
            let plugin_key = self.api_key_key(plugin_id);
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
    use crate::models::SecretString;

    use super::*;
    use chrono::Utc;

    fn create_test_api_key(id: &str) -> ApiKeyRepoModel {
        ApiKeyRepoModel {
            id: id.to_string(),
            value: SecretString::new("test-value"),
            name: "test-name".to_string(),
            allowed_origins: vec!["*".to_string()],
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: Utc::now().to_string(),
        }
    }

    async fn setup_test_repo() -> RedisApiKeyRepository {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let mut connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        // Clear the api key list
        connection_manager
            .del::<&str, ()>("test_api_key:apikey_list")
            .await
            .unwrap();

        RedisApiKeyRepository::new(Arc::new(connection_manager), "test_api_key".to_string())
            .expect("Failed to create Redis api key repository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert_eq!(repo.key_prefix, "test_api_key");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_empty_prefix_fails() {
        let client =
            redis::Client::open("redis://127.0.0.1:6379/").expect("Failed to create Redis client");
        let connection_manager = redis::aio::ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        let result = RedisApiKeyRepository::new(Arc::new(connection_manager), "".to_string());
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

        let api_key_key = repo.api_key_key("test-api-key");
        assert_eq!(api_key_key, "test_api_key:apikey:test-api-key");

        let list_key = repo.api_key_list_key();
        assert_eq!(list_key, "test_api_key:apikey_list");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_serialize_deserialize_api_key() {
        let repo = setup_test_repo().await;
        let api_key = create_test_api_key("test-api-key");

        let json = repo
            .serialize_entity(&api_key, |a| &a.id, "apikey")
            .unwrap();
        let deserialized: ApiKeyRepoModel = repo
            .deserialize_entity(&json, &api_key.id, "apikey")
            .unwrap();

        assert_eq!(api_key.id, deserialized.id);
        assert_eq!(api_key.value, deserialized.value);
        assert_eq!(api_key.name, deserialized.name);
        assert_eq!(api_key.allowed_origins, deserialized.allowed_origins);
        assert_eq!(api_key.permissions, deserialized.permissions);
        assert_eq!(api_key.created_at, deserialized.created_at);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_api_key() {
        let repo = setup_test_repo().await;
        let api_key_id = uuid::Uuid::new_v4().to_string();
        let api_key = create_test_api_key(&api_key_id);

        let result = repo.create(api_key.clone()).await;
        assert!(result.is_ok());

        let retrieved = repo.get_by_id(&api_key_id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, api_key.id);
        assert_eq!(retrieved.value, api_key.value);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_api_key() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("nonexistent-api-key").await;
        assert!(matches!(result, Ok(None)));
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
    async fn test_get_by_ids_api_keys() {
        let repo = setup_test_repo().await;
        let api_key_id1 = uuid::Uuid::new_v4().to_string();
        let api_key_id2 = uuid::Uuid::new_v4().to_string();
        let api_key1 = create_test_api_key(&api_key_id1);
        let api_key2 = create_test_api_key(&api_key_id2);

        repo.create(api_key1.clone()).await.unwrap();
        repo.create(api_key2.clone()).await.unwrap();

        let retrieved = repo
            .get_by_ids(&[api_key1.id.clone(), api_key2.id.clone()])
            .await
            .unwrap();
        assert!(retrieved.results.len() == 2);
        assert_eq!(retrieved.results[0].id, api_key1.id);
        assert_eq!(retrieved.results[1].id, api_key2.id);
        assert_eq!(retrieved.failed_ids.len(), 0);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_paginated_api_keys() {
        let repo = setup_test_repo().await;

        let api_key_id1 = uuid::Uuid::new_v4().to_string();
        let api_key_id2 = uuid::Uuid::new_v4().to_string();
        let api_key_id3 = uuid::Uuid::new_v4().to_string();
        let api_key1 = create_test_api_key(&api_key_id1);
        let api_key2 = create_test_api_key(&api_key_id2);
        let api_key3 = create_test_api_key(&api_key_id3);

        repo.create(api_key1.clone()).await.unwrap();
        repo.create(api_key2.clone()).await.unwrap();
        repo.create(api_key3.clone()).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };

        let result = repo.list_paginated(query).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        println!("result: {:?}", result);
        assert!(result.items.len() == 2);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries() {
        let repo = setup_test_repo().await;
        assert!(!repo.has_entries().await.unwrap());
        repo.create(create_test_api_key("test-api-key"))
            .await
            .unwrap();
        assert!(repo.has_entries().await.unwrap());
        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}
