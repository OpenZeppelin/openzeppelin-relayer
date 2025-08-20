//! Network Repository Module
//!
//! This module provides the network repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract network configuration persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete network configurations
//! - **Multi-Chain Support**: Handle EVM, Solana, and Stellar network configurations
//! - **Specialized Queries**: Find networks by name and chain ID
//! - **Pagination Support**: Efficient paginated listing of networks
//! - **Type Safety**: Strongly typed network configurations per blockchain type
//!
//! ## Repository Implementations
//!
//! - [`InMemoryNetworkRepository`]: Fast in-memory storage for testing/development
//! - [`RedisNetworkRepository`]: Redis-backed storage for production environments
//!

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;

mod network_in_memory;
mod network_redis;

pub use network_in_memory::InMemoryNetworkRepository;
pub use network_redis::RedisNetworkRepository;

use crate::models::{NetworkRepoModel, NetworkType, RepositoryError};
use crate::repositories::{PaginatedResult, PaginationQuery, Repository};

#[async_trait]
pub trait NetworkRepository: Repository<NetworkRepoModel, String> {
    /// Get a network by network type and name
    async fn get_by_name(
        &self,
        network_type: NetworkType,
        name: &str,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError>;

    /// Get a network by network type and chain ID
    async fn get_by_chain_id(
        &self,
        network_type: NetworkType,
        chain_id: u64,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError>;
}

#[derive(Debug, Clone)]
pub enum NetworkRepositoryStorage {
    InMemory(InMemoryNetworkRepository),
    Redis(RedisNetworkRepository),
}

impl NetworkRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryNetworkRepository::new())
    }

    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        let redis_repo = RedisNetworkRepository::new(connection_manager, key_prefix)?;
        Ok(Self::Redis(redis_repo))
    }
}

#[async_trait]
impl Repository<NetworkRepoModel, String> for NetworkRepositoryStorage {
    async fn create(&self, entity: NetworkRepoModel) -> Result<NetworkRepoModel, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            NetworkRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<NetworkRepoModel, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            NetworkRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<NetworkRepoModel>, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.list_all().await,
            NetworkRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NetworkRepoModel>, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            NetworkRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: NetworkRepoModel,
    ) -> Result<NetworkRepoModel, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            NetworkRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            NetworkRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.count().await,
            NetworkRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            NetworkRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            NetworkRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }
}

#[async_trait]
impl NetworkRepository for NetworkRepositoryStorage {
    async fn get_by_name(
        &self,
        network_type: NetworkType,
        name: &str,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => repo.get_by_name(network_type, name).await,
            NetworkRepositoryStorage::Redis(repo) => repo.get_by_name(network_type, name).await,
        }
    }

    async fn get_by_chain_id(
        &self,
        network_type: NetworkType,
        chain_id: u64,
    ) -> Result<Option<NetworkRepoModel>, RepositoryError> {
        match self {
            NetworkRepositoryStorage::InMemory(repo) => {
                repo.get_by_chain_id(network_type, chain_id).await
            }
            NetworkRepositoryStorage::Redis(repo) => {
                repo.get_by_chain_id(network_type, chain_id).await
            }
        }
    }
}

#[cfg(test)]
mockall::mock! {
    pub NetworkRepository {}

    #[async_trait]
    impl Repository<NetworkRepoModel, String> for NetworkRepository {
        async fn create(&self, entity: NetworkRepoModel) -> Result<NetworkRepoModel, RepositoryError>;
        async fn get_by_id(&self, id: String) -> Result<NetworkRepoModel, RepositoryError>;
        async fn list_all(&self) -> Result<Vec<NetworkRepoModel>, RepositoryError>;
        async fn list_paginated(&self, query: PaginationQuery) -> Result<PaginatedResult<NetworkRepoModel>, RepositoryError>;
        async fn update(&self, id: String, entity: NetworkRepoModel) -> Result<NetworkRepoModel, RepositoryError>;
        async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError>;
        async fn count(&self) -> Result<usize, RepositoryError>;
        async fn has_entries(&self) -> Result<bool, RepositoryError>;
        async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
    }

    #[async_trait]
    impl NetworkRepository for NetworkRepository {
        async fn get_by_name(&self, network_type: NetworkType, name: &str) -> Result<Option<NetworkRepoModel>, RepositoryError>;
        async fn get_by_chain_id(&self, network_type: NetworkType, chain_id: u64) -> Result<Option<NetworkRepoModel>, RepositoryError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_network;
    #[tokio::test]
    async fn test_trait_methods_accessibility() {
        // Create in-memory repository through the storage enum
        let repo: NetworkRepositoryStorage = NetworkRepositoryStorage::new_in_memory();

        // These methods are now accessible through the trait!
        assert!(!repo.has_entries().await.unwrap());

        // Add a network
        let network = create_mock_network();
        repo.create(network).await.unwrap();

        // Check entries exist
        assert!(repo.has_entries().await.unwrap());

        // Drop all entries
        repo.drop_all_entries().await.unwrap();

        // Verify everything is cleaned up
        assert!(!repo.has_entries().await.unwrap());
        assert_eq!(repo.count().await.unwrap(), 0);
    }
}
