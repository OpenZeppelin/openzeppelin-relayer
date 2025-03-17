//! # Repository Module
//!
//! Implements data persistence layer for the relayer service using Repository pattern.

use crate::domain::RelayerUpdateRequest;
use crate::models::RelayerNetworkPolicy;
use crate::models::{PaginationQuery, RelayerRepoModel, RepositoryError};
use async_trait::async_trait;
use eyre::Result;

mod relayer;
pub use relayer::*;

mod transaction;
use serde::Serialize;
use thiserror::Error;
pub use transaction::*;

mod signer;
pub use signer::*;

mod notification;
pub use notification::*;

mod transaction_counter;
pub use transaction_counter::*;

#[derive(Debug)]
pub struct PaginatedResult<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

#[async_trait]
#[allow(dead_code)]
pub trait Repository<T, ID> {
    async fn create(&self, entity: T) -> Result<T, RepositoryError>;
    async fn get_by_id(&self, id: ID) -> Result<T, RepositoryError>;
    async fn list_all(&self) -> Result<Vec<T>, RepositoryError>;
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<T>, RepositoryError>;
    async fn update(&self, id: ID, entity: T) -> Result<T, RepositoryError>;
    async fn delete_by_id(&self, id: ID) -> Result<(), RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;
}

#[derive(Error, Debug, Serialize)]
pub enum TransactionCounterError {
    #[error("No sequence found for relayer {relayer_id} and address {address}")]
    SequenceNotFound { relayer_id: String, address: String },
    #[error("Counter not found for {0}")]
    NotFound(String),
}

#[allow(dead_code)]
pub trait TransactionCounterTrait {
    fn get(&self, relayer_id: &str, address: &str) -> Result<Option<u64>, TransactionCounterError>;

    fn get_and_increment(
        &self,
        relayer_id: &str,
        address: &str,
    ) -> Result<u64, TransactionCounterError>;

    fn decrement(&self, relayer_id: &str, address: &str) -> Result<u64, TransactionCounterError>;

    fn set(
        &self,
        relayer_id: &str,
        address: &str,
        value: u64,
    ) -> Result<(), TransactionCounterError>;
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct RelayerRepositoryStorage<T: Repository<RelayerRepoModel, String>> {
    pub repository: Box<T>,
}

impl RelayerRepositoryStorage<InMemoryRelayerRepository> {
    pub fn in_memory(repository: InMemoryRelayerRepository) -> Self {
        Self {
            repository: Box::new(repository),
        }
    }
}

#[async_trait]
impl<T> Repository<RelayerRepoModel, String> for RelayerRepositoryStorage<T>
where
    T: Repository<RelayerRepoModel, String> + Send + Sync,
{
    async fn create(&self, entity: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.create(entity).await
    }

    async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.get_by_id(id).await
    }

    async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        self.repository.list_all().await
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
        self.repository.list_paginated(query).await
    }

    async fn update(
        &self,
        id: String,
        entity: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.update(id, entity).await
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        self.repository.delete_by_id(id).await
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        self.repository.count().await
    }
}

#[async_trait]
impl<T> RelayerRepository for RelayerRepositoryStorage<T>
where
    T: RelayerRepository + Send + Sync,
{
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        self.repository.list_active().await
    }

    async fn partial_update(
        &self,
        id: String,
        update: RelayerUpdateRequest,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.partial_update(id, update).await
    }

    async fn enable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.enable_relayer(relayer_id).await
    }

    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.disable_relayer(relayer_id).await
    }

    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        self.repository.update_policy(id, policy).await
    }
}
