//! Transaction Repository
mod transaction_in_memory;
mod transaction_redis;

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

// pub async fn create_transaction_repository(config: &ServerConfig) -> Arc<dyn TransactionRepository + Send + Sync> {
//   match config.transaction_repo_backend.as_str() {
//       "inmemory" => Arc::new(InMemoryTransactionRepository::new()),
//       _ => {
//           // Default to Redis
//           match RedisTransactionRepository::new(&config.redis_url).await {
//               Ok(repo) => Arc::new(repo),
//               Err(e) => {
//                   eprintln!("[WARN] Failed to create RedisTransactionRepository: {e}. Falling back to InMemoryTransactionRepository.");
//                   Arc::new(InMemoryTransactionRepository::new())
//               }
//           }
//       }
//   }
// }
