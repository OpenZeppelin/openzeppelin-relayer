//! This module defines an in-memory repository for managing signer models.
//! It provides asynchronous CRUD operations and supports pagination.
//! The repository is thread-safe, using a `Mutex` to protect access to the underlying data store.
use crate::{
    models::{RepositoryError, SignerRepoModel},
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemorySignerRepository {
    store: Mutex<HashMap<String, SignerRepoModel>>,
}

impl Clone for InMemorySignerRepository {
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

#[allow(dead_code)]
impl InMemorySignerRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemorySignerRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repository<SignerRepoModel, String> for InMemorySignerRepository {
    async fn create(&self, signer: SignerRepoModel) -> Result<SignerRepoModel, RepositoryError> {
        let mut store: MutexGuard<'_, HashMap<String, SignerRepoModel>> =
            Self::acquire_lock(&self.store).await?;
        if store.contains_key(&signer.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Signer with ID {} already exists",
                signer.id
            )));
        }
        store.insert(signer.id.clone(), signer.clone());
        Ok(signer)
    }

    async fn get_by_id(&self, id: String) -> Result<SignerRepoModel, RepositoryError> {
        let store: MutexGuard<'_, HashMap<String, SignerRepoModel>> =
            Self::acquire_lock(&self.store).await?;
        match store.get(&id) {
            Some(signer) => Ok(signer.clone()),
            None => Err(RepositoryError::NotFound(format!(
                "Signer with ID {} not found",
                id
            ))),
        }
    }

    #[allow(clippy::map_entry)]
    async fn update(
        &self,
        id: String,
        signer: SignerRepoModel,
    ) -> Result<SignerRepoModel, RepositoryError> {
        let mut store: MutexGuard<'_, HashMap<String, SignerRepoModel>> =
            Self::acquire_lock(&self.store).await?;
        if !store.contains_key(&id) {
            return Err(RepositoryError::NotFound(format!(
                "Signer with ID {} not found",
                id
            )));
        }
        store.insert(id, signer.clone());
        Ok(signer)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let mut store: MutexGuard<'_, HashMap<String, SignerRepoModel>> =
            Self::acquire_lock(&self.store).await?;
        if !store.contains_key(&id) {
            return Err(RepositoryError::NotFound(format!(
                "Signer with ID {} not found",
                id
            )));
        }
        store.remove(&id);
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<SignerRepoModel>, RepositoryError> {
        let store: MutexGuard<'_, HashMap<String, SignerRepoModel>> =
            Self::acquire_lock(&self.store).await?;
        let signers: Vec<SignerRepoModel> = store.values().cloned().collect();
        Ok(signers)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<SignerRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;
        let items: Vec<SignerRepoModel> = self
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
        let store: MutexGuard<'_, HashMap<String, SignerRepoModel>> =
            Self::acquire_lock(&self.store).await?;
        let length = store.len();
        Ok(length)
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
    use secrets::SecretVec;

    use crate::models::{LocalSignerConfigStorage, SignerConfigStorage};

    use super::*;

    fn create_test_signer(id: String) -> SignerRepoModel {
        SignerRepoModel {
            id: id.clone(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::zero(0),
            }),
        }
    }

    #[actix_web::test]
    async fn test_new_repository_is_empty() {
        let repo = InMemorySignerRepository::new();
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[actix_web::test]
    async fn test_add_signer() {
        let repo = InMemorySignerRepository::new();
        let signer = create_test_signer("test".to_string());

        repo.create(signer.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        let stored = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(stored.id, signer.id);
    }

    #[actix_web::test]
    async fn test_update_signer() {
        let repo = InMemorySignerRepository::new();
        let signer = create_test_signer("test".to_string());

        // Create the signer first
        repo.create(signer.clone()).await.unwrap();

        // Update the signer
        let updated_signer = SignerRepoModel {
            id: "test".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[2; 32])),
            }),
        };

        let result = repo.update("test".to_string(), updated_signer).await;
        assert!(result.is_ok());
    }

    #[actix_web::test]
    async fn test_list_signers() {
        let repo = InMemorySignerRepository::new();
        let signer1 = create_test_signer("test".to_string());
        let signer2 = create_test_signer("test2".to_string());

        repo.create(signer1.clone()).await.unwrap();
        repo.create(signer2).await.unwrap();

        let signers = repo.list_all().await.unwrap();
        assert_eq!(signers.len(), 2);
    }

    #[actix_web::test]
    async fn test_update_nonexistent_signer() {
        let repo = InMemorySignerRepository::new();
        let signer = create_test_signer("test".to_string());

        let result = repo.update("test2".to_string(), signer).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_get_nonexistent_relayer() {
        let repo = InMemorySignerRepository::new();

        let result = repo.get_by_id("test".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_has_entries() {
        let repo = InMemorySignerRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let signer = create_test_signer("test".to_string());

        repo.create(signer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[actix_web::test]
    async fn test_drop_all_entries() {
        let repo = InMemorySignerRepository::new();
        let signer = create_test_signer("test".to_string());
        repo.create(signer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}
