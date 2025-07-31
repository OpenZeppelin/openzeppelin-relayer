//! Redis-backed implementation of the signer repository.

use crate::models::{RepositoryError, SignerRepoModel};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::*;
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisError};
use std::fmt;
use std::sync::Arc;

const SIGNER_PREFIX: &str = "signer";
const SIGNER_LIST_KEY: &str = "signer_list";

#[derive(Clone)]
pub struct RedisSignerRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisSignerRepository {}

impl RedisSignerRepository {
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

    fn signer_key(&self, id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, SIGNER_PREFIX, id)
    }

    fn signer_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, SIGNER_LIST_KEY)
    }

    async fn add_to_list(&self, id: &str) -> Result<(), RepositoryError> {
        let key = self.signer_list_key();
        let mut conn = self.client.as_ref().clone();

        let result: Result<i64, RedisError> = conn.sadd(&key, id).await;
        result.map_err(|e| {
            error!("Failed to add signer {} to list: {}", id, e);
            RepositoryError::Other(format!("Failed to add signer to list: {}", e))
        })?;

        debug!("Added signer {} to list", id);
        Ok(())
    }

    async fn remove_from_list(&self, id: &str) -> Result<(), RepositoryError> {
        let key = self.signer_list_key();
        let mut conn = self.client.as_ref().clone();

        let result: Result<i64, RedisError> = conn.srem(&key, id).await;
        result.map_err(|e| {
            error!("Failed to remove signer {} from list: {}", id, e);
            RepositoryError::Other(format!("Failed to remove signer from list: {}", e))
        })?;

        debug!("Removed signer {} from list", id);
        Ok(())
    }

    async fn get_all_ids(&self) -> Result<Vec<String>, RepositoryError> {
        let key = self.signer_list_key();
        let mut conn = self.client.as_ref().clone();

        let result: Result<Vec<String>, RedisError> = conn.smembers(&key).await;
        result.map_err(|e| {
            error!("Failed to get signer IDs: {}", e);
            RepositoryError::Other(format!("Failed to get signer IDs: {}", e))
        })
    }

    /// Batch fetch signers by IDs
    async fn get_signers_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<SignerRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No signer IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.signer_key(id)).collect();

        debug!("Batch fetching {} signers", ids.len());

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_signers"))?;

        let mut signers = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();

        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_entity::<SignerRepoModel>(&json, &ids[i], "signer") {
                        Ok(signer) => signers.push(signer),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize signer {}: {}", ids[i], e);
                            failed_ids.push(ids[i].clone());
                        }
                    }
                }
                None => {
                    warn!("Signer {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} signers in batch",
                failed_count,
                ids.len()
            );
            warn!("Failed to deserialize signers: {:?}", failed_ids);
        }

        debug!("Successfully fetched {} signers", signers.len());
        Ok(BatchRetrievalResult {
            results: signers,
            failed_ids,
        })
    }
}

