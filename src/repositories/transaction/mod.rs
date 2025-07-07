//! Transaction Repository Module
//!
//! This module provides the transaction repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract transaction data persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete transactions
//! - **Specialized Queries**: Find transactions by relayer ID, status, and nonce
//! - **Pagination Support**: Efficient paginated listing of transactions
//! - **Status Management**: Update transaction status and timestamps
//! - **Partial Updates**: Support for partial transaction updates
//! - **Network Data**: Manage transaction network-specific data
//!
//! ## Repository Implementations
//!
//! - [`InMemoryTransactionRepository`]: Fast in-memory storage for testing/development
//! - [`RedisTransactionRepository`]: Redis-backed storage for production environments
//!
mod transaction_in_memory;
mod transaction_redis;

use redis::aio::ConnectionManager;
pub use transaction_in_memory::*;
pub use transaction_redis::*;

use crate::{
    models::{
        NetworkTransactionData, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

/// A trait defining transaction repository operations
#[async_trait]
pub trait TransactionRepository: Repository<TransactionRepoModel, String> {
    /// Find transactions by relayer ID with pagination
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError>;

    /// Find transactions by relayer ID and status(es)
    async fn find_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError>;

    /// Find a transaction by relayer ID and nonce
    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError>;

    /// Update the status of a transaction
    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Partially update a transaction
    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Update the network data of a transaction
    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Set the sent_at timestamp of a transaction
    async fn set_sent_at(
        &self,
        tx_id: String,
        sent_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Set the confirmed_at timestamp of a transaction
    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError>;
}

#[cfg(test)]
mockall::mock! {
  pub TransactionRepository {}

  #[async_trait]
  impl Repository<TransactionRepoModel, String> for TransactionRepository {
      async fn create(&self, entity: TransactionRepoModel) -> Result<TransactionRepoModel, RepositoryError>;
      async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError>;
      async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError>;
      async fn list_paginated(&self, query: PaginationQuery) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError>;
      async fn update(&self, id: String, entity: TransactionRepoModel) -> Result<TransactionRepoModel, RepositoryError>;
      async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError>;
      async fn count(&self) -> Result<usize, RepositoryError>;
  }

  #[async_trait]
  impl TransactionRepository for TransactionRepository {
      async fn find_by_relayer_id(&self, relayer_id: &str, query: PaginationQuery) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError>;
      async fn find_by_status(&self, relayer_id: &str, statuses: &[TransactionStatus]) -> Result<Vec<TransactionRepoModel>, RepositoryError>;
      async fn find_by_nonce(&self, relayer_id: &str, nonce: u64) -> Result<Option<TransactionRepoModel>, RepositoryError>;
      async fn update_status(&self, tx_id: String, status: TransactionStatus) -> Result<TransactionRepoModel, RepositoryError>;
      async fn partial_update(&self, tx_id: String, update: TransactionUpdateRequest) -> Result<TransactionRepoModel, RepositoryError>;
      async fn update_network_data(&self, tx_id: String, network_data: NetworkTransactionData) -> Result<TransactionRepoModel, RepositoryError>;
      async fn set_sent_at(&self, tx_id: String, sent_at: String) -> Result<TransactionRepoModel, RepositoryError>;
      async fn set_confirmed_at(&self, tx_id: String, confirmed_at: String) -> Result<TransactionRepoModel, RepositoryError>;

  }
}

/// Enum wrapper for different transaction repository implementations
#[derive(Debug, Clone)]
pub enum TransactionRepositoryStorage {
    InMemory(InMemoryTransactionRepository),
    Redis(RedisTransactionRepository),
}

impl TransactionRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryTransactionRepository::new())
    }
    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisTransactionRepository::new(
            connection_manager,
            key_prefix,
        )?))
    }
}

#[async_trait]
impl TransactionRepository for TransactionRepositoryStorage {
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.find_by_relayer_id(relayer_id, query).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.find_by_relayer_id(relayer_id, query).await
            }
        }
    }

    async fn find_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.find_by_status(relayer_id, statuses).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.find_by_status(relayer_id, statuses).await
            }
        }
    }

    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.find_by_nonce(relayer_id, nonce).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.find_by_nonce(relayer_id, nonce).await
            }
        }
    }

    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.update_status(tx_id, status).await,
            TransactionRepositoryStorage::Redis(repo) => repo.update_status(tx_id, status).await,
        }
    }

    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.partial_update(tx_id, update).await
            }
            TransactionRepositoryStorage::Redis(repo) => repo.partial_update(tx_id, update).await,
        }
    }

    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.update_network_data(tx_id, network_data).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.update_network_data(tx_id, network_data).await
            }
        }
    }

    async fn set_sent_at(
        &self,
        tx_id: String,
        sent_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.set_sent_at(tx_id, sent_at).await,
            TransactionRepositoryStorage::Redis(repo) => repo.set_sent_at(tx_id, sent_at).await,
        }
    }

    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.set_confirmed_at(tx_id, confirmed_at).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.set_confirmed_at(tx_id, confirmed_at).await
            }
        }
    }
}

#[async_trait]
impl Repository<TransactionRepoModel, String> for TransactionRepositoryStorage {
    async fn create(
        &self,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            TransactionRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            TransactionRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.list_all().await,
            TransactionRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            TransactionRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            TransactionRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            TransactionRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.count().await,
            TransactionRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }
}
