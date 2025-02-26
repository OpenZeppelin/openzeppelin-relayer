//! This module defines an in-memory transaction repository for managing
//! transaction data. It provides asynchronous methods for creating, retrieving,
//! updating, and deleting transactions, as well as querying transactions by
//! various criteria such as relayer ID, status, and nonce. The repository
//! is implemented using a `Mutex`-protected `HashMap` to store transaction
//! data, ensuring thread-safe access in an asynchronous context.
use crate::{
    models::{NetworkTransactionData, TransactionRepoModel, TransactionStatus},
    repositories::*,
};
use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct InMemoryTransactionRepository {
    store: Mutex<HashMap<String, TransactionRepoModel>>,
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

    pub async fn find_by_relayer_id(
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

    pub async fn find_by_status(
        &self,
        status: TransactionStatus,
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store
            .values()
            .filter(|tx| tx.status == status)
            .cloned()
            .collect())
    }

    pub async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store
            .values()
            .find(|tx| {
                tx.relayer_id == relayer_id
                    && matches!(&tx.network_data,
                        NetworkTransactionData::Evm(data) if data.nonce == Some(nonce)
                    )
            })
            .cloned())
    }

    pub async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        tx.status = status;
        self.update(tx_id, tx).await
    }

    pub async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        tx.network_data = network_data;
        self.update(tx_id, tx).await
    }
}

impl Default for InMemoryTransactionRepository {
    fn default() -> Self {
        Self::new()
    }
}

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
}

#[cfg(test)]
mod tests {
    use crate::models::{evm::Speed, EvmTransactionData, NetworkType};
    use std::str::FromStr;

    use crate::models::U256;

    use super::*;

    fn create_test_transaction(id: &str) -> TransactionRepoModel {
        TransactionRepoModel {
            id: id.to_string(),
            relayer_id: "relayer-1".to_string(),
            status: TransactionStatus::Pending,
            created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
            sent_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
            confirmed_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                gas_price: Some(1000000000),
                gas_limit: 21000,
                nonce: Some(1),
                value: U256::from_str("1000000000000000000").unwrap(),
                data: Some("Ox".to_string()),
                from: "0x".to_string(),
                to: Some("0x".to_string()),
                chain_id: 1,
                signature: None,
                hash: Some(format!("0x{}", id)),
                speed: Some(Speed::Fast),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                raw: None,
            }),
        }
    }

    #[actix_web::test]
    async fn test_create_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        let result = repo.create(tx.clone()).await.unwrap();
        assert_eq!(result.id, tx.id);
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[actix_web::test]
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

    #[actix_web::test]
    async fn test_update_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let mut tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();
        tx.status = TransactionStatus::Confirmed;

        let updated = repo.update("test-1".to_string(), tx).await.unwrap();
        assert!(matches!(updated.status, TransactionStatus::Confirmed));
    }

    #[actix_web::test]
    async fn test_delete_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx).await.unwrap();
        repo.delete_by_id("test-1".to_string()).await.unwrap();

        let result = repo.get_by_id("test-1".to_string()).await;
        assert!(result.is_err());
    }

    #[actix_web::test]
    async fn test_list_all_transactions() {
        let repo = InMemoryTransactionRepository::new();
        let tx1 = create_test_transaction("test-1");
        let tx2 = create_test_transaction("test-2");

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();

        let transactions = repo.list_all().await.unwrap();
        assert_eq!(transactions.len(), 2);
    }

    #[actix_web::test]
    async fn test_count_transactions() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        assert_eq!(repo.count().await.unwrap(), 0);
        repo.create(tx).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);
    }

    #[actix_web::test]
    async fn test_get_nonexistent_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let result = repo.get_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
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

    #[actix_web::test]
    async fn test_update_nonexistent_transaction() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        let result = repo.update("nonexistent".to_string(), tx).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_update_network_data() {
        let repo = InMemoryTransactionRepository::new();
        let tx = create_test_transaction("test-1");

        repo.create(tx.clone()).await.unwrap();

        // Create new network data with updated values
        let updated_network_data = NetworkTransactionData::Evm(EvmTransactionData {
            gas_price: Some(2000000000),
            gas_limit: 30000,
            nonce: Some(2),
            value: U256::from_str("2000000000000000000").unwrap(),
            data: Some("0xUpdated".to_string()),
            from: "0x".to_string(),
            to: Some("0x".to_string()),
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
            assert_eq!(data.gas_limit, 30000);
            assert_eq!(data.nonce, Some(2));
            assert_eq!(data.hash, Some("0xUpdated".to_string()));
            assert_eq!(data.data, Some("0xUpdated".to_string()));
        } else {
            panic!("Expected EVM network data");
        }
    }
}
