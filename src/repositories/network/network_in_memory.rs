//! This module defines an in-memory network repository for managing
//! network configurations. It provides functionality to create and retrieve
//! network configurations, while update and delete operations are not supported.
//! The repository is implemented using a `Mutex`-protected `HashMap` to
//! ensure thread safety in asynchronous contexts.

use crate::{
    models::{NetworkRepoModel, NetworkType, RepositoryError},
    repositories::{NetworkRepository, PaginatedResult, PaginationQuery, Repository},
};
use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryNetworkRepository {
    store: Mutex<HashMap<String, NetworkRepoModel>>,
}

impl Clone for InMemoryNetworkRepository {
    fn clone(&self) -> Self {
        // Try to get the current data, or use empty HashMap if lock fails
        let data = self
            .store
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| HashMap::new());

        Self {
            store: Mutex::new(data),
        }
    }
}

impl InMemoryNetworkRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }

    /// Gets a network by network type and name
    pub async fn get(
        &self,
        network_type: NetworkType,
        name: &str,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        for (_, network) in store.iter() {
            if network.network_type == network_type && network.name == name {
                return Ok(Some(network.clone()));
            }
        }
        Ok(None)
    }
}

impl Default for InMemoryNetworkRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repository<NetworkRepoModel, String> for InMemoryNetworkRepository {
    async fn create(&self, network: NetworkRepoModel) -> Result<NetworkRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&network.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Network with ID {} already exists",
                network.id
            )));
        }
        store.insert(network.id.clone(), network.clone());
        Ok(network)
    }

    async fn get_by_id(&self, id: String) -> Result<NetworkRepoModel, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        match store.get(&id) {
            Some(network) => Ok(network.clone()),
            None => Err(RepositoryError::NotFound(format!(
                "Network with ID {} not found",
                id
            ))),
        }
    }

    async fn update(
        &self,
        _id: String,
        _network: NetworkRepoModel,
    ) -> Result<NetworkRepoModel, RepositoryError> {
        Err(RepositoryError::NotSupported("Not supported".to_string()))
    }

    async fn delete_by_id(&self, _id: String) -> Result<(), RepositoryError> {
        Err(RepositoryError::NotSupported("Not supported".to_string()))
    }

    async fn list_all(&self) -> Result<Vec<NetworkRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let networks: Vec<NetworkRepoModel> = store.values().cloned().collect();
        Ok(networks)
    }

    async fn list_paginated(
        &self,
        _query: PaginationQuery,
    ) -> Result<PaginatedResult<NetworkRepoModel>, RepositoryError> {
        Err(RepositoryError::NotSupported("Not supported".to_string()))
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.len())
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(!store.is_empty())
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.clear();
        Ok(())
    }
}

#[async_trait]
impl NetworkRepository for InMemoryNetworkRepository {
    async fn get_by_name(
        &self,
        network_type: NetworkType,
        name: &str,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError> {
        self.get(network_type, name).await
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

        let store = Self::acquire_lock(&self.store).await?;
        for (_, network) in store.iter() {
            if network.network_type == network_type {
                if let crate::models::NetworkConfigData::Evm(evm_config) = &network.config {
                    if evm_config.chain_id == Some(chain_id) {
                        return Ok(Some(network.clone()));
                    }
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{
        EvmNetworkConfig, NetworkConfigCommon, SolanaNetworkConfig, StellarNetworkConfig,
    };

    use super::*;

    fn create_test_network(name: String, network_type: NetworkType) -> NetworkRepoModel {
        let common = NetworkConfigCommon {
            network: name.clone(),
            from: None,
            rpc_urls: Some(vec!["https://rpc.example.com".to_string()]),
            explorer_urls: None,
            average_blocktime_ms: None,
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

    #[tokio::test]
    async fn test_new_repository_is_empty() {
        let repo = InMemoryNetworkRepository::new();
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_create_network() {
        let repo = InMemoryNetworkRepository::new();
        let network = create_test_network("mainnet".to_string(), NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        let stored = repo.get_by_id(network.id.clone()).await.unwrap();
        assert_eq!(stored.id, network.id);
        assert_eq!(stored.name, network.name);
    }

    #[tokio::test]
    async fn test_get_network_by_type_and_name() {
        let repo = InMemoryNetworkRepository::new();
        let network = create_test_network("mainnet".to_string(), NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();

        let retrieved = repo.get(NetworkType::Evm, "mainnet").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "mainnet");
    }

    #[tokio::test]
    async fn test_get_nonexistent_network() {
        let repo = InMemoryNetworkRepository::new();

        let result = repo.get(NetworkType::Evm, "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_create_duplicate_network() {
        let repo = InMemoryNetworkRepository::new();
        let network = create_test_network("mainnet".to_string(), NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();
        let result = repo.create(network).await;

        assert!(matches!(
            result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
    }

    #[tokio::test]
    async fn test_different_network_types_same_name() {
        let repo = InMemoryNetworkRepository::new();
        let evm_network = create_test_network("mainnet".to_string(), NetworkType::Evm);
        let solana_network = create_test_network("mainnet".to_string(), NetworkType::Solana);

        repo.create(evm_network.clone()).await.unwrap();
        repo.create(solana_network.clone()).await.unwrap();

        assert_eq!(repo.count().await.unwrap(), 2);

        let evm_retrieved = repo.get(NetworkType::Evm, "mainnet").await.unwrap();
        let solana_retrieved = repo.get(NetworkType::Solana, "mainnet").await.unwrap();

        assert!(evm_retrieved.is_some());
        assert!(solana_retrieved.is_some());
        assert_eq!(evm_retrieved.unwrap().network_type, NetworkType::Evm);
        assert_eq!(solana_retrieved.unwrap().network_type, NetworkType::Solana);
    }

    #[tokio::test]
    async fn test_unsupported_operations() {
        let repo = InMemoryNetworkRepository::new();
        let network = create_test_network("test".to_string(), NetworkType::Evm);

        let update_result = repo.update("test".to_string(), network.clone()).await;
        assert!(matches!(
            update_result,
            Err(RepositoryError::NotSupported(_))
        ));

        let delete_result = repo.delete_by_id("test".to_string()).await;
        assert!(matches!(
            delete_result,
            Err(RepositoryError::NotSupported(_))
        ));

        let pagination_result = repo
            .list_paginated(PaginationQuery {
                page: 1,
                per_page: 10,
            })
            .await;
        assert!(matches!(
            pagination_result,
            Err(RepositoryError::NotSupported(_))
        ));
    }

    #[tokio::test]
    async fn test_has_entries() {
        let repo = InMemoryNetworkRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let network = create_test_network("test".to_string(), NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_drop_all_entries() {
        let repo = InMemoryNetworkRepository::new();
        let network = create_test_network("test".to_string(), NetworkType::Evm);

        repo.create(network.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}
