//! Redis implementation of the network repository.
//!
//! This module provides a Redis-based implementation of the `NetworkRepository` trait,
//! allowing network configurations to be stored and retrieved from a Redis database.
//! The implementation includes comprehensive error handling, logging, validation, and
//! efficient indexing for fast lookups by name and chain ID.

use super::NetworkRepository;
use crate::models::{NetworkRepoModel, NetworkType, RepositoryError};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{BatchRetrievalResult, PaginatedResult, PaginationQuery, Repository};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const NETWORK_PREFIX: &str = "network";
const NETWORK_LIST_KEY: &str = "network_list";
const NETWORK_NAME_INDEX_PREFIX: &str = "network_name";
const NETWORK_CHAIN_ID_INDEX_PREFIX: &str = "network_chain_id";

#[derive(Clone)]
pub struct RedisNetworkRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisNetworkRepository {}

impl RedisNetworkRepository {
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

    /// Generate key for network data: network:{network_id}
    fn network_key(&self, network_id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, NETWORK_PREFIX, network_id)
    }

    /// Generate key for network list: network_list (set of all network IDs)
    fn network_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, NETWORK_LIST_KEY)
    }

    /// Generate key for network name index: network_name:{network_type}:{name}
    fn network_name_index_key(&self, network_type: &NetworkType, name: &str) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix, NETWORK_NAME_INDEX_PREFIX, network_type, name
        )
    }

    /// Generate key for network chain ID index: network_chain_id:{network_type}:{chain_id}
    fn network_chain_id_index_key(&self, network_type: &NetworkType, chain_id: u64) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix, NETWORK_CHAIN_ID_INDEX_PREFIX, network_type, chain_id
        )
    }

    /// Extract chain ID from network configuration
    fn extract_chain_id(&self, network: &NetworkRepoModel) -> Option<u64> {
        match &network.config {
            crate::models::NetworkConfigData::Evm(evm_config) => evm_config.chain_id,
            _ => None,
        }
    }

    /// Update indexes for a network
    async fn update_indexes(
        &self,
        network: &NetworkRepoModel,
        old_network: Option<&NetworkRepoModel>,
    ) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let mut pipe = redis::pipe();
        pipe.atomic();

        debug!("Updating indexes for network {}", network.id);

        // Add name index
        let name_key = self.network_name_index_key(&network.network_type, &network.name);
        pipe.set(&name_key, &network.id);

        // Add chain ID index if applicable
        if let Some(chain_id) = self.extract_chain_id(network) {
            let chain_id_key = self.network_chain_id_index_key(&network.network_type, chain_id);
            pipe.set(&chain_id_key, &network.id);
            debug!(
                "Added chain ID index for network {} with chain_id {}",
                network.id, chain_id
            );
        }

        // Remove old indexes if updating
        if let Some(old) = old_network {
            // Remove old name index if name or type changed
            if old.name != network.name || old.network_type != network.network_type {
                let old_name_key = self.network_name_index_key(&old.network_type, &old.name);
                pipe.del(&old_name_key);
                debug!(
                    "Removing old name index for network {} (name: {} -> {})",
                    network.id, old.name, network.name
                );
            }

            // Handle chain ID index cleanup
            let old_chain_id = self.extract_chain_id(old);
            let new_chain_id = self.extract_chain_id(network);

            if old_chain_id != new_chain_id {
                if let Some(old_chain_id) = old_chain_id {
                    let old_chain_id_key =
                        self.network_chain_id_index_key(&old.network_type, old_chain_id);
                    pipe.del(&old_chain_id_key);
                    debug!(
                        "Removing old chain ID index for network {} (chain_id: {} -> {:?})",
                        network.id, old_chain_id, new_chain_id
                    );
                }
            }
        }

        // Execute all operations in a single pipeline
        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!(
                "Index update pipeline failed for network {}: {}",
                network.id, e
            );
            self.map_redis_error(e, &format!("update_indexes_for_network_{}", network.id))
        })?;

        debug!("Successfully updated indexes for network {}", network.id);
        Ok(())
    }

    /// Remove all indexes for a network
    async fn remove_all_indexes(&self, network: &NetworkRepoModel) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let mut pipe = redis::pipe();
        pipe.atomic();

        debug!("Removing all indexes for network {}", network.id);

        // Remove name index
        let name_key = self.network_name_index_key(&network.network_type, &network.name);
        pipe.del(&name_key);

        // Remove chain ID index if applicable
        if let Some(chain_id) = self.extract_chain_id(network) {
            let chain_id_key = self.network_chain_id_index_key(&network.network_type, chain_id);
            pipe.del(&chain_id_key);
            debug!(
                "Removing chain ID index for network {} with chain_id {}",
                network.id, chain_id
            );
        }

        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!("Index removal failed for network {}: {}", network.id, e);
            self.map_redis_error(e, &format!("remove_indexes_for_network_{}", network.id))
        })?;

        debug!(
            "Successfully removed all indexes for network {}",
            network.id
        );
        Ok(())
    }

    /// Batch fetch networks by IDs
    async fn get_networks_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<NetworkRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No network IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.network_key(id)).collect();

        debug!("Batch fetching {} networks", ids.len());

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_networks"))?;

        let mut networks = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();

        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_entity::<NetworkRepoModel>(&json, &ids[i], "network") {
                        Ok(network) => networks.push(network),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize network {}: {}", ids[i], e);
                            failed_ids.push(ids[i].clone());
                        }
                    }
                }
                None => {
                    warn!("Network {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} networks in batch",
                failed_count,
                ids.len()
            );
            warn!("Failed to deserialize networks: {:?}", failed_ids);
        }

        debug!("Successfully fetched {} networks", networks.len());
        Ok(BatchRetrievalResult {
            results: networks,
            failed_ids,
        })
    }
}

