//! Relayer Repository Module
//!
//! This module provides the relayer repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract relayer data persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete relayer configurations
//! - **Status Management**: Enable/disable relayers and track their state
//! - **Policy Management**: Update relayer network policies
//! - **Partial Updates**: Support for partial relayer configuration updates
//! - **Active Filtering**: Query for active (non-paused) relayers
//! - **Pagination Support**: Efficient paginated listing of relayers
//!
//! ## Repository Implementations
//!
//! - [`InMemoryRelayerRepository`]: Fast in-memory storage for testing/development
//! - [`RedisRelayerRepository`]: Redis-backed storage for production environments
//!

mod relayer_in_memory;
mod relayer_redis;

pub use relayer_in_memory::*;
pub use relayer_redis::*;

use crate::{
    models::UpdateRelayerRequest,
    models::{PaginationQuery, RelayerNetworkPolicy, RelayerRepoModel, RepositoryError},
    repositories::{PaginatedResult, Repository},
};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use std::sync::Arc;

#[async_trait]
pub trait RelayerRepository: Repository<RelayerRepoModel, String> + Send + Sync {
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
    async fn list_by_signer_id(
        &self,
        signer_id: &str,
    ) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
    async fn list_by_notification_id(
        &self,
        notification_id: &str,
    ) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
    async fn partial_update(
        &self,
        id: String,
        update: UpdateRelayerRequest,
    ) -> Result<RelayerRepoModel, RepositoryError>;
    async fn enable_relayer(&self, relayer_id: String)
        -> Result<RelayerRepoModel, RepositoryError>;
    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError>;
    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError>;
}

/// Enum wrapper for different relayer repository implementations
#[derive(Debug, Clone)]
pub enum RelayerRepositoryStorage {
    InMemory(InMemoryRelayerRepository),
    Redis(RedisRelayerRepository),
}

impl RelayerRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryRelayerRepository::new())
    }

    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisRelayerRepository::new(
            connection_manager,
            key_prefix,
        )?))
    }
}

