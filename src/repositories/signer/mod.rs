//! Signer Repository Module
//!
//! This module provides the signer repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract signer data persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete signer configurations
//! - **Multi-Provider Support**: Handle various signer types (Local, AWS KMS, Google Cloud KMS, etc.)
//! - **Secure Storage**: Proper handling of cryptographic keys and secrets
//! - **Pagination Support**: Efficient paginated listing of signers
//! - **Configuration Management**: Convert between file configs and repository models
//!
//! ## Repository Implementations
//!
//! - [`InMemorySignerRepository`]: Fast in-memory storage for testing/development
//! - [`RedisSignerRepository`]: Redis-backed storage for production environments
//!
//! ## Supported Signer Types
//!
//! - **Local Signers**: Direct private key management
//! - **AWS KMS**: AWS Key Management Service integration
//! - **Google Cloud KMS**: Google Cloud Key Management Service integration
//! - **Turnkey**: Turnkey service integration
//! - **Vault**: HashiCorp Vault integration

mod signer_in_memory;
mod signer_redis;

pub use signer_in_memory::*;
pub use signer_redis::*;

use crate::{
    models::{RepositoryError, SignerRepoModel},
    repositories::{PaginatedResult, PaginationQuery, Repository},
};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;

/// Enum wrapper for different signer repository implementations
#[derive(Debug, Clone)]
pub enum SignerRepositoryStorage {
    InMemory(InMemorySignerRepository),
    Redis(RedisSignerRepository),
}

impl SignerRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemorySignerRepository::new())
    }

    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        let redis_repo = RedisSignerRepository::new(connection_manager, key_prefix)?;
        Ok(Self::Redis(redis_repo))
    }
}

#[async_trait]
impl Repository<SignerRepoModel, String> for SignerRepositoryStorage {
    async fn create(&self, entity: SignerRepoModel) -> Result<SignerRepoModel, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            SignerRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<SignerRepoModel, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            SignerRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<SignerRepoModel>, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.list_all().await,
            SignerRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<SignerRepoModel>, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            SignerRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: SignerRepoModel,
    ) -> Result<SignerRepoModel, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            SignerRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            SignerRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.count().await,
            SignerRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            SignerRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            SignerRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            SignerRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalSignerConfigStorage, SignerConfigStorage};
    use secrets::SecretVec;

    fn create_local_signer(id: String) -> SignerRepoModel {
        SignerRepoModel {
            id: id.clone(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        }
    }

    #[actix_web::test]
    async fn test_in_memory_impl_creation() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();
        match impl_repo {
            SignerRepositoryStorage::InMemory(_) => (),
            _ => panic!("Expected InMemory variant"),
        }
    }

    #[actix_web::test]
    async fn test_in_memory_impl_operations() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();
        let signer = create_local_signer("test-signer".to_string());

        // Test create
        let created = impl_repo.create(signer.clone()).await.unwrap();
        assert_eq!(created.id, signer.id);

        // Test get
        let retrieved = impl_repo
            .get_by_id("test-signer".to_string())
            .await
            .unwrap();
        assert_eq!(retrieved.id, signer.id);

        // Test count
        let count = impl_repo.count().await.unwrap();
        assert!(count >= 1);

        // Test list_all
        let all_signers = impl_repo.list_all().await.unwrap();
        assert!(!all_signers.is_empty());

        // Test pagination
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let paginated = impl_repo.list_paginated(query).await.unwrap();
        assert!(!paginated.items.is_empty());
    }

    #[actix_web::test]
    async fn test_impl_error_handling() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();

        // Test getting non-existent signer
        let result = impl_repo.get_by_id("non-existent".to_string()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_impl_debug() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();
        let debug_string = format!("{:?}", impl_repo);
        assert!(debug_string.contains("InMemory"));
    }

    #[actix_web::test]
    async fn test_duplicate_creation_error() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();
        let signer = create_local_signer("duplicate-test".to_string());

        // Create the signer first time
        impl_repo.create(signer.clone()).await.unwrap();

        // Try to create again - should fail
        let result = impl_repo.create(signer).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RepositoryError::ConstraintViolation(_)
        ));
    }

    #[actix_web::test]
    async fn test_update_operations() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();
        let signer = create_local_signer("update-test".to_string());

        // Create the signer first
        impl_repo.create(signer.clone()).await.unwrap();

        // Update with different config
        let updated_signer = SignerRepoModel {
            id: "update-test".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[2; 32])),
            }),
        };

        let result = impl_repo
            .update("update-test".to_string(), updated_signer)
            .await;
        assert!(result.is_ok());

        // Test updating non-existent signer
        let non_existent_signer = SignerRepoModel {
            id: "non-existent".to_string(),
            config: SignerConfigStorage::Local(LocalSignerConfigStorage {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[3; 32])),
            }),
        };

        let result = impl_repo
            .update("non-existent".to_string(), non_existent_signer)
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_delete_operations() {
        let impl_repo = SignerRepositoryStorage::new_in_memory();
        let signer = create_local_signer("delete-test".to_string());

        // Create the signer first
        impl_repo.create(signer).await.unwrap();

        // Delete the signer
        let result = impl_repo.delete_by_id("delete-test".to_string()).await;
        assert!(result.is_ok());

        // Verify it's gone
        let get_result = impl_repo.get_by_id("delete-test".to_string()).await;
        assert!(get_result.is_err());
        assert!(matches!(
            get_result.unwrap_err(),
            RepositoryError::NotFound(_)
        ));

        // Test deleting non-existent signer
        let result = impl_repo.delete_by_id("non-existent".to_string()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_has_entries() {
        let repo = InMemorySignerRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let signer = create_local_signer("test".to_string());
        repo.create(signer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[actix_web::test]
    async fn test_drop_all_entries() {
        let repo = InMemorySignerRepository::new();
        let signer = create_local_signer("test".to_string());
        repo.create(signer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}

#[cfg(test)]
mockall::mock! {
    pub SignerRepository {}

    #[async_trait]
    impl Repository<SignerRepoModel, String> for SignerRepository {
        async fn create(&self, entity: SignerRepoModel) -> Result<SignerRepoModel, RepositoryError>;
        async fn get_by_id(&self, id: String) -> Result<SignerRepoModel, RepositoryError>;
        async fn list_all(&self) -> Result<Vec<SignerRepoModel>, RepositoryError>;
        async fn list_paginated(&self, query: PaginationQuery) -> Result<PaginatedResult<SignerRepoModel>, RepositoryError>;
        async fn update(&self, id: String, entity: SignerRepoModel) -> Result<SignerRepoModel, RepositoryError>;
        async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError>;
        async fn count(&self) -> Result<usize, RepositoryError>;
        async fn has_entries(&self) -> Result<bool, RepositoryError>;
        async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
    }
}
