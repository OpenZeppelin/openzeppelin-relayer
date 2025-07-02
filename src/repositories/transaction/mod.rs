//! Transaction Repository
mod transaction_in_memory;
mod transaction_redis;

pub use transaction_in_memory::*;
pub use transaction_redis::*;

use crate::{
    config::ServerConfig,
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

/// Enum representing the type of transaction repository to use
pub enum TransactionRepositoryType {
    InMemory,
    Redis,
}

/// Enum wrapper for different transaction repository implementations
#[derive(Debug, Clone)]
pub enum TransactionRepositoryImpl {
    InMemory(InMemoryTransactionRepository),
    Redis(RedisTransactionRepository),
}

impl TransactionRepositoryType {
    /// Creates a transaction repository based on the enum variant
    pub async fn create_repository(self, config: &ServerConfig) -> TransactionRepositoryImpl {
        match self {
            TransactionRepositoryType::InMemory => {
                TransactionRepositoryImpl::InMemory(InMemoryTransactionRepository::new())
            }
            TransactionRepositoryType::Redis => {
                let client = redis::Client::open(config.redis_url.clone())
                    .expect("Failed to create Redis client");
                let connection_manager = redis::aio::ConnectionManager::new(client)
                    .await
                    .expect("Failed to create Redis connection manager");
                TransactionRepositoryImpl::Redis(
                    RedisTransactionRepository::new(Arc::new(connection_manager))
                        .expect("Failed to create Redis transaction repository"),
                )
            }
        }
    }
}

// Implement the trait for the enum wrapper
#[async_trait]
impl TransactionRepository for TransactionRepositoryImpl {
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => {
                repo.find_by_relayer_id(relayer_id, query).await
            }
            TransactionRepositoryImpl::Redis(repo) => {
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
            TransactionRepositoryImpl::InMemory(repo) => {
                repo.find_by_status(relayer_id, statuses).await
            }
            TransactionRepositoryImpl::Redis(repo) => {
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
            TransactionRepositoryImpl::InMemory(repo) => {
                repo.find_by_nonce(relayer_id, nonce).await
            }
            TransactionRepositoryImpl::Redis(repo) => repo.find_by_nonce(relayer_id, nonce).await,
        }
    }

    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.update_status(tx_id, status).await,
            TransactionRepositoryImpl::Redis(repo) => repo.update_status(tx_id, status).await,
        }
    }

    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.partial_update(tx_id, update).await,
            TransactionRepositoryImpl::Redis(repo) => repo.partial_update(tx_id, update).await,
        }
    }

    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => {
                repo.update_network_data(tx_id, network_data).await
            }
            TransactionRepositoryImpl::Redis(repo) => {
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
            TransactionRepositoryImpl::InMemory(repo) => repo.set_sent_at(tx_id, sent_at).await,
            TransactionRepositoryImpl::Redis(repo) => repo.set_sent_at(tx_id, sent_at).await,
        }
    }

    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => {
                repo.set_confirmed_at(tx_id, confirmed_at).await
            }
            TransactionRepositoryImpl::Redis(repo) => {
                repo.set_confirmed_at(tx_id, confirmed_at).await
            }
        }
    }
}

// Also implement the base Repository trait
#[async_trait]
impl Repository<TransactionRepoModel, String> for TransactionRepositoryImpl {
    async fn create(
        &self,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.create(entity).await,
            TransactionRepositoryImpl::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.get_by_id(id).await,
            TransactionRepositoryImpl::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.list_all().await,
            TransactionRepositoryImpl::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.list_paginated(query).await,
            TransactionRepositoryImpl::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.update(id, entity).await,
            TransactionRepositoryImpl::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.delete_by_id(id).await,
            TransactionRepositoryImpl::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            TransactionRepositoryImpl::InMemory(repo) => repo.count().await,
            TransactionRepositoryImpl::Redis(repo) => repo.count().await,
        }
    }
}