impl Default for RelayerRepositoryStorage {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

#[async_trait]
impl Repository<RelayerRepoModel, String> for RelayerRepositoryStorage {
    async fn create(&self, entity: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            RelayerRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            RelayerRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_all().await,
            RelayerRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            RelayerRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            RelayerRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            RelayerRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.count().await,
            RelayerRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            RelayerRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            RelayerRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }
}

#[async_trait]
impl RelayerRepository for RelayerRepositoryStorage {
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_active().await,
            RelayerRepositoryStorage::Redis(repo) => repo.list_active().await,
        }
    }

    async fn list_by_signer_id(
        &self,
        signer_id: &str,
    ) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.list_by_signer_id(signer_id).await,
            RelayerRepositoryStorage::Redis(repo) => repo.list_by_signer_id(signer_id).await,
        }
    }

    async fn list_by_notification_id(
        &self,
        notification_id: &str,
    ) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => {
                repo.list_by_notification_id(notification_id).await
            }
            RelayerRepositoryStorage::Redis(repo) => {
                repo.list_by_notification_id(notification_id).await
            }
        }
    }

    async fn partial_update(
        &self,
        id: String,
        update: UpdateRelayerRequest,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.partial_update(id, update).await,
            RelayerRepositoryStorage::Redis(repo) => repo.partial_update(id, update).await,
        }
    }

    async fn enable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.enable_relayer(relayer_id).await,
            RelayerRepositoryStorage::Redis(repo) => repo.enable_relayer(relayer_id).await,
        }
    }

    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.disable_relayer(relayer_id).await,
            RelayerRepositoryStorage::Redis(repo) => repo.disable_relayer(relayer_id).await,
        }
    }

    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        match self {
            RelayerRepositoryStorage::InMemory(repo) => repo.update_policy(id, policy).await,
            RelayerRepositoryStorage::Redis(repo) => repo.update_policy(id, policy).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy};

    fn create_test_relayer(id: String) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.clone(),
            name: format!("Relayer {}", id.clone()),
            network: "TestNet".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                min_balance: Some(0),
                gas_limit_estimation: Some(true),
                gas_price_cap: None,
                whitelist_receivers: None,
                eip1559_pricing: Some(false),
                private_transactions: Some(false),
            }),
            signer_id: "test".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    #[actix_web::test]
    async fn test_in_memory_repository_impl() {
        let impl_repo = RelayerRepositoryStorage::new_in_memory();
        let relayer = create_test_relayer("test-relayer".to_string());

        // Test create
        let created = impl_repo.create(relayer.clone()).await.unwrap();
        assert_eq!(created.id, relayer.id);

        // Test get
        let retrieved = impl_repo
            .get_by_id("test-relayer".to_string())
            .await
            .unwrap();
        assert_eq!(retrieved.id, relayer.id);

        // Test list all
        let all_relayers = impl_repo.list_all().await.unwrap();
        assert!(!all_relayers.is_empty());

        // Test count
        let count = impl_repo.count().await.unwrap();
        assert!(count >= 1);

        // Test update
        let mut updated_relayer = relayer.clone();
        updated_relayer.name = "Updated Name".to_string();
        let updated = impl_repo
            .update(relayer.id.clone(), updated_relayer)
            .await
            .unwrap();
        assert_eq!(updated.name, "Updated Name");

        // Test delete
        impl_repo.delete_by_id(relayer.id.clone()).await.unwrap();
        let get_result = impl_repo.get_by_id("test-relayer".to_string()).await;
        assert!(get_result.is_err());
    }

    #[actix_web::test]
    async fn test_relayer_repository_trait_methods() {
        let impl_repo = RelayerRepositoryStorage::new_in_memory();
        let relayer = create_test_relayer("test-relayer".to_string());

        // Create the relayer first
        impl_repo.create(relayer.clone()).await.unwrap();

        // Test list_active
        let active_relayers = impl_repo.list_active().await.unwrap();
        assert!(!active_relayers.is_empty());

        // Test partial_update
        let update = UpdateRelayerRequest {
            paused: Some(true),
            ..Default::default()
        };
        let updated = impl_repo
            .partial_update(relayer.id.clone(), update)
            .await
            .unwrap();
        assert!(updated.paused);

        // Test enable/disable
        let disabled = impl_repo.disable_relayer(relayer.id.clone()).await.unwrap();
        assert!(disabled.system_disabled);

        let enabled = impl_repo.enable_relayer(relayer.id.clone()).await.unwrap();
        assert!(!enabled.system_disabled);

        // Test update_policy
        let new_policy = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            min_balance: Some(1000000000000000000),
            gas_limit_estimation: Some(true),
            gas_price_cap: Some(50_000_000_000),
            whitelist_receivers: None,
            eip1559_pricing: Some(true),
            private_transactions: Some(false),
        });
        let policy_updated = impl_repo
            .update_policy(relayer.id.clone(), new_policy)
            .await
            .unwrap();

        if let RelayerNetworkPolicy::Evm(evm_policy) = policy_updated.policies {
            assert_eq!(evm_policy.gas_price_cap, Some(50_000_000_000));
            assert_eq!(evm_policy.eip1559_pricing, Some(true));
        } else {
            panic!("Expected EVM policy");
        }
    }

    #[actix_web::test]
    async fn test_create_repository_in_memory() {
        let result = RelayerRepositoryStorage::new_in_memory();

        assert!(matches!(result, RelayerRepositoryStorage::InMemory(_)));
    }

    #[actix_web::test]
    async fn test_pagination() {
        let impl_repo = RelayerRepositoryStorage::new_in_memory();
        let relayer1 = create_test_relayer("test-relayer-1".to_string());
        let relayer2 = create_test_relayer("test-relayer-2".to_string());

        impl_repo.create(relayer1).await.unwrap();
        impl_repo.create(relayer2).await.unwrap();

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };

        let result = impl_repo.list_paginated(query).await.unwrap();
        assert!(result.total >= 2);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 10);
    }

    #[actix_web::test]
    async fn test_delete_relayer() {
        let impl_repo = RelayerRepositoryStorage::new_in_memory();
        let relayer = create_test_relayer("delete-test".to_string());

        // Create relayer
        impl_repo.create(relayer.clone()).await.unwrap();

        // Delete relayer
        impl_repo
            .delete_by_id("delete-test".to_string())
            .await
            .unwrap();

        // Verify deletion
        let get_result = impl_repo.get_by_id("delete-test".to_string()).await;
        assert!(get_result.is_err());
        assert!(matches!(
            get_result.unwrap_err(),
            RepositoryError::NotFound(_)
        ));

        // Test deleting non-existent relayer
        let delete_result = impl_repo.delete_by_id("nonexistent".to_string()).await;
        assert!(delete_result.is_err());
    }

    #[actix_web::test]
    async fn test_has_entries() {
        let repo = InMemoryRelayerRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.delete_by_id(relayer.id.clone()).await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }

    #[actix_web::test]
    async fn test_drop_all_entries() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}

#[cfg(test)]
mockall::mock! {
    pub RelayerRepository {}

    #[async_trait]
    impl Repository<RelayerRepoModel, String> for RelayerRepository {
        async fn create(&self, entity: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError>;
        async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError>;
        async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
        async fn list_paginated(&self, query: PaginationQuery) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError>;
        async fn update(&self, id: String, entity: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError>;
        async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError>;
        async fn count(&self) -> Result<usize, RepositoryError>;
        async fn has_entries(&self) -> Result<bool, RepositoryError>;
        async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
    }

    #[async_trait]
    impl RelayerRepository for RelayerRepository {
        async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
        async fn list_by_signer_id(&self, signer_id: &str) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
        async fn list_by_notification_id(&self, notification_id: &str) -> Result<Vec<RelayerRepoModel>, RepositoryError>;
        async fn partial_update(&self, id: String, update: UpdateRelayerRequest) -> Result<RelayerRepoModel, RepositoryError>;
        async fn enable_relayer(&self, relayer_id: String) -> Result<RelayerRepoModel, RepositoryError>;
        async fn disable_relayer(&self, relayer_id: String) -> Result<RelayerRepoModel, RepositoryError>;
        async fn update_policy(&self, id: String, policy: RelayerNetworkPolicy) -> Result<RelayerRepoModel, RepositoryError>;
    }
}
