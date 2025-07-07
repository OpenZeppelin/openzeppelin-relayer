//! Notification Repository
mod notification_in_memory;
mod notification_redis;

pub use notification_in_memory::*;
pub use notification_redis::*;
use redis::aio::ConnectionManager;

use crate::{
    config::ServerConfig,
    models::{NotificationRepoModel, RepositoryError},
    repositories::{PaginatedResult, PaginationQuery, Repository},
};
use async_trait::async_trait;
use std::sync::Arc;

/// Enum representing the type of notification repository to use
pub enum NotificationRepositoryType {
    InMemory,
    Redis,
}

/// Enum wrapper for different notification repository implementations
#[derive(Debug, Clone)]
pub enum NotificationRepositoryImpl {
    InMemory(InMemoryNotificationRepository),
    Redis(RedisNotificationRepository),
}

impl NotificationRepositoryImpl {
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

impl NotificationRepositoryType {
    /// Creates a notification repository based on the enum variant
    pub async fn create_repository(self, config: &ServerConfig) -> NotificationRepositoryImpl {
        match self {
            NotificationRepositoryType::InMemory => {
                NotificationRepositoryImpl::InMemory(InMemoryNotificationRepository::new())
            }
            NotificationRepositoryType::Redis => {
                let client = redis::Client::open(config.redis_url.clone())
                    .expect("Failed to create Redis client");
                let connection_manager = redis::aio::ConnectionManager::new(client)
                    .await
                    .expect("Failed to create Redis connection manager");
                NotificationRepositoryImpl::Redis(
                    RedisNotificationRepository::new(
                        Arc::new(connection_manager),
                        config.redis_key_prefix.clone(),
                    )
                    .expect("Failed to create Redis notification repository"),
                )
            }
        }
    }
}

#[async_trait]
impl Repository<NotificationRepoModel, String> for NotificationRepositoryImpl {
    async fn create(
        &self,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.create(entity).await,
            NotificationRepositoryImpl::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.get_by_id(id).await,
            NotificationRepositoryImpl::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<NotificationRepoModel>, RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.list_all().await,
            NotificationRepositoryImpl::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<NotificationRepoModel>, RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.list_paginated(query).await,
            NotificationRepositoryImpl::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: NotificationRepoModel,
    ) -> Result<NotificationRepoModel, RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.update(id, entity).await,
            NotificationRepositoryImpl::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.delete_by_id(id).await,
            NotificationRepositoryImpl::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            NotificationRepositoryImpl::InMemory(repo) => repo.count().await,
            NotificationRepositoryImpl::Redis(repo) => repo.count().await,
        }
    }
}
