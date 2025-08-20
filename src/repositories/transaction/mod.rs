//! Transaction Repository Module
//!
//! This module provides the transaction repository layer for the OpenZeppelin Relayer service.
//! It implements the Repository pattern to abstract transaction data persistence operations,
//! supporting both in-memory and Redis-backed storage implementations.
//!
//! ## Features
//!
//! - **CRUD Operations**: Create, read, update, and delete transactions
//! - **Specialized Queries**: Find transactions by relayer ID, status, and nonce
//! - **Pagination Support**: Efficient paginated listing of transactions
//! - **Status Management**: Update transaction status and timestamps
//! - **Partial Updates**: Support for partial transaction updates
//! - **Network Data**: Manage transaction network-specific data
//!
//! ## Repository Implementations
//!
//! - [`InMemoryTransactionRepository`]: Fast in-memory storage for testing/development
//! - [`RedisTransactionRepository`]: Redis-backed storage for production environments
//!
mod transaction_in_memory;
mod transaction_redis;

use redis::aio::ConnectionManager;
pub use transaction_in_memory::*;
pub use transaction_redis::*;

use crate::{
    models::{
        NetworkTransactionData, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use std::sync::Arc;

/// A trait defining transaction repository operations
#[async_trait]
pub trait TransactionRepository: Repository<TransactionRepoModel, String> {
    /// Find transactions by relayer ID with pagination
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError>;

    /// Find transactions by relayer ID and status(es)
    async fn find_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError>;

    /// Find a transaction by relayer ID and nonce
    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError>;

    /// Update the status of a transaction
    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Partially update a transaction
    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Update the network data of a transaction
    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Set the sent_at timestamp of a transaction
    async fn set_sent_at(
        &self,
        tx_id: String,
        sent_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError>;

    /// Set the confirmed_at timestamp of a transaction
    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError>;
}

#[cfg(test)]
mockall::mock! {
  pub TransactionRepository {}

  #[async_trait]
  impl Repository<TransactionRepoModel, String> for TransactionRepository {
      async fn create(&self, entity: TransactionRepoModel) -> Result<TransactionRepoModel, RepositoryError>;
      async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError>;
      async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError>;
      async fn list_paginated(&self, query: PaginationQuery) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError>;
      async fn update(&self, id: String, entity: TransactionRepoModel) -> Result<TransactionRepoModel, RepositoryError>;
      async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError>;
      async fn count(&self) -> Result<usize, RepositoryError>;
      async fn has_entries(&self) -> Result<bool, RepositoryError>;
      async fn drop_all_entries(&self) -> Result<(), RepositoryError>;
  }

  #[async_trait]
  impl TransactionRepository for TransactionRepository {
      async fn find_by_relayer_id(&self, relayer_id: &str, query: PaginationQuery) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError>;
      async fn find_by_status(&self, relayer_id: &str, statuses: &[TransactionStatus]) -> Result<Vec<TransactionRepoModel>, RepositoryError>;
      async fn find_by_nonce(&self, relayer_id: &str, nonce: u64) -> Result<Option<TransactionRepoModel>, RepositoryError>;
      async fn update_status(&self, tx_id: String, status: TransactionStatus) -> Result<TransactionRepoModel, RepositoryError>;
      async fn partial_update(&self, tx_id: String, update: TransactionUpdateRequest) -> Result<TransactionRepoModel, RepositoryError>;
      async fn update_network_data(&self, tx_id: String, network_data: NetworkTransactionData) -> Result<TransactionRepoModel, RepositoryError>;
      async fn set_sent_at(&self, tx_id: String, sent_at: String) -> Result<TransactionRepoModel, RepositoryError>;
      async fn set_confirmed_at(&self, tx_id: String, confirmed_at: String) -> Result<TransactionRepoModel, RepositoryError>;

  }
}

/// Enum wrapper for different transaction repository implementations
#[derive(Debug, Clone)]
pub enum TransactionRepositoryStorage {
    InMemory(InMemoryTransactionRepository),
    Redis(RedisTransactionRepository),
}

impl TransactionRepositoryStorage {
    pub fn new_in_memory() -> Self {
        Self::InMemory(InMemoryTransactionRepository::new())
    }
    pub fn new_redis(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::Redis(RedisTransactionRepository::new(
            connection_manager,
            key_prefix,
        )?))
    }
}

#[async_trait]
impl TransactionRepository for TransactionRepositoryStorage {
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.find_by_relayer_id(relayer_id, query).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.find_by_relayer_id(relayer_id, query).await
            }
        }
    }

    async fn find_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.find_by_status(relayer_id, statuses).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.find_by_status(relayer_id, statuses).await
            }
        }
    }

    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.find_by_nonce(relayer_id, nonce).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.find_by_nonce(relayer_id, nonce).await
            }
        }
    }

    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.update_status(tx_id, status).await,
            TransactionRepositoryStorage::Redis(repo) => repo.update_status(tx_id, status).await,
        }
    }

    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.partial_update(tx_id, update).await
            }
            TransactionRepositoryStorage::Redis(repo) => repo.partial_update(tx_id, update).await,
        }
    }

    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.update_network_data(tx_id, network_data).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.update_network_data(tx_id, network_data).await
            }
        }
    }

    async fn set_sent_at(
        &self,
        tx_id: String,
        sent_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.set_sent_at(tx_id, sent_at).await,
            TransactionRepositoryStorage::Redis(repo) => repo.set_sent_at(tx_id, sent_at).await,
        }
    }

    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => {
                repo.set_confirmed_at(tx_id, confirmed_at).await
            }
            TransactionRepositoryStorage::Redis(repo) => {
                repo.set_confirmed_at(tx_id, confirmed_at).await
            }
        }
    }
}

