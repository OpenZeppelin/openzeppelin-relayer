//! Improved Redis-backed implementation of the TransactionRepository.

use crate::models::{
    NetworkTransactionData, PaginationQuery, RepositoryError, TransactionRepoModel,
    TransactionStatus, TransactionUpdateRequest,
};
use crate::repositories::{PaginatedResult, Repository, TransactionRepository};
use async_trait::async_trait;
use log::{error, warn};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const TX_KEY_PREFIX: &str = "tx:";
const TX_IDS_SET: &str = "tx:ids";
const RELAYER_INDEX_PREFIX: &str = "tx:relayer:";
const RELAYER_STATUS_INDEX_PREFIX: &str = "tx:relayer:status:";
const NONCE_INDEX_PREFIX: &str = "tx:relayer:nonce:";

#[derive(Clone)]
pub struct RedisTransactionRepository {
    pub client: Arc<ConnectionManager>,
}

impl RedisTransactionRepository {
    pub fn new(connection_manager: Arc<ConnectionManager>) -> Result<Self, RepositoryError> {
        Ok(Self {
            client: connection_manager,
        })
    }

    fn tx_key(id: &str) -> String {
        format!("{}{}", TX_KEY_PREFIX, id)
    }

    fn relayer_key(relayer_id: &str) -> String {
        format!("{}{}", RELAYER_INDEX_PREFIX, relayer_id)
    }

    fn relayer_status_key(relayer_id: &str, status: &TransactionStatus) -> String {
        format!(
            "{}{}:{}",
            RELAYER_STATUS_INDEX_PREFIX,
            relayer_id,
            status.to_string()
        )
    }

    fn relayer_nonce_key(relayer_id: &str, nonce: u64) -> String {
        format!("{}{}:{}", NONCE_INDEX_PREFIX, relayer_id, nonce)
    }

    // Batch fetch transactions by IDs - more efficient than individual gets
    async fn get_transactions_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| Self::tx_key(id)).collect();

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        let mut transactions = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match serde_json::from_str::<TransactionRepoModel>(&json) {
                        Ok(tx) => transactions.push(tx),
                        Err(e) => {
                            error!("Failed to deserialize transaction {}: {}", ids[i], e);
                            // Continue processing other transactions instead of failing completely
                        }
                    }
                }
                None => {
                    warn!("Transaction {} not found in batch fetch", ids[i]);
                }
            }
        }

        Ok(transactions)
    }

    // Atomic index management
    async fn update_indexes_atomic(
        &self,
        tx: &TransactionRepoModel,
        old_tx: Option<&TransactionRepoModel>,
    ) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Always add to relayer index
        let relayer_key = Self::relayer_key(&tx.relayer_id);
        pipe.sadd(&relayer_key, &tx.id);

        // Handle status index updates
        let new_status_key = Self::relayer_status_key(&tx.relayer_id, &tx.status);
        pipe.sadd(&new_status_key, &tx.id);

        // Handle nonce index
        if let Some(nonce) = tx
            .network_data
            .get_evm_transaction_data()
            .ok()
            .and_then(|tx_data| tx_data.nonce)
        {
            let nonce_key = Self::relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.sadd(&nonce_key, &tx.id);
        }

        // Remove old indexes if updating
        if let Some(old) = old_tx {
            if old.status != tx.status {
                let old_status_key = Self::relayer_status_key(&old.relayer_id, &old.status);
                pipe.srem(&old_status_key, &tx.id);
            }

            // Handle nonce index cleanup
            if let Some(old_nonce) = old
                .network_data
                .get_evm_transaction_data()
                .ok()
                .and_then(|tx_data| tx_data.nonce)
            {
                let new_nonce = tx
                    .network_data
                    .get_evm_transaction_data()
                    .ok()
                    .and_then(|tx_data| tx_data.nonce);
                if Some(old_nonce) != new_nonce {
                    let old_nonce_key = Self::relayer_nonce_key(&old.relayer_id, old_nonce);
                    pipe.srem(&old_nonce_key, &tx.id);
                }
            }
        }

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        Ok(())
    }

    async fn remove_all_indexes(&self, tx: &TransactionRepoModel) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let mut pipe = redis::pipe();
        pipe.atomic();

        let relayer_key = Self::relayer_key(&tx.relayer_id);
        let status_key = Self::relayer_status_key(&tx.relayer_id, &tx.status);

        pipe.srem(&relayer_key, &tx.id);
        pipe.srem(&status_key, &tx.id);

        // Remove nonce index if exists
        if let Some(nonce) = tx
            .network_data
            .get_evm_transaction_data()
            .ok()
            .and_then(|tx_data| tx_data.nonce)
        {
            let nonce_key = Self::relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.srem(&nonce_key, &tx.id);
        }

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        Ok(())
    }
}

