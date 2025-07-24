//! API Key Repository Module
//!
//! This module provides the api key repository for the OpenZeppelin Relayer service.
//! It implements a specialized repository pattern for managing api key configurations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Repository Implementations
//!
//! - [`InMemoryApiKeyRepository`]: In-memory storage for testing/development
//! - [`RedisApiKeyRepository`]: Redis-backed storage for production environments
//!
//! ## API Keys
//!
//! The api key system allows extending relayer authorization scheme through api keys.
//! Each api key is identified by a unique ID and contains a list of permissions that
//! restrict the api key's access to the server.
//!
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;

pub mod api_key_in_memory;
pub mod api_key_redis;

pub use api_key_in_memory::*;
pub use api_key_redis::*;

#[cfg(test)]
use mockall::automock;

use crate::{
    models::{ApiKeyModel, PaginationQuery, RepositoryError},
    repositories::PaginatedResult,
};

#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait ApiKeyRepositoryTrait {
    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyModel>, RepositoryError>;
    async fn create(&self, api_key: ApiKeyModel) -> Result<ApiKeyModel, RepositoryError>;
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyModel>, RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;
    async fn list_permissions(&self, api_key_id: &str) -> Result<Vec<String>, RepositoryError>;
    async fn delete_by_id(&self, api_key_id: &str) -> Result<(), RepositoryError>;
}

/// Enum wrapper for different plugin repository implementations
#[derive(Debug, Clone)]
pub enum ApiKeyRepositoryStorage {
    InMemory(InMemoryApiKeyRepository),
    Redis(RedisApiKeyRepository),
}

impl ApiKeyRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryApiKeyRepository::new())
    }

    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        let redis_repo = RedisApiKeyRepository::new(connection_manager, key_prefix)?;
        Ok(Self::Redis(redis_repo))
    }
}

#[async_trait]
impl ApiKeyRepositoryTrait for ApiKeyRepositoryStorage {
    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyModel>, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn create(&self, api_key: ApiKeyModel) -> Result<ApiKeyModel, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.create(api_key).await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.create(api_key).await,
        }
    }

    async fn list_permissions(&self, api_key_id: &str) -> Result<Vec<String>, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.list_permissions(api_key_id).await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.list_permissions(api_key_id).await,
        }
    }

    async fn delete_by_id(&self, api_key_id: &str) -> Result<(), RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.delete_by_id(api_key_id).await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.delete_by_id(api_key_id).await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyModel>, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.count().await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }
}

impl PartialEq for ApiKeyModel {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.allowed_origins == other.allowed_origins
            && self.permissions == other.permissions
    }
}

#[cfg(test)]
mod tests {}
