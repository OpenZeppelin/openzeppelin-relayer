//! Redis implementation of the network repository.
//!
//! This module provides a Redis-based implementation of the `NetworkRepository` trait,
//! allowing network configurations to be stored and retrieved from a Redis database.
//! The implementation includes comprehensive error handling, logging, and validation.

use super::NetworkRepository;
use crate::models::{NetworkRepoModel, NetworkType, RepositoryError};
use crate::repositories::{PaginatedResult, PaginationQuery, Repository};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisError};
use std::fmt;
use std::sync::Arc;

const NETWORK_PREFIX: &str = "network";
const NETWORK_LIST_KEY: &str = "network_list";

#[derive(Clone)]
pub struct RedisNetworkRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

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

    /// Convert Redis errors to appropriate RepositoryError types
    fn map_redis_error(&self, error: RedisError, context: &str) -> RepositoryError {
        match error.kind() {
            redis::ErrorKind::IoError => {
                error!("Redis IO error in {}: {}", context, error);
                RepositoryError::ConnectionError(format!("Redis connection failed: {}", error))
            }
            redis::ErrorKind::AuthenticationFailed => {
                error!("Redis authentication failed in {}: {}", context, error);
                RepositoryError::PermissionDenied(format!("Redis authentication failed: {}", error))
            }
            redis::ErrorKind::TypeError => {
                error!("Redis type error in {}: {}", context, error);
                RepositoryError::InvalidData(format!("Redis data type error: {}", error))
            }
            redis::ErrorKind::ExecAbortError => {
                warn!("Redis transaction aborted in {}: {}", context, error);
                RepositoryError::TransactionFailure(format!("Redis transaction aborted: {}", error))
            }
            redis::ErrorKind::BusyLoadingError => {
                warn!("Redis busy loading in {}: {}", context, error);
                RepositoryError::ConnectionError(format!("Redis is loading: {}", error))
            }
            redis::ErrorKind::NoScriptError => {
                error!("Redis script error in {}: {}", context, error);
                RepositoryError::Other(format!("Redis script error: {}", error))
            }
            _ => {
                error!("Unexpected Redis error in {}: {}", context, error);
                RepositoryError::Other(format!("Redis error in {}: {}", context, error))
            }
        }
    }

    /// Serialize network with detailed error context
    fn serialize_network(&self, network: &NetworkRepoModel) -> Result<String, RepositoryError> {
        serde_json::to_string(network).map_err(|e| {
            error!("Serialization failed for network {}: {}", network.id, e);
            RepositoryError::InvalidData(format!(
                "Failed to serialize network {}: {}",
                network.id, e
            ))
        })
    }

    /// Deserialize network with detailed error context
    fn deserialize_network(
        &self,
        json: &str,
        network_id: &str,
    ) -> Result<NetworkRepoModel, RepositoryError> {
        serde_json::from_str(json).map_err(|e| {
            error!("Deserialization failed for network {}: {}", network_id, e);
            RepositoryError::InvalidData(format!(
                "Failed to deserialize network {}: {} (JSON length: {})",
                network_id,
                e,
                json.len()
            ))
        })
    }

    /// Batch fetch networks by IDs
    async fn get_networks_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<NetworkRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No network IDs provided for batch fetch");
            return Ok(vec![]);
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.network_key(id)).collect();

        debug!("Batch fetching {} network data", keys.len());

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_networks"))?;

        let mut networks = Vec::new();
        let mut failed_count = 0;

        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_network(&json, &ids[i]) {
                        Ok(network) => networks.push(network),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize network {}: {}", ids[i], e);
                            // Continue processing other networks
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
        }

        debug!("Successfully fetched {} networks", networks.len());
        Ok(networks)
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
        let key = self.network_key(&entity.id);
        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!("Creating network with ID: {}", entity.id);

        let value = self.serialize_network(&entity)?;

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

        debug!("Successfully created network with ID: {}", entity.id);
        Ok(entity)
    }

    async fn get_by_id(&self, id: String) -> Result<NetworkRepoModel, RepositoryError> {
        let key = self.network_key(&id);
        let mut conn = self.client.as_ref().clone();

        debug!("Retrieving network with ID: {}", id);

        let network_data: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_by_id"))?;

        match network_data {
            Some(data) => {
                let network = self.deserialize_network(&data, &id)?;
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
        debug!("Successfully retrieved {} networks", networks.len());
        Ok(networks)
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
        let total_pages = (all_ids.len() + per_page - 1) / per_page;

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
            networks.len(),
            query.page
        );
        Ok(PaginatedResult {
            items: networks,
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

        // Check if network exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "update_network_check_existing"))?;

        if existing.is_none() {
            warn!("Attempted to update network {} that doesn't exist", id);
            return Err(RepositoryError::NotFound(format!(
                "Network with ID {} not found",
                id
            )));
        }

        let value = self.serialize_network(&entity)?;

        conn.set(&key, &value)
            .await
            .map_err(|e| self.map_redis_error(e, "update_network"))?;

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

        // Check if network exists
        let existing: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_network_check_existing"))?;

        if existing.is_none() {
            warn!("Attempted to delete network {} that doesn't exist", id);
            return Err(RepositoryError::NotFound(format!(
                "Network with ID {} not found",
                id
            )));
        }

        // Use Redis pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.del(&key);
        pipe.srem(&network_list_key, &id);

        pipe.query_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_network_pipeline"))?;

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

        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!(
            "Getting network by name: {} (type: {:?})",
            name, network_type
        );

        let all_ids: Vec<String> = conn
            .smembers(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_by_name"))?;

        if all_ids.is_empty() {
            debug!("No networks found for name lookup");
            return Ok(None);
        }

        let networks = self.get_networks_by_ids(&all_ids).await?;

        for network in networks {
            if network.network_type == network_type && network.name == name {
                debug!("Found network by name: {}", name);
                return Ok(Some(network));
            }
        }

        debug!("Network not found by name: {}", name);
        Ok(None)
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

        let network_list_key = self.network_list_key();
        let mut conn = self.client.as_ref().clone();

        debug!(
            "Getting network by chain ID: {} (type: {:?})",
            chain_id, network_type
        );

        let all_ids: Vec<String> = conn
            .smembers(&network_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_network_by_chain_id"))?;

        if all_ids.is_empty() {
            debug!("No networks found for chain ID lookup");
            return Ok(None);
        }

        let networks = self.get_networks_by_ids(&all_ids).await?;

        for network in networks {
            if network.network_type == network_type {
                // For EVM networks, check if the chain_id matches
                if let crate::models::NetworkConfigData::Evm(evm_config) = &network.config {
                    if let Some(network_chain_id) = evm_config.chain_id {
                        if network_chain_id == chain_id {
                            debug!("Found network by chain ID: {}", chain_id);
                            return Ok(Some(network));
                        }
                    }
                }
            }
        }

        debug!("Network not found by chain ID: {}", chain_id);
        Ok(None)
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
        let key_prefix = format!("test_network_{}", Uuid::new_v4());

        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        RedisNetworkRepository::new(Arc::new(connection_manager), key_prefix)
            .expect("Failed to create repository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_network() {
        let repo = setup_test_repo().await;
        let network = create_test_network("test-network", NetworkType::Evm);

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
        let network = create_test_network("test-network", NetworkType::Evm);

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
        let network = create_test_network("test-network", NetworkType::Evm);

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
        let mut network = create_test_network("test-network", NetworkType::Evm);
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
        let mut network = create_test_network("test-network", NetworkType::Evm);
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
        let network1 = create_test_network("network1", NetworkType::Evm);
        let network2 = create_test_network("network2", NetworkType::Solana);

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
        let network1 = create_test_network("network1", NetworkType::Evm);
        let network2 = create_test_network("network2", NetworkType::Solana);

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
        let network1 = create_test_network("network1", NetworkType::Evm);
        let network2 = create_test_network("network2", NetworkType::Solana);
        let network3 = create_test_network("network3", NetworkType::Stellar);

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
        let network = create_test_network("mainnet", NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let retrieved = repo.get_by_name(NetworkType::Evm, "mainnet").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "mainnet");

        let not_found = repo
            .get_by_name(NetworkType::Solana, "mainnet")
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_by_chain_id() {
        let repo = setup_test_repo().await;
        let network = create_test_network("mainnet", NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let retrieved = repo.get_by_chain_id(NetworkType::Evm, 1).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "mainnet");

        let not_found = repo.get_by_chain_id(NetworkType::Evm, 999).await.unwrap();
        assert!(not_found.is_none());

        let solana_result = repo.get_by_chain_id(NetworkType::Solana, 1).await.unwrap();
        assert!(solana_result.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_nonexistent_network() {
        let repo = setup_test_repo().await;
        let network = create_test_network("test-network", NetworkType::Evm);

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
        let network = create_test_network("test-network", NetworkType::Evm);

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
}
