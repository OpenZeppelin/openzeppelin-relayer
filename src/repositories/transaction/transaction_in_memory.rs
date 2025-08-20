//! This module defines an in-memory transaction repository for managing
//! transaction data. It provides asynchronous methods for creating, retrieving,
//! updating, and deleting transactions, as well as querying transactions by
//! various criteria such as relayer ID, status, and nonce. The repository
//! is implemented using a `Mutex`-protected `HashMap` to store transaction
//! data, ensuring thread-safe access in an asynchronous context.
use crate::{
    models::{
        NetworkTransactionData, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest,
    },
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use itertools::Itertools;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryTransactionRepository {
    store: Mutex<HashMap<String, TransactionRepoModel>>,
}

impl Clone for InMemoryTransactionRepository {
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

impl InMemoryTransactionRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }

    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

// Implement both traits for InMemoryTransactionRepository

#[async_trait]
impl Repository<TransactionRepoModel, String> for InMemoryTransactionRepository {
    async fn create(
        &self,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&tx.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Transaction with ID {} already exists",
                tx.id
            )));
        }
        store.insert(tx.id.clone(), tx.clone());
        Ok(tx)
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        store.get(&id).cloned().ok_or_else(|| {
            RepositoryError::NotFound(format!("Transaction with ID {} not found", id))
        })
    }

    #[allow(clippy::map_entry)]
    async fn update(
        &self,
        id: String,
        tx: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&id) {
            let mut updated_tx = tx;
            updated_tx.id = id.clone();
            store.insert(id, updated_tx.clone());
            Ok(updated_tx)
        } else {
            Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
                id
            )))
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.remove(&id).is_some() {
            Ok(())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
                id
            )))
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.values().cloned().collect())
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;
        let store = Self::acquire_lock(&self.store).await?;
        let items: Vec<TransactionRepoModel> = store
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
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.len())
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

#[async_trait]
impl TransactionRepository for InMemoryTransactionRepository {
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let filtered: Vec<TransactionRepoModel> = store
            .values()
            .filter(|tx| tx.relayer_id == relayer_id)
            .cloned()
            .collect();

        let total = filtered.len() as u64;

        if total == 0 {
            return Ok(PaginatedResult::<TransactionRepoModel> {
                items: vec![],
                total: 0,
                page: query.page,
                per_page: query.per_page,
            });
        }

        let start = ((query.page - 1) * query.per_page) as usize;

        // Sort and paginate
        let items = filtered
            .into_iter()
            .sorted_by(|a, b| a.created_at.cmp(&b.created_at)) // Sort by created_at
            .skip(start)
            .take(query.per_page as usize)
            .collect();

        Ok(PaginatedResult {
            items,
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn find_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let filtered: Vec<TransactionRepoModel> = store
            .values()
            .filter(|tx| tx.relayer_id == relayer_id && statuses.contains(&tx.status))
            .cloned()
            .collect();

        // Sort by created_at (oldest first)
        let sorted = filtered
            .into_iter()
            .sorted_by_key(|tx| tx.created_at.clone())
            .collect();

        Ok(sorted)
    }

    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let filtered: Vec<TransactionRepoModel> = store
            .values()
            .filter(|tx| {
                tx.relayer_id == relayer_id
                    && match &tx.network_data {
                        NetworkTransactionData::Evm(data) => data.nonce == Some(nonce),
                        _ => false,
                    }
            })
            .cloned()
            .collect();

        Ok(filtered.into_iter().next())
    }

    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let update = TransactionUpdateRequest {
            status: Some(status),
            ..Default::default()
        };
        self.partial_update(tx_id, update).await
    }

    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;

        if let Some(tx) = store.get_mut(&tx_id) {
            // Apply partial updates using the model's business logic
            tx.apply_partial_update(update);
            Ok(tx.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
                tx_id
            )))
        }
    }

    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        tx.network_data = network_data;
        self.update(tx_id, tx).await
    }

    async fn set_sent_at(
        &self,
        tx_id: String,
        sent_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        tx.sent_at = Some(sent_at);
        self.update(tx_id, tx).await
    }

    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        tx.confirmed_at = Some(confirmed_at);
        self.update(tx_id, tx).await
    }
}

