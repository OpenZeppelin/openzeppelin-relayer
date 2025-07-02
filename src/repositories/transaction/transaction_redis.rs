//! Improved Redis-backed implementation of the TransactionRepository with enhanced error handling.

use crate::models::{
    NetworkTransactionData, PaginationQuery, RepositoryError, TransactionRepoModel,
    TransactionStatus, TransactionUpdateRequest,
};
use crate::repositories::{PaginatedResult, Repository, TransactionRepository};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, RedisError};
use std::fmt;
use std::sync::Arc;

const TX_KEY_PREFIX: &str = "tx";
const TX_IDS_SET: &str = "tx:ids";
const RELAYER_INDEX_PREFIX: &str = "tx:relayer";
const RELAYER_STATUS_INDEX_PREFIX: &str = "tx:relayer:status";
const NONCE_INDEX_PREFIX: &str = "tx:relayer:nonce";

#[derive(Clone)]
pub struct RedisTransactionRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisTransactionRepository {
    pub fn new(
        connection_manager: Arc<ConnectionManager>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        if key_prefix.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Redis key prefix cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            client: connection_manager,
            key_prefix,
        })
    }

    fn tx_key(&self, id: &str) -> String {
        format!("{}:{}:{}", self.key_prefix, TX_KEY_PREFIX, id)
    }

    fn tx_ids_set(&self) -> String {
        format!("{}:{}", self.key_prefix, TX_IDS_SET)
    }

    fn relayer_key(&self, relayer_id: &str) -> String {
        format!(
            "{}:{}:{}",
            self.key_prefix, RELAYER_INDEX_PREFIX, relayer_id
        )
    }

    fn relayer_status_key(&self, relayer_id: &str, status: &TransactionStatus) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix,
            RELAYER_STATUS_INDEX_PREFIX,
            relayer_id,
            status.to_string()
        )
    }

    fn relayer_nonce_key(&self, relayer_id: &str, nonce: u64) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix, NONCE_INDEX_PREFIX, relayer_id, nonce
        )
    }

    /// Convert Redis errors to appropriate RepositoryError types
    fn map_redis_error(&self, error: RedisError, context: &str) -> RepositoryError {
        match error.kind() {
            redis::ErrorKind::IoError => {
                error!("Redis IO error in {}: {}", context, error);
                RepositoryError::ConnectionError(format!("Redis connection failed: {}", error))
            }
            redis::ErrorKind::AuthenticationFailed => {
                error!("Redis authentication failed in {}: {}", context, error);
                RepositoryError::PermissionDenied(format!("Redis authentication failed: {}", error))
            }
            redis::ErrorKind::TypeError => {
                error!("Redis type error in {}: {}", context, error);
                RepositoryError::InvalidData(format!("Redis data type error: {}", error))
            }
            redis::ErrorKind::ExecAbortError => {
                warn!("Redis transaction aborted in {}: {}", context, error);
                RepositoryError::TransactionFailure(format!("Redis transaction aborted: {}", error))
            }
            redis::ErrorKind::BusyLoadingError => {
                warn!("Redis busy loading in {}: {}", context, error);
                RepositoryError::ConnectionError(format!("Redis is loading: {}", error))
            }
            redis::ErrorKind::NoScriptError => {
                error!("Redis script error in {}: {}", context, error);
                RepositoryError::Other(format!("Redis script error: {}", error))
            }
            _ => {
                error!("Unexpected Redis error in {}: {}", context, error);
                RepositoryError::Other(format!("Redis error in {}: {}", context, error))
            }
        }
    }

    /// Batch fetch transactions by IDs
    async fn get_transactions_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No transaction IDs provided for batch fetch");
            return Ok(vec![]);
        }

        let mut conn = self.client.as_ref().clone();
        let keys: Vec<String> = ids.iter().map(|id| self.tx_key(id)).collect();

        debug!("Batch fetching {} transactions", ids.len());

        let values: Vec<Option<String>> = conn
            .mget(&keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_transactions"))?;

        let mut transactions = Vec::new();
        let mut failed_count = 0;

        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_transaction(&json, &ids[i]) {
                        Ok(tx) => transactions.push(tx),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize transaction {}: {}", ids[i], e);
                            // Continue processing other transactions
                        }
                    }
                }
                None => {
                    warn!("Transaction {} not found in batch fetch", ids[i]);
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} transactions in batch",
                failed_count,
                ids.len()
            );
        }

        debug!("Successfully fetched {} transactions", transactions.len());
        Ok(transactions)
    }

    /// Extract nonce with better error context
    fn extract_nonce(&self, network_data: &NetworkTransactionData) -> Option<u64> {
        match network_data.get_evm_transaction_data() {
            Ok(tx_data) => tx_data.nonce,
            Err(_) => {
                debug!("No EVM transaction data available for nonce extraction");
                None
            }
        }
    }

    /// Serialize transaction with detailed error context
    fn serialize_transaction(&self, tx: &TransactionRepoModel) -> Result<String, RepositoryError> {
        serde_json::to_string(tx).map_err(|e| {
            error!("Serialization failed for transaction {}: {}", tx.id, e);
            RepositoryError::InvalidData(format!(
                "Failed to serialize transaction {} (relayer: {}): {}",
                tx.id, tx.relayer_id, e
            ))
        })
    }

    /// Deserialize transaction with detailed error context
    fn deserialize_transaction(
        &self,
        json: &str,
        tx_id: &str,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        serde_json::from_str(json).map_err(|e| {
            error!("Deserialization failed for transaction {}: {}", tx_id, e);
            RepositoryError::InvalidData(format!(
                "Failed to deserialize transaction {}: {} (JSON length: {})",
                tx_id,
                e,
                json.len()
            ))
        })
    }

    /// Update indexes atomically with comprehensive error handling
    async fn update_indexes(
        &self,
        tx: &TransactionRepoModel,
        old_tx: Option<&TransactionRepoModel>,
    ) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let mut pipe = redis::pipe();
        pipe.atomic();

        debug!("Updating indexes for transaction {}", tx.id);

        // Always add to relayer index
        let relayer_key = self.relayer_key(&tx.relayer_id);
        pipe.sadd(&relayer_key, &tx.id);

        // Handle status index updates
        let new_status_key = self.relayer_status_key(&tx.relayer_id, &tx.status);
        pipe.sadd(&new_status_key, &tx.id);

        // Handle nonce index
        if let Some(nonce) = self.extract_nonce(&tx.network_data) {
            let nonce_key = self.relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.sadd(&nonce_key, &tx.id);
            debug!("Added nonce index for tx {} with nonce {}", tx.id, nonce);
        }

        // Remove old indexes if updating
        if let Some(old) = old_tx {
            if old.status != tx.status {
                let old_status_key = self.relayer_status_key(&old.relayer_id, &old.status);
                pipe.srem(&old_status_key, &tx.id);
                debug!(
                    "Removing old status index for tx {} (status: {} -> {})",
                    tx.id, old.status, tx.status
                );
            }

            // Handle nonce index cleanup
            if let Some(old_nonce) = self.extract_nonce(&old.network_data) {
                let new_nonce = self.extract_nonce(&tx.network_data);
                if Some(old_nonce) != new_nonce {
                    let old_nonce_key = self.relayer_nonce_key(&old.relayer_id, old_nonce);
                    pipe.srem(&old_nonce_key, &tx.id);
                    debug!(
                        "Removing old nonce index for tx {} (nonce: {} -> {:?})",
                        tx.id, old_nonce, new_nonce
                    );
                }
            }
        }

        // Execute all operations in a single pipeline
        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!(
                "Index update pipeline failed for transaction {}: {}",
                tx.id, e
            );
            self.map_redis_error(e, &format!("update_indexes_for_tx_{}", tx.id))
        })?;

        debug!("Successfully updated indexes for transaction {}", tx.id);
        Ok(())
    }

    /// Remove all indexes with error recovery
    async fn remove_all_indexes(&self, tx: &TransactionRepoModel) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let mut pipe = redis::pipe();
        pipe.atomic();

        debug!("Removing all indexes for transaction {}", tx.id);

        let relayer_key = self.relayer_key(&tx.relayer_id);
        let status_key = self.relayer_status_key(&tx.relayer_id, &tx.status);

        pipe.srem(&relayer_key, &tx.id);
        pipe.srem(&status_key, &tx.id);

        // Remove nonce index if exists
        if let Some(nonce) = self.extract_nonce(&tx.network_data) {
            let nonce_key = self.relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.srem(&nonce_key, &tx.id);
            debug!("Removing nonce index for tx {} with nonce {}", tx.id, nonce);
        }

        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!("Index removal failed for transaction {}: {}", tx.id, e);
            self.map_redis_error(e, &format!("remove_indexes_for_tx_{}", tx.id))
        })?;

        debug!("Successfully removed all indexes for transaction {}", tx.id);
        Ok(())
    }
}

