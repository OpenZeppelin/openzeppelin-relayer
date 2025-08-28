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
    models::{ApiKeyRepoModel, PaginationQuery, RepositoryError},
    repositories::PaginatedResult,
};

#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait ApiKeyRepositoryTrait {
    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyRepoModel>, RepositoryError>;
    async fn create(&self, api_key: ApiKeyRepoModel) -> Result<ApiKeyRepoModel, RepositoryError>;
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyRepoModel>, RepositoryError>;
    async fn count(&self) -> Result<usize, RepositoryError>;
    async fn list_permissions(&self, api_key_id: &str) -> Result<Vec<String>, RepositoryError>;
    async fn delete_by_id(&self, api_key_id: &str) -> Result<(), RepositoryError>;
    async fn has_entries(&self) -> Result<bool, RepositoryError>;
    async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
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
    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyRepoModel>, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn create(&self, api_key: ApiKeyRepoModel) -> Result<ApiKeyRepoModel, RepositoryError> {
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
    ) -> Result<PaginatedResult<ApiKeyRepoModel>, RepositoryError> {
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

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            ApiKeyRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            ApiKeyRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }
}

impl PartialEq for ApiKeyRepoModel {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.allowed_origins == other.allowed_origins
            && self.permissions == other.permissions
    }
}

#[cfg(test)]
mod tests {
    use crate::models::SecretString;

    use super::*;

    use chrono::Utc;

    // Helper function to create a test api key
    fn create_test_api_key(
        id: &str,
        name: &str,
        value: &str,
        allowed_origins: &[&str],
        permissions: &[&str],
    ) -> ApiKeyRepoModel {
        ApiKeyRepoModel {
            id: id.to_string(),
            name: name.to_string(),
            value: SecretString::new(value),
            allowed_origins: allowed_origins.iter().map(|s| s.to_string()).collect(),
            permissions: permissions.iter().map(|s| s.to_string()).collect(),
            created_at: Utc::now().to_string(),
        }
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_get_by_id_existing() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();
        let api_key = create_test_api_key(
            "test-api-key",
            "test-name",
            "test-value",
            &["*"],
            &["relayer:all:execute"],
        );

        // Add the api key first
        storage.create(api_key.clone()).await.unwrap();

        // Get the api key
        let result = storage.get_by_id("test-api-key").await.unwrap();
        assert_eq!(result, Some(api_key));
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_get_by_id_non_existing() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        let result = storage.get_by_id("non-existent").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_add_success() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();
        let api_key = create_test_api_key(
            "test-api-key",
            "test-name",
            "test-value",
            &["*"],
            &["relayer:all:execute"],
        );

        let result = storage.create(api_key).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_add_duplicate() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();
        let api_key = create_test_api_key(
            "test-api-key",
            "test-name",
            "test-value",
            &["*"],
            &["relayer:all:execute"],
        );

        // Add the api key first time
        storage.create(api_key.clone()).await.unwrap();

        // Try to add the same api key again - should succeed (overwrite)
        let result = storage.create(api_key).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_count_empty() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_count_with_api_keys() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        // Add multiple plugins
        storage
            .create(create_test_api_key(
                "api-key1",
                "test-name1",
                "test-value1",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();
        storage
            .create(create_test_api_key(
                "api-key2",
                "test-name2",
                "test-value2",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();
        storage
            .create(create_test_api_key(
                "api-key3",
                "test-name3",
                "test-value3",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();

        let count = storage.count().await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_has_entries_empty() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        let has_entries = storage.has_entries().await.unwrap();
        assert!(!has_entries);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_has_entries_with_api_keys() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        storage
            .create(create_test_api_key(
                "api-key1",
                "test-name1",
                "test-value1",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();

        let has_entries = storage.has_entries().await.unwrap();
        assert!(has_entries);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_drop_all_entries_empty() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        let result = storage.drop_all_entries().await;
        assert!(result.is_ok());

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_drop_all_entries_with_api_keys() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        // Add multiple plugins
        storage
            .create(create_test_api_key(
                "api-key1",
                "test-name1",
                "test-value1",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();
        storage
            .create(create_test_api_key(
                "api-key2",
                "test-name2",
                "test-value2",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();

        let result = storage.drop_all_entries().await;
        assert!(result.is_ok());

        let count = storage.count().await.unwrap();
        assert_eq!(count, 0);

        let has_entries = storage.has_entries().await.unwrap();
        assert!(!has_entries);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_list_paginated_empty() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 0);
        assert_eq!(result.total, 0);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 10);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_list_paginated_with_api_keys() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        // Add multiple plugins
        storage
            .create(create_test_api_key(
                "api-key1",
                "test-name1",
                "test-value1",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();
        storage
            .create(create_test_api_key(
                "api-key2",
                "test-name2",
                "test-value2",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();
        storage
            .create(create_test_api_key(
                "api-key3",
                "test-name3",
                "test-value3",
                &["*"],
                &["relayer:all:execute"],
            ))
            .await
            .unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };
        let result = storage.list_paginated(query).await.unwrap();

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total, 3);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 2);
    }

    #[tokio::test]
    async fn test_api_key_repository_storage_workflow() {
        let storage = ApiKeyRepositoryStorage::new_in_memory();

        // Initially empty
        assert!(!storage.has_entries().await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 0);

        // Add plugins
        let api_key1 = create_test_api_key(
            "api-key1",
            "test-name1",
            "test-value1",
            &["*"],
            &["relayer:all:execute"],
        );
        let api_key2 = create_test_api_key(
            "api-key2",
            "test-name2",
            "test-value2",
            &["*"],
            &["relayer:all:execute"],
        );

        storage.create(api_key1.clone()).await.unwrap();
        storage.create(api_key2.clone()).await.unwrap();

        // Check state
        assert!(storage.has_entries().await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 2);

        // Retrieve specific plugin
        let retrieved = storage.get_by_id("api-key1").await.unwrap();
        assert_eq!(retrieved, Some(api_key1));

        // List all plugins
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = storage.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total, 2);

        // Clear all plugins
        storage.drop_all_entries().await.unwrap();
        assert!(!storage.has_entries().await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 0);
    }
}
