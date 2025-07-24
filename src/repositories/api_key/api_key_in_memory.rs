//! This module provides an in-memory implementation of api keys.
//!
//! The `InMemoryApiKeyRepository` struct is used to store and retrieve api keys
//! permissions.
use crate::{
    models::{ApiKeyModel, PaginationQuery},
    repositories::{ApiKeyRepositoryTrait, PaginatedResult, RepositoryError},
};

use async_trait::async_trait;

use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryApiKeyRepository {
    store: Mutex<HashMap<String, ApiKeyModel>>,
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
    async fn create(&self, api_key: ApiKeyModel) -> Result<ApiKeyModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.insert(api_key.id.clone(), api_key.clone());
        Ok(api_key)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<ApiKeyModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.get(id).cloned())
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<ApiKeyModel>, RepositoryError> {
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
}

#[cfg(test)]
mod tests {}