impl fmt::Debug for RedisTransactionRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisTransactionRepository")
            .field("client", &"<ConnectionManager>")
            .field("key_prefix", &self.key_prefix)
            .finish()
    }
}

#[async_trait]
impl Repository<TransactionRepoModel, String> for RedisTransactionRepository {
    async fn create(
        &self,
        entity: TransactionRepoModel,
    ) -> Result<TransactionRepoModel, RepositoryError> {
        let key = self.tx_key(&entity.id);
        let mut conn = self.client.as_ref().clone();

        debug!("Creating transaction with ID: {}", entity.id);

        let value = self.serialize_transaction(&entity)?;

        // Check if transaction already exists
        let exists: bool = conn
            .exists(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "check_transaction_exists"))?;

        if exists {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Transaction with ID {} already exists",
                entity.id
            )));
        }

        // Use atomic pipeline for consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&key, &value);
        pipe.sadd(self.tx_ids_set(), &entity.id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "create_transaction"))?;

        // Update indexes separately to handle partial failures gracefully
        if let Err(e) = self.update_indexes(&entity, None).await {
            error!(
                "Failed to update indexes for new transaction {}: {}",
                entity.id, e
            );
            return Err(e);
        }

        debug!("Successfully created transaction {}", entity.id);
        Ok(entity)
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Transaction ID cannot be empty".to_string(),
            ));
        }

        let key = self.tx_key(&id);
        let mut conn = self.client.as_ref().clone();

        debug!("Fetching transaction with ID: {}", id);

        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_transaction_by_id"))?;

        match value {
            Some(json) => {
                let tx = self.deserialize_transaction(&json, &id)?;
                debug!("Successfully fetched transaction {}", id);
                Ok(tx)
            }
            None => {
                debug!("Transaction {} not found", id);
                Err(RepositoryError::NotFound(format!(
                    "Transaction with ID {} not found",
                    id
                )))
            }
        }
    }

    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();

        debug!("Fetching all transaction IDs");

        let ids: Vec<String> = conn
            .smembers(self.tx_ids_set())
            .await
            .map_err(|e| self.map_redis_error(e, "list_all_transaction_ids"))?;

        debug!("Found {} transaction IDs", ids.len());

        self.get_transactions_by_ids(&ids).await
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "per_page must be greater than 0".to_string(),
            ));
        }

        let mut conn = self.client.as_ref().clone();

        debug!(
            "Fetching paginated transactions (page: {}, per_page: {})",
            query.page, query.per_page
        );

        let total_ids: Vec<String> = conn
            .smembers(self.tx_ids_set())
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_transaction_ids"))?;

        let total = total_ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(total_ids.len());

        if start >= total_ids.len() {
            debug!(
                "Page {} is beyond available data (total: {})",
                query.page, total
            );
            return Ok(PaginatedResult {
                items: vec![],
                total,
                page: query.page,
                per_page: query.per_page,
            });
        }

        let page_ids = &total_ids[start..end];
        let items = self.get_transactions_by_ids(page_ids).await?;

        debug!(
            "Successfully fetched {} transactions for page {}",
            items.len(),
            query.page
        );

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
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Transaction ID cannot be empty".to_string(),
            ));
        }

        debug!("Updating transaction with ID: {}", id);

        // Get the old transaction for index cleanup
        let old_tx = self.get_by_id(id.clone()).await?;

        let key = self.tx_key(&id);
        let mut conn = self.client.as_ref().clone();

        let value = self.serialize_transaction(&entity)?;

        // Update transaction
        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| self.map_redis_error(e, "update_transaction"))?;

        // Update indexes
        self.update_indexes(&entity, Some(&old_tx)).await?;

        debug!("Successfully updated transaction {}", id);
        Ok(entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Transaction ID cannot be empty".to_string(),
            ));
        }

        debug!("Deleting transaction with ID: {}", id);

        // Get transaction first for index cleanup
        let tx = self.get_by_id(id.clone()).await?;

        let key = self.tx_key(&id);
        let mut conn = self.client.as_ref().clone();

        // Use atomic pipeline
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.srem(self.tx_ids_set(), &id);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_transaction"))?;

        // Remove indexes (log errors but don't fail the delete)
        if let Err(e) = self.remove_all_indexes(&tx).await {
            error!(
                "Failed to remove indexes for deleted transaction {}: {}",
                id, e
            );
            // Could consider this a warning rather than an error since the main data is deleted
        }

        debug!("Successfully deleted transaction {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();

        debug!("Counting transactions");

        let count: usize = conn
            .scard(self.tx_ids_set())
            .await
            .map_err(|e| self.map_redis_error(e, "count_transactions"))?;

        debug!("Transaction count: {}", count);
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
        let relayer_key = self.relayer_key(relayer_id);
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
            let status_key = self.relayer_status_key(relayer_id, status);
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
        let nonce_key = self.relayer_nonce_key(relayer_id, nonce);

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
        let key = self.tx_key(&tx_id);
        let mut conn = self.client.as_ref().clone();

        let value =
            serde_json::to_string(&tx).map_err(|e| RepositoryError::Other(e.to_string()))?;

        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| RepositoryError::Other(e.to_string()))?;

        self.update_indexes(&tx, Some(&old_tx)).await?;
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
