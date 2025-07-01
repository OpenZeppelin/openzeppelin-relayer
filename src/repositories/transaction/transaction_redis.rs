//! Redis-backed implementation of the TransactionRepository.

use crate::models::{
    NetworkTransactionData, PaginationQuery, RepositoryError, TransactionRepoModel,
    TransactionStatus, TransactionUpdateRequest,
};
use crate::repositories::{PaginatedResult, Repository, TransactionRepository};
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::sync::Arc;

const TX_KEY_PREFIX: &str = "transaction:";
const TX_IDS_SET: &str = "transaction:ids";
const RELAYER_INDEX_PREFIX: &str = "transaction:relayer:";
const STATUS_INDEX_PREFIX: &str = "transaction:status:";

#[derive(Clone)]
pub struct RedisTransactionRepository {
    pub client: Arc<ConnectionManager>,
}

impl RedisTransactionRepository {
    pub async fn new(connection_manager: Arc<ConnectionManager>) -> Result<Self, RepositoryError> {
        Ok(Self {
            client: connection_manager,
        })
    }

    fn tx_key(id: &str) -> String {
        format!("{}{}", TX_KEY_PREFIX, id)
    }

    async fn index_transaction(&self, tx: &TransactionRepoModel) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_key = format!("{}{}", RELAYER_INDEX_PREFIX, tx.relayer_id);
        let status_key = format!("{}{}", STATUS_INDEX_PREFIX, tx.status.to_string());
        let _: () = conn
            .sadd(relayer_key, &tx.id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        let _: () = conn
            .sadd(status_key, &tx.id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        Ok(())
    }

    async fn remove_from_indexes(&self, tx: &TransactionRepoModel) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_key = format!("{}{}", RELAYER_INDEX_PREFIX, tx.relayer_id);
        let status_key = format!("{}{}", STATUS_INDEX_PREFIX, tx.status.to_string());
        let _: () = conn
            .srem(relayer_key, &tx.id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        let _: () = conn
            .srem(status_key, &tx.id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        Ok(())
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
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        if exists {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Transaction with ID {} already exists",
                entity.id
            )));
        }
        let value =
            serde_json::to_string(&entity).map_err(|e| RepositoryError::Other(e.to_string()))?;
        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        let _: () = conn
            .sadd(TX_IDS_SET, &entity.id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        self.index_transaction(&entity).await?;
        Ok(entity)
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
        let mut txs = Vec::new();
        for id in ids {
            if let Ok(tx) = self.get_by_id(id).await {
                txs.push(tx);
            }
        }
        Ok(txs)
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let ids: Vec<String> = conn
            .smembers(TX_IDS_SET)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        let total = ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(ids.len());
        let mut items = Vec::new();
        for id in ids[start..end].iter() {
            if let Ok(tx) = self.get_by_id(id.clone()).await {
                items.push(tx);
            }
        }
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
        let key = Self::tx_key(&id);
        let mut conn = self.client.as_ref().clone();
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        if !exists {
            return Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
                id
            )));
        }
        let value =
            serde_json::to_string(&entity).map_err(|e| RepositoryError::Other(e.to_string()))?;
        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        self.remove_from_indexes(&entity).await?;
        self.index_transaction(&entity).await?;
        Ok(entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let key = Self::tx_key(&id);
        let mut conn = self.client.as_ref().clone();
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        if !exists {
            return Err(RepositoryError::NotFound(format!(
                "Transaction with ID {} not found",
                id
            )));
        }
        let _: () = conn
            .del(&key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        let _: () = conn
            .srem(TX_IDS_SET, &id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
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
        let relayer_key = format!("{}{}", RELAYER_INDEX_PREFIX, relayer_id);
        let ids: Vec<String> = conn
            .smembers(relayer_key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        let total = ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(ids.len());
        let mut items = Vec::new();
        for id in ids[start..end].iter() {
            if let Ok(tx) = self.get_by_id(id.clone()).await {
                items.push(tx);
            }
        }
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
        let mut result = Vec::new();
        for status in statuses {
            let status_key = format!("{}{}", STATUS_INDEX_PREFIX, status.to_string());
            let ids: Vec<String> = conn
                .smembers(status_key)
                .await
                .map_err(|e| RepositoryError::Other(e.to_string()))?;
            for id in ids {
                if let Ok(tx) = self.get_by_id(id).await {
                    if tx.relayer_id == relayer_id {
                        result.push(tx);
                    }
                }
            }
        }
        Ok(result)
    }

    async fn find_by_nonce(
        &self,
        _relayer_id: &str,
        _nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        // TODO: Implement efficient index for nonce
        unimplemented!("Implement find_by_nonce for RedisTransactionRepository");
    }

    async fn update_status(
        &self,
        tx_id: String,
        status: TransactionStatus,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let mut tx = self.get_by_id(tx_id.clone()).await?;
        let old_status = tx.status.clone();
        tx.status = status.clone();
        self.update(tx_id.clone(), tx.clone()).await?;
        let mut conn = self.client.as_ref().clone();
        let old_status_key = format!("{}{}", STATUS_INDEX_PREFIX, old_status.to_string());
        let new_status_key = format!("{}{}", STATUS_INDEX_PREFIX, status.to_string());
        conn.srem(old_status_key, &tx_id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        conn.sadd(new_status_key, &tx_id)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;
        Ok(tx)
    }

    async fn partial_update(
        &self,
        tx_id: String,
        update: TransactionUpdateRequest,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let key = Self::tx_key(&tx_id);
        let mut conn = self.client.as_ref().clone();

        // Use Redis transaction for atomicity
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Get current transaction
        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        let mut tx: TransactionRepoModel = match value {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| RepositoryError::Other(e.to_string()))?
            }
            None => {
                return Err(RepositoryError::NotFound(format!(
                    "Transaction with ID {} not found",
                    tx_id
                )))
            }
        };

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

        // Serialize and update atomically
        let value =
            serde_json::to_string(&tx).map_err(|e| RepositoryError::Other(e.to_string()))?;

        pipe.set(&key, value);
        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

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