impl fmt::Debug for RedisSignerRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisSignerRepository")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[async_trait]
impl Repository<SignerRepoModel, String> for RedisSignerRepository {
    async fn create(&self, signer: SignerRepoModel) -> Result<SignerRepoModel, RepositoryError> {
        if signer.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Signer ID cannot be empty".to_string(),
            ));
        }

        let key = self.signer_key(&signer.id);
        let mut conn = self.client.as_ref().clone();

        // Check if signer already exists
        let exists: Result<bool, RedisError> = conn.exists(&key).await;
        match exists {
            Ok(true) => {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Signer with ID {} already exists",
                    signer.id
                )));
            }
            Ok(false) => {
                // Continue with creation
            }
            Err(e) => {
                error!("Failed to check if signer exists: {}", e);
                return Err(RepositoryError::Other(format!(
                    "Failed to check signer existence: {}",
                    e
                )));
            }
        }

        // Serialize signer (encryption happens automatically for human-readable formats)
        let serialized = self.serialize_entity(&signer, |s| &s.id, "signer")?;

        // Store signer
        let result: Result<(), RedisError> = conn.set(&key, &serialized).await;
        result.map_err(|e| {
            error!("Failed to store signer {}: {}", signer.id, e);
            RepositoryError::Other(format!("Failed to store signer: {}", e))
        })?;

        // Add to list
        self.add_to_list(&signer.id).await?;

        debug!("Created signer with ID: {}", signer.id);
        Ok(signer)
    }

    async fn get_by_id(&self, id: String) -> Result<SignerRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Signer ID cannot be empty".to_string(),
            ));
        }

        let key = self.signer_key(&id);
        let mut conn = self.client.as_ref().clone();

        let result: Result<Option<String>, RedisError> = conn.get(&key).await;
        match result {
            Ok(Some(data)) => {
                // Deserialize signer (decryption happens automatically)
                let signer = self.deserialize_entity::<SignerRepoModel>(&data, &id, "signer")?;
                debug!("Retrieved signer with ID: {}", id);
                Ok(signer)
            }
            Ok(None) => {
                debug!("Signer with ID {} not found", id);
                Err(RepositoryError::NotFound(format!(
                    "Signer with ID {} not found",
                    id
                )))
            }
            Err(e) => {
                error!("Failed to retrieve signer {}: {}", id, e);
                Err(RepositoryError::Other(format!(
                    "Failed to retrieve signer: {}",
                    e
                )))
            }
        }
    }

    async fn update(
        &self,
        id: String,
        signer: SignerRepoModel,
    ) -> Result<SignerRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Signer ID cannot be empty".to_string(),
            ));
        }

        if signer.id != id {
            return Err(RepositoryError::InvalidData(
                "Signer ID in data does not match provided ID".to_string(),
            ));
        }

        let key = self.signer_key(&id);
        let mut conn = self.client.as_ref().clone();

        // Check if signer exists
        let exists: Result<bool, RedisError> = conn.exists(&key).await;
        match exists {
            Ok(false) => {
                return Err(RepositoryError::NotFound(format!(
                    "Signer with ID {} not found",
                    id
                )));
            }
            Ok(true) => {
                // Continue with update
            }
            Err(e) => {
                error!("Failed to check if signer exists: {}", e);
                return Err(RepositoryError::Other(format!(
                    "Failed to check signer existence: {}",
                    e
                )));
            }
        }

        // Serialize signer (encryption happens automatically for human-readable formats)
        let serialized = self.serialize_entity(&signer, |s| &s.id, "signer")?;

        // Update signer
        let result: Result<(), RedisError> = conn.set(&key, &serialized).await;
        result.map_err(|e| {
            error!("Failed to update signer {}: {}", id, e);
            RepositoryError::Other(format!("Failed to update signer: {}", e))
        })?;

        debug!("Updated signer with ID: {}", id);
        Ok(signer)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Signer ID cannot be empty".to_string(),
            ));
        }

        let key = self.signer_key(&id);
        let mut conn = self.client.as_ref().clone();

        // Check if signer exists
        let exists: Result<bool, RedisError> = conn.exists(&key).await;
        match exists {
            Ok(false) => {
                return Err(RepositoryError::NotFound(format!(
                    "Signer with ID {} not found",
                    id
                )));
            }
            Ok(true) => {
                // Continue with deletion
            }
            Err(e) => {
                error!("Failed to check if signer exists: {}", e);
                return Err(RepositoryError::Other(format!(
                    "Failed to check signer existence: {}",
                    e
                )));
            }
        }

        // Delete signer
        let result: Result<i64, RedisError> = conn.del(&key).await;
        result.map_err(|e| {
            error!("Failed to delete signer {}: {}", id, e);
            RepositoryError::Other(format!("Failed to delete signer: {}", e))
        })?;

        // Remove from list
        self.remove_from_list(&id).await?;

        debug!("Deleted signer with ID: {}", id);
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<SignerRepoModel>, RepositoryError> {
        let ids = self.get_all_ids().await?;

        if ids.is_empty() {
            debug!("No signers found");
            return Ok(Vec::new());
        }

        let signers = self.get_signers_by_ids(&ids).await?;
        debug!("Successfully fetched {} signers", signers.results.len());
        Ok(signers.results)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<SignerRepoModel>, RepositoryError> {
        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "per_page must be greater than 0".to_string(),
            ));
        }

        debug!(
            "Listing paginated signers: page {}, per_page {}",
            query.page, query.per_page
        );

        let all_ids: Vec<String> = self.get_all_ids().await?;
        let total = all_ids.len() as u64;
        let per_page = query.per_page as usize;
        let page = query.page as usize;
        let total_pages = all_ids.len().div_ceil(per_page);

        if page > total_pages && !all_ids.is_empty() {
            debug!(
                "Requested page {} exceeds total pages {}",
                page, total_pages
            );
            return Ok(PaginatedResult {
                items: Vec::new(),
                total,
                page: query.page,
                per_page: query.per_page,
            });
        }

        let start_idx = (page - 1) * per_page;
        let end_idx = std::cmp::min(start_idx + per_page, all_ids.len());

        let page_ids = all_ids[start_idx..end_idx].to_vec();
        let signers = self.get_signers_by_ids(&page_ids).await?;

        debug!(
            "Successfully retrieved {} signers for page {}",
            signers.results.len(),
            query.page
        );
        Ok(PaginatedResult {
            items: signers.results.clone(),
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let ids = self.get_all_ids().await?;
        Ok(ids.len())
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let signer_list_key = self.signer_list_key();

        debug!("Checking if signer entries exist");

        let exists: bool = conn
            .exists(&signer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "has_entries_check"))?;

        debug!("Signer entries exist: {}", exists);
        Ok(exists)
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let signer_list_key = self.signer_list_key();

        debug!("Dropping all signer entries");

        // Get all signer IDs first
        let signer_ids: Vec<String> = conn
            .smembers(&signer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_ids"))?;

        if signer_ids.is_empty() {
            debug!("No signer entries to drop");
            return Ok(());
        }

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all individual signer entries
        for signer_id in &signer_ids {
            let signer_key = self.signer_key(signer_id);
            pipe.del(&signer_key);
        }

        // Delete the signer list key
        pipe.del(&signer_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!("Dropped {} signer entries", signer_ids.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalSignerConfigStorage, SignerConfigStorage};
    use secrets::SecretVec;
    use std::sync::Arc;

    fn create_local_signer(id: &str) -> SignerRepoModel {
        SignerRepoModel {
            id: id.to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        }
    }

    async fn setup_test_repo() -> RedisSignerRepository {
        let client =
            redis::Client::open("redis://127.0.0.1:6379/").expect("Failed to create Redis client");
        let connection_manager = redis::aio::ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        RedisSignerRepository::new(Arc::new(connection_manager), "test".to_string())
            .expect("Failed to create repository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert_eq!(repo.key_prefix, "test");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_empty_prefix_fails() {
        let client =
            redis::Client::open("redis://127.0.0.1:6379/").expect("Failed to create Redis client");
        let connection_manager = redis::aio::ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        let result = RedisSignerRepository::new(Arc::new(connection_manager), "".to_string());
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
        let signer_key = repo.signer_key("test-id");
        let list_key = repo.signer_list_key();

        assert_eq!(signer_key, "test:signer:test-id");
        assert_eq!(list_key, "test:signer_list");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_serialize_deserialize_signer() {
        let repo = setup_test_repo().await;
        let signer = create_local_signer("test-signer");

        let serialized = repo.serialize_entity(&signer, |s| &s.id, "signer").unwrap();
        let deserialized: SignerRepoModel = repo
            .deserialize_entity(&serialized, &signer.id, "signer")
            .unwrap();

        assert_eq!(signer.id, deserialized.id);
        assert!(matches!(signer.config, SignerConfigStorage::Local(_)));
        assert!(matches!(deserialized.config, SignerConfigStorage::Local(_)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_signer() {
        let repo = setup_test_repo().await;
        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        let result = repo.create(signer).await;
        assert!(result.is_ok());

        let created_signer = result.unwrap();
        assert_eq!(created_signer.id, signer_name);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_signer() {
        let repo = setup_test_repo().await;
        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        // Create the signer first
        repo.create(signer.clone()).await.unwrap();

        // Get the signer
        let retrieved = repo.get_by_id(signer_name.clone()).await.unwrap();
        assert_eq!(retrieved.id, signer.id);
        assert!(matches!(retrieved.config, SignerConfigStorage::Local(_)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_signer() {
        let repo = setup_test_repo().await;
        let result = repo.get_by_id("nonexistent-id".to_string()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_signer() {
        let repo = setup_test_repo().await;
        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        // Create the signer first
        repo.create(signer.clone()).await.unwrap();

        // Update the signer
        let updated_signer = SignerRepoModel {
            id: signer_name.clone(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[2; 32])),
            }),
        };

        let result = repo.update(signer_name.clone(), updated_signer).await;
        assert!(result.is_ok());

        // Verify the update
        let retrieved = repo.get_by_id(signer_name).await.unwrap();
        assert!(matches!(retrieved.config, SignerConfigStorage::Local(_)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_signer() {
        let repo = setup_test_repo().await;
        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        // Create the signer first
        repo.create(signer).await.unwrap();

        // Delete the signer
        let result = repo.delete_by_id(signer_name.clone()).await;
        assert!(result.is_ok());

        // Verify deletion
        let get_result = repo.get_by_id(signer_name).await;
        assert!(get_result.is_err());
        assert!(matches!(
            get_result.unwrap_err(),
            RepositoryError::NotFound(_)
        ));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_all_signers() {
        let repo = setup_test_repo().await;
        let signer1_name = uuid::Uuid::new_v4().to_string();
        let signer2_name = uuid::Uuid::new_v4().to_string();
        let signer1 = create_local_signer(&signer1_name);
        let signer2 = create_local_signer(&signer2_name);

        // Create signers
        repo.create(signer1).await.unwrap();
        repo.create(signer2).await.unwrap();

        // List all signers
        let signers = repo.list_all().await.unwrap();
        assert!(signers.len() >= 2);

        let ids: Vec<String> = signers.iter().map(|s| s.id.clone()).collect();
        assert!(ids.contains(&signer1_name));
        assert!(ids.contains(&signer2_name));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_count_signers() {
        let repo = setup_test_repo().await;
        let initial_count = repo.count().await.unwrap();

        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        // Create a signer
        repo.create(signer).await.unwrap();

        // Check count increased
        let new_count = repo.count().await.unwrap();
        assert!(new_count > initial_count);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_paginated_signers() {
        let repo = setup_test_repo().await;
        let signer1_name = uuid::Uuid::new_v4().to_string();
        let signer2_name = uuid::Uuid::new_v4().to_string();
        let signer1 = create_local_signer(&signer1_name);
        let signer2 = create_local_signer(&signer2_name);

        // Create signers
        repo.create(signer1).await.unwrap();
        repo.create(signer2).await.unwrap();

        // Test pagination
        let query = PaginationQuery {
            page: 1,
            per_page: 1,
        };

        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 1);
        assert!(result.total >= 2);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 1);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_duplicate_signer_creation() {
        let repo = setup_test_repo().await;
        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        // Create the signer first time
        repo.create(signer.clone()).await.unwrap();

        // Try to create the same signer again
        let result = repo.create(signer).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RepositoryError::ConstraintViolation(_)
        ));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_debug_implementation() {
        let repo = setup_test_repo().await;
        let debug_str = format!("{:?}", repo);
        assert!(debug_str.contains("RedisSignerRepository"));
        assert!(debug_str.contains("test"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_error_handling_empty_id() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("".to_string()).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ID cannot be empty"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_signer_with_empty_id() {
        let repo = setup_test_repo().await;
        let signer = SignerRepoModel {
            id: "".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        };

        let result = repo.create(signer).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ID cannot be empty"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_nonexistent_signer() {
        let repo = setup_test_repo().await;
        let signer = create_local_signer("nonexistent-id");

        let result = repo.update("nonexistent-id".to_string(), signer).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_nonexistent_signer() {
        let repo = setup_test_repo().await;

        let result = repo.delete_by_id("nonexistent-id".to_string()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_with_mismatched_id() {
        let repo = setup_test_repo().await;
        let signer_name = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_name);

        // Create the signer first
        repo.create(signer).await.unwrap();

        // Try to update with different ID
        let updated_signer = create_local_signer("different-id");
        let result = repo.update(signer_name, updated_signer).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ID in data does not match"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries() {
        let repo = setup_test_repo().await;

        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_id);
        repo.create(signer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries() {
        let repo = setup_test_repo().await;
        let signer_id = uuid::Uuid::new_v4().to_string();
        let signer = create_local_signer(&signer_id);

        repo.create(signer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}
