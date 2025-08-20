//! Redis-backed implementation of the RelayerRepository.

use crate::models::UpdateRelayerRequest;
use crate::models::{PaginationQuery, RelayerNetworkPolicy, RelayerRepoModel, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{BatchRetrievalResult, PaginatedResult, RelayerRepository, Repository};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const RELAYER_PREFIX: &str = "relayer";
const RELAYER_LIST_KEY: &str = "relayer_list";

#[derive(Clone)]
pub struct RedisRelayerRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisRelayerRepository {}

impl RedisRelayerRepository {
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

    /// Generate key for relayer data: relayer:{relayer_id}
    fn relayer_key(&self, relayer_id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, RELAYER_PREFIX, relayer_id)
    }

    /// Generate key for relayer list: relayer_list (set of all relayer IDs)
    fn relayer_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, RELAYER_LIST_KEY)
    }

    /// Batch fetch relayers by IDs
    async fn get_relayers_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<RelayerRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No relayer IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.relayer_key(id)).collect();

        debug!("Batch fetching {} relayer data", keys.len());

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_relayers"))?;

        let mut relayers = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_entity(&json, &ids[i], "relayer") {
                        Ok(relayer) => relayers.push(relayer),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize relayer {}: {}", ids[i], e);
                            failed_ids.push(ids[i].clone());
                            // Continue processing other relayers
                        }
                    }
                }
                None => {
                    warn!("Relayer {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} relayers in batch",
                failed_count,
                ids.len()
            );
        }

        debug!("Successfully fetched {} relayers", relayers.len());
        Ok(BatchRetrievalResult {
            results: relayers,
            failed_ids,
        })
    }
}

impl fmt::Debug for RedisRelayerRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisRelayerRepository")
            .field("client", &"<ConnectionManager>")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[async_trait]
