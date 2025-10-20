//! Redis-backed implementation of the ApiKeyRepository.

use crate::models::{ApiKeyRepoModel, PaginationQuery, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{ApiKeyRepositoryTrait, BatchRetrievalResult, PaginatedResult};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;
use tracing::{debug, error, warn};

const API_KEY_PREFIX: &str = "apikey";
const API_KEY_LIST_KEY: &str = "apikey_list";
const API_KEY_VALUE_INDEX_PREFIX: &str = "apikey_value_index";

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

    /// Generate key for value index: apikey_value_index:{hashed_value}
    fn api_key_value_index_key(&self, value: &str) -> String {
        let hash = self.hash_api_key_value(value);
        format!(
            "{}:{}:{}",
            self.key_prefix, API_KEY_VALUE_INDEX_PREFIX, hash
        )
    }

    /// Hash API key value for secure indexing
    fn hash_api_key_value(&self, value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
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
        let value_index_key = entity
            .value
            .as_str(|secret_value| self.api_key_value_index_key(secret_value));
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
        pipe.set(&value_index_key, &entity.id);

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

    async fn get_by_value(&self, value: &str) -> Result<Option<ApiKeyRepoModel>, RepositoryError> {
        if value.is_empty() {
            return Err(RepositoryError::InvalidData(
                "API Key value cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let value_index_key = self.api_key_value_index_key(value);

        debug!("Fetching api key by value using index");

        // Get API key ID from value index
        let api_key_id: Option<String> = conn
            .get(&value_index_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_api_key_by_value_index"))?;

        match api_key_id {
            Some(id) => {
                debug!("Found API key ID from value index: {}", id);
                self.get_by_id(&id).await
            }
            None => {
                debug!("No API key found with matching value");
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

        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_api_key_check"))?;

        // Get the API key to extract value for value index cleanup
        let api_key = match existing {
            Some(json) => match self.deserialize_entity::<ApiKeyRepoModel>(&json, id, "apikey") {
                Ok(api_key) => api_key,
                Err(_) => {
                    return Err(RepositoryError::NotFound(format!(
                        "Api key with ID {} not found or corrupted",
                        id
                    )));
                }
            },
            None => {
                return Err(RepositoryError::NotFound(format!(
                    "Api key with ID {} not found",
                    id
                )));
            }
        };

        let value_index_key = api_key
            .value
            .as_str(|secret_value| self.api_key_value_index_key(secret_value));

        // Use atomic pipeline to ensure consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.srem(&api_key_list_key, id);
        pipe.del(&value_index_key);

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
        let api_key_list_key = self.api_key_list_key();

        debug!("Dropping all api key entries");

        // Get all API key IDs first
        let api_key_ids: Vec<String> = conn
            .smembers(&api_key_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_ids"))?;

        if api_key_ids.is_empty() {
            debug!("No API key entries to drop");
            return Ok(());
        }

        // Get all API keys to extract values for index cleanup
        let batch_result = self.get_by_ids(&api_key_ids).await?;

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all individual API key entries and their value indexes
        for api_key in &batch_result.results {
            let api_key_key = self.api_key_key(&api_key.id);
            let value_index_key = api_key
                .value
                .as_str(|secret_value| self.api_key_value_index_key(secret_value));
            pipe.del(&api_key_key);
            pipe.del(&value_index_key);
        }

        // Delete the API key list key
        pipe.del(&api_key_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!("Dropped {} API key entries", api_key_ids.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::models::SecretString;
    use uuid::Uuid;

    use super::*;
    use chrono::Utc;

    fn create_test_api_key(id: &str) -> ApiKeyRepoModel {
        ApiKeyRepoModel {
            id: id.to_string(),
            value: SecretString::new(&Uuid::new_v4().to_string()),
            name: "test-name".to_string(),
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: Utc::now().to_string(),
        }
    }

    async fn setup_test_repo() -> RedisApiKeyRepository {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        // Use unique test prefix to avoid conflicts between tests
        let test_prefix = format!(
            "test_api_key_{}",
            uuid::Uuid::new_v4().to_string().replace("-", "")
        );

        RedisApiKeyRepository::new(Arc::new(connection_manager), test_prefix)
            .expect("Failed to create Redis api key repository")
    }

    async fn cleanup_test_repo(repo: &RedisApiKeyRepository) {
        let mut conn = repo.client.as_ref().clone();

        // Use Redis SCAN to find all keys with our test prefix and delete them
        let pattern = format!("{}:*", repo.key_prefix);
        let mut cursor = 0;
        loop {
            let result: (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
                .unwrap_or((0, vec![]));

            let (new_cursor, keys) = result;

            if !keys.is_empty() {
                let _: () = conn.del(keys).await.unwrap_or(());
            }

            if new_cursor == 0 {
                break;
            }
            cursor = new_cursor;
        }
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert!(repo.key_prefix.starts_with("test_api_key_"));
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
        assert!(api_key_key.contains(":apikey:test-api-key"));

        let list_key = repo.api_key_list_key();
        assert!(list_key.contains(":apikey_list"));

        let value_index_key = repo.api_key_value_index_key("test-value");
        assert!(value_index_key.contains(":apikey_value_index:"));

        cleanup_test_repo(&repo).await;
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
        assert_eq!(api_key.permissions, deserialized.permissions);
        assert_eq!(api_key.created_at, deserialized.created_at);

        cleanup_test_repo(&repo).await;
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

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_api_key() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("nonexistent-api-key").await;
        assert!(matches!(result, Ok(None)));

        cleanup_test_repo(&repo).await;
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

        cleanup_test_repo(&repo).await;
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

        cleanup_test_repo(&repo).await;
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

        cleanup_test_repo(&repo).await;
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

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_value_existing() {
        let repo = setup_test_repo().await;
        let api_key_id = "test-api-key";
        let mut api_key = create_test_api_key(&api_key_id);
        api_key.value = SecretString::new("unique-test-value-123");

        repo.create(api_key.clone()).await.unwrap();

        let result = repo.get_by_value("unique-test-value-123").await.unwrap();
        assert!(result.is_some());
        let retrieved = result.unwrap();
        assert_eq!(retrieved.id, api_key.id);
        assert_eq!(retrieved.value, api_key.value);

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_value_non_existing() {
        let repo = setup_test_repo().await;
        let api_key_id = uuid::Uuid::new_v4().to_string();
        let api_key = create_test_api_key(&api_key_id);

        repo.create(api_key).await.unwrap();

        let result = repo.get_by_value("non-existing-value").await.unwrap();
        assert_eq!(result, None);

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_value_empty_store() {
        let repo = setup_test_repo().await;
        let result = repo.get_by_value("any-value").await.unwrap();
        assert_eq!(result, None);

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_duplicate_value_fails() {
        let repo = setup_test_repo().await;
        let api_key_id1 = "test-api-key-1";
        let api_key_id2 = "test-api-key-2";
        let mut api_key1 = create_test_api_key(&api_key_id1);
        let mut api_key2 = create_test_api_key(&api_key_id2);

        // Set same value for both
        let shared_value = "shared-api-key-value";
        api_key1.value = SecretString::new(shared_value);
        api_key2.value = SecretString::new(shared_value);

        // First creation should succeed
        repo.create(api_key1).await.unwrap();

        // Second creation with same value should fail
        let result = repo.create(api_key2).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_removes_value_index() {
        let repo = setup_test_repo().await;
        let api_key_id = "test-api-key";
        let mut api_key = create_test_api_key(&api_key_id);
        api_key.value = SecretString::new("value-to-delete");

        // Create API key
        repo.create(api_key.clone()).await.unwrap();

        // Verify it can be found by value
        let result = repo.get_by_value("value-to-delete").await.unwrap();
        assert!(result.is_some());

        // Delete the API key
        repo.delete_by_id(&api_key_id).await.unwrap();

        // Verify it can no longer be found by value
        let result = repo.get_by_value("value-to-delete").await.unwrap();
        assert_eq!(result, None);

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_hash_api_key_value() {
        let repo = setup_test_repo().await;

        let hash1 = repo.hash_api_key_value("test-value");
        let hash2 = repo.hash_api_key_value("test-value");
        let hash3 = repo.hash_api_key_value("different-value");

        // Same input should produce same hash
        assert_eq!(hash1, hash2);
        // Different input should produce different hash
        assert_ne!(hash1, hash3);
        // Hash should be consistent (SHA256 produces 64 hex chars)
        assert_eq!(hash1.len(), 64);

        cleanup_test_repo(&repo).await;
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries() {
        let repo = setup_test_repo().await;
        repo.create(create_test_api_key("test-api-key"))
            .await
            .unwrap();
        assert!(repo.has_entries().await.unwrap());

        let result = repo.get_by_id("test-api-key").await.unwrap();
        assert!(result.is_some());

        let value = result.unwrap().value.as_str(|v| v.to_string());

        let result = repo.get_by_value(&value).await.unwrap();
        assert!(result.is_some());

        let list = repo
            .list_paginated(PaginationQuery {
                page: 1,
                per_page: 10,
            })
            .await
            .unwrap();
        assert!(list.items.len() == 1);

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());

        let result = repo.get_by_id("test-api-key").await.unwrap();
        assert!(result.is_none());

        let result = repo.get_by_value(&value).await.unwrap();
        assert!(result.is_none());

        let list = repo
            .list_paginated(PaginationQuery {
                page: 1,
                per_page: 10,
            })
            .await
            .unwrap();
        assert!(list.items.len() == 0);

        cleanup_test_repo(&repo).await;
    }
}