impl fmt::Debug for RedisTransactionRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisTransactionRepository")
            .field("client", &"<ConnectionManager>")
            .finish()
    }
}

#[async_trait]
impl Repository<TransactionRepoModel, String> for RedisTransactionRepository {
    async fn create(
        &self,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let key = Self::tx_key(&entity.id);
        let mut conn = self.client.as_ref().clone();

        let value =
            serde_json::to_string(&entity).map_err(|e| RepositoryError::Other(e.to_string()))?;

        // Use atomic pipeline for consistency
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Check existence and create atomically using SET NX
        let result: Result<String, redis::RedisError> = conn.set_nx(&key, &value).await;

        match result {
            Ok(_) => {
                // Transaction was created, now update indexes
                pipe.sadd(TX_IDS_SET, &entity.id);
                pipe.exec_async(&mut conn)
                    .await
                    .map_err(|e| RepositoryError::Other(e.to_string()))?;

                self.update_indexes_atomic(&entity, None).await?;
                Ok(entity)
            }
            Err(_) => Err(RepositoryError::ConstraintViolation(format!(
                "Transaction with ID {} already exists",
                entity.id
            ))),
        }
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        let key = Self::tx_key(&id);
        let mut conn = self.client.as_ref().clone();
        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        match value {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| RepositoryError::Other(e.to_string()))
            }
            None => Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
                id
            ))),
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let ids: Vec<String> = conn
            .smembers(TX_IDS_SET)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        self.get_transactions_by_ids(&ids).await
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let total_ids: Vec<String> = conn
            .smembers(TX_IDS_SET)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        let total = total_ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(total_ids.len());

        let page_ids = &total_ids[start..end];
        let items = self.get_transactions_by_ids(page_ids).await?;

        Ok(PaginatedResult {
            items,
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn update(
        &self,
        id: String,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        // Get the old transaction for index cleanup
        let old_tx = self.get_by_id(id.clone()).await?;

        let key = Self::tx_key(&id);
        let mut conn = self.client.as_ref().clone();

        let value =
            serde_json::to_string(&entity).map_err(|e| RepositoryError::Other(e.to_string()))?;

        // Update atomically
        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        self.update_indexes_atomic(&entity, Some(&old_tx)).await?;
        Ok(entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        // Get transaction first for index cleanup
        let tx = self.get_by_id(id.clone()).await?;

        let key = Self::tx_key(&id);
        let mut conn = self.client.as_ref().clone();

        // Use atomic pipeline
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.srem(TX_IDS_SET, &id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        self.remove_all_indexes(&tx).await?;
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let count: usize = conn
            .scard(TX_IDS_SET)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        Ok(count)
    }
}

#[async_trait]
impl TransactionRepository for RedisTransactionRepository {
    async fn find_by_relayer_id(
        &self,
        relayer_id: &str,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_key = Self::relayer_key(relayer_id);
        let ids: Vec<String> = conn
            .smembers(relayer_key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        let total = ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(ids.len());

        let page_ids = &ids[start..end];
        let items = self.get_transactions_by_ids(page_ids).await?;

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
        let mut conn = self.client.as_ref().clone();
        let mut all_ids = Vec::new();

        // Collect IDs from all status sets
        for status in statuses {
            let status_key = Self::relayer_status_key(relayer_id, status);
            let ids: Vec<String> = conn
                .smembers(status_key)
                .await
                .map_err(|e| RepositoryError::Other(e.to_string()))?;
            all_ids.extend(ids);
        }

        // Remove duplicates and batch fetch
        all_ids.sort();
        all_ids.dedup();

        self.get_transactions_by_ids(&all_ids).await
    }

    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let nonce_key = Self::relayer_nonce_key(relayer_id, nonce);

        // Get all transaction IDs with this nonce for this relayer
        let tx_ids: Vec<String> = conn
            .smembers(nonce_key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        match tx_ids.len() {
            0 => Ok(None),
            1 => {
                // Single transaction found - the normal case
                match self.get_by_id(tx_ids[0].clone()).await {
                    Ok(tx) => Ok(Some(tx)),
                    Err(RepositoryError::NotFound(_)) => {
                        // Transaction was deleted but index wasn't cleaned up
                        warn!(
                            "Stale nonce index found for relayer {} nonce {}",
                            relayer_id, nonce
                        );
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
            _ => {
                // Multiple transactions with same nonce - this might be valid in some scenarios
                // (e.g., failed transaction retries) but let's return the first valid one
                warn!(
                    "Multiple transactions found for relayer {} nonce {}: {:?}",
                    relayer_id, nonce, tx_ids
                );

                for tx_id in tx_ids {
                    match self.get_by_id(tx_id).await {
                        Ok(tx) => return Ok(Some(tx)),
                        Err(RepositoryError::NotFound(_)) => continue,
                        Err(e) => return Err(e),
                    }
                }
                Ok(None)
            }
        }
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
        // Get current transaction
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        let old_tx = tx.clone(); // Keep copy for index updates

        // Apply partial updates
        if let Some(status) = update.status {
            tx.status = status;
        }
        if let Some(status_reason) = update.status_reason {
            tx.status_reason = Some(status_reason);
        }
        if let Some(sent_at) = update.sent_at {
            tx.sent_at = Some(sent_at);
        }
        if let Some(confirmed_at) = update.confirmed_at {
            tx.confirmed_at = Some(confirmed_at);
        }
        if let Some(network_data) = update.network_data {
            tx.network_data = network_data;
        }
        if let Some(hashes) = update.hashes {
            tx.hashes = hashes;
        }
        if let Some(is_canceled) = update.is_canceled {
            tx.is_canceled = Some(is_canceled);
        }

        // Update transaction and indexes atomically
        let key = Self::tx_key(&tx_id);
        let mut conn = self.client.as_ref().clone();

        let value =
            serde_json::to_string(&tx).map_err(|e| RepositoryError::Other(e.to_string()))?;

        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        self.update_indexes_atomic(&tx, Some(&old_tx)).await?;
        Ok(tx)
    }

    async fn update_network_data(
        &self,
        tx_id: String,
        network_data: NetworkTransactionData,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let update = TransactionUpdateRequest {
            network_data: Some(network_data),
            ..Default::default()
        };
        self.partial_update(tx_id, update).await
    }

    async fn set_sent_at(
        &self,
        tx_id: String,
        sent_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let update = TransactionUpdateRequest {
            sent_at: Some(sent_at),
            ..Default::default()
        };
        self.partial_update(tx_id, update).await
    }

    async fn set_confirmed_at(
        &self,
        tx_id: String,
        confirmed_at: String,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let update = TransactionUpdateRequest {
            confirmed_at: Some(confirmed_at),
            ..Default::default()
        };
        self.partial_update(tx_id, update).await
    }
}
