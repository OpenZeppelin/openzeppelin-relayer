//! Notification Repository Module
//!
//! This module provides the notification repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract notification data persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete notification configurations
//! - **Webhook Support**: Store webhook notification configurations
//! - **Pagination Support**: Efficient paginated listing of notifications
//! - **Configuration Management**: Handle notification signing keys and URLs
//!
//! ## Repository Implementations
//!
//! - [`InMemoryNotificationRepository`]: Fast in-memory storage for testing/development
//! - [`RedisNotificationRepository`]: Redis-backed storage for production environments
//!
mod notification_in_memory;
mod notification_redis;

pub use notification_in_memory::*;
pub use notification_redis::*;
use redis::aio::ConnectionManager;

use crate::{
    models::{NotificationRepoModel, RepositoryError},
    repositories::{PaginatedResult, PaginationQuery, Repository},
};
use async_trait::async_trait;
use std::sync::Arc;

/// Enum wrapper for different notification repository implementations
#[derive(Debug, Clone)]
pub enum NotificationRepositoryStorage {
    InMemory(InMemoryNotificationRepository),
    Redis(RedisNotificationRepository),
}

impl NotificationRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryNotificationRepository::new())
    }
    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisNotificationRepository::new(
            connection_manager,
            key_prefix,
        )?))
    }
}

#[async_trait]
impl Repository<NotificationRepoModel, String> for NotificationRepositoryStorage {
    async fn create(
        &self,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            NotificationRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            NotificationRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<NotificationRepoModel>, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.list_all().await,
            NotificationRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NotificationRepoModel>, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            NotificationRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            NotificationRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            NotificationRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            NotificationRepositoryStorage::InMemory(repo) => repo.count().await,
            NotificationRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }
}