impl fmt::Debug for RedisNetworkRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisNetworkRepository")
            .field("client", &"<ConnectionManager>")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[async_trait]
impl Repository<NetworkRepoModel, String> for RedisNetworkRepository {
    async fn create(&self, entity: NetworkRepoModel) -> Result<NetworkRepoModel, RepositoryError> {
        if entity.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Network ID cannot be empty".to_string(),
            ));
        }
        if entity.name.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Network name cannot be empty".to_string(),
            ));
        }
        let key = self.network_key(&entity.id);
        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Creating network with ID: {}", entity.id);

        let value = self.serialize_entity(&entity, |n| &n.id, "network")?;

        // Check if network already exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "create_network_check_existing"))?;

        if existing.is_some() {
            warn!(
                "Attempted to create network {} that already exists",
                entity.id
            );
            return Err(RepositoryError::ConstraintViolation(format!(
                "Network with ID {} already exists",
                entity.id
            )));
        }

        // Use Redis pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.set(&key, &value);
        pipe.sadd(&network_list_key, &entity.id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "create_network_pipeline"))?;

        // Update indexes
        self.update_indexes(&entity, None).await?;

        debug!("Successfully created network with ID: {}", entity.id);
        Ok(entity)
    }

    async fn get_by_id(&self, id: String) -> Result<NetworkRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Network ID cannot be empty".to_string(),
            ));
        }

        let key = self.network_key(&id);
        let mut conn = self.client.as_ref().clone();

        debug!("Retrieving network with ID: {}", id);

        let network_data: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_by_id"))?;

        match network_data {
            Some(data) => {
                let network = self.deserialize_entity::<NetworkRepoModel>(&data, &id, "network")?;
                debug!("Successfully retrieved network with ID: {}", id);
                Ok(network)
            }
            None => {
                debug!("Network with ID {} not found", id);
                Err(RepositoryError::NotFound(format!(
                    "Network with ID {} not found",
                    id
                )))
            }
        }
    }

    async fn list_all(&self) -> Result<Vec<NetworkRepoModel>, RepositoryError> {
        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Listing all networks");

        let ids: Vec<String> = conn
            .smembers(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_all_networks"))?;

        if ids.is_empty() {
            debug!("No networks found");
            return Ok(Vec::new());
        }

        let networks = self.get_networks_by_ids(&ids).await?;
        debug!("Successfully retrieved {} networks", networks.results.len());
        Ok(networks.results)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NetworkRepoModel>, RepositoryError> {
        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "per_page must be greater than 0".to_string(),
            ));
        }

        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!(
            "Listing paginated networks: page {}, per_page {}",
            query.page, query.per_page
        );

        let all_ids: Vec<String> = conn
            .smembers(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_networks"))?;

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
        let networks = self.get_networks_by_ids(&page_ids).await?;

        debug!(
            "Successfully retrieved {} networks for page {}",
            networks.results.len(),
            query.page
        );
        Ok(PaginatedResult {
            items: networks.results.clone(),
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn update(
        &self,
        id: String,
        entity: NetworkRepoModel,
    ) -> Result<NetworkRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Network ID cannot be empty".to_string(),
            ));
        }

        if id != entity.id {
            return Err(RepositoryError::InvalidData(format!(
                "ID mismatch: provided ID '{}' doesn't match network ID '{}'",
                id, entity.id
            )));
        }

        let key = self.network_key(&id);
        let mut conn = self.client.as_ref().clone();

        debug!("Updating network with ID: {}", id);

        // Get the old network for index cleanup
        let old_network = self.get_by_id(id.clone()).await?;

        let value = self.serialize_entity(&entity, |n| &n.id, "network")?;

        let _: () = conn
            .set(&key, &value)
            .await
            .map_err(|e| self.map_redis_error(e, "update_network"))?;

        // Update indexes
        self.update_indexes(&entity, Some(&old_network)).await?;

        debug!("Successfully updated network with ID: {}", id);
        Ok(entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Network ID cannot be empty".to_string(),
            ));
        }

        let key = self.network_key(&id);
        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Deleting network with ID: {}", id);

        // Get network for index cleanup
        let network = self.get_by_id(id.clone()).await?;

        // Use Redis pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.del(&key);
        pipe.srem(&network_list_key, &id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_network_pipeline"))?;

        // Remove indexes (log errors but don't fail the delete)
        if let Err(e) = self.remove_all_indexes(&network).await {
            error!("Failed to remove indexes for deleted network {}: {}", id, e);
        }

        debug!("Successfully deleted network with ID: {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Counting networks");

        let count: usize = conn
            .scard(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_networks"))?;

        debug!("Total networks count: {}", count);
        Ok(count)
    }

    /// Check if Redis storage contains any network entries.
    /// This is used to determine if Redis storage is being used for networks.
    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Checking if network storage has entries");

        let exists: bool = conn
            .exists(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "check_network_entries_exist"))?;

        debug!("Network storage has entries: {}", exists);
        Ok(exists)
    }

    /// Drop all network-related entries from Redis storage.
    /// This includes all network data, indexes, and the network list.
    /// Use with caution as this will permanently delete all network data.
    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();

        debug!("Starting to drop all network entries from Redis storage");

        // First, get all network IDs to clean up their data and indexes
        let network_list_key = self.network_list_key();
        let network_ids: Vec<String> = conn
            .smembers(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_ids_for_cleanup"))?;

        if network_ids.is_empty() {
            debug!("No network entries found to clean up");
            return Ok(());
        }

        debug!("Found {} networks to clean up", network_ids.len());

        // Get all networks to clean up their indexes properly
        let networks_result = self.get_networks_by_ids(&network_ids).await?;
        let networks = networks_result.results;

        // Use a pipeline for efficient batch operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all network data entries
        for network_id in &network_ids {
            let network_key = self.network_key(network_id);
            pipe.del(&network_key);
        }

        // Delete all index entries
        for network in &networks {
            // Delete name index
            let name_key = self.network_name_index_key(&network.network_type, &network.name);
            pipe.del(&name_key);

            // Delete chain ID index if applicable
            if let Some(chain_id) = self.extract_chain_id(network) {
                let chain_id_key = self.network_chain_id_index_key(&network.network_type, chain_id);
                pipe.del(&chain_id_key);
            }
        }

        // Delete the network list
        pipe.del(&network_list_key);

        // Execute all deletions
        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!("Failed to execute cleanup pipeline: {}", e);
            self.map_redis_error(e, "drop_all_network_entries_pipeline")
        })?;

        debug!("Successfully dropped all network entries from Redis storage");
        Ok(())
    }
}

