//! Redis-backed implementation of the TransactionRepository.

use crate::models::{
    NetworkTransactionData, PaginationQuery, RepositoryError, TransactionRepoModel,
    TransactionStatus, TransactionUpdateRequest,
};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{
    BatchRetrievalResult, PaginatedResult, Repository, TransactionRepository,
};
use async_trait::async_trait;
use log::{debug, error, warn};
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use std::fmt;
use std::sync::Arc;

const RELAYER_PREFIX: &str = "relayer";
const TX_PREFIX: &str = "tx";
const STATUS_PREFIX: &str = "status";
const NONCE_PREFIX: &str = "nonce";
const TX_TO_RELAYER_PREFIX: &str = "tx_to_relayer";
const RELAYER_LIST_KEY: &str = "relayer_list";

#[derive(Clone)]
pub struct RedisTransactionRepository {
    pub client: Arc<ConnectionManager>,
    pub key_prefix: String,
}

impl RedisRepository for RedisTransactionRepository {}

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

    /// Generate key for transaction data: relayer:{relayer_id}:tx:{tx_id}
    fn tx_key(&self, relayer_id: &str, tx_id: &str) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX, tx_id
        )
    }

    /// Generate key for reverse lookup: tx_to_relayer:{tx_id}
    fn tx_to_relayer_key(&self, tx_id: &str) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, TX_TO_RELAYER_PREFIX, tx_id
        )
    }

    /// Generate key for relayer status index: relayer:{relayer_id}:status:{status}
    fn relayer_status_key(&self, relayer_id: &str, status: &TransactionStatus) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, relayer_id, STATUS_PREFIX, status
        )
    }

    /// Generate key for relayer nonce index: relayer:{relayer_id}:nonce:{nonce}
    fn relayer_nonce_key(&self, relayer_id: &str, nonce: u64) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, relayer_id, NONCE_PREFIX, nonce
        )
    }

    /// Generate key for relayer list: relayer_list (set of all relayer IDs)
    fn relayer_list_key(&self) -> String {
        format!("{}:{}", self.key_prefix, RELAYER_LIST_KEY)
    }

    /// Batch fetch transactions by IDs using reverse lookup
    async fn get_transactions_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<TransactionRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("No transaction IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self.client.as_ref().clone();

        let reverse_keys: Vec<String> = ids.iter().map(|id| self.tx_to_relayer_key(id)).collect();

        debug!("Fetching relayer IDs for {} transactions", ids.len());

        let relayer_ids: Vec<Option<String>> = conn
            .mget(&reverse_keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_relayer_ids"))?;

        let mut tx_keys = Vec::new();
        let mut valid_ids = Vec::new();
        let mut failed_ids = Vec::new();
        for (i, relayer_id) in relayer_ids.into_iter().enumerate() {
            match relayer_id {
                Some(relayer_id) => {
                    tx_keys.push(self.tx_key(&relayer_id, &ids[i]));
                    valid_ids.push(ids[i].clone());
                }
                None => {
                    warn!("No relayer found for transaction {}", ids[i]);
                    failed_ids.push(ids[i].clone());
                }
            }
        }

        if tx_keys.is_empty() {
            debug!("No valid transactions found for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids,
            });
        }

        debug!("Batch fetching {} transaction data", tx_keys.len());

        let values: Vec<Option<String>> = conn
            .mget(&tx_keys)
            .await
            .map_err(|e| self.map_redis_error(e, "batch_fetch_transactions"))?;

        let mut transactions = Vec::new();
        let mut failed_count = 0;
        let mut failed_ids = Vec::new();
        for (i, value) in values.into_iter().enumerate() {
            match value {
                Some(json) => {
                    match self.deserialize_entity::<TransactionRepoModel>(
                        &json,
                        &valid_ids[i],
                        "transaction",
                    ) {
                        Ok(tx) => transactions.push(tx),
                        Err(e) => {
                            failed_count += 1;
                            error!("Failed to deserialize transaction {}: {}", valid_ids[i], e);
                            // Continue processing other transactions
                        }
                    }
                }
                None => {
                    warn!("Transaction {} not found in batch fetch", valid_ids[i]);
                    failed_ids.push(valid_ids[i].clone());
                }
            }
        }

        if failed_count > 0 {
            warn!(
                "Failed to deserialize {} out of {} transactions in batch",
                failed_count,
                valid_ids.len()
            );
        }

        debug!("Successfully fetched {} transactions", transactions.len());
        Ok(BatchRetrievalResult {
            results: transactions,
            failed_ids,
        })
    }

    /// Extract nonce from EVM transaction data
    fn extract_nonce(&self, network_data: &NetworkTransactionData) -> Option<u64> {
        match network_data.get_evm_transaction_data() {
            Ok(tx_data) => tx_data.nonce,
            Err(_) => {
                debug!("No EVM transaction data available for nonce extraction");
                None
            }
        }
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

        // Add relayer to the global relayer list
        let relayer_list_key = self.relayer_list_key();
        pipe.sadd(&relayer_list_key, &tx.relayer_id);

        // Handle status index updates
        let new_status_key = self.relayer_status_key(&tx.relayer_id, &tx.status);
        pipe.sadd(&new_status_key, &tx.id);

        if let Some(nonce) = self.extract_nonce(&tx.network_data) {
            let nonce_key = self.relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.set(&nonce_key, &tx.id);
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
                    pipe.del(&old_nonce_key);
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

        // Remove from status index
        let status_key = self.relayer_status_key(&tx.relayer_id, &tx.status);
        pipe.srem(&status_key, &tx.id);

        // Remove nonce index if exists
        if let Some(nonce) = self.extract_nonce(&tx.network_data) {
            let nonce_key = self.relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.del(&nonce_key);
            debug!("Removing nonce index for tx {} with nonce {}", tx.id, nonce);
        }

        // Remove reverse lookup
        let reverse_key = self.tx_to_relayer_key(&tx.id);
        pipe.del(&reverse_key);

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
        if entity.id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Transaction ID cannot be empty".to_string(),
            ));
        }

        let key = self.tx_key(&entity.relayer_id, &entity.id);
        let reverse_key = self.tx_to_relayer_key(&entity.id);
        let mut conn = self.client.as_ref().clone();

        debug!("Creating transaction with ID: {}", entity.id);

        let value = self.serialize_entity(&entity, |t| &t.id, "transaction")?;

        // Check if transaction already exists by checking reverse lookup
        let existing: Option<String> = conn
            .get(&reverse_key)
            .await
            .map_err(|e| self.map_redis_error(e, "create_transaction_check"))?;

        if existing.is_some() {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Transaction with ID {} already exists",
                entity.id
            )));
        }

        // Use atomic pipeline for consistency
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.set(&key, &value);
        pipe.set(&reverse_key, &entity.relayer_id);

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

        let mut conn = self.client.as_ref().clone();

        debug!("Fetching transaction with ID: {}", id);

        let reverse_key = self.tx_to_relayer_key(&id);
        let relayer_id: Option<String> = conn
            .get(&reverse_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_transaction_reverse_lookup"))?;

        let relayer_id = match relayer_id {
            Some(relayer_id) => relayer_id,
            None => {
                debug!("Transaction {} not found (no reverse lookup)", id);
                return Err(RepositoryError::NotFound(format!(
                    "Transaction with ID {} not found",
                    id
                )));
            }
        };

        let key = self.tx_key(&relayer_id, &id);
        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_transaction_by_id"))?;

        match value {
            Some(json) => {
                let tx =
                    self.deserialize_entity::<TransactionRepoModel>(&json, &id, "transaction")?;
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

        // Get all relayer IDs
        let relayer_list_key = self.relayer_list_key();
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_all_relayer_ids"))?;

        debug!("Found {} relayers", relayer_ids.len());

        // Collect all transaction IDs from all relayers
        let mut all_tx_ids = Vec::new();
        for relayer_id in relayer_ids {
            let pattern = format!(
                "{}:{}:{}:{}:*",
                self.key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX
            );
            let mut cursor = 0;
            loop {
                let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .cursor_arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| self.map_redis_error(e, "list_all_scan_keys"))?;

                // Extract transaction IDs from keys
                for key in keys {
                    if let Some(tx_id) = key.split(':').next_back() {
                        all_tx_ids.push(tx_id.to_string());
                    }
                }

                cursor = next_cursor;
                if cursor == 0 {
                    break;
                }
            }
        }

        debug!("Found {} transaction IDs", all_tx_ids.len());

        let transactions = self.get_transactions_by_ids(&all_tx_ids).await?;
        Ok(transactions.results)
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

        // Get all relayer IDs
        let relayer_list_key = self.relayer_list_key();
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_relayer_ids"))?;

        // Collect all transaction IDs from all relayers
        let mut all_tx_ids = Vec::new();
        for relayer_id in relayer_ids {
            let pattern = format!(
                "{}:{}:{}:{}:*",
                self.key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX
            );
            let mut cursor = 0;
            loop {
                let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .cursor_arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| self.map_redis_error(e, "list_paginated_scan_keys"))?;

                // Extract transaction IDs from keys
                for key in keys {
                    if let Some(tx_id) = key.split(':').next_back() {
                        all_tx_ids.push(tx_id.to_string());
                    }
                }

                cursor = next_cursor;
                if cursor == 0 {
                    break;
                }
            }
        }

        let total = all_tx_ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_tx_ids.len());

        if start >= all_tx_ids.len() {
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

        let page_ids = &all_tx_ids[start..end];
        let items = self.get_transactions_by_ids(page_ids).await?;

        debug!(
            "Successfully fetched {} transactions for page {}",
            items.results.len(),
            query.page
        );

        Ok(PaginatedResult {
            items: items.results.clone(),
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

        let key = self.tx_key(&entity.relayer_id, &id);
        let mut conn = self.client.as_ref().clone();

        let value = self.serialize_entity(&entity, |t| &t.id, "transaction")?;

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

        let key = self.tx_key(&tx.relayer_id, &id);
        let reverse_key = self.tx_to_relayer_key(&id);
        let mut conn = self.client.as_ref().clone();

        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.del(&reverse_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_transaction"))?;

        // Remove indexes (log errors but don't fail the delete)
        if let Err(e) = self.remove_all_indexes(&tx).await {
            error!(
                "Failed to remove indexes for deleted transaction {}: {}",
                id, e
            );
        }

        debug!("Successfully deleted transaction {}", id);
        Ok(())
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self.client.as_ref().clone();

        debug!("Counting transactions");

        // Get all relayer IDs
        let relayer_list_key = self.relayer_list_key();
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_relayer_ids"))?;

        // Count transactions across all relayers
        let mut total_count = 0;
        for relayer_id in relayer_ids {
            let pattern = format!(
                "{}:{}:{}:{}:*",
                self.key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX
            );
            let mut cursor = 0;
            loop {
                let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .cursor_arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| self.map_redis_error(e, "count_scan_keys"))?;

                total_count += keys.len();

                cursor = next_cursor;
                if cursor == 0 {
                    break;
                }
            }
        }

        debug!("Transaction count: {}", total_count);
        Ok(total_count)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_list_key = self.relayer_list_key();

        debug!("Checking if transaction entries exist");

        let exists: bool = conn
            .exists(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "has_entries_check"))?;

        debug!("Transaction entries exist: {}", exists);
        Ok(exists)
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let relayer_list_key = self.relayer_list_key();

        debug!("Dropping all transaction entries");

        // Get all relayer IDs first
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_relayer_ids"))?;

        if relayer_ids.is_empty() {
            debug!("No transaction entries to drop");
            return Ok(());
        }

        // Use pipeline for atomic operations
        let mut pipe = redis::pipe();
        pipe.atomic();

        // Delete all transactions and their indexes for each relayer
        for relayer_id in &relayer_ids {
            // Get all transaction IDs for this relayer
            let pattern = format!(
                "{}:{}:{}:{}:*",
                self.key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX
            );
            let mut cursor = 0;
            let mut tx_ids = Vec::new();

            loop {
                let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .cursor_arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| self.map_redis_error(e, "drop_all_entries_scan"))?;

                // Extract transaction IDs from keys and delete keys
                for key in keys {
                    pipe.del(&key);
                    if let Some(tx_id) = key.split(':').next_back() {
                        tx_ids.push(tx_id.to_string());
                    }
                }

                cursor = next_cursor;
                if cursor == 0 {
                    break;
                }
            }

            // Delete reverse lookup keys and indexes
            for tx_id in tx_ids {
                let reverse_key = self.tx_to_relayer_key(&tx_id);
                pipe.del(&reverse_key);

                // Delete status indexes (we can't know the specific status, so we'll clean up known ones)
                for status in &[
                    TransactionStatus::Pending,
                    TransactionStatus::Sent,
                    TransactionStatus::Confirmed,
                    TransactionStatus::Failed,
                    TransactionStatus::Canceled,
                ] {
                    let status_key = self.relayer_status_key(relayer_id, status);
                    pipe.srem(&status_key, &tx_id);
                }
            }
        }

        // Delete the relayer list key
        pipe.del(&relayer_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!(
            "Dropped all transaction entries for {} relayers",
            relayer_ids.len()
        );
        Ok(())
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

        // Scan for all transaction keys for this relayer
        let pattern = format!(
            "{}:{}:{}:{}:*",
            self.key_prefix, RELAYER_PREFIX, relayer_id, TX_PREFIX
        );
        let mut all_tx_ids = Vec::new();
        let mut cursor = 0;

        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .cursor_arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .query_async(&mut conn)
                .await
                .map_err(|e| self.map_redis_error(e, "find_by_relayer_id_scan"))?;

            // Extract transaction IDs from keys
            for key in keys {
                if let Some(tx_id) = key.split(':').next_back() {
                    all_tx_ids.push(tx_id.to_string());
                }
            }

            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }

        let total = all_tx_ids.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_tx_ids.len());

        let page_ids = &all_tx_ids[start..end];
        let items = self.get_transactions_by_ids(page_ids).await?;

        Ok(PaginatedResult {
            items: items.results.clone(),
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
                .map_err(|e| self.map_redis_error(e, "find_by_status"))?;

            all_ids.extend(ids);
        }

        // Remove duplicates and batch fetch
        all_ids.sort();
        all_ids.dedup();

        let transactions = self.get_transactions_by_ids(&all_ids).await?;
        Ok(transactions.results)
    }

    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        let mut conn = self.client.as_ref().clone();
        let nonce_key = self.relayer_nonce_key(relayer_id, nonce);

        // Get transaction ID with this nonce for this relayer (should be single value)
        let tx_id: Option<String> = conn
            .get(nonce_key)
            .await
            .map_err(|e| self.map_redis_error(e, "find_by_nonce"))?;

        match tx_id {
            Some(tx_id) => {
                match self.get_by_id(tx_id.clone()).await {
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
            None => Ok(None),
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

        // Apply partial updates using the model's business logic
        tx.apply_partial_update(update);

        // Update transaction and indexes atomically
        let key = self.tx_key(&tx.relayer_id, &tx_id);
        let mut conn = self.client.as_ref().clone();

        let value = self.serialize_entity(&tx, |t| &t.id, "transaction")?;

        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| self.map_redis_error(e, "partial_update"))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{evm::Speed, EvmTransactionData, NetworkType};
    use alloy::primitives::U256;
    use lazy_static::lazy_static;
    use redis::Client;
    use std::str::FromStr;
    use tokio;
    use uuid::Uuid;

    use tokio::sync::Mutex;

    // Use a mutex to ensure tests don't run in parallel when modifying env vars
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

    fn create_test_transaction_with_relayer(id: &str, relayer_id: &str) -> TransactionRepoModel {
        let mut tx = create_test_transaction(id);
        tx.relayer_id = relayer_id.to_string();
        tx
    }

    fn create_test_transaction_with_status(
        id: &str,
        relayer_id: &str,
        status: TransactionStatus,
    ) -> TransactionRepoModel {
        let mut tx = create_test_transaction_with_relayer(id, relayer_id);
        tx.status = status;
        tx
    }

    fn create_test_transaction_with_nonce(
        id: &str,
        nonce: u64,
        relayer_id: &str,
    ) -> TransactionRepoModel {
        let mut tx = create_test_transaction_with_relayer(id, relayer_id);
        if let NetworkTransactionData::Evm(ref mut evm_data) = tx.network_data {
            evm_data.nonce = Some(nonce);
        }
        tx
    }

    async fn setup_test_repo() -> RedisTransactionRepository {
        // Use a mock Redis URL - in real integration tests, this would connect to a test Redis instance
        let redis_url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

        let client = Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        let random_id = Uuid::new_v4().to_string();
        let key_prefix = format!("test_prefix:{}", random_id);

        RedisTransactionRepository::new(Arc::new(connection_manager), key_prefix)
            .expect("Failed to create RedisTransactionRepository")
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_creation() {
        let repo = setup_test_repo().await;
        assert!(repo.key_prefix.contains("test_prefix"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_new_repository_empty_prefix_fails() {
        let redis_url = std::env::var("REDIS_TEST_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = Client::open(redis_url).expect("Failed to create Redis client");
        let connection_manager = ConnectionManager::new(client)
            .await
            .expect("Failed to create connection manager");

        let result = RedisTransactionRepository::new(Arc::new(connection_manager), "".to_string());
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_key_generation() {
        let repo = setup_test_repo().await;

        assert!(repo
            .tx_key("relayer-1", "test-id")
            .contains(":relayer:relayer-1:tx:test-id"));
        assert!(repo
            .tx_to_relayer_key("test-id")
            .contains(":relayer:tx_to_relayer:test-id"));
        assert!(repo.relayer_list_key().contains(":relayer_list"));
        assert!(repo
            .relayer_status_key("relayer-1", &TransactionStatus::Pending)
            .contains(":relayer:relayer-1:status:Pending"));
        assert!(repo
            .relayer_nonce_key("relayer-1", 42)
            .contains(":relayer:relayer-1:nonce:42"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_serialize_deserialize_transaction() {
        let repo = setup_test_repo().await;
        let tx = create_test_transaction("test-1");

        let serialized = repo
            .serialize_entity(&tx, |t| &t.id, "transaction")
            .expect("Serialization should succeed");
        let deserialized: TransactionRepoModel = repo
            .deserialize_entity(&serialized, "test-1", "transaction")
            .expect("Deserialization should succeed");

        assert_eq!(tx.id, deserialized.id);
        assert_eq!(tx.relayer_id, deserialized.relayer_id);
        assert_eq!(tx.status, deserialized.status);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_extract_nonce() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let relayer_id = Uuid::new_v4().to_string();
        let tx_with_nonce = create_test_transaction_with_nonce(&random_id, 42, &relayer_id);

        let nonce = repo.extract_nonce(&tx_with_nonce.network_data);
        assert_eq!(nonce, Some(42));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_create_transaction() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        let result = repo.create(tx.clone()).await.unwrap();
        assert_eq!(result.id, tx.id);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_transaction() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx.clone()).await.unwrap();
        let stored = repo.get_by_id(random_id.to_string()).await.unwrap();
        assert_eq!(stored.id, tx.id);
        assert_eq!(stored.relayer_id, tx.relayer_id);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_transaction() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let mut tx = create_test_transaction(&random_id);

        repo.create(tx.clone()).await.unwrap();
        tx.status = TransactionStatus::Confirmed;

        let updated = repo.update(random_id.to_string(), tx).await.unwrap();
        assert!(matches!(updated.status, TransactionStatus::Confirmed));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_transaction() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx).await.unwrap();
        repo.delete_by_id(random_id.to_string()).await.unwrap();

        let result = repo.get_by_id(random_id.to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_all_transactions() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_id2 = Uuid::new_v4().to_string();

        let tx1 = create_test_transaction(&random_id);
        let tx2 = create_test_transaction(&random_id2);

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();

        let transactions = repo.list_all().await.unwrap();
        assert!(transactions.len() >= 2);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_count_transactions() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        let count = repo.count().await.unwrap();
        repo.create(tx).await.unwrap();
        assert!(repo.count().await.unwrap() > count);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_get_nonexistent_transaction() {
        let repo = setup_test_repo().await;
        let result = repo.get_by_id("nonexistent".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_duplicate_transaction_creation() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();

        let tx = create_test_transaction(&random_id);

        repo.create(tx.clone()).await.unwrap();
        let result = repo.create(tx).await;

        assert!(matches!(
            result,
            Err(RepositoryError::ConstraintViolation(_))
        ));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_nonexistent_transaction() {
        let repo = setup_test_repo().await;
        let tx = create_test_transaction("test-1");

        let result = repo.update("nonexistent".to_string(), tx).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_list_paginated() {
        let repo = setup_test_repo().await;

        // Create multiple transactions
        for _ in 1..=10 {
            let random_id = Uuid::new_v4().to_string();
            let tx = create_test_transaction(&random_id);
            repo.create(tx).await.unwrap();
        }

        // Test first page with 3 items per page
        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 3);
        assert!(result.total >= 10);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 3);

        // Test empty page (beyond total items)
        let query = PaginationQuery {
            page: 1000,
            per_page: 3,
        };
        let result = repo.list_paginated(query).await.unwrap();
        assert_eq!(result.items.len(), 0);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_find_by_relayer_id() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_id2 = Uuid::new_v4().to_string();
        let random_id3 = Uuid::new_v4().to_string();

        let tx1 = create_test_transaction_with_relayer(&random_id, "relayer-1");
        let tx2 = create_test_transaction_with_relayer(&random_id2, "relayer-1");
        let tx3 = create_test_transaction_with_relayer(&random_id3, "relayer-2");

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
        assert!(result.total >= 2);
        assert!(result.items.len() >= 2);
        assert!(result.items.iter().all(|tx| tx.relayer_id == "relayer-1"));

        // Test finding transactions for relayer-2
        let result = repo
            .find_by_relayer_id("relayer-2", query.clone())
            .await
            .unwrap();
        assert!(result.total >= 1);
        assert!(!result.items.is_empty());
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
    #[ignore = "Requires active Redis instance"]
    async fn test_find_by_status() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_id2 = Uuid::new_v4().to_string();
        let random_id3 = Uuid::new_v4().to_string();
        let relayer_id = Uuid::new_v4().to_string();
        let tx1 = create_test_transaction_with_status(
            &random_id,
            &relayer_id,
            TransactionStatus::Pending,
        );
        let tx2 =
            create_test_transaction_with_status(&random_id2, &relayer_id, TransactionStatus::Sent);
        let tx3 = create_test_transaction_with_status(
            &random_id3,
            &relayer_id,
            TransactionStatus::Confirmed,
        );

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();
        repo.create(tx3).await.unwrap();

        // Test finding pending transactions
        let result = repo
            .find_by_status(&relayer_id, &[TransactionStatus::Pending])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, TransactionStatus::Pending);

        // Test finding multiple statuses
        let result = repo
            .find_by_status(
                &relayer_id,
                &[TransactionStatus::Pending, TransactionStatus::Sent],
            )
            .await
            .unwrap();
        assert_eq!(result.len(), 2);

        // Test finding non-existent status
        let result = repo
            .find_by_status(&relayer_id, &[TransactionStatus::Failed])
            .await
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_find_by_nonce() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let random_id2 = Uuid::new_v4().to_string();
        let relayer_id = Uuid::new_v4().to_string();

        let tx1 = create_test_transaction_with_nonce(&random_id, 42, &relayer_id);
        let tx2 = create_test_transaction_with_nonce(&random_id2, 43, &relayer_id);

        repo.create(tx1.clone()).await.unwrap();
        repo.create(tx2).await.unwrap();

        // Test finding existing nonce
        let result = repo.find_by_nonce(&relayer_id, 42).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, random_id);

        // Test finding non-existent nonce
        let result = repo.find_by_nonce(&relayer_id, 99).await.unwrap();
        assert!(result.is_none());

        // Test finding nonce for non-existent relayer
        let result = repo.find_by_nonce("non-existent", 42).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_status() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx).await.unwrap();
        let updated = repo
            .update_status(random_id.to_string(), TransactionStatus::Confirmed)
            .await
            .unwrap();
        assert_eq!(updated.status, TransactionStatus::Confirmed);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_partial_update() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx).await.unwrap();

        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Sent),
            status_reason: Some("Transaction sent".to_string()),
            sent_at: Some("2025-01-27T16:00:00.000000+00:00".to_string()),
            confirmed_at: None,
            network_data: None,
            hashes: None,
            is_canceled: None,
            priced_at: None,
            noop_count: None,
            delete_at: None,
        };

        let updated = repo
            .partial_update(random_id.to_string(), update)
            .await
            .unwrap();
        assert_eq!(updated.status, TransactionStatus::Sent);
        assert_eq!(updated.status_reason, Some("Transaction sent".to_string()));
        assert_eq!(
            updated.sent_at,
            Some("2025-01-27T16:00:00.000000+00:00".to_string())
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_set_sent_at() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx).await.unwrap();
        let updated = repo
            .set_sent_at(
                random_id.to_string(),
                "2025-01-27T16:00:00.000000+00:00".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            updated.sent_at,
            Some("2025-01-27T16:00:00.000000+00:00".to_string())
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_set_confirmed_at() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx).await.unwrap();
        let updated = repo
            .set_confirmed_at(
                random_id.to_string(),
                "2025-01-27T16:00:00.000000+00:00".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            updated.confirmed_at,
            Some("2025-01-27T16:00:00.000000+00:00".to_string())
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_network_data() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction(&random_id);

        repo.create(tx).await.unwrap();

        let new_network_data = NetworkTransactionData::Evm(EvmTransactionData {
            gas_price: Some(2000000000),
            gas_limit: Some(42000),
            nonce: Some(2),
            value: U256::from_str("2000000000000000000").unwrap(),
            data: Some("0x1234".to_string()),
            from: "0xNewSender".to_string(),
            to: Some("0xNewRecipient".to_string()),
            chain_id: 1,
            signature: None,
            hash: Some("0xnewhash".to_string()),
            speed: Some(Speed::SafeLow),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        });

        let updated = repo
            .update_network_data(random_id.to_string(), new_network_data.clone())
            .await
            .unwrap();
        assert_eq!(
            updated
                .network_data
                .get_evm_transaction_data()
                .unwrap()
                .hash,
            new_network_data.get_evm_transaction_data().unwrap().hash
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_debug_implementation() {
        let repo = setup_test_repo().await;
        let debug_str = format!("{:?}", repo);
        assert!(debug_str.contains("RedisTransactionRepository"));
        assert!(debug_str.contains("test_prefix"));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_error_handling_empty_id() {
        let repo = setup_test_repo().await;

        let result = repo.get_by_id("".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));

        let result = repo
            .update("".to_string(), create_test_transaction("test"))
            .await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));

        let result = repo.delete_by_id("".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_pagination_validation() {
        let repo = setup_test_repo().await;

        let query = PaginationQuery {
            page: 1,
            per_page: 0,
        };
        let result = repo.list_paginated(query).await;
        assert!(matches!(result, Err(RepositoryError::InvalidData(_))));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_index_consistency() {
        let repo = setup_test_repo().await;
        let random_id = Uuid::new_v4().to_string();
        let relayer_id = Uuid::new_v4().to_string();
        let tx = create_test_transaction_with_nonce(&random_id, 42, &relayer_id);

        // Create transaction
        repo.create(tx.clone()).await.unwrap();

        // Verify it can be found by nonce
        let found = repo.find_by_nonce(&relayer_id, 42).await.unwrap();
        assert!(found.is_some());

        // Update the transaction with a new nonce
        let mut updated_tx = tx.clone();
        if let NetworkTransactionData::Evm(ref mut evm_data) = updated_tx.network_data {
            evm_data.nonce = Some(43);
        }

        repo.update(random_id.to_string(), updated_tx)
            .await
            .unwrap();

        // Verify old nonce index is cleaned up
        let old_nonce_result = repo.find_by_nonce(&relayer_id, 42).await.unwrap();
        assert!(old_nonce_result.is_none());

        // Verify new nonce index works
        let new_nonce_result = repo.find_by_nonce(&relayer_id, 43).await.unwrap();
        assert!(new_nonce_result.is_some());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_has_entries() {
        let repo = setup_test_repo().await;
        assert!(!repo.has_entries().await.unwrap());

        let tx_id = uuid::Uuid::new_v4().to_string();
        let tx = create_test_transaction(&tx_id);
        repo.create(tx.clone()).await.unwrap();

        assert!(repo.has_entries().await.unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_drop_all_entries() {
        let repo = setup_test_repo().await;
        let tx_id = uuid::Uuid::new_v4().to_string();
        let tx = create_test_transaction(&tx_id);
        repo.create(tx.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }

    // Tests for delete_at field setting on final status updates
    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_update_status_sets_delete_at_for_final_statuses() {
        let _lock = ENV_MUTEX.lock().await;

        use chrono::{DateTime, Duration, Utc};
        use std::env;

        // Use a unique test environment variable to avoid conflicts
        env::set_var("TRANSACTION_EXPIRATION_HOURS", "6");

        let repo = setup_test_repo().await;

        let final_statuses = [
            TransactionStatus::Canceled,
            TransactionStatus::Confirmed,
            TransactionStatus::Failed,
            TransactionStatus::Expired,
        ];

        for (i, status) in final_statuses.iter().enumerate() {
            let tx_id = format!("test-final-{}-{}", i, Uuid::new_v4());
            let mut tx = create_test_transaction(&tx_id);

            // Ensure transaction has no delete_at initially and is in pending state
            tx.delete_at = None;
            tx.status = TransactionStatus::Pending;

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
    #[ignore = "Requires active Redis instance"]
    async fn test_update_status_does_not_set_delete_at_for_non_final_statuses() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "4");

        let repo = setup_test_repo().await;

        let non_final_statuses = [
            TransactionStatus::Pending,
            TransactionStatus::Sent,
            TransactionStatus::Submitted,
            TransactionStatus::Mined,
        ];

        for (i, status) in non_final_statuses.iter().enumerate() {
            let tx_id = format!("test-non-final-{}-{}", i, Uuid::new_v4());
            let mut tx = create_test_transaction(&tx_id);
            tx.delete_at = None;
            tx.status = TransactionStatus::Pending;

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
    #[ignore = "Requires active Redis instance"]
    async fn test_partial_update_sets_delete_at_for_final_statuses() {
        let _lock = ENV_MUTEX.lock().await;

        use chrono::{DateTime, Duration, Utc};
        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "8");

        let repo = setup_test_repo().await;
        let tx_id = format!("test-partial-final-{}", Uuid::new_v4());
        let mut tx = create_test_transaction(&tx_id);
        tx.delete_at = None;
        tx.status = TransactionStatus::Pending;

        repo.create(tx).await.unwrap();

        let before_update = Utc::now();

        // Use partial_update to set status to Confirmed (final status)
        let update = TransactionUpdateRequest {
            status: Some(TransactionStatus::Confirmed),
            status_reason: Some("Transaction completed".to_string()),
            confirmed_at: Some("2023-01-01T12:05:00Z".to_string()),
            ..Default::default()
        };

        let updated = repo.partial_update(tx_id.clone(), update).await.unwrap();

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
    #[ignore = "Requires active Redis instance"]
    async fn test_update_status_preserves_existing_delete_at() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "2");

        let repo = setup_test_repo().await;
        let tx_id = format!("test-preserve-delete-at-{}", Uuid::new_v4());
        let mut tx = create_test_transaction(&tx_id);

        // Set an existing delete_at value
        let existing_delete_at = "2025-01-01T12:00:00Z".to_string();
        tx.delete_at = Some(existing_delete_at.clone());
        tx.status = TransactionStatus::Pending;

        repo.create(tx).await.unwrap();

        // Update to final status
        let updated = repo
            .update_status(tx_id.clone(), TransactionStatus::Confirmed)
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
    #[ignore = "Requires active Redis instance"]
    async fn test_partial_update_without_status_change_preserves_delete_at() {
        let _lock = ENV_MUTEX.lock().await;

        use std::env;

        env::set_var("TRANSACTION_EXPIRATION_HOURS", "3");

        let repo = setup_test_repo().await;
        let tx_id = format!("test-preserve-no-status-{}", Uuid::new_v4());
        let mut tx = create_test_transaction(&tx_id);
        tx.delete_at = None;
        tx.status = TransactionStatus::Pending;

        repo.create(tx).await.unwrap();

        // First, update to final status to set delete_at
        let updated1 = repo
            .update_status(tx_id.clone(), TransactionStatus::Confirmed)
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

        let updated2 = repo.partial_update(tx_id.clone(), update).await.unwrap();

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
}