impl Default for InMemoryTransactionRepository {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{evm::Speed, EvmTransactionData, NetworkType};
    use lazy_static::lazy_static;
    use std::str::FromStr;

    use crate::models::U256;

    use super::*;

    use tokio::sync::Mutex;

    lazy_static! {
        static ref ENV_MUTEX: Mutex<()> = Mutex::new(());
    }
    // Helper function to create test transactions
    fn create_test_transaction(id: &str) -> TransactionRepoModel {
        TransactionRepoModel {
            id: id.to_string(),
            relayer_id: "relayer-1".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
            sent_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
            confirmed_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Evm,
            priced_at: None,
            hashes: vec![],
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                gas_price: Some(1000000000),
                gas_limit: Some(21000),
                nonce: Some(1),
                value: U256::from_str("1000000000000000000").unwrap(),
                data: Some("0x".to_string()),
                from: "0xSender".to_string(),
                to: Some("0xRecipient".to_string()),
                chain_id: 1,
                signature: None,
                hash: Some(format!("0x{}", id)),
                speed: Some(Speed::Fast),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                raw: None,
            }),
            noop_count: None,
            is_canceled: Some(false),
        }
    }

    fn create_test_transaction_pending_state(id: &str) -> TransactionRepoModel {
        TransactionRepoModel {
            id: id.to_string(),
            relayer_id: "relayer-1".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Evm,
            priced_at: None,
            hashes: vec![],
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                gas_price: Some(1000000000),
                gas_limit: Some(21000),
                nonce: Some(1),
                value: U256::from_str("1000000000000000000").unwrap(),
                data: Some("0x".to_string()),
                from: "0xSender".to_string(),
                to: Some("0xRecipient".to_string()),
                chain_id: 1,
                signature: None,
                hash: Some(format!("0x{}", id)),
                speed: Some(Speed::Fast),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                raw: None,
            }),
            noop_count: None,
            is_canceled: Some(false),
        }
    }

    #[tokio::test]
    async fn test_create_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        let result = repo.create(tx.clone()).await.unwrap();
        assert_eq!(result.id, tx.id);
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_get_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();
        let stored = repo.get_by_id("test-1".to_string()).await.unwrap();
        if let NetworkTransactionData::Evm(stored_data) = &stored.network_data {
            if let NetworkTransactionData::Evm(tx_data) = &tx.network_data {
                assert_eq!(stored_data.hash, tx_data.hash);
            }
        }
    }

    #[tokio::test]
    async fn test_update_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let mut tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();
        tx.status = TransactionStatus::Confirmed;

        let updated = repo.update("test-1".to_string(), tx).await.unwrap();
        assert!(matches!(updated.status, TransactionStatus::Confirmed));
    }

    #[tokio::test]
    async fn test_delete_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx).await.unwrap();
        repo.delete_by_id("test-1".to_string()).await.unwrap();

        let result = repo.get_by_id("test-1".to_string()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_all_transactions() {
        let repo = InMemoryTransactionRepository::new();
        let tx1 = create_test_transaction("test-1");
        let tx2 = create_test_transaction("test-2");

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();

        let transactions = repo.list_all().await.unwrap();
        assert_eq!(transactions.len(), 2);
    }

    #[tokio::test]
    async fn test_count_transactions() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        assert_eq!(repo.count().await.unwrap(), 0);
        repo.create(tx).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let result = repo.get_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_duplicate_transaction_creation() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();
        let result = repo.create(tx).await;

        assert!(matches!(
            result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
    }

    #[tokio::test]
    async fn test_update_nonexistent_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        let result = repo.update("nonexistent".to_string(), tx).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_partial_update() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction_pending_state("test-tx-id");
        repo.create(tx.clone()).await.unwrap();

        // Test updating only status
        let update1 = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            status_reason: None,
            sent_at: None,
            confirmed_at: None,
            network_data: None,
            hashes: None,
            priced_at: None,
            noop_count: None,
            is_canceled: None,
            delete_at: None,
        };
        let updated_tx1 = repo
            .partial_update("test-tx-id".to_string(), update1)
            .await
            .unwrap();
        assert_eq!(updated_tx1.status, TransactionStatus::Sent);
        assert_eq!(updated_tx1.sent_at, None);

        // Test updating multiple fields
        let update2 = TransactionUpdateRequest {
            status: Some(TransactionStatus::Confirmed),
            status_reason: None,
            sent_at: Some("2023-01-01T12:00:00Z".to_string()),
            confirmed_at: Some("2023-01-01T12:05:00Z".to_string()),
            network_data: None,
            hashes: None,
            priced_at: None,
            noop_count: None,
            is_canceled: None,
            delete_at: None,
        };
        let updated_tx2 = repo
            .partial_update("test-tx-id".to_string(), update2)
            .await
            .unwrap();
        assert_eq!(updated_tx2.status, TransactionStatus::Confirmed);
        assert_eq!(
            updated_tx2.sent_at,
            Some("2023-01-01T12:00:00Z".to_string())
        );
        assert_eq!(
            updated_tx2.confirmed_at,
            Some("2023-01-01T12:05:00Z".to_string())
        );

        // Test updating non-existent transaction
        let update3 = TransactionUpdateRequest {
            status: Some(TransactionStatus::Failed),
            status_reason: None,
            sent_at: None,
            confirmed_at: None,
            network_data: None,
            hashes: None,
            priced_at: None,
            noop_count: None,
            is_canceled: None,
            delete_at: None,
        };
        let result = repo
            .partial_update("non-existent-id".to_string(), update3)
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), RepositoryError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_update_status() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx).await.unwrap();

        // Update status to Confirmed
        let updated = repo
            .update_status("test-1".to_string(), TransactionStatus::Confirmed)
            .await
            .unwrap();

        // Verify the status was updated in the returned transaction
        assert_eq!(updated.status, TransactionStatus::Confirmed);

        // Also verify by getting the transaction directly
        let stored = repo.get_by_id("test-1".to_string()).await.unwrap();
        assert_eq!(stored.status, TransactionStatus::Confirmed);

        // Update status to Failed
        let updated = repo
            .update_status("test-1".to_string(), TransactionStatus::Failed)
            .await
            .unwrap();

        // Verify the status was updated
        assert_eq!(updated.status, TransactionStatus::Failed);

        // Verify updating a non-existent transaction
        let result = repo
            .update_status("non-existent".to_string(), TransactionStatus::Confirmed)
            .await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_paginated() {
        let repo = InMemoryTransactionRepository::new();

        // Create multiple transactions
        for i in 1..=10 {
            let tx = create_test_transaction(&format!("test-{}", i));
            repo.create(tx).await.unwrap();
        }

        // Test first page with 3 items per page
        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 3);

        // Test second page with 3 items per page
        let query = PaginationQuery {
            page: 2,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 2);
        assert_eq!(result.per_page, 3);

        // Test page with fewer items than per_page
        let query = PaginationQuery {
            page: 4,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.total, 10);
        assert_eq!(result.page, 4);
        assert_eq!(result.per_page, 3);

        // Test empty page (beyond total items)
        let query = PaginationQuery {
            page: 5,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 0);
        assert_eq!(result.total, 10);
    }

    #[tokio::test]
    async fn test_find_by_nonce() {
        let repo = InMemoryTransactionRepository::new();

        // Create transactions with different nonces
        let tx1 = create_test_transaction("test-1");

        let mut tx2 = create_test_transaction("test-2");
        if let NetworkTransactionData::Evm(ref mut data) = tx2.network_data {
            data.nonce = Some(2);
        }

        let mut tx3 = create_test_transaction("test-3");
        tx3.relayer_id = "relayer-2".to_string();
        if let NetworkTransactionData::Evm(ref mut data) = tx3.network_data {
            data.nonce = Some(1);
        }

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();
        repo.create(tx3).await.unwrap();

        // Test finding transaction with specific relayer_id and nonce
        let result = repo.find_by_nonce("relayer-1", 1).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.as_ref().unwrap().id, "test-1");

        // Test finding transaction with a different nonce
        let result = repo.find_by_nonce("relayer-1", 2).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.as_ref().unwrap().id, "test-2");

        // Test finding transaction from a different relayer
        let result = repo.find_by_nonce("relayer-2", 1).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.as_ref().unwrap().id, "test-3");

        // Test finding transaction that doesn't exist
        let result = repo.find_by_nonce("relayer-1", 99).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_update_network_data() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();

        // Create new network data with updated values
        let updated_network_data = NetworkTransactionData::Evm(EvmTransactionData {
            gas_price: Some(2000000000),
            gas_limit: Some(30000),
            nonce: Some(2),
            value: U256::from_str("2000000000000000000").unwrap(),
            data: Some("0xUpdated".to_string()),
            from: "0xSender".to_string(),
            to: Some("0xRecipient".to_string()),
            chain_id: 1,
            signature: None,
            hash: Some("0xUpdated".to_string()),
            raw: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        });

        let updated = repo
            .update_network_data("test-1".to_string(), updated_network_data)
            .await
            .unwrap();

        // Verify the network data was updated
        if let NetworkTransactionData::Evm(data) = &updated.network_data {
            assert_eq!(data.gas_price, Some(2000000000));
            assert_eq!(data.gas_limit, Some(30000));
            assert_eq!(data.nonce, Some(2));
            assert_eq!(data.hash, Some("0xUpdated".to_string()));
            assert_eq!(data.data, Some("0xUpdated".to_string()));
        } else {
            panic!("Expected EVM network data");
        }
    }

    #[tokio::test]
    async fn test_set_sent_at() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx).await.unwrap();

        // Updated sent_at timestamp
        let new_sent_at = "2025-02-01T10:00:00.000000+00:00".to_string();

        let updated = repo
            .set_sent_at("test-1".to_string(), new_sent_at.clone())
            .await
            .unwrap();

        // Verify the sent_at timestamp was updated
        assert_eq!(updated.sent_at, Some(new_sent_at.clone()));

        // Also verify by getting the transaction directly
        let stored = repo.get_by_id("test-1".to_string()).await.unwrap();
        assert_eq!(stored.sent_at, Some(new_sent_at.clone()));
    }

    #[tokio::test]
    async fn test_set_confirmed_at() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx).await.unwrap();

        // Updated confirmed_at timestamp
        let new_confirmed_at = "2025-02-01T11:30:45.123456+00:00".to_string();

        let updated = repo
            .set_confirmed_at("test-1".to_string(), new_confirmed_at.clone())
            .await
            .unwrap();

        // Verify the confirmed_at timestamp was updated
        assert_eq!(updated.confirmed_at, Some(new_confirmed_at.clone()));

        // Also verify by getting the transaction directly
        let stored = repo.get_by_id("test-1".to_string()).await.unwrap();
        assert_eq!(stored.confirmed_at, Some(new_confirmed_at.clone()));
    }

    #[tokio::test]
    async fn test_find_by_relayer_id() {
        let repo = InMemoryTransactionRepository::new();
        let tx1 = create_test_transaction("test-1");
        let tx2 = create_test_transaction("test-2");

        // Create a transaction with a different relayer_id
        let mut tx3 = create_test_transaction("test-3");
        tx3.relayer_id = "relayer-2".to_string();

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();
        repo.create(tx3).await.unwrap();

        // Test finding transactions for relayer-1
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = repo
            .find_by_relayer_id("relayer-1", query.clone())
            .await
            .unwrap();
        assert_eq!(result.total, 2);
        assert_eq!(result.items.len(), 2);
        assert!(result.items.iter().all(|tx| tx.relayer_id == "relayer-1"));

        // Test finding transactions for relayer-2
        let result = repo
            .find_by_relayer_id("relayer-2", query.clone())
            .await
            .unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.items.len(), 1);
        assert!(result.items.iter().all(|tx| tx.relayer_id == "relayer-2"));

        // Test finding transactions for non-existent relayer
        let result = repo
            .find_by_relayer_id("non-existent", query.clone())
            .await
            .unwrap();
        assert_eq!(result.total, 0);
        assert_eq!(result.items.len(), 0);
    }

    #[tokio::test]
    async fn test_find_by_status() {
        let repo = InMemoryTransactionRepository::new();
        let tx1 = create_test_transaction_pending_state("tx1");
        let mut tx2 = create_test_transaction_pending_state("tx2");
        tx2.status = TransactionStatus::Submitted;
        let mut tx3 = create_test_transaction_pending_state("tx3");
        tx3.relayer_id = "relayer-2".to_string();
        tx3.status = TransactionStatus::Pending;

        repo.create(tx1.clone()).await.unwrap();
        repo.create(tx2.clone()).await.unwrap();
        repo.create(tx3.clone()).await.unwrap();

        // Test finding by single status
        let pending_txs = repo
            .find_by_status("relayer-1", &[TransactionStatus::Pending])
            .await
            .unwrap();
        assert_eq!(pending_txs.len(), 1);
        assert_eq!(pending_txs[0].id, "tx1");

        let submitted_txs = repo
            .find_by_status("relayer-1", &[TransactionStatus::Submitted])
            .await
            .unwrap();
        assert_eq!(submitted_txs.len(), 1);
        assert_eq!(submitted_txs[0].id, "tx2");

        // Test finding by multiple statuses
        let multiple_status_txs = repo
            .find_by_status(
                "relayer-1",
                &[TransactionStatus::Pending, TransactionStatus::Submitted],
            )
            .await
            .unwrap();
        assert_eq!(multiple_status_txs.len(), 2);

        // Test finding for different relayer
        let relayer2_pending = repo
            .find_by_status("relayer-2", &[TransactionStatus::Pending])
            .await
            .unwrap();
        assert_eq!(relayer2_pending.len(), 1);
        assert_eq!(relayer2_pending[0].id, "tx3");

        // Test finding for non-existent relayer
        let no_txs = repo
            .find_by_status("non-existent", &[TransactionStatus::Pending])
            .await
            .unwrap();
        assert_eq!(no_txs.len(), 0);
    }

    #[tokio::test]
    async fn test_find_by_status_sorted_by_created_at() {
        let repo = InMemoryTransactionRepository::new();

        // Helper function to create transaction with custom created_at timestamp
        let create_tx_with_timestamp = |id: &str, timestamp: &str| -> TransactionRepoModel {
            let mut tx = create_test_transaction_pending_state(id);
            tx.created_at = timestamp.to_string();
            tx.status = TransactionStatus::Pending;
            tx
        };

        // Create transactions with different timestamps (out of chronological order)
        let tx3 = create_tx_with_timestamp("tx3", "2025-01-27T17:00:00.000000+00:00"); // Latest
        let tx1 = create_tx_with_timestamp("tx1", "2025-01-27T15:00:00.000000+00:00"); // Earliest
        let tx2 = create_tx_with_timestamp("tx2", "2025-01-27T16:00:00.000000+00:00"); // Middle

        // Create them in reverse chronological order to test sorting
        repo.create(tx3.clone()).await.unwrap();
        repo.create(tx1.clone()).await.unwrap();
        repo.create(tx2.clone()).await.unwrap();

        // Find by status
        let result = repo
            .find_by_status("relayer-1", &[TransactionStatus::Pending])
            .await
            .unwrap();

        // Verify they are sorted by created_at (oldest first)
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].id, "tx1"); // Earliest
        assert_eq!(result[1].id, "tx2"); // Middle
        assert_eq!(result[2].id, "tx3"); // Latest

        // Verify the timestamps are in ascending order
        assert_eq!(result[0].created_at, "2025-01-27T15:00:00.000000+00:00");
        assert_eq!(result[1].created_at, "2025-01-27T16:00:00.000000+00:00");
        assert_eq!(result[2].created_at, "2025-01-27T17:00:00.000000+00:00");
    }

    #[tokio::test]
    async fn test_has_entries() {
        let repo = InMemoryTransactionRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let tx = create_test_transaction("test");
        repo.create(tx.clone()).await.unwrap();

        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    async fn test_drop_all_entries() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test");
        repo.create(tx.clone()).await.unwrap();

        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }

    // Tests for delete_at field setting on final status updates

    #[tokio::test]
    async fn test_update_status_sets_delete_at_for_final_statuses() {
        let _lock = ENV_MUTEX.lock().await;

        use chrono::{DateTime, Duration, Utc};
        use std::env;

        // Use a unique test environment variable to avoid conflicts
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "6");

        let repo = InMemoryTransactionRepository::new();

        let final_statuses = [
            TransactionStatus::Canceled,
            TransactionStatus::Confirmed,
            TransactionStatus::Failed,
            TransactionStatus::Expired,
        ];

        for (i, status) in final_statuses.iter().enumerate() {
            let tx_id = format!("test-final-{}", i);
            let tx = create_test_transaction_pending_state(&tx_id);

            // Ensure transaction has no delete_at initially
            assert!(tx.delete_at.is_none());

            repo.create(tx).await.unwrap();

            let before_update = Utc::now();

            // Update to final status
            let updated = repo
                .update_status(tx_id.clone(), status.clone())
                .await
                .unwrap();

            // Should have delete_at set
            assert!(
                updated.delete_at.is_some(),
                "delete_at should be set for status: {:?}",
                status
            );

            // Verify the timestamp is reasonable (approximately 6 hours from now)
            let delete_at_str = updated.delete_at.unwrap();
            let delete_at = DateTime::parse_from_rfc3339(&delete_at_str)
                .expect("delete_at should be valid RFC3339")
                .with_timezone(&Utc);

            let duration_from_before = delete_at.signed_duration_since(before_update);
            let expected_duration = Duration::hours(6);
            let tolerance = Duration::minutes(5);

            assert!(
                duration_from_before >= expected_duration - tolerance &&
                duration_from_before <= expected_duration + tolerance,
                "delete_at should be approximately 6 hours from now for status: {:?}. Duration: {:?}",
                status, duration_from_before
            );
        }

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }

    #[tokio::test]
    async fn test_update_status_does_not_set_delete_at_for_non_final_statuses() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "4");

        let repo = InMemoryTransactionRepository::new();

        let non_final_statuses = [
            TransactionStatus::Pending,
            TransactionStatus::Sent,
            TransactionStatus::Submitted,
            TransactionStatus::Mined,
        ];

        for (i, status) in non_final_statuses.iter().enumerate() {
            let tx_id = format!("test-non-final-{}", i);
            let tx = create_test_transaction_pending_state(&tx_id);

            repo.create(tx).await.unwrap();

            // Update to non-final status
            let updated = repo
                .update_status(tx_id.clone(), status.clone())
                .await
                .unwrap();

            // Should NOT have delete_at set
            assert!(
                updated.delete_at.is_none(),
                "delete_at should NOT be set for status: {:?}",
                status
            );
        }

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }

    #[tokio::test]
    async fn test_partial_update_sets_delete_at_for_final_statuses() {
        let _lock = ENV_MUTEX.lock().await;

        use chrono::{DateTime, Duration, Utc};
        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "8");

        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction_pending_state("test-partial-final");

        repo.create(tx).await.unwrap();

        let before_update = Utc::now();

        // Use partial_update to set status to Confirmed (final status)
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Confirmed),
            status_reason: Some("Transaction completed".to_string()),
            confirmed_at: Some("2023-01-01T12:05:00Z".to_string()),
            ..Default::default()
        };

        let updated = repo
            .partial_update("test-partial-final".to_string(), update)
            .await
            .unwrap();

        // Should have delete_at set
        assert!(
            updated.delete_at.is_some(),
            "delete_at should be set when updating to Confirmed status"
        );

        // Verify the timestamp is reasonable (approximately 8 hours from now)
        let delete_at_str = updated.delete_at.unwrap();
        let delete_at = DateTime::parse_from_rfc3339(&delete_at_str)
            .expect("delete_at should be valid RFC3339")
            .with_timezone(&Utc);

        let duration_from_before = delete_at.signed_duration_since(before_update);
        let expected_duration = Duration::hours(8);
        let tolerance = Duration::minutes(5);

        assert!(
            duration_from_before >= expected_duration - tolerance
                && duration_from_before <= expected_duration + tolerance,
            "delete_at should be approximately 8 hours from now. Duration: {:?}",
            duration_from_before
        );

        // Also verify other fields were updated
        assert_eq!(updated.status, TransactionStatus::Confirmed);
        assert_eq!(
            updated.status_reason,
            Some("Transaction completed".to_string())
        );
        assert_eq!(
            updated.confirmed_at,
            Some("2023-01-01T12:05:00Z".to_string())
        );

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }

    #[tokio::test]
    async fn test_update_status_preserves_existing_delete_at() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "2");

        let repo = InMemoryTransactionRepository::new();
        let mut tx = create_test_transaction_pending_state("test-preserve-delete-at");

        // Set an existing delete_at value
        let existing_delete_at = "2025-01-01T12:00:00Z".to_string();
        tx.delete_at = Some(existing_delete_at.clone());

        repo.create(tx).await.unwrap();

        // Update to final status
        let updated = repo
            .update_status(
                "test-preserve-delete-at".to_string(),
                TransactionStatus::Confirmed,
            )
            .await
            .unwrap();

        // Should preserve the existing delete_at value
        assert_eq!(
            updated.delete_at,
            Some(existing_delete_at),
            "Existing delete_at should be preserved when updating to final status"
        );

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }

    #[tokio::test]
    async fn test_partial_update_without_status_change_preserves_delete_at() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "3");

        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction_pending_state("test-preserve-no-status");

        repo.create(tx).await.unwrap();

        // First, update to final status to set delete_at
        let updated1 = repo
            .update_status(
                "test-preserve-no-status".to_string(),
                TransactionStatus::Confirmed,
            )
            .await
            .unwrap();

        assert!(updated1.delete_at.is_some());
        let original_delete_at = updated1.delete_at.clone();

        // Now update other fields without changing status
        let update = TransactionUpdateRequest {
            status: None, // No status change
            status_reason: Some("Updated reason".to_string()),
            confirmed_at: Some("2023-01-01T12:10:00Z".to_string()),
            ..Default::default()
        };

        let updated2 = repo
            .partial_update("test-preserve-no-status".to_string(), update)
            .await
            .unwrap();

        // delete_at should be preserved
        assert_eq!(
            updated2.delete_at, original_delete_at,
            "delete_at should be preserved when status is not updated"
        );

        // Other fields should be updated
        assert_eq!(updated2.status, TransactionStatus::Confirmed); // Unchanged
        assert_eq!(updated2.status_reason, Some("Updated reason".to_string()));
        assert_eq!(
            updated2.confirmed_at,
            Some("2023-01-01T12:10:00Z".to_string())
        );

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }

    #[tokio::test]
    async fn test_update_status_multiple_updates_idempotent() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "12");

        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction_pending_state("test-idempotent");

        repo.create(tx).await.unwrap();

        // First update to final status
        let updated1 = repo
            .update_status("test-idempotent".to_string(), TransactionStatus::Confirmed)
            .await
            .unwrap();

        assert!(updated1.delete_at.is_some());
        let first_delete_at = updated1.delete_at.clone();

        // Second update to another final status
        let updated2 = repo
            .update_status("test-idempotent".to_string(), TransactionStatus::Failed)
            .await
            .unwrap();

        // delete_at should remain the same (idempotent)
        assert_eq!(
            updated2.delete_at, first_delete_at,
            "delete_at should not change on subsequent final status updates"
        );

        // Status should be updated
        assert_eq!(updated2.status, TransactionStatus::Failed);

        // Cleanup
        env::remove_var("TRANSACTION_EXPIRATION_HOURS");
    }
}