impl Repository<RelayerRepoModel, String> for RedisRelayerRepository {
    async fn create(&self, entity: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError> {
        if entity.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        if entity.name.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer name cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let relayer_key = self.relayer_key(&entity.id);

        // Check if relayer already exists
        let exists: bool = conn
            .exists(&relayer_key)
            .await
            .map_err(|e| self.map_redis_error(e, "create_relayer_exists_check"))?;

        if exists {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Relayer with ID {} already exists",
                entity.id
            )));
        }

        let serialized = self.serialize_entity(&entity, |r| &r.id, "relayer")?;

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&relayer_key, &serialized);
        pipe.sadd(self.relayer_list_key(), &entity.id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "create_relayer_pipeline"))?;

        debug!("Created relayer {}", entity.id);
        Ok(entity)
    }

    async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let relayer_key = self.relayer_key(&id);

        debug!("Fetching relayer {}", id);

        let json: Option<String> = conn
            .get(&relayer_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_relayer_by_id"))?;

        match json {
            Some(json) => {
                debug!("Found relayer {}", id);
                self.deserialize_entity(&json, &id, "relayer")
            }
            None => {
                debug!("Relayer {} not found", id);
                Err(RepositoryError::NotFound(format!(
                    "Relayer with ID {} not found",
                    id
                )))
            }
        }
    }

    async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_list_key = self.relayer_list_key();

        debug!("Listing all relayers");

        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_all_relayers"))?;

        debug!("Found {} relayers in index", relayer_ids.len());

        let relayers = self.get_relayers_by_ids(&relayer_ids).await?;
        Ok(relayers.results)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
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
        let relayer_list_key = self.relayer_list_key();

        // Get total count
        let total: u64 = conn
            .scard(&relayer_list_key)
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
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_members"))?;

        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_ids.len());

        let page_ids = &all_ids[start..end];
        let items = self.get_relayers_by_ids(page_ids).await?;

        Ok(PaginatedResult {
            items: items.results.clone(),
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn update(
        &self,
        id: String,
        entity: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        if entity.name.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer name cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let relayer_key = self.relayer_key(&id);

        // Check if relayer exists
        let exists: bool = conn
            .exists(&relayer_key)
            .await
            .map_err(|e| self.map_redis_error(e, "update_relayer_exists_check"))?;

        if !exists {
            return Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )));
        }

        // Ensure we preserve the original ID
        let mut updated_entity = entity;
        updated_entity.id = id.clone();

        let serialized = self.serialize_entity(&updated_entity, |r| &r.id, "relayer")?;

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&relayer_key, &serialized);
        pipe.sadd(self.relayer_list_key(), &id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "update_relayer_pipeline"))?;

        debug!("Updated relayer {}", id);
        Ok(updated_entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Relayer ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();
        let relayer_key = self.relayer_key(&id);

        // Check if relayer exists
        let exists: bool = conn
            .exists(&relayer_key)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_relayer_exists_check"))?;

        if !exists {
            return Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )));
        }

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&relayer_key);
        pipe.srem(self.relayer_list_key(), &id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_relayer_pipeline"))?;

        debug!("Deleted relayer {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_list_key = self.relayer_list_key();

        let count: u64 = conn
            .scard(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_relayers"))?;

        Ok(count as usize)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_list_key = self.relayer_list_key();

        debug!("Checking if relayer entries exist");

        let exists: bool = conn
            .exists(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "has_entries_check"))?;

        debug!("Relayer entries exist: {}", exists);
        Ok(exists)
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_list_key = self.relayer_list_key();

        debug!("Dropping all relayer entries");

        // Get all relayer IDs first
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_ids"))?;

        if relayer_ids.is_empty() {
            debug!("No relayer entries to drop");
            return Ok(());
        }

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all individual relayer entries
        for relayer_id in &relayer_ids {
            let relayer_key = self.relayer_key(relayer_id);
            pipe.del(&relayer_key);
        }

        // Delete the relayer list key
        pipe.del(&relayer_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!("Dropped {} relayer entries", relayer_ids.len());
        Ok(())
    }
}

#[async_trait]
impl RelayerRepository for RedisRelayerRepository {
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let all_relayers = self.list_all().await?;
        let active_relayers: Vec<RelayerRepoModel> = all_relayers
            .into_iter()
            .filter(|relayer| !relayer.paused)
            .collect();

        debug!("Found {} active relayers", active_relayers.len());
        Ok(active_relayers)
    }

    async fn list_by_signer_id(
        &self,
        signer_id: &str,
    ) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let all_relayers = self.list_all().await?;
        let relayers_with_signer: Vec<RelayerRepoModel> = all_relayers
            .into_iter()
            .filter(|relayer| relayer.signer_id == signer_id)
            .collect();

        debug!(
            "Found {} relayers using signer '{}'",
            relayers_with_signer.len(),
            signer_id
        );
        Ok(relayers_with_signer)
    }

    async fn list_by_notification_id(
        &self,
        notification_id: &str,
    ) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let all_relayers = self.list_all().await?;
        let relayers_with_notification: Vec<RelayerRepoModel> = all_relayers
            .into_iter()
            .filter(|relayer| {
                relayer
                    .notification_id
                    .as_ref()
                    .is_some_and(|id| id == notification_id)
            })
            .collect();

        debug!(
            "Found {} relayers using notification '{}'",
            relayers_with_notification.len(),
            notification_id
        );
        Ok(relayers_with_notification)
    }

    async fn partial_update(
        &self,
        id: String,
        update: UpdateRelayerRequest,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        // First get the current relayer
        let mut relayer = self.get_by_id(id.clone()).await?;

        // Apply the partial update
        if let Some(paused) = update.paused {
            relayer.paused = paused;
        }

        // Update the relayer
        self.update(id, relayer).await
    }

    async fn enable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        // First get the current relayer
        let mut relayer = self.get_by_id(relayer_id.clone()).await?;

        // Update the system_disabled flag
        relayer.system_disabled = false;

        // Update the relayer
        self.update(relayer_id, relayer).await
    }

    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        // First get the current relayer
        let mut relayer = self.get_by_id(relayer_id.clone()).await?;

        // Update the system_disabled flag
        relayer.system_disabled = true;

        // Update the relayer
        self.update(relayer_id, relayer).await
    }

    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        // First get the current relayer
        let mut relayer = self.get_by_id(id.clone()).await?;

        // Update the policy
        relayer.policies = policy;

        // Update the relayer
        self.update(id, relayer).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy};
    use redis::aio::ConnectionManager;
    use std::sync::Arc;

    fn create_test_relayer(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Test Relayer {}", id),
            network: "ethereum".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    fn create_test_relayer_with_pause(id: &str, paused: bool) -> RelayerRepoModel {
        let mut relayer = create_test_relayer(id);
        relayer.paused = paused;
        relayer
    }

    async fn setup_test_repo() -> RedisRelayerRepository {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        RedisRelayerRepository::new(Arc::new(connection_manager), "test".to_string())
            .expect("Failed to create Redis relayer repository")
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert_eq!(repo.key_prefix, "test");
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_new_repository_empty_prefix_fails() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create Redis connection manager");

        let result = RedisRelayerRepository::new(Arc::new(connection_manager), "".to_string());
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_key_generation() {
        let repo = setup_test_repo().await;

        let relayer_key = repo.relayer_key("test-relayer");
        assert_eq!(relayer_key, "test:relayer:test-relayer");

        let list_key = repo.relayer_list_key();
        assert_eq!(list_key, "test:relayer_list");
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_serialize_deserialize_relayer() {
        let repo = setup_test_repo().await;
        let relayer = create_test_relayer("test-relayer");

        let serialized = repo
            .serialize_entity(&relayer, |r| &r.id, "relayer")
            .unwrap();
        let deserialized: RelayerRepoModel = repo
            .deserialize_entity(&serialized, &relayer.id, "relayer")
            .unwrap();

        assert_eq!(relayer.id, deserialized.id);
        assert_eq!(relayer.name, deserialized.name);
        assert_eq!(relayer.network, deserialized.network);
        assert_eq!(relayer.paused, deserialized.paused);
        assert_eq!(relayer.network_type, deserialized.network_type);
        assert_eq!(relayer.signer_id, deserialized.signer_id);
        assert_eq!(relayer.address, deserialized.address);
        assert_eq!(relayer.notification_id, deserialized.notification_id);
        assert_eq!(relayer.system_disabled, deserialized.system_disabled);
        assert_eq!(relayer.custom_rpc_urls, deserialized.custom_rpc_urls);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_create_relayer() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        let result = repo.create(relayer.clone()).await;
        assert!(result.is_ok());

        let created_relayer = result.unwrap();
        assert_eq!(created_relayer.id, relayer_id);
        assert_eq!(created_relayer.name, relayer.name);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_get_relayer() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        let retrieved = repo.get_by_id(relayer_id).await.unwrap();
        assert_eq!(retrieved.id, relayer.id);
        assert_eq!(retrieved.name, relayer.name);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_list_all_relayers() {
        let repo = setup_test_repo().await;
        let relayer1_id = uuid::Uuid::new_v4().to_string();
        let relayer2_id = uuid::Uuid::new_v4().to_string();
        let relayer1 = create_test_relayer(&relayer1_id);
        let relayer2 = create_test_relayer(&relayer2_id);

        repo.create(relayer1).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let all_relayers = repo.list_all().await.unwrap();
        assert!(all_relayers.len() >= 2);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_list_active_relayers() {
        let repo = setup_test_repo().await;
        let relayer1_id = uuid::Uuid::new_v4().to_string();
        let relayer2_id = uuid::Uuid::new_v4().to_string();
        let relayer1 = create_test_relayer_with_pause(&relayer1_id, false);
        let relayer2 = create_test_relayer_with_pause(&relayer2_id, true);

        repo.create(relayer1).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let active_relayers = repo.list_active().await.unwrap();
        // Should have at least 1 active relayer
        assert!(!active_relayers.is_empty());
        // All returned relayers should be active
        assert!(active_relayers.iter().all(|r| !r.paused));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_count_relayers() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer).await.unwrap();

        let count = repo.count().await.unwrap();
        assert!(count >= 1);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_get_nonexistent_relayer() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("nonexistent-relayer".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_duplicate_relayer_creation() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        let duplicate_result = repo.create(relayer).await;
        assert!(matches!(
            duplicate_result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_update_relayer() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        let mut updated_relayer = relayer.clone();
        updated_relayer.name = "Updated Relayer Name".to_string();

        let result = repo.update(relayer.id.clone(), updated_relayer).await;
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.name, "Updated Relayer Name");
        assert_eq!(updated.id, relayer.id);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_delete_relayer() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        let delete_result = repo.delete_by_id(relayer.id.clone()).await;
        assert!(delete_result.is_ok());

        let get_result = repo.get_by_id(relayer.id).await;
        assert!(matches!(get_result, Err(RepositoryError::NotFound(_))));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_list_paginated() {
        let repo = setup_test_repo().await;
        let relayer1_id = uuid::Uuid::new_v4().to_string();
        let relayer2_id = uuid::Uuid::new_v4().to_string();
        let relayer1 = create_test_relayer(&relayer1_id);
        let relayer2 = create_test_relayer(&relayer2_id);

        repo.create(relayer1).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = repo.list_paginated(query).await.unwrap();
        assert!(result.total >= 2);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 10);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_partial_update_relayer() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        let update = UpdateRelayerRequest {
            paused: Some(true),
            ..Default::default()
        };
        let result = repo.partial_update(relayer.id.clone(), update).await;
        assert!(result.is_ok());

        let updated = result.unwrap();
        assert_eq!(updated.id, relayer.id);
        assert!(updated.paused);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_enable_disable_relayer() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        // Test disable
        let disabled = repo.disable_relayer(relayer.id.clone()).await.unwrap();
        assert!(disabled.system_disabled);

        // Test enable
        let enabled = repo.enable_relayer(relayer.id.clone()).await.unwrap();
        assert!(!enabled.system_disabled);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_update_policy() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);

        repo.create(relayer.clone()).await.unwrap();

        let new_policy = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(50_000_000_000),
            whitelist_receivers: Some(vec!["0x123".to_string()]),
            eip1559_pricing: Some(true),
            private_transactions: Some(true),
            min_balance: Some(1000000000000000000),
            gas_limit_estimation: Some(true),
        });

        let result = repo.update_policy(relayer.id.clone(), new_policy).await;
        assert!(result.is_ok());

        let updated = result.unwrap();
        if let RelayerNetworkPolicy::Evm(evm_policy) = updated.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(50_000_000_000));
            assert_eq!(
                evm_policy.whitelist_receivers,
                Some(vec!["0x123".to_string()])
            );
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
            assert!(evm_policy.private_transactions.unwrap_or(false));
            assert_eq!(evm_policy.min_balance, Some(1000000000000000000));
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_debug_implementation() {
        let repo = setup_test_repo().await;
        let debug_str = format!("{:?}", repo);
        assert!(debug_str.contains("RedisRelayerRepository"));
        assert!(debug_str.contains("key_prefix"));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_error_handling_empty_id() {
        let repo = setup_test_repo().await;

        let create_result = repo
            .create(RelayerRepoModel {
                id: "".to_string(),
                ..create_test_relayer("test")
            })
            .await;
        assert!(matches!(
            create_result,
            Err(RepositoryError::InvalidData(_))
        ));

        let get_result = repo.get_by_id("".to_string()).await;
        assert!(matches!(get_result, Err(RepositoryError::InvalidData(_))));

        let update_result = repo
            .update("".to_string(), create_test_relayer("test"))
            .await;
        assert!(matches!(
            update_result,
            Err(RepositoryError::InvalidData(_))
        ));

        let delete_result = repo.delete_by_id("".to_string()).await;
        assert!(matches!(
            delete_result,
            Err(RepositoryError::InvalidData(_))
        ));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_error_handling_empty_name() {
        let repo = setup_test_repo().await;

        let create_result = repo
            .create(RelayerRepoModel {
                name: "".to_string(),
                ..create_test_relayer("test")
            })
            .await;
        assert!(matches!(
            create_result,
            Err(RepositoryError::InvalidData(_))
        ));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_pagination_validation() {
        let repo = setup_test_repo().await;

        let invalid_page = PaginationQuery {
            page: 0,
            per_page: 10,
        };
        let result = repo.list_paginated(invalid_page).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));

        let invalid_per_page = PaginationQuery {
            page: 1,
            per_page: 0,
        };
        let result = repo.list_paginated(invalid_per_page).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_update_nonexistent_relayer() {
        let repo = setup_test_repo().await;
        let relayer = create_test_relayer("nonexistent-relayer");

        let result = repo
            .update("nonexistent-relayer".to_string(), relayer)
            .await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_delete_nonexistent_relayer() {
        let repo = setup_test_repo().await;

        let result = repo.delete_by_id("nonexistent-relayer".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries() {
        let repo = setup_test_repo().await;
        assert!(!repo.has_entries().await.unwrap());

        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);
        repo.create(relayer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries() {
        let repo = setup_test_repo().await;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        let relayer = create_test_relayer(&relayer_id);
        repo.create(relayer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_list_by_signer_id() {
        let repo = setup_test_repo().await;

        let relayer1_id = uuid::Uuid::new_v4().to_string();
        let relayer2_id = uuid::Uuid::new_v4().to_string();
        let relayer3_id = uuid::Uuid::new_v4().to_string();
        let signer1_id = uuid::Uuid::new_v4().to_string();
        let signer2_id = uuid::Uuid::new_v4().to_string();

        let mut relayer1 = create_test_relayer(&relayer1_id);
        relayer1.signer_id = signer1_id.clone();
        repo.create(relayer1).await.unwrap();

        let mut relayer2 = create_test_relayer(&relayer2_id);

        relayer2.signer_id = signer2_id.clone();
        repo.create(relayer2).await.unwrap();

        let mut relayer3 = create_test_relayer(&relayer3_id);
        relayer3.signer_id = signer1_id.clone();
        repo.create(relayer3).await.unwrap();

        let result = repo.list_by_signer_id(&signer1_id).await.unwrap();
        assert_eq!(result.len(), 2);
        let ids: Vec<_> = result.iter().map(|r| r.id.clone()).collect();
        assert!(ids.contains(&relayer1_id));
        assert!(ids.contains(&relayer3_id));

        let result = repo.list_by_signer_id(&signer2_id).await.unwrap();
        assert_eq!(result.len(), 1);

        let result = repo.list_by_signer_id("nonexistent").await.unwrap();
        assert_eq!(result.len(), 0);
    }

    #[ignore = "Requires active Redis instance"]
    #[tokio::test]
    async fn test_list_by_notification_id() {
        let repo = setup_test_repo().await;

        let relayer1_id = uuid::Uuid::new_v4().to_string();
        let mut relayer1 = create_test_relayer(&relayer1_id);
        relayer1.notification_id = Some("notif1".to_string());
        repo.create(relayer1).await.unwrap();

        let relayer2_id = uuid::Uuid::new_v4().to_string();
        let mut relayer2 = create_test_relayer(&relayer2_id);
        relayer2.notification_id = Some("notif2".to_string());
        repo.create(relayer2).await.unwrap();

        let relayer3_id = uuid::Uuid::new_v4().to_string();
        let mut relayer3 = create_test_relayer(&relayer3_id);
        relayer3.notification_id = Some("notif1".to_string());
        repo.create(relayer3).await.unwrap();

        let relayer4_id = uuid::Uuid::new_v4().to_string();
        let mut relayer4 = create_test_relayer(&relayer4_id);
        relayer4.notification_id = None;
        repo.create(relayer4).await.unwrap();

        let result = repo.list_by_notification_id("notif1").await.unwrap();
        assert_eq!(result.len(), 2);
        let ids: Vec<_> = result.iter().map(|r| r.id.clone()).collect();
        assert!(ids.contains(&relayer1_id));
        assert!(ids.contains(&relayer3_id));

        let result = repo.list_by_notification_id("notif2").await.unwrap();
        assert_eq!(result.len(), 1);

        let result = repo.list_by_notification_id("nonexistent").await.unwrap();
        assert_eq!(result.len(), 0);
    }
}