#[async_trait]
impl Repository<TransactionRepoModel, String> for TransactionRepositoryStorage {
    async fn create(
        &self,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.create(entity).await,
            TransactionRepositoryStorage::Redis(repo) => repo.create(entity).await,
        }
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.get_by_id(id).await,
            TransactionRepositoryStorage::Redis(repo) => repo.get_by_id(id).await,
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.list_all().await,
            TransactionRepositoryStorage::Redis(repo) => repo.list_all().await,
        }
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.list_paginated(query).await,
            TransactionRepositoryStorage::Redis(repo) => repo.list_paginated(query).await,
        }
    }

    async fn update(
        &self,
        id: String,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.update(id, entity).await,
            TransactionRepositoryStorage::Redis(repo) => repo.update(id, entity).await,
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.delete_by_id(id).await,
            TransactionRepositoryStorage::Redis(repo) => repo.delete_by_id(id).await,
        }
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.count().await,
            TransactionRepositoryStorage::Redis(repo) => repo.count().await,
        }
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.has_entries().await,
            TransactionRepositoryStorage::Redis(repo) => repo.has_entries().await,
        }
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        match self {
            TransactionRepositoryStorage::InMemory(repo) => repo.drop_all_entries().await,
            TransactionRepositoryStorage::Redis(repo) => repo.drop_all_entries().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionData, NetworkTransactionData, TransactionStatus, TransactionUpdateRequest,
    };
    use crate::repositories::PaginationQuery;
    use crate::utils::mocks::mockutils::create_mock_transaction;
    use chrono::Utc;
    use color_eyre::Result;

    fn create_test_transaction(id: &str, relayer_id: &str) -> TransactionRepoModel {
        let mut transaction = create_mock_transaction();
        transaction.id = id.to_string();
        transaction.relayer_id = relayer_id.to_string();
        transaction
    }

    fn create_test_transaction_with_status(
        id: &str,
        relayer_id: &str,
        status: TransactionStatus,
    ) -> TransactionRepoModel {
        let mut transaction = create_test_transaction(id, relayer_id);
        transaction.status = status;
        transaction
    }

    fn create_test_transaction_with_nonce(
        id: &str,
        relayer_id: &str,
        nonce: u64,
    ) -> TransactionRepoModel {
        let mut transaction = create_test_transaction(id, relayer_id);
        if let NetworkTransactionData::Evm(ref mut evm_data) = transaction.network_data {
            evm_data.nonce = Some(nonce);
        }
        transaction
    }

    fn create_test_update_request() -> TransactionUpdateRequest {
        TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            status_reason: Some("Test reason".to_string()),
            sent_at: Some(Utc::now().to_string()),
            confirmed_at: None,
            network_data: None,
            priced_at: None,
            hashes: Some(vec!["test_hash".to_string()]),
            noop_count: None,
            is_canceled: None,
            delete_at: None,
        }
    }

    #[tokio::test]
    async fn test_new_in_memory() {
        let storage = TransactionRepositoryStorage::new_in_memory();

        match storage {
            TransactionRepositoryStorage::InMemory(_) => {
                // Success - verify it's the InMemory variant
            }
            TransactionRepositoryStorage::Redis(_) => {
                panic!("Expected InMemory variant, got Redis");
            }
        }
    }

    #[tokio::test]
    async fn test_create_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        let created = storage.create(transaction.clone()).await?;
        assert_eq!(created.id, transaction.id);
        assert_eq!(created.relayer_id, transaction.relayer_id);
        assert_eq!(created.status, transaction.status);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_by_id_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction.clone()).await?;

        // Get by ID
        let retrieved = storage.get_by_id("test-tx".to_string()).await?;
        assert_eq!(retrieved.id, transaction.id);
        assert_eq!(retrieved.relayer_id, transaction.relayer_id);
        assert_eq!(retrieved.status, transaction.status);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_by_id_not_found_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        let result = storage.get_by_id("non-existent".to_string()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_list_all_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Initially empty
        let transactions = storage.list_all().await?;
        assert!(transactions.is_empty());

        // Add transactions
        let tx1 = create_test_transaction("tx-1", "relayer-1");
        let tx2 = create_test_transaction("tx-2", "relayer-2");

        storage.create(tx1.clone()).await?;
        storage.create(tx2.clone()).await?;

        let all_transactions = storage.list_all().await?;
        assert_eq!(all_transactions.len(), 2);

        let ids: Vec<&str> = all_transactions.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"tx-1"));
        assert!(ids.contains(&"tx-2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_list_paginated_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add test transactions
        for i in 1..=5 {
            let tx = create_test_transaction(&format!("tx-{}", i), "test-relayer");
            storage.create(tx).await?;
        }

        // Test pagination
        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };
        let page = storage.list_paginated(query).await?;

        assert_eq!(page.items.len(), 2);
        assert_eq!(page.total, 5);
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 2);

        // Test second page
        let query2 = PaginationQuery {
            page: 2,
            per_page: 2,
        };
        let page2 = storage.list_paginated(query2).await?;

        assert_eq!(page2.items.len(), 2);
        assert_eq!(page2.total, 5);
        assert_eq!(page2.page, 2);
        assert_eq!(page2.per_page, 2);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction.clone()).await?;

        // Update it
        let mut updated_transaction = transaction.clone();
        updated_transaction.status = TransactionStatus::Sent;
        updated_transaction.status_reason = Some("Updated reason".to_string());

        let result = storage
            .update("test-tx".to_string(), updated_transaction.clone())
            .await?;
        assert_eq!(result.id, "test-tx");
        assert_eq!(result.status, TransactionStatus::Sent);
        assert_eq!(result.status_reason, Some("Updated reason".to_string()));

        // Verify the update persisted
        let retrieved = storage.get_by_id("test-tx".to_string()).await?;
        assert_eq!(retrieved.status, TransactionStatus::Sent);
        assert_eq!(retrieved.status_reason, Some("Updated reason".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_update_not_found_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("non-existent", "test-relayer");

        let result = storage
            .update("non-existent".to_string(), transaction)
            .await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_by_id_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction.clone()).await?;

        // Verify it exists
        let retrieved = storage.get_by_id("test-tx".to_string()).await?;
        assert_eq!(retrieved.id, "test-tx");

        // Delete it
        storage.delete_by_id("test-tx".to_string()).await?;

        // Verify it's gone
        let result = storage.get_by_id("test-tx".to_string()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_by_id_not_found_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        let result = storage.delete_by_id("non-existent".to_string()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_count_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Initially empty
        let count = storage.count().await?;
        assert_eq!(count, 0);

        // Add transactions
        let tx1 = create_test_transaction("tx-1", "relayer-1");
        let tx2 = create_test_transaction("tx-2", "relayer-2");

        storage.create(tx1).await?;
        let count_after_one = storage.count().await?;
        assert_eq!(count_after_one, 1);

        storage.create(tx2).await?;
        let count_after_two = storage.count().await?;
        assert_eq!(count_after_two, 2);

        // Delete one
        storage.delete_by_id("tx-1".to_string()).await?;
        let count_after_delete = storage.count().await?;
        assert_eq!(count_after_delete, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_has_entries_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Initially empty
        let has_entries = storage.has_entries().await?;
        assert!(!has_entries);

        // Add transaction
        let transaction = create_test_transaction("test-tx", "test-relayer");
        storage.create(transaction).await?;

        let has_entries_after_create = storage.has_entries().await?;
        assert!(has_entries_after_create);

        // Delete transaction
        storage.delete_by_id("test-tx".to_string()).await?;

        let has_entries_after_delete = storage.has_entries().await?;
        assert!(!has_entries_after_delete);

        Ok(())
    }

    #[tokio::test]
    async fn test_drop_all_entries_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add multiple transactions
        for i in 1..=5 {
            let tx = create_test_transaction(&format!("tx-{}", i), "test-relayer");
            storage.create(tx).await?;
        }

        // Verify they exist
        let count_before = storage.count().await?;
        assert_eq!(count_before, 5);

        let has_entries_before = storage.has_entries().await?;
        assert!(has_entries_before);

        // Drop all entries
        storage.drop_all_entries().await?;

        // Verify they're gone
        let count_after = storage.count().await?;
        assert_eq!(count_after, 0);

        let has_entries_after = storage.has_entries().await?;
        assert!(!has_entries_after);

        let all_transactions = storage.list_all().await?;
        assert!(all_transactions.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_relayer_id_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add transactions for different relayers
        let tx1 = create_test_transaction("tx-1", "relayer-1");
        let tx2 = create_test_transaction("tx-2", "relayer-1");
        let tx3 = create_test_transaction("tx-3", "relayer-2");

        storage.create(tx1).await?;
        storage.create(tx2).await?;
        storage.create(tx3).await?;

        // Find by relayer ID
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = storage.find_by_relayer_id("relayer-1", query).await?;

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.total, 2);

        // Verify all transactions belong to relayer-1
        for tx in result.items {
            assert_eq!(tx.relayer_id, "relayer-1");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_status_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add transactions with different statuses
        let tx1 =
            create_test_transaction_with_status("tx-1", "relayer-1", TransactionStatus::Pending);
        let tx2 = create_test_transaction_with_status("tx-2", "relayer-1", TransactionStatus::Sent);
        let tx3 =
            create_test_transaction_with_status("tx-3", "relayer-1", TransactionStatus::Pending);
        let tx4 =
            create_test_transaction_with_status("tx-4", "relayer-2", TransactionStatus::Pending);

        storage.create(tx1).await?;
        storage.create(tx2).await?;
        storage.create(tx3).await?;
        storage.create(tx4).await?;

        // Find by status
        let statuses = vec![TransactionStatus::Pending];
        let result = storage.find_by_status("relayer-1", &statuses).await?;

        assert_eq!(result.len(), 2);

        // Verify all transactions have Pending status and belong to relayer-1
        for tx in result {
            assert_eq!(tx.status, TransactionStatus::Pending);
            assert_eq!(tx.relayer_id, "relayer-1");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_nonce_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add transactions with different nonces
        let tx1 = create_test_transaction_with_nonce("tx-1", "relayer-1", 10);
        let tx2 = create_test_transaction_with_nonce("tx-2", "relayer-1", 20);
        let tx3 = create_test_transaction_with_nonce("tx-3", "relayer-2", 10);

        storage.create(tx1).await?;
        storage.create(tx2).await?;
        storage.create(tx3).await?;

        // Find by nonce
        let result = storage.find_by_nonce("relayer-1", 10).await?;

        assert!(result.is_some());
        let found_tx = result.unwrap();
        assert_eq!(found_tx.id, "tx-1");
        assert_eq!(found_tx.relayer_id, "relayer-1");

        // Check EVM nonce
        if let NetworkTransactionData::Evm(evm_data) = found_tx.network_data {
            assert_eq!(evm_data.nonce, Some(10));
        }

        // Test not found
        let not_found = storage.find_by_nonce("relayer-1", 99).await?;
        assert!(not_found.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_update_status_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction).await?;

        // Update status
        let updated = storage
            .update_status("test-tx".to_string(), TransactionStatus::Sent)
            .await?;

        assert_eq!(updated.id, "test-tx");
        assert_eq!(updated.status, TransactionStatus::Sent);

        // Verify the update persisted
        let retrieved = storage.get_by_id("test-tx".to_string()).await?;
        assert_eq!(retrieved.status, TransactionStatus::Sent);

        Ok(())
    }

    #[tokio::test]
    async fn test_partial_update_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction).await?;

        // Partial update
        let update_request = create_test_update_request();
        let updated = storage
            .partial_update("test-tx".to_string(), update_request)
            .await?;

        assert_eq!(updated.id, "test-tx");
        assert_eq!(updated.status, TransactionStatus::Sent);
        assert_eq!(updated.status_reason, Some("Test reason".to_string()));
        assert!(updated.sent_at.is_some());
        assert_eq!(updated.hashes, vec!["test_hash".to_string()]);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_network_data_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction).await?;

        // Update network data
        let new_evm_data = EvmTransactionData {
            nonce: Some(42),
            gas_limit: Some(21000),
            ..Default::default()
        };
        let new_network_data = NetworkTransactionData::Evm(new_evm_data);

        let updated = storage
            .update_network_data("test-tx".to_string(), new_network_data)
            .await?;

        assert_eq!(updated.id, "test-tx");
        if let NetworkTransactionData::Evm(evm_data) = updated.network_data {
            assert_eq!(evm_data.nonce, Some(42));
            assert_eq!(evm_data.gas_limit, Some(21000));
        } else {
            panic!("Expected EVM network data");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_set_sent_at_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction).await?;

        // Set sent_at
        let sent_at = Utc::now().to_string();
        let updated = storage
            .set_sent_at("test-tx".to_string(), sent_at.clone())
            .await?;

        assert_eq!(updated.id, "test-tx");
        assert_eq!(updated.sent_at, Some(sent_at));

        Ok(())
    }

    #[tokio::test]
    async fn test_set_confirmed_at_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("test-tx", "test-relayer");

        // Create transaction first
        storage.create(transaction).await?;

        // Set confirmed_at
        let confirmed_at = Utc::now().to_string();
        let updated = storage
            .set_confirmed_at("test-tx".to_string(), confirmed_at.clone())
            .await?;

        assert_eq!(updated.id, "test-tx");
        assert_eq!(updated.confirmed_at, Some(confirmed_at));

        Ok(())
    }

    #[tokio::test]
    async fn test_create_duplicate_id_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();
        let transaction = create_test_transaction("duplicate-id", "test-relayer");

        // Create first transaction
        storage.create(transaction.clone()).await?;

        // Try to create another with same ID - should fail
        let result = storage.create(transaction.clone()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_workflow_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // 1. Start with empty storage
        assert!(!storage.has_entries().await?);
        assert_eq!(storage.count().await?, 0);

        // 2. Create transaction
        let transaction = create_test_transaction("workflow-test", "test-relayer");
        let created = storage.create(transaction.clone()).await?;
        assert_eq!(created.id, "workflow-test");

        // 3. Verify it exists
        assert!(storage.has_entries().await?);
        assert_eq!(storage.count().await?, 1);

        // 4. Retrieve it
        let retrieved = storage.get_by_id("workflow-test".to_string()).await?;
        assert_eq!(retrieved.id, "workflow-test");

        // 5. Update status
        let updated = storage
            .update_status("workflow-test".to_string(), TransactionStatus::Sent)
            .await?;
        assert_eq!(updated.status, TransactionStatus::Sent);

        // 6. Verify update
        let retrieved_updated = storage.get_by_id("workflow-test".to_string()).await?;
        assert_eq!(retrieved_updated.status, TransactionStatus::Sent);

        // 7. Delete it
        storage.delete_by_id("workflow-test".to_string()).await?;

        // 8. Verify it's gone
        assert!(!storage.has_entries().await?);
        assert_eq!(storage.count().await?, 0);

        let result = storage.get_by_id("workflow-test".to_string()).await;
        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_relayers_workflow() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add transactions for multiple relayers
        let tx1 =
            create_test_transaction_with_status("tx-1", "relayer-1", TransactionStatus::Pending);
        let tx2 = create_test_transaction_with_status("tx-2", "relayer-1", TransactionStatus::Sent);
        let tx3 =
            create_test_transaction_with_status("tx-3", "relayer-2", TransactionStatus::Pending);

        storage.create(tx1).await?;
        storage.create(tx2).await?;
        storage.create(tx3).await?;

        // Test find_by_relayer_id
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let relayer1_txs = storage.find_by_relayer_id("relayer-1", query).await?;
        assert_eq!(relayer1_txs.items.len(), 2);

        // Test find_by_status
        let pending_txs = storage
            .find_by_status("relayer-1", &[TransactionStatus::Pending])
            .await?;
        assert_eq!(pending_txs.len(), 1);
        assert_eq!(pending_txs[0].id, "tx-1");

        // Test count remains accurate
        assert_eq!(storage.count().await?, 3);

        Ok(())
    }

    #[tokio::test]
    async fn test_pagination_edge_cases_in_memory() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Test pagination with empty storage
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let page = storage.list_paginated(query).await?;
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.total, 0);
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 10);

        // Add one transaction
        let transaction = create_test_transaction("single-tx", "test-relayer");
        storage.create(transaction).await?;

        // Test pagination with single item
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let page = storage.list_paginated(query).await?;
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.total, 1);
        assert_eq!(page.page, 1);
        assert_eq!(page.per_page, 10);

        // Test pagination with page beyond total
        let query = PaginationQuery {
            page: 3,
            per_page: 10,
        };
        let page = storage.list_paginated(query).await?;
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.total, 1);
        assert_eq!(page.page, 3);
        assert_eq!(page.per_page, 10);

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_relayer_id_pagination() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add many transactions for one relayer
        for i in 1..=10 {
            let tx = create_test_transaction(&format!("tx-{}", i), "test-relayer");
            storage.create(tx).await?;
        }

        // Test first page
        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };
        let page1 = storage.find_by_relayer_id("test-relayer", query).await?;
        assert_eq!(page1.items.len(), 3);
        assert_eq!(page1.total, 10);
        assert_eq!(page1.page, 1);
        assert_eq!(page1.per_page, 3);

        // Test second page
        let query = PaginationQuery {
            page: 2,
            per_page: 3,
        };
        let page2 = storage.find_by_relayer_id("test-relayer", query).await?;
        assert_eq!(page2.items.len(), 3);
        assert_eq!(page2.total, 10);
        assert_eq!(page2.page, 2);
        assert_eq!(page2.per_page, 3);

        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_multiple_statuses() -> Result<()> {
        let storage = TransactionRepositoryStorage::new_in_memory();

        // Add transactions with different statuses
        let tx1 =
            create_test_transaction_with_status("tx-1", "test-relayer", TransactionStatus::Pending);
        let tx2 =
            create_test_transaction_with_status("tx-2", "test-relayer", TransactionStatus::Sent);
        let tx3 = create_test_transaction_with_status(
            "tx-3",
            "test-relayer",
            TransactionStatus::Confirmed,
        );
        let tx4 =
            create_test_transaction_with_status("tx-4", "test-relayer", TransactionStatus::Failed);

        storage.create(tx1).await?;
        storage.create(tx2).await?;
        storage.create(tx3).await?;
        storage.create(tx4).await?;

        // Find by multiple statuses
        let statuses = vec![TransactionStatus::Pending, TransactionStatus::Sent];
        let result = storage.find_by_status("test-relayer", &statuses).await?;

        assert_eq!(result.len(), 2);

        // Verify all transactions have the correct statuses
        let found_statuses: Vec<TransactionStatus> =
            result.iter().map(|tx| tx.status.clone()).collect();
        assert!(found_statuses.contains(&TransactionStatus::Pending));
        assert!(found_statuses.contains(&TransactionStatus::Sent));

        Ok(())
    }
}
