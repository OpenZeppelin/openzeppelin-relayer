//! This module provides an in-memory implementation of api keys.
//!
//! The `InMemoryApiKeyRepository` struct is used to store and retrieve api keys
//! permissions.
use crate::{
    models::{ApiKeyRepoModel, PaginationQuery},
    repositories::{ApiKeyRepositoryTrait, PaginatedResult, RepositoryError},
};

use async_trait::async_trait;

use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryApiKeyRepository {
    store: Mutex<HashMap<String, ApiKeyRepoModel>>,
}

impl Clone for InMemoryApiKeyRepository {
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

impl InMemoryApiKeyRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemoryApiKeyRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApiKeyRepositoryTrait for InMemoryApiKeyRepository {
    async fn create(&self, api_key: ApiKeyRepoModel) -> Result<ApiKeyRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.insert(api_key.id.clone(), api_key.clone());
        Ok(api_key)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.get(id).cloned())
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;

        let items = self
            .store
            .lock()
            .await
            .values()
            .skip(start)
            .take(query.per_page as usize)
            .cloned()
            .collect();

        Ok(PaginatedResult {
            items,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let store = self.store.lock().await;
        Ok(store.len())
    }

    async fn list_permissions(&self, api_key_id: &str) -> Result<Vec<String>, RepositoryError> {
        let store = self.store.lock().await;
        let api_key = store
            .get(api_key_id)
            .ok_or(RepositoryError::NotFound(format!(
                "Api key with id {} not found",
                api_key_id
            )))?;
        Ok(api_key.permissions.clone())
    }

    async fn delete_by_id(&self, api_key_id: &str) -> Result<(), RepositoryError> {
        let mut store = self.store.lock().await;
        store.remove(api_key_id);
        Ok(())
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

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use std::sync::Arc;

    use crate::models::SecretString;

    use super::*;

    #[tokio::test]
    async fn test_in_memory_api_key_repository() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());

        // Test add and get_by_id
        let api_key = ApiKeyRepoModel {
            id: "test-api-key".to_string(),
            value: SecretString::new("test-value"),
            name: "test-name".to_string(),
            allowed_origins: vec!["*".to_string()],
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: Utc::now().to_string(),
        };
        api_key_repository.create(api_key.clone()).await.unwrap();
        assert_eq!(
            api_key_repository.get_by_id("test-api-key").await.unwrap(),
            Some(api_key)
        );
    }

    #[tokio::test]
    async fn test_get_nonexistent_api_key() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());

        let result = api_key_repository.get_by_id("test-api-key").await;
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn test_get_by_id() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());

        let api_key = ApiKeyRepoModel {
            id: "test-api-key".to_string(),
            value: SecretString::new("test-value"),
            name: "test-name".to_string(),
            allowed_origins: vec!["*".to_string()],
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: Utc::now().to_string(),
        };
        api_key_repository.create(api_key.clone()).await.unwrap();
        assert_eq!(
            api_key_repository.get_by_id("test-api-key").await.unwrap(),
            Some(api_key)
        );
    }

    #[tokio::test]
    async fn test_list_paginated_api_keys() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());

        let api_key1 = ApiKeyRepoModel {
            id: "test-api-key1".to_string(),
            value: SecretString::new("test-value1"),
            name: "test-name1".to_string(),
            allowed_origins: vec!["*".to_string()],
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: Utc::now().to_string(),
        };

        let api_key2 = ApiKeyRepoModel {
            id: "test-api-key2".to_string(),
            value: SecretString::new("test-value2"),
            name: "test-name2".to_string(),
            allowed_origins: vec!["*".to_string()],
            permissions: vec!["relayer:all:execute".to_string()],
            created_at: Utc::now().to_string(),
        };

        api_key_repository.create(api_key1.clone()).await.unwrap();
        api_key_repository.create(api_key2.clone()).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };

        let result = api_key_repository.list_paginated(query).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.items.len(), 2);
    }

    #[tokio::test]
    async fn test_has_entries() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());
        assert!(!api_key_repository.has_entries().await.unwrap());
        api_key_repository
            .create(ApiKeyRepoModel {
                id: "test-api-key".to_string(),
                value: SecretString::new("test-value"),
                name: "test-name".to_string(),
                allowed_origins: vec!["*".to_string()],
                permissions: vec!["relayer:all:execute".to_string()],
                created_at: Utc::now().to_string(),
            })
            .await
            .unwrap();

        assert!(api_key_repository.has_entries().await.unwrap());
        api_key_repository.drop_all_entries().await.unwrap();
        assert!(!api_key_repository.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_delete_by_id_api_key() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());
        api_key_repository
            .create(ApiKeyRepoModel {
                id: "test-api-key".to_string(),
                value: SecretString::new("test-value"),
                name: "test-name".to_string(),
                allowed_origins: vec!["*".to_string()],
                permissions: vec!["relayer:all:execute".to_string()],
                created_at: Utc::now().to_string(),
            })
            .await
            .unwrap();

        assert!(api_key_repository.has_entries().await.unwrap());
        api_key_repository
            .delete_by_id("test-api-key")
            .await
            .unwrap();
        assert!(!api_key_repository.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_list_permissions_api_key() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());
        api_key_repository
            .create(ApiKeyRepoModel {
                id: "test-api-key".to_string(),
                value: SecretString::new("test-value"),
                name: "test-name".to_string(),
                allowed_origins: vec!["*".to_string()],
                permissions: vec![
                    "relayer:all:execute".to_string(),
                    "relayer:all:read".to_string(),
                ],
                created_at: Utc::now().to_string(),
            })
            .await
            .unwrap();

        let permissions = api_key_repository
            .list_permissions("test-api-key")
            .await
            .unwrap();
        assert_eq!(permissions, vec!["relayer:all:execute", "relayer:all:read"]);
    }

    #[tokio::test]
    async fn test_drop_all_entries() {
        let api_key_repository = Arc::new(InMemoryApiKeyRepository::new());
        api_key_repository
            .create(ApiKeyRepoModel {
                id: "test-api-key".to_string(),
                value: SecretString::new("test-value"),
                name: "test-name".to_string(),
                allowed_origins: vec!["*".to_string()],
                permissions: vec!["relayer:all:execute".to_string()],
                created_at: Utc::now().to_string(),
            })
            .await
            .unwrap();

        assert!(api_key_repository.has_entries().await.unwrap());
        api_key_repository.drop_all_entries().await.unwrap();
        assert!(!api_key_repository.has_entries().await.unwrap());
    }
}
