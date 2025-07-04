//! Signer Repository
mod signer_in_memory;
mod signer_redis;

pub use signer_in_memory::*;
pub use signer_redis::*;

use crate::{
    config::ServerConfig,
    models::{RepositoryError, SignerRepoModel},
    repositories::{PaginatedResult, PaginationQuery, Repository},
};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;

/// Enum representing the type of signer repository to use
pub enum SignerRepositoryType {
    InMemory,
    Redis,
}

/// Enum wrapper for different signer repository implementations
#[derive(Debug, Clone)]
pub enum SignerRepositoryImpl {
    InMemory(InMemorySignerRepository),
    Redis(RedisSignerRepository),
}

impl SignerRepositoryImpl {
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

impl SignerRepositoryType {
    /// Creates a signer repository based on the enum variant
    pub async fn create_repository(self, config: &ServerConfig) -> SignerRepositoryImpl {
        match self {
            SignerRepositoryType::InMemory => {
                SignerRepositoryImpl::InMemory(InMemorySignerRepository::new())
            }
            SignerRepositoryType::Redis => {
                let client = redis::Client::open(config.redis_url.clone())
                    .expect("Failed to create Redis client");
                let connection_manager = redis::aio::ConnectionManager::new(client)
                    .await
                    .expect("Failed to create Redis connection manager");
                SignerRepositoryImpl::Redis(
                    RedisSignerRepository::new(
                        Arc::new(connection_manager),
                        config.redis_key_prefix.clone(),
                    )
                    .expect("Failed to create Redis signer repository"),
                )
            }
        }
    }
}

#[async_trait]
impl Repository<SignerRepoModel, String> for SignerRepositoryImpl {
    async fn create(&self, entity: SignerRepoModel) -> Result<SignerRepoModel, RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.create(entity).await,
            SignerRepositoryImpl::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<SignerRepoModel, RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.get_by_id(id).await,
            SignerRepositoryImpl::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<SignerRepoModel>, RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.list_all().await,
            SignerRepositoryImpl::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<SignerRepoModel>, RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.list_paginated(query).await,
            SignerRepositoryImpl::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: SignerRepoModel,
    ) -> Result<SignerRepoModel, RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.update(id, entity).await,
            SignerRepositoryImpl::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.delete_by_id(id).await,
            SignerRepositoryImpl::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            SignerRepositoryImpl::InMemory(repo) => repo.count().await,
            SignerRepositoryImpl::Redis(repo) => repo.count().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LocalSignerConfig, SignerConfig};
    use secrets::SecretVec;

    fn create_test_signer(id: String) -> SignerRepoModel {
        SignerRepoModel {
            id: id.clone(),
            config: SignerConfig::Test(LocalSignerConfig {
                raw_key: SecretVec::new(32, |v| v.copy_from_slice(&[1; 32])),
            }),
        }
    }

    #[actix_web::test]
    async fn test_in_memory_impl_creation() {
        let impl_repo = SignerRepositoryImpl::new_in_memory();
        match impl_repo {
            SignerRepositoryImpl::InMemory(_) => (),
            _ => panic!("Expected InMemory variant"),
        }
    }

    #[actix_web::test]
    async fn test_in_memory_impl_operations() {
        let impl_repo = SignerRepositoryImpl::new_in_memory();
        let signer = create_test_signer("test-signer".to_string());

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
        let impl_repo = SignerRepositoryImpl::new_in_memory();

        // Test getting non-existent signer
        let result = impl_repo.get_by_id("non-existent".to_string()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[actix_web::test]
    async fn test_impl_debug() {
        let impl_repo = SignerRepositoryImpl::new_in_memory();
        let debug_string = format!("{:?}", impl_repo);
        assert!(debug_string.contains("InMemory"));
    }

    #[actix_web::test]
    async fn test_duplicate_creation_error() {
        let impl_repo = SignerRepositoryImpl::new_in_memory();
        let signer = create_test_signer("duplicate-test".to_string());

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
        let impl_repo = SignerRepositoryImpl::new_in_memory();
        let signer = create_test_signer("update-test".to_string());

        // Create the signer first
        impl_repo.create(signer.clone()).await.unwrap();

        // Update with different config
        let updated_signer = SignerRepoModel {
            id: "update-test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig {
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
            config: SignerConfig::Local(LocalSignerConfig {
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
        let impl_repo = SignerRepositoryImpl::new_in_memory();
        let signer = create_test_signer("delete-test".to_string());

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
    }
}