#[async_trait]
impl NetworkRepository for RedisNetworkRepository {
    async fn get_by_name(
        &self,
        network_type: NetworkType,
        name: &str,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError> {
        if name.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Network name cannot be empty".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();

        debug!(
            "Getting network by name: {} (type: {:?})",
            name, network_type
        );

        // Use name index for O(1) lookup
        let name_index_key = self.network_name_index_key(&network_type, name);
        let network_id: Option<String> = conn
            .get(&name_index_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_by_name_index"))?;

        match network_id {
            Some(id) => {
                match self.get_by_id(id.clone()).await {
                    Ok(network) => {
                        debug!("Found network by name: {}", name);
                        Ok(Some(network))
                    }
                    Err(RepositoryError::NotFound(_)) => {
                        // Network was deleted but index wasn't cleaned up
                        warn!(
                            "Stale name index found for network type {:?} name {}",
                            network_type, name
                        );
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
            None => {
                debug!("Network not found by name: {}", name);
                Ok(None)
            }
        }
    }

    async fn get_by_chain_id(
        &self,
        network_type: NetworkType,
        chain_id: u64,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError> {
        // Only EVM networks have chain_id
        if network_type != NetworkType::Evm {
            return Ok(None);
        }

        let mut conn = self.client.as_ref().clone();

        debug!(
            "Getting network by chain ID: {} (type: {:?})",
            chain_id, network_type
        );

        // Use chain ID index for O(1) lookup
        let chain_id_index_key = self.network_chain_id_index_key(&network_type, chain_id);
        let network_id: Option<String> = conn
            .get(&chain_id_index_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_by_chain_id_index"))?;

        match network_id {
            Some(id) => {
                match self.get_by_id(id.clone()).await {
                    Ok(network) => {
                        debug!("Found network by chain ID: {}", chain_id);
                        Ok(Some(network))
                    }
                    Err(RepositoryError::NotFound(_)) => {
                        // Network was deleted but index wasn't cleaned up
                        warn!(
                            "Stale chain ID index found for network type {:?} chain_id {}",
                            network_type, chain_id
                        );
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
            None => {
                debug!("Network not found by chain ID: {}", chain_id);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EvmNetworkConfig, NetworkConfigCommon, SolanaNetworkConfig, StellarNetworkConfig,
    };
    use crate::models::NetworkConfigData;
    use redis::aio::ConnectionManager;
    use uuid::Uuid;

    fn create_test_network(name: &str, network_type: NetworkType) -> NetworkRepoModel {
        let common = NetworkConfigCommon {
            network: name.to_string(),
            from: None,
            rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
            explorer_urls: None,
            average_blocktime_ms: Some(12000),
            is_testnet: Some(true),
            tags: None,
        };

        match network_type {
            NetworkType::Evm => {
                let evm_config = EvmNetworkConfig {
                    common,
                    chain_id: Some(1),
                    required_confirmations: Some(1),
                    features: None,
                    symbol: Some("ETH".to_string()),
                };
                NetworkRepoModel::new_evm(evm_config)
            }
            NetworkType::Solana => {
                let solana_config = SolanaNetworkConfig { common };
                NetworkRepoModel::new_solana(solana_config)
            }
            NetworkType::Stellar => {
                let stellar_config = StellarNetworkConfig {
                    common,
                    passphrase: None,
                };
                NetworkRepoModel::new_stellar(stellar_config)
            }
        }
    }

    async fn setup_test_repo() -> RedisNetworkRepository {
        let redis_url = "redis://localhost:6379";
        let random_id = Uuid::new_v4().to_string();
        let key_prefix = format!("test_prefix_{}", random_id);

        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        RedisNetworkRepository::new(Arc::new(connection_manager), key_prefix.to_string())
            .expect("Failed to create repository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_network() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        let result = repo.create(network.clone()).await;
        assert!(result.is_ok());

        let created = result.unwrap();
        assert_eq!(created.id, network.id);
        assert_eq!(created.name, network.name);
        assert_eq!(created.network_type, network.network_type);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_network_by_id() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let retrieved = repo.get_by_id(network.id.clone()).await;
        assert!(retrieved.is_ok());

        let retrieved_network = retrieved.unwrap();
        assert_eq!(retrieved_network.id, network.id);
        assert_eq!(retrieved_network.name, network.name);
        assert_eq!(retrieved_network.network_type, network.network_type);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_network() {
        let repo = setup_test_repo().await;
        let result = repo.get_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_duplicate_network() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();
        let result = repo.create(network).await;
        assert!(matches!(
            result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_network() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_name = Uuid::new_v4().to_string();
        let mut network = create_test_network(&random_name, NetworkType::Evm);
        network.id = format!("evm:{}", random_id);

        // Create the network first
        repo.create(network.clone()).await.unwrap();

        // Update the network
        let updated = repo.update(network.id.clone(), network.clone()).await;
        assert!(updated.is_ok());

        let updated_network = updated.unwrap();
        assert_eq!(updated_network.id, network.id);
        assert_eq!(updated_network.name, network.name);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_network() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_name = Uuid::new_v4().to_string();
        let mut network = create_test_network(&random_name, NetworkType::Evm);
        network.id = format!("evm:{}", random_id);

        // Create the network first
        repo.create(network.clone()).await.unwrap();

        // Delete the network
        let result = repo.delete_by_id(network.id.clone()).await;
        assert!(result.is_ok());

        // Verify it's deleted
        let get_result = repo.get_by_id(network.id).await;
        assert!(matches!(get_result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_all_networks() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let test_network_random2 = Uuid::new_v4().to_string();
        let network1 = create_test_network(&test_network_random, NetworkType::Evm);
        let network2 = create_test_network(&test_network_random2, NetworkType::Solana);

        repo.create(network1.clone()).await.unwrap();
        repo.create(network2.clone()).await.unwrap();

        let networks = repo.list_all().await.unwrap();
        assert_eq!(networks.len(), 2);

        let ids: Vec<String> = networks.iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains(&network1.id));
        assert!(ids.contains(&network2.id));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_count_networks() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let test_network_random2 = Uuid::new_v4().to_string();
        let network1 = create_test_network(&test_network_random, NetworkType::Evm);
        let network2 = create_test_network(&test_network_random2, NetworkType::Solana);

        assert_eq!(repo.count().await.unwrap(), 0);

        repo.create(network1).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        repo.create(network2).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 2);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_paginated() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let test_network_random2 = Uuid::new_v4().to_string();
        let test_network_random3 = Uuid::new_v4().to_string();
        let network1 = create_test_network(&test_network_random, NetworkType::Evm);
        let network2 = create_test_network(&test_network_random2, NetworkType::Solana);
        let network3 = create_test_network(&test_network_random3, NetworkType::Stellar);

        repo.create(network1).await.unwrap();
        repo.create(network2).await.unwrap();
        repo.create(network3).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };

        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total, 3);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 2);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_name() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let retrieved = repo
            .get_by_name(NetworkType::Evm, &test_network_random)
            .await
            .unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, test_network_random);

        let not_found = repo
            .get_by_name(NetworkType::Solana, &test_network_random)
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_chain_id() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let retrieved = repo.get_by_chain_id(NetworkType::Evm, 1).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, test_network_random);

        let not_found = repo.get_by_chain_id(NetworkType::Evm, 999).await.unwrap();
        assert!(not_found.is_none());

        let solana_result = repo.get_by_chain_id(NetworkType::Solana, 1).await.unwrap();
        assert!(solana_result.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_nonexistent_network() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        let result = repo.update(network.id.clone(), network).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_nonexistent_network() {
        let repo = setup_test_repo().await;

        let result = repo.delete_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_empty_id_validation() {
        let repo = setup_test_repo().await;

        let create_result = repo
            .create(NetworkRepoModel {
                id: "".to_string(),
                name: "test".to_string(),
                network_type: NetworkType::Evm,
                config: NetworkConfigData::Evm(EvmNetworkConfig {
                    common: NetworkConfigCommon {
                        network: "test".to_string(),
                        from: None,
                        rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
                        explorer_urls: None,
                        average_blocktime_ms: Some(12000),
                        is_testnet: Some(true),
                        tags: None,
                    },
                    chain_id: Some(1),
                    required_confirmations: Some(1),
                    features: None,
                    symbol: Some("ETH".to_string()),
                }),
            })
            .await;

        assert!(matches!(
            create_result,
            Err(RepositoryError::InvalidData(_))
        ));

        let get_result = repo.get_by_id("".to_string()).await;
        assert!(matches!(get_result, Err(RepositoryError::InvalidData(_))));

        let update_result = repo
            .update(
                "".to_string(),
                create_test_network("test", NetworkType::Evm),
            )
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

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_pagination_validation() {
        let repo = setup_test_repo().await;

        let query = PaginationQuery {
            page: 1,
            per_page: 0,
        };
        let result = repo.list_paginated(query).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_id_mismatch_validation() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let result = repo.update("different-id".to_string(), network).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_empty_name_validation() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_name(NetworkType::Evm, "").await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries_empty_storage() {
        let repo = setup_test_repo().await;

        let result = repo.has_entries().await.unwrap();
        assert!(!result, "Empty storage should return false");
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries_with_data() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let network = create_test_network(&test_network_random, NetworkType::Evm);

        assert!(!repo.has_entries().await.unwrap());

        repo.create(network).await.unwrap();

        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries_empty_storage() {
        let repo = setup_test_repo().await;

        let result = repo.drop_all_entries().await;
        assert!(result.is_ok());

        assert!(!repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries_with_data() {
        let repo = setup_test_repo().await;
        let test_network_random1 = Uuid::new_v4().to_string();
        let test_network_random2 = Uuid::new_v4().to_string();
        let network1 = create_test_network(&test_network_random1, NetworkType::Evm);
        let network2 = create_test_network(&test_network_random2, NetworkType::Solana);

        // Add networks
        repo.create(network1.clone()).await.unwrap();
        repo.create(network2.clone()).await.unwrap();

        // Verify they exist
        assert!(repo.has_entries().await.unwrap());
        assert_eq!(repo.count().await.unwrap(), 2);
        assert!(repo
            .get_by_name(NetworkType::Evm, &test_network_random1)
            .await
            .unwrap()
            .is_some());

        // Drop all entries
        let result = repo.drop_all_entries().await;
        assert!(result.is_ok());

        // Verify everything is cleaned up
        assert!(!repo.has_entries().await.unwrap());
        assert_eq!(repo.count().await.unwrap(), 0);
        assert!(repo
            .get_by_name(NetworkType::Evm, &test_network_random1)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .get_by_name(NetworkType::Solana, &test_network_random2)
            .await
            .unwrap()
            .is_none());

        // Verify individual networks are gone
        assert!(matches!(
            repo.get_by_id(network1.id).await,
            Err(RepositoryError::NotFound(_))
        ));
        assert!(matches!(
            repo.get_by_id(network2.id).await,
            Err(RepositoryError::NotFound(_))
        ));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries_cleans_indexes() {
        let repo = setup_test_repo().await;
        let test_network_random = Uuid::new_v4().to_string();
        let mut network = create_test_network(&test_network_random, NetworkType::Evm);

        // Ensure we have a specific chain ID for testing
        if let crate::models::NetworkConfigData::Evm(ref mut evm_config) = network.config {
            evm_config.chain_id = Some(12345);
        }

        // Add network
        repo.create(network.clone()).await.unwrap();

        // Verify indexes work
        assert!(repo
            .get_by_name(NetworkType::Evm, &test_network_random)
            .await
            .unwrap()
            .is_some());
        assert!(repo
            .get_by_chain_id(NetworkType::Evm, 12345)
            .await
            .unwrap()
            .is_some());

        // Drop all entries
        repo.drop_all_entries().await.unwrap();

        // Verify indexes are cleaned up
        assert!(repo
            .get_by_name(NetworkType::Evm, &test_network_random)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .get_by_chain_id(NetworkType::Evm, 12345)
            .await
            .unwrap()
            .is_none());
    }
}
