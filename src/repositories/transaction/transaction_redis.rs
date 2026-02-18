//! Redis-backed implementation of the TransactionRepository.

use crate::domain::transaction::common::is_final_state;
use crate::metrics::{
    TRANSACTIONS_BY_STATUS, TRANSACTIONS_CREATED, TRANSACTIONS_FAILED, TRANSACTIONS_SUBMITTED,
    TRANSACTIONS_SUCCESS, TRANSACTION_PROCESSING_TIME,
};
use crate::models::{
    NetworkTransactionData, PaginationQuery, RepositoryError, TransactionRepoModel,
    TransactionStatus, TransactionUpdateRequest,
};
use crate::repositories::redis_base::RedisRepository;
use crate::repositories::{
    BatchDeleteResult, BatchRetrievalResult, PaginatedResult, Repository, TransactionDeleteRequest,
    TransactionRepository,
};
use crate::utils::RedisConnections;
use async_trait::async_trait;
use chrono::Utc;
use redis::{AsyncCommands, Script};
use std::fmt;
use std::sync::Arc;
use tracing::{debug, error, warn};

const RELAYER_PREFIX: &str = "relayer";
const TX_PREFIX: &str = "tx";
const STATUS_PREFIX: &str = "status";
const STATUS_SORTED_PREFIX: &str = "status_sorted";
const NONCE_PREFIX: &str = "nonce";
const TX_TO_RELAYER_PREFIX: &str = "tx_to_relayer";
const RELAYER_LIST_KEY: &str = "relayer_list";
const TX_BY_CREATED_AT_PREFIX: &str = "tx_by_created_at";

#[derive(Clone)]
pub struct RedisTransactionRepository {
    pub connections: Arc<RedisConnections>,
    pub key_prefix: String,
}

impl RedisRepository for RedisTransactionRepository {}

impl RedisTransactionRepository {
    pub fn new(
        connections: Arc<RedisConnections>,
        key_prefix: String,
    ) -> Result<Self, RepositoryError> {
        if key_prefix.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Redis key prefix cannot be empty".to_string(),
            ));
        }

        Ok(Self {
            connections,
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

    /// Generate key for relayer status index (legacy SET): relayer:{relayer_id}:status:{status}
    fn relayer_status_key(&self, relayer_id: &str, status: &TransactionStatus) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, relayer_id, STATUS_PREFIX, status
        )
    }

    /// Generate key for relayer status sorted index (SORTED SET): relayer:{relayer_id}:status_sorted:{status}
    /// Score is created_at timestamp in milliseconds for efficient ordering.
    fn relayer_status_sorted_key(&self, relayer_id: &str, status: &TransactionStatus) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, relayer_id, STATUS_SORTED_PREFIX, status
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

    /// Generate key for relayer's sorted set by created_at: relayer:{relayer_id}:tx_by_created_at
    fn relayer_tx_by_created_at_key(&self, relayer_id: &str) -> String {
        format!(
            "{}:{}:{}:{}",
            self.key_prefix, RELAYER_PREFIX, relayer_id, TX_BY_CREATED_AT_PREFIX
        )
    }

    /// Parse timestamp string to score for sorted set (milliseconds since epoch)
    fn timestamp_to_score(&self, timestamp: &str) -> f64 {
        chrono::DateTime::parse_from_rfc3339(timestamp)
            .map(|dt| dt.timestamp_millis() as f64)
            .unwrap_or_else(|_| {
                warn!(timestamp = %timestamp, "failed to parse timestamp, using 0");
                0.0
            })
    }

    /// Compute the appropriate score for a transaction's status sorted set.
    /// - For Confirmed status: use confirmed_at (on-chain confirmation order)
    /// - For all other statuses: use created_at (queue/processing order)
    fn status_sorted_score(&self, tx: &TransactionRepoModel) -> f64 {
        if tx.status == TransactionStatus::Confirmed {
            // For Confirmed, prefer confirmed_at for accurate on-chain ordering
            if let Some(ref confirmed_at) = tx.confirmed_at {
                return self.timestamp_to_score(confirmed_at);
            }
            // Fallback to created_at if confirmed_at not set (shouldn't happen)
            warn!(tx_id = %tx.id, "Confirmed transaction missing confirmed_at, using created_at");
        }
        self.timestamp_to_score(&tx.created_at)
    }

    /// Batch fetch transactions by IDs using reverse lookup
    async fn get_transactions_by_ids(
        &self,
        ids: &[String],
    ) -> Result<BatchRetrievalResult<TransactionRepoModel>, RepositoryError> {
        if ids.is_empty() {
            debug!("no transaction IDs provided for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids: vec![],
            });
        }

        let mut conn = self
            .get_connection(self.connections.reader(), "batch_fetch_transactions")
            .await?;

        let reverse_keys: Vec<String> = ids.iter().map(|id| self.tx_to_relayer_key(id)).collect();

        debug!(count = %ids.len(), "fetching relayer IDs for transactions");

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
                    warn!(tx_id = %ids[i], "no relayer found for transaction");
                    failed_ids.push(ids[i].clone());
                }
            }
        }

        if tx_keys.is_empty() {
            debug!("no valid transactions found for batch fetch");
            return Ok(BatchRetrievalResult {
                results: vec![],
                failed_ids,
            });
        }

        debug!(count = %tx_keys.len(), "batch fetching transaction data");

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
                            error!(tx_id = %valid_ids[i], error = %e, "failed to deserialize transaction");
                            // Continue processing other transactions
                        }
                    }
                }
                None => {
                    warn!(tx_id = %valid_ids[i], "transaction not found in batch fetch");
                    failed_ids.push(valid_ids[i].clone());
                }
            }
        }

        if failed_count > 0 {
            warn!(failed_count = %failed_count, total_count = %valid_ids.len(), "failed to deserialize transactions in batch");
        }

        debug!(count = %transactions.len(), "successfully fetched transactions");
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
                debug!("no EVM transaction data available for nonce extraction");
                None
            }
        }
    }

    /// Ensures the status sorted set exists, migrating from legacy SET if needed.
    ///
    /// This handles the transition from unordered SETs to sorted SETs for status indexing.
    /// If the sorted set is empty but the legacy set has data, it migrates the data
    /// by looking up each transaction's created_at timestamp to compute the score.
    ///
    /// # Concurrency
    /// This function is safe for concurrent calls. If multiple calls race to migrate
    /// the same status set:
    /// - ZADD is idempotent (same member + score = no-op)
    /// - DEL on non-existent key is safe (returns 0)
    /// - After first successful migration, subsequent calls hit the fast path (ZCARD > 0)
    ///
    /// The only downside of concurrent migrations is wasted work, not data corruption.
    ///
    /// Returns the count of items in the sorted set after migration.
    async fn ensure_status_sorted_set(
        &self,
        relayer_id: &str,
        status: &TransactionStatus,
    ) -> Result<u64, RepositoryError> {
        let sorted_key = self.relayer_status_sorted_key(relayer_id, status);
        let legacy_key = self.relayer_status_key(relayer_id, status);

        // Phase 1: Check if migration is needed
        let legacy_ids = {
            let mut conn = self
                .get_connection(self.connections.primary(), "ensure_status_sorted_set_check")
                .await?;

            // Always check if legacy set has data that needs migration
            let legacy_count: u64 = conn
                .scard(&legacy_key)
                .await
                .map_err(|e| self.map_redis_error(e, "ensure_status_sorted_set_scard"))?;

            if legacy_count == 0 {
                // No legacy data to migrate, return current ZSET count
                let sorted_count: u64 = conn
                    .zcard(&sorted_key)
                    .await
                    .map_err(|e| self.map_redis_error(e, "ensure_status_sorted_set_zcard"))?;
                return Ok(sorted_count);
            }

            // Migration needed: get all IDs from legacy set
            debug!(
                relayer_id = %relayer_id,
                status = %status,
                legacy_count = %legacy_count,
                "migrating status set to sorted set"
            );

            let ids: Vec<String> = conn
                .smembers(&legacy_key)
                .await
                .map_err(|e| self.map_redis_error(e, "ensure_status_sorted_set_smembers"))?;

            ids
            // Connection dropped here before nested call to avoid connection doubling
        };

        if legacy_ids.is_empty() {
            return Ok(0);
        }

        // Phase 2: Fetch transactions (uses its own connection internally)
        let transactions = self.get_transactions_by_ids(&legacy_ids).await?;

        // Phase 3: Perform migration with a new connection
        let mut conn = self
            .get_connection(
                self.connections.primary(),
                "ensure_status_sorted_set_migrate",
            )
            .await?;

        if transactions.results.is_empty() {
            // All transactions were stale/deleted, clean up legacy set
            let _: () = conn
                .del(&legacy_key)
                .await
                .map_err(|e| self.map_redis_error(e, "ensure_status_sorted_set_del_stale"))?;
            return Ok(0);
        }

        // Build sorted set entries and migrate atomically
        // Use status-aware scoring: confirmed_at for Confirmed, created_at for others
        let mut pipe = redis::pipe();
        pipe.atomic();

        for tx in &transactions.results {
            let score = self.status_sorted_score(tx);
            pipe.zadd(&sorted_key, &tx.id, score);
        }

        // Delete legacy set after migration
        pipe.del(&legacy_key);

        pipe.query_async::<()>(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "ensure_status_sorted_set_migrate"))?;

        let migrated_count = transactions.results.len() as u64;
        debug!(
            relayer_id = %relayer_id,
            status = %status,
            migrated_count = %migrated_count,
            "completed migration of status set to sorted set"
        );

        Ok(migrated_count)
    }

    /// Update indexes atomically with comprehensive error handling
    async fn update_indexes(
        &self,
        tx: &TransactionRepoModel,
        old_tx: Option<&TransactionRepoModel>,
    ) -> Result<(), RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.primary(), "update_indexes")
            .await?;
        let mut pipe = redis::pipe();
        pipe.atomic();

        debug!(tx_id = %tx.id, "updating indexes for transaction");

        // Add relayer to the global relayer list
        let relayer_list_key = self.relayer_list_key();
        pipe.sadd(&relayer_list_key, &tx.relayer_id);

        // Compute scores for sorted sets
        // Status sorted set: uses confirmed_at for Confirmed status, created_at for others
        let status_score = self.status_sorted_score(tx);
        // Global tx_by_created_at: always uses created_at for consistent ordering
        let created_at_score = self.timestamp_to_score(&tx.created_at);

        // Handle status index updates - write to SORTED SET (new format)
        let new_status_sorted_key = self.relayer_status_sorted_key(&tx.relayer_id, &tx.status);
        pipe.zadd(&new_status_sorted_key, &tx.id, status_score);
        debug!(tx_id = %tx.id, status = %tx.status, score = %status_score, "added transaction to status sorted set");

        if let Some(nonce) = self.extract_nonce(&tx.network_data) {
            let nonce_key = self.relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.set(&nonce_key, &tx.id);
            debug!(tx_id = %tx.id, nonce = %nonce, "added nonce index for transaction");
        }

        // Add to per-relayer sorted set by created_at (for efficient sorted pagination)
        let relayer_sorted_key = self.relayer_tx_by_created_at_key(&tx.relayer_id);
        pipe.zadd(&relayer_sorted_key, &tx.id, created_at_score);
        debug!(tx_id = %tx.id, score = %created_at_score, "added transaction to sorted set by created_at");

        // Remove old indexes if updating
        if let Some(old) = old_tx {
            if old.status != tx.status {
                // Remove from old status sorted set (new format)
                let old_status_sorted_key =
                    self.relayer_status_sorted_key(&old.relayer_id, &old.status);
                pipe.zrem(&old_status_sorted_key, &tx.id);

                // Also clean up legacy SET if it exists (for migration cleanup)
                let old_status_legacy_key = self.relayer_status_key(&old.relayer_id, &old.status);
                pipe.srem(&old_status_legacy_key, &tx.id);

                debug!(tx_id = %tx.id, old_status = %old.status, new_status = %tx.status, "removing old status indexes for transaction");
            }

            // Handle nonce index cleanup
            if let Some(old_nonce) = self.extract_nonce(&old.network_data) {
                let new_nonce = self.extract_nonce(&tx.network_data);
                if Some(old_nonce) != new_nonce {
                    let old_nonce_key = self.relayer_nonce_key(&old.relayer_id, old_nonce);
                    pipe.del(&old_nonce_key);
                    debug!(tx_id = %tx.id, old_nonce = %old_nonce, new_nonce = ?new_nonce, "removing old nonce index for transaction");
                }
            }
        }

        // Execute all operations in a single pipeline
        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!(tx_id = %tx.id, error = %e, "index update pipeline failed for transaction");
            self.map_redis_error(e, &format!("update_indexes_for_tx_{}", tx.id))
        })?;

        debug!(tx_id = %tx.id, "successfully updated indexes for transaction");
        Ok(())
    }

    /// Remove all indexes with error recovery
    async fn remove_all_indexes(&self, tx: &TransactionRepoModel) -> Result<(), RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.primary(), "remove_all_indexes")
            .await?;
        let mut pipe = redis::pipe();
        pipe.atomic();

        debug!(tx_id = %tx.id, "removing all indexes for transaction");

        // Remove from ALL possible status indexes to ensure complete cleanup
        // This handles cases where a transaction might be in multiple status sets
        // due to race conditions, partial failures, or bugs
        for status in &[
            TransactionStatus::Canceled,
            TransactionStatus::Pending,
            TransactionStatus::Sent,
            TransactionStatus::Submitted,
            TransactionStatus::Mined,
            TransactionStatus::Confirmed,
            TransactionStatus::Failed,
            TransactionStatus::Expired,
        ] {
            // Remove from sorted status set (new format)
            let status_sorted_key = self.relayer_status_sorted_key(&tx.relayer_id, status);
            pipe.zrem(&status_sorted_key, &tx.id);

            // Remove from legacy status set (for migration cleanup)
            let status_legacy_key = self.relayer_status_key(&tx.relayer_id, status);
            pipe.srem(&status_legacy_key, &tx.id);
        }

        // Remove nonce index if exists
        if let Some(nonce) = self.extract_nonce(&tx.network_data) {
            let nonce_key = self.relayer_nonce_key(&tx.relayer_id, nonce);
            pipe.del(&nonce_key);
            debug!(tx_id = %tx.id, nonce = %nonce, "removing nonce index for transaction");
        }

        // Remove from per-relayer sorted set by created_at
        let relayer_sorted_key = self.relayer_tx_by_created_at_key(&tx.relayer_id);
        pipe.zrem(&relayer_sorted_key, &tx.id);
        debug!(tx_id = %tx.id, "removing transaction from sorted set by created_at");

        // Remove reverse lookup
        let reverse_key = self.tx_to_relayer_key(&tx.id);
        pipe.del(&reverse_key);

        pipe.exec_async(&mut conn).await.map_err(|e| {
            error!(tx_id = %tx.id, error = %e, "index removal failed for transaction");
            self.map_redis_error(e, &format!("remove_indexes_for_tx_{}", tx.id))
        })?;

        debug!(tx_id = %tx.id, "successfully removed all indexes for transaction");
        Ok(())
    }
}

impl fmt::Debug for RedisTransactionRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisTransactionRepository")
            .field("connections", &"<RedisConnections>")
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
        let mut conn = self
            .get_connection(self.connections.primary(), "create")
            .await?;

        debug!(tx_id = %entity.id, "creating transaction");

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
            error!(tx_id = %entity.id, error = %e, "failed to update indexes for new transaction");
            return Err(e);
        }

        // Track transaction creation metric
        let network_type = format!("{:?}", entity.network_type).to_lowercase();
        let relayer_id = entity.relayer_id.as_str();
        TRANSACTIONS_CREATED
            .with_label_values(&[relayer_id, &network_type])
            .inc();

        // Track initial status distribution (Pending)
        let status = &entity.status;
        let status_str = format!("{status:?}").to_lowercase();
        TRANSACTIONS_BY_STATUS
            .with_label_values(&[relayer_id, &network_type, &status_str])
            .inc();

        debug!(tx_id = %entity.id, "successfully created transaction");
        Ok(entity)
    }

    async fn get_by_id(&self, id: String) -> Result<TransactionRepoModel, RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Transaction ID cannot be empty".to_string(),
            ));
        }

        let mut conn = self
            .get_connection(self.connections.reader(), "get_by_id")
            .await?;

        debug!(tx_id = %id, "fetching transaction");

        let reverse_key = self.tx_to_relayer_key(&id);
        let relayer_id: Option<String> = conn
            .get(&reverse_key)
            .await
            .map_err(|e| self.map_redis_error(e, "get_transaction_reverse_lookup"))?;

        let relayer_id = match relayer_id {
            Some(relayer_id) => relayer_id,
            None => {
                debug!(tx_id = %id, "transaction not found (no reverse lookup)");
                return Err(RepositoryError::NotFound(format!(
                    "Transaction with ID {id} not found"
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
                debug!(tx_id = %id, "successfully fetched transaction");
                Ok(tx)
            }
            None => {
                debug!(tx_id = %id, "transaction not found");
                Err(RepositoryError::NotFound(format!(
                    "Transaction with ID {id} not found"
                )))
            }
        }
    }

    // Unoptimized implementation of list_paginated. Rarely used. find_by_relayer_id is preferred.
    async fn list_all(&self) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.reader(), "list_all")
            .await?;

        debug!("fetching all transactions sorted by created_at (newest first)");

        // Get all relayer IDs
        let relayer_list_key = self.relayer_list_key();
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_all_relayer_ids"))?;

        debug!(count = %relayer_ids.len(), "found relayers");

        // Collect all transaction IDs from all relayers using their sorted sets
        let mut all_tx_ids = Vec::new();
        for relayer_id in relayer_ids {
            let relayer_sorted_key = self.relayer_tx_by_created_at_key(&relayer_id);
            let tx_ids: Vec<String> = redis::cmd("ZRANGE")
                .arg(&relayer_sorted_key)
                .arg(0)
                .arg(-1)
                .arg("REV")
                .query_async(&mut conn)
                .await
                .map_err(|e| self.map_redis_error(e, "list_all_relayer_sorted"))?;

            all_tx_ids.extend(tx_ids);
        }

        // Release connection before nested call to avoid connection doubling
        drop(conn);

        // Batch fetch all transactions at once
        let batch_result = self.get_transactions_by_ids(&all_tx_ids).await?;
        let mut all_transactions = batch_result.results;

        // Sort all transactions by created_at (newest first)
        all_transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        debug!(count = %all_transactions.len(), "found transactions");
        Ok(all_transactions)
    }

    // Unoptimized implementation of list_paginated. Rarely used. find_by_relayer_id is preferred.
    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        if query.per_page == 0 {
            return Err(RepositoryError::InvalidData(
                "per_page must be greater than 0".to_string(),
            ));
        }

        let mut conn = self
            .get_connection(self.connections.reader(), "list_paginated")
            .await?;

        debug!(page = %query.page, per_page = %query.per_page, "fetching paginated transactions sorted by created_at (newest first)");

        // Get all relayer IDs
        let relayer_list_key = self.relayer_list_key();
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "list_paginated_relayer_ids"))?;

        // Collect all transaction IDs from all relayers using their sorted sets
        let mut all_tx_ids = Vec::new();
        for relayer_id in relayer_ids {
            let relayer_sorted_key = self.relayer_tx_by_created_at_key(&relayer_id);
            let tx_ids: Vec<String> = redis::cmd("ZRANGE")
                .arg(&relayer_sorted_key)
                .arg(0)
                .arg(-1)
                .arg("REV")
                .query_async(&mut conn)
                .await
                .map_err(|e| self.map_redis_error(e, "list_paginated_relayer_sorted"))?;

            all_tx_ids.extend(tx_ids);
        }

        // Release connection before nested call to avoid connection doubling
        drop(conn);

        // Batch fetch all transactions at once
        let batch_result = self.get_transactions_by_ids(&all_tx_ids).await?;
        let mut all_transactions = batch_result.results;

        // Sort all transactions by created_at (newest first)
        all_transactions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let total = all_transactions.len() as u64;
        let start = ((query.page - 1) * query.per_page) as usize;
        let end = (start + query.per_page as usize).min(all_transactions.len());

        if start >= all_transactions.len() {
            debug!(page = %query.page, total = %total, "page is beyond available data");
            return Ok(PaginatedResult {
                items: vec![],
                total,
                page: query.page,
                per_page: query.per_page,
            });
        }

        let items = all_transactions[start..end].to_vec();

        debug!(count = %items.len(), page = %query.page, "successfully fetched transactions for page");

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

        debug!(tx_id = %id, "updating transaction");

        // Get the old transaction for index cleanup
        let old_tx = self.get_by_id(id.clone()).await?;

        let key = self.tx_key(&entity.relayer_id, &id);
        let mut conn = self
            .get_connection(self.connections.primary(), "update")
            .await?;

        let value = self.serialize_entity(&entity, |t| &t.id, "transaction")?;

        // Update transaction
        let _: () = conn
            .set(&key, value)
            .await
            .map_err(|e| self.map_redis_error(e, "update_transaction"))?;

        // Update indexes
        self.update_indexes(&entity, Some(&old_tx)).await?;

        debug!(tx_id = %id, "successfully updated transaction");
        Ok(entity)
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        if id.is_empty() {
            return Err(RepositoryError::InvalidData(
                "Transaction ID cannot be empty".to_string(),
            ));
        }

        debug!(tx_id = %id, "deleting transaction");

        // Get transaction first for index cleanup
        let tx = self.get_by_id(id.clone()).await?;

        let key = self.tx_key(&tx.relayer_id, &id);
        let reverse_key = self.tx_to_relayer_key(&id);
        let mut conn = self
            .get_connection(self.connections.primary(), "delete_by_id")
            .await?;

        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.del(&key);
        pipe.del(&reverse_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "delete_transaction"))?;

        // Remove indexes (log errors but don't fail the delete)
        if let Err(e) = self.remove_all_indexes(&tx).await {
            error!(tx_id = %id, error = %e, "failed to remove indexes for deleted transaction");
        }

        debug!(tx_id = %id, "successfully deleted transaction");
        Ok(())
    }

    // Unoptimized implementation of count. Rarely used. find_by_relayer_id is preferred.
    async fn count(&self) -> Result<usize, RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.reader(), "count")
            .await?;

        debug!("counting transactions");

        // Get all relayer IDs and sum their sorted set counts
        let relayer_list_key = self.relayer_list_key();
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "count_relayer_ids"))?;

        let mut total_count = 0usize;
        for relayer_id in relayer_ids {
            let relayer_sorted_key = self.relayer_tx_by_created_at_key(&relayer_id);
            let count: usize = conn
                .zcard(&relayer_sorted_key)
                .await
                .map_err(|e| self.map_redis_error(e, "count_relayer_transactions"))?;
            total_count += count;
        }

        debug!(count = %total_count, "transaction count");
        Ok(total_count)
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.reader(), "has_entries")
            .await?;
        let relayer_list_key = self.relayer_list_key();

        debug!("checking if transaction entries exist");

        let exists: bool = conn
            .exists(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "has_entries_check"))?;

        debug!(exists = %exists, "transaction entries exist");
        Ok(exists)
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.primary(), "drop_all_entries")
            .await?;
        let relayer_list_key = self.relayer_list_key();

        debug!("dropping all transaction entries");

        // Get all relayer IDs first
        let relayer_ids: Vec<String> = conn
            .smembers(&relayer_list_key)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_get_relayer_ids"))?;

        if relayer_ids.is_empty() {
            debug!("no transaction entries to drop");
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

                // Delete status indexes (we can't know the specific status, so we'll clean up all possible ones)
                // This ensures complete cleanup even if there are orphaned entries
                for status in &[
                    TransactionStatus::Canceled,
                    TransactionStatus::Pending,
                    TransactionStatus::Sent,
                    TransactionStatus::Submitted,
                    TransactionStatus::Mined,
                    TransactionStatus::Confirmed,
                    TransactionStatus::Failed,
                    TransactionStatus::Expired,
                ] {
                    // Remove from sorted status set (new format)
                    let status_sorted_key = self.relayer_status_sorted_key(relayer_id, status);
                    pipe.zrem(&status_sorted_key, &tx_id);

                    // Remove from legacy status set (for migration cleanup)
                    let status_key = self.relayer_status_key(relayer_id, status);
                    pipe.srem(&status_key, &tx_id);
                }
            }

            // Delete the relayer's sorted set by created_at
            let relayer_sorted_key = self.relayer_tx_by_created_at_key(relayer_id);
            pipe.del(&relayer_sorted_key);
        }

        // Delete the relayer list key
        pipe.del(&relayer_list_key);

        pipe.exec_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "drop_all_entries_pipeline"))?;

        debug!(count = %relayer_ids.len(), "dropped all transaction entries for relayers");
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
        let mut conn = self
            .get_connection(self.connections.reader(), "find_by_relayer_id")
            .await?;

        debug!(relayer_id = %relayer_id, page = %query.page, per_page = %query.per_page, "fetching transactions for relayer sorted by created_at (newest first)");

        let relayer_sorted_key = self.relayer_tx_by_created_at_key(relayer_id);

        // Get total count from relayer's sorted set
        let sorted_set_count: u64 = conn
            .zcard(&relayer_sorted_key)
            .await
            .map_err(|e| self.map_redis_error(e, "find_by_relayer_id_count"))?;

        // If sorted set is empty, return empty result immediately
        // All new transactions are automatically added to the sorted set
        if sorted_set_count == 0 {
            debug!(relayer_id = %relayer_id, "no transactions found for relayer (sorted set is empty)");
            return Ok(PaginatedResult {
                items: vec![],
                total: 0,
                page: query.page,
                per_page: query.per_page,
            });
        }

        let total = sorted_set_count;

        // Calculate pagination range (0-indexed for Redis ZRANGE with REV)
        let start = ((query.page - 1) * query.per_page) as isize;
        let end = start + query.per_page as isize - 1;

        if start as u64 >= total {
            debug!(relayer_id = %relayer_id, page = %query.page, total = %total, "page is beyond available data");
            return Ok(PaginatedResult {
                items: vec![],
                total,
                page: query.page,
                per_page: query.per_page,
            });
        }

        // Get page of transaction IDs from sorted set (newest first using ZRANGE with REV)
        let page_ids: Vec<String> = redis::cmd("ZRANGE")
            .arg(&relayer_sorted_key)
            .arg(start)
            .arg(end)
            .arg("REV")
            .query_async(&mut conn)
            .await
            .map_err(|e| self.map_redis_error(e, "find_by_relayer_id_sorted"))?;

        // Release connection before nested call to avoid connection doubling
        drop(conn);

        let items = self.get_transactions_by_ids(&page_ids).await?;

        debug!(relayer_id = %relayer_id, count = %items.results.len(), page = %query.page, "successfully fetched transactions for relayer");

        Ok(PaginatedResult {
            items: items.results,
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    // Unoptimized implementation of find_by_status. Rarely used. find_by_status_paginated is preferred.
    async fn find_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<Vec<TransactionRepoModel>, RepositoryError> {
        // Ensure all status sorted sets are migrated first (releases connection after each)
        for status in statuses {
            self.ensure_status_sorted_set(relayer_id, status).await?;
        }

        // Now get a connection and collect all IDs
        let mut conn = self
            .get_connection(self.connections.reader(), "find_by_status")
            .await?;

        let mut all_ids: Vec<String> = Vec::new();
        for status in statuses {
            // Get IDs from sorted set (already ordered by created_at)
            let sorted_key = self.relayer_status_sorted_key(relayer_id, status);
            let ids: Vec<String> = redis::cmd("ZRANGE")
                .arg(&sorted_key)
                .arg(0)
                .arg(-1)
                .arg("REV") // Newest first
                .query_async(&mut conn)
                .await
                .map_err(|e| self.map_redis_error(e, "find_by_status"))?;

            all_ids.extend(ids);
        }

        // Release connection before nested call to avoid connection doubling
        drop(conn);

        if all_ids.is_empty() {
            return Ok(vec![]);
        }

        // Remove duplicates (can happen if a transaction is in multiple status sets due to partial failures)
        all_ids.sort();
        all_ids.dedup();

        // Fetch all transactions and sort by created_at (newest first)
        let mut transactions = self.get_transactions_by_ids(&all_ids).await?;

        // Sort by created_at descending (newest first)
        transactions
            .results
            .sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(transactions.results)
    }

    async fn find_by_status_paginated(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
        query: PaginationQuery,
        oldest_first: bool,
    ) -> Result<PaginatedResult<TransactionRepoModel>, RepositoryError> {
        // Ensure all status sorted sets are migrated first (releases connection after each)
        for status in statuses {
            self.ensure_status_sorted_set(relayer_id, status).await?;
        }

        let mut conn = self
            .get_connection(self.connections.reader(), "find_by_status_paginated")
            .await?;

        // For single status, we can paginate directly from the sorted set
        if statuses.len() == 1 {
            let sorted_key = self.relayer_status_sorted_key(relayer_id, &statuses[0]);

            // Get total count
            let total: u64 = conn
                .zcard(&sorted_key)
                .await
                .map_err(|e| self.map_redis_error(e, "find_by_status_paginated_count"))?;

            if total == 0 {
                return Ok(PaginatedResult {
                    items: vec![],
                    total: 0,
                    page: query.page,
                    per_page: query.per_page,
                });
            }

            // Calculate pagination bounds
            let start = ((query.page.saturating_sub(1)) * query.per_page) as isize;
            let end = start + query.per_page as isize - 1;

            // Get page of IDs directly from sorted set
            // REV = newest first (descending), no REV = oldest first (ascending)
            let mut cmd = redis::cmd("ZRANGE");
            cmd.arg(&sorted_key).arg(start).arg(end);
            if !oldest_first {
                cmd.arg("REV");
            }
            let page_ids: Vec<String> = cmd
                .query_async(&mut conn)
                .await
                .map_err(|e| self.map_redis_error(e, "find_by_status_paginated"))?;

            // Release connection before nested call to avoid connection doubling
            drop(conn);

            let transactions = self.get_transactions_by_ids(&page_ids).await?;

            debug!(
                relayer_id = %relayer_id,
                status = %statuses[0],
                total = %total,
                page = %query.page,
                page_size = %transactions.results.len(),
                "fetched paginated transactions by single status"
            );

            return Ok(PaginatedResult {
                items: transactions.results,
                total,
                page: query.page,
                per_page: query.per_page,
            });
        }

        // For multiple statuses, collect all IDs and merge
        let mut all_ids: Vec<(String, f64)> = Vec::new();
        for status in statuses {
            let sorted_key = self.relayer_status_sorted_key(relayer_id, status);

            // Get IDs with scores for proper sorting
            let ids_with_scores: Vec<(String, f64)> = redis::cmd("ZRANGE")
                .arg(&sorted_key)
                .arg(0)
                .arg(-1)
                .arg("WITHSCORES")
                .query_async(&mut conn)
                .await
                .map_err(|e| self.map_redis_error(e, "find_by_status_paginated_multi"))?;

            all_ids.extend(ids_with_scores);
        }

        // Release connection before nested call to avoid connection doubling
        drop(conn);

        // Remove duplicates (keep highest/lowest score based on sort order)
        let mut id_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (id, score) in all_ids {
            id_map
                .entry(id)
                .and_modify(|s| {
                    // For oldest_first, keep the lowest score; otherwise keep highest
                    if oldest_first {
                        if score < *s {
                            *s = score
                        }
                    } else if score > *s {
                        *s = score
                    }
                })
                .or_insert(score);
        }

        // Sort by score: descending for newest first, ascending for oldest first
        let mut sorted_ids: Vec<(String, f64)> = id_map.into_iter().collect();
        if oldest_first {
            sorted_ids.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            sorted_ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }

        let total = sorted_ids.len() as u64;

        if total == 0 {
            return Ok(PaginatedResult {
                items: vec![],
                total: 0,
                page: query.page,
                per_page: query.per_page,
            });
        }

        // Apply pagination
        let start = ((query.page.saturating_sub(1)) * query.per_page) as usize;
        let page_ids: Vec<String> = sorted_ids
            .into_iter()
            .skip(start)
            .take(query.per_page as usize)
            .map(|(id, _)| id)
            .collect();

        // Fetch only the transactions for this page
        let transactions = self.get_transactions_by_ids(&page_ids).await?;

        debug!(
            relayer_id = %relayer_id,
            total = %total,
            page = %query.page,
            page_size = %transactions.results.len(),
            "fetched paginated transactions by status"
        );

        Ok(PaginatedResult {
            items: transactions.results,
            total,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn find_by_nonce(
        &self,
        relayer_id: &str,
        nonce: u64,
    ) -> Result<Option<TransactionRepoModel>, RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.reader(), "find_by_nonce")
            .await?;
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
                        warn!(relayer_id = %relayer_id, nonce = %nonce, "stale nonce index found for relayer");
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
        const MAX_RETRIES: u32 = 3;
        const BACKOFF_MS: u64 = 100;

        // Optimistic CAS: only apply update if the current stored value still matches the
        // expected pre-update value. This avoids duplicate status metric updates on races.
        let mut original_tx = self.get_by_id(tx_id.clone()).await?;
        let mut updated_tx = original_tx.clone();
        updated_tx.apply_partial_update(update.clone());

        let key = self.tx_key(&updated_tx.relayer_id, &tx_id);
        let mut original_value = self.serialize_entity(&original_tx, |t| &t.id, "transaction")?;
        let mut updated_value = self.serialize_entity(&updated_tx, |t| &t.id, "transaction")?;
        let mut data_updated = false;

        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            let mut conn = match self
                .get_connection(self.connections.primary(), "partial_update")
                .await
            {
                Ok(conn) => conn,
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES - 1 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(BACKOFF_MS)).await;
                        continue;
                    }
                    return Err(last_error.unwrap());
                }
            };

            if !data_updated {
                let cas_script = Script::new(
                    r#"
                    local current = redis.call('GET', KEYS[1])
                    if not current then
                        return -1
                    end
                    if current == ARGV[1] then
                        redis.call('SET', KEYS[1], ARGV[2])
                        return 1
                    end
                    return 0
                    "#,
                );

                let cas_result: i32 = match cas_script
                    .key(&key)
                    .arg(&original_value)
                    .arg(&updated_value)
                    .invoke_async(&mut conn)
                    .await
                {
                    Ok(result) => result,
                    Err(e) => {
                        if attempt < MAX_RETRIES - 1 {
                            warn!(tx_id = %tx_id, attempt = %attempt, error = %e, "failed CAS transaction update, retrying");
                            last_error = Some(self.map_redis_error(e, "partial_update_cas"));
                            tokio::time::sleep(tokio::time::Duration::from_millis(BACKOFF_MS))
                                .await;
                            continue;
                        }
                        return Err(self.map_redis_error(e, "partial_update_cas"));
                    }
                };

                if cas_result == -1 {
                    return Err(RepositoryError::NotFound(format!(
                        "Transaction with ID {tx_id} not found"
                    )));
                }

                if cas_result == 0 {
                    if attempt < MAX_RETRIES - 1 {
                        warn!(tx_id = %tx_id, attempt = %attempt, "concurrent transaction update detected, rebasing retry");
                        original_tx = self.get_by_id(tx_id.clone()).await?;
                        updated_tx = original_tx.clone();
                        updated_tx.apply_partial_update(update.clone());
                        original_value =
                            self.serialize_entity(&original_tx, |t| &t.id, "transaction")?;
                        updated_value =
                            self.serialize_entity(&updated_tx, |t| &t.id, "transaction")?;
                        tokio::time::sleep(tokio::time::Duration::from_millis(BACKOFF_MS)).await;
                        continue;
                    }
                    return Err(RepositoryError::TransactionFailure(format!(
                        "Concurrent update conflict for transaction {tx_id}"
                    )));
                }

                data_updated = true;
            }

            // Try to update indexes with the original pre-update state
            // This ensures stale indexes are removed even on retry attempts
            match self.update_indexes(&updated_tx, Some(&original_tx)).await {
                Ok(_) => {
                    debug!(tx_id = %tx_id, attempt = %attempt, "successfully updated transaction");

                    // Track metrics for transaction state changes
                    if let Some(new_status) = &update.status {
                        let network_type = format!("{:?}", updated_tx.network_type).to_lowercase();
                        let relayer_id = updated_tx.relayer_id.as_str();

                        // Track submission (when status changes to Submitted)
                        if original_tx.status != TransactionStatus::Submitted
                            && *new_status == TransactionStatus::Submitted
                        {
                            TRANSACTIONS_SUBMITTED
                                .with_label_values(&[relayer_id, &network_type])
                                .inc();

                            // Track processing time: creation to submission
                            if let Ok(created_time) =
                                chrono::DateTime::parse_from_rfc3339(&updated_tx.created_at)
                            {
                                let processing_seconds =
                                    (Utc::now() - created_time.with_timezone(&Utc)).num_seconds()
                                        as f64;
                                TRANSACTION_PROCESSING_TIME
                                    .with_label_values(&[
                                        relayer_id,
                                        &network_type,
                                        "creation_to_submission",
                                    ])
                                    .observe(processing_seconds);
                            }
                        }

                        // Track status distribution (update gauge when status changes)
                        if original_tx.status != *new_status {
                            // Decrement old status and clamp to zero to avoid negative gauges.
                            let old_status = &original_tx.status;
                            let old_status_str = format!("{old_status:?}").to_lowercase();
                            let old_status_gauge = TRANSACTIONS_BY_STATUS.with_label_values(&[
                                relayer_id,
                                &network_type,
                                &old_status_str,
                            ]);
                            let clamped_value = (old_status_gauge.get() - 1.0).max(0.0);
                            old_status_gauge.set(clamped_value);

                            // Increment new status
                            let new_status_str = format!("{new_status:?}").to_lowercase();
                            TRANSACTIONS_BY_STATUS
                                .with_label_values(&[relayer_id, &network_type, &new_status_str])
                                .inc();
                        }

                        // Track metrics for final transaction states
                        // Only track when status changes from non-final to final state
                        let was_final = is_final_state(&original_tx.status);
                        let is_final = is_final_state(new_status);

                        if !was_final && is_final {
                            match new_status {
                                TransactionStatus::Confirmed => {
                                    TRANSACTIONS_SUCCESS
                                        .with_label_values(&[relayer_id, &network_type])
                                        .inc();

                                    // Track processing time: submission to confirmation
                                    if let (Some(sent_at_str), Some(confirmed_at_str)) =
                                        (&updated_tx.sent_at, &updated_tx.confirmed_at)
                                    {
                                        if let (Ok(sent_time), Ok(confirmed_time)) = (
                                            chrono::DateTime::parse_from_rfc3339(sent_at_str),
                                            chrono::DateTime::parse_from_rfc3339(confirmed_at_str),
                                        ) {
                                            let processing_seconds = (confirmed_time
                                                .with_timezone(&Utc)
                                                - sent_time.with_timezone(&Utc))
                                            .num_seconds()
                                                as f64;
                                            TRANSACTION_PROCESSING_TIME
                                                .with_label_values(&[
                                                    relayer_id,
                                                    &network_type,
                                                    "submission_to_confirmation",
                                                ])
                                                .observe(processing_seconds);
                                        }
                                    }

                                    // Track processing time: creation to confirmation
                                    if let Ok(created_time) =
                                        chrono::DateTime::parse_from_rfc3339(&updated_tx.created_at)
                                    {
                                        if let Some(confirmed_at_str) = &updated_tx.confirmed_at {
                                            if let Ok(confirmed_time) =
                                                chrono::DateTime::parse_from_rfc3339(
                                                    confirmed_at_str,
                                                )
                                            {
                                                let processing_seconds = (confirmed_time
                                                    .with_timezone(&Utc)
                                                    - created_time.with_timezone(&Utc))
                                                .num_seconds()
                                                    as f64;
                                                TRANSACTION_PROCESSING_TIME
                                                    .with_label_values(&[
                                                        relayer_id,
                                                        &network_type,
                                                        "creation_to_confirmation",
                                                    ])
                                                    .observe(processing_seconds);
                                            }
                                        }
                                    }
                                }
                                TransactionStatus::Failed => {
                                    // Parse status_reason to determine failure type
                                    let failure_reason = updated_tx
                                        .status_reason
                                        .as_deref()
                                        .map(|reason| {
                                            if reason.starts_with("Submission failed:") {
                                                "submission_failed"
                                            } else if reason.starts_with("Preparation failed:") {
                                                "preparation_failed"
                                            } else {
                                                "failed"
                                            }
                                        })
                                        .unwrap_or("failed");
                                    TRANSACTIONS_FAILED
                                        .with_label_values(&[
                                            relayer_id,
                                            &network_type,
                                            failure_reason,
                                        ])
                                        .inc();
                                }
                                TransactionStatus::Expired => {
                                    TRANSACTIONS_FAILED
                                        .with_label_values(&[relayer_id, &network_type, "expired"])
                                        .inc();
                                }
                                TransactionStatus::Canceled => {
                                    TRANSACTIONS_FAILED
                                        .with_label_values(&[relayer_id, &network_type, "canceled"])
                                        .inc();
                                }
                                _ => {
                                    // Other final states (shouldn't happen, but handle gracefully)
                                }
                            }
                        }
                    }
                    return Ok(updated_tx);
                }
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    warn!(tx_id = %tx_id, attempt = %attempt, error = %e, "failed to update indexes, retrying");
                    last_error = Some(e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(BACKOFF_MS)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            RepositoryError::UnexpectedError("partial_update exhausted retries".to_string())
        }))
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

    /// Count transactions by status using Redis ZCARD (O(1) per sorted set).
    /// Much more efficient than find_by_status when you only need the count.
    /// Triggers migration from legacy SETs if needed.
    async fn count_by_status(
        &self,
        relayer_id: &str,
        statuses: &[TransactionStatus],
    ) -> Result<u64, RepositoryError> {
        let mut conn = self
            .get_connection(self.connections.reader(), "count_by_status")
            .await?;
        let mut total_count: u64 = 0;

        for status in statuses {
            // Ensure sorted set is migrated
            self.ensure_status_sorted_set(relayer_id, status).await?;

            let sorted_key = self.relayer_status_sorted_key(relayer_id, status);
            let count: u64 = conn
                .zcard(&sorted_key)
                .await
                .map_err(|e| self.map_redis_error(e, "count_by_status"))?;
            total_count += count;
        }

        debug!(relayer_id = %relayer_id, count = %total_count, "counted transactions by status");
        Ok(total_count)
    }

    async fn delete_by_ids(&self, ids: Vec<String>) -> Result<BatchDeleteResult, RepositoryError> {
        if ids.is_empty() {
            debug!("no transaction IDs provided for batch delete");
            return Ok(BatchDeleteResult::default());
        }

        debug!(count = %ids.len(), "batch deleting transactions by IDs (with fetch)");

        // Fetch transactions to get their data for index cleanup
        let batch_result = self.get_transactions_by_ids(&ids).await?;

        // Convert to delete requests
        let requests: Vec<TransactionDeleteRequest> = batch_result
            .results
            .iter()
            .map(|tx| TransactionDeleteRequest {
                id: tx.id.clone(),
                relayer_id: tx.relayer_id.clone(),
                nonce: self.extract_nonce(&tx.network_data),
            })
            .collect();

        // Track IDs that weren't found
        let mut result = self.delete_by_requests(requests).await?;

        // Add the IDs that weren't found during fetch
        for id in batch_result.failed_ids {
            result
                .failed
                .push((id.clone(), format!("Transaction with ID {id} not found")));
        }

        Ok(result)
    }

    async fn delete_by_requests(
        &self,
        requests: Vec<TransactionDeleteRequest>,
    ) -> Result<BatchDeleteResult, RepositoryError> {
        if requests.is_empty() {
            debug!("no delete requests provided for batch delete");
            return Ok(BatchDeleteResult::default());
        }

        debug!(count = %requests.len(), "batch deleting transactions by requests (no fetch)");
        let mut conn = self
            .get_connection(self.connections.primary(), "batch_delete_no_fetch")
            .await?;
        let mut pipe = redis::pipe();
        pipe.atomic();

        // All possible statuses for index cleanup
        let all_statuses = [
            TransactionStatus::Canceled,
            TransactionStatus::Pending,
            TransactionStatus::Sent,
            TransactionStatus::Submitted,
            TransactionStatus::Mined,
            TransactionStatus::Confirmed,
            TransactionStatus::Failed,
            TransactionStatus::Expired,
        ];

        // Build pipeline for all deletions and index removals
        for req in &requests {
            // Delete transaction data
            let tx_key = self.tx_key(&req.relayer_id, &req.id);
            pipe.del(&tx_key);

            // Delete reverse lookup
            let reverse_key = self.tx_to_relayer_key(&req.id);
            pipe.del(&reverse_key);

            // Remove from all possible status indexes
            for status in &all_statuses {
                let status_sorted_key = self.relayer_status_sorted_key(&req.relayer_id, status);
                pipe.zrem(&status_sorted_key, &req.id);

                let status_legacy_key = self.relayer_status_key(&req.relayer_id, status);
                pipe.srem(&status_legacy_key, &req.id);
            }

            // Remove nonce index if exists
            if let Some(nonce) = req.nonce {
                let nonce_key = self.relayer_nonce_key(&req.relayer_id, nonce);
                pipe.del(&nonce_key);
            }

            // Remove from per-relayer sorted set by created_at
            let relayer_sorted_key = self.relayer_tx_by_created_at_key(&req.relayer_id);
            pipe.zrem(&relayer_sorted_key, &req.id);
        }

        // Execute the entire pipeline in one round-trip
        match pipe.exec_async(&mut conn).await {
            Ok(_) => {
                let deleted_count = requests.len();
                debug!(
                    deleted_count = %deleted_count,
                    "batch delete completed"
                );
                Ok(BatchDeleteResult {
                    deleted_count,
                    failed: vec![],
                })
            }
            Err(e) => {
                error!(error = %e, "batch delete pipeline failed");
                // Mark all requests as failed
                let failed: Vec<(String, String)> = requests
                    .iter()
                    .map(|req| (req.id.clone(), format!("Redis pipeline error: {e}")))
                    .collect();
                Ok(BatchDeleteResult {
                    deleted_count: 0,
                    failed,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{evm::Speed, EvmTransactionData, NetworkType};
    use alloy::primitives::U256;
    use deadpool_redis::{Config, Runtime};
    use lazy_static::lazy_static;
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
                hash: Some(format!("0x{id}")),
                speed: Some(Speed::Fast),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                raw: None,
            }),
            noop_count: None,
            is_canceled: Some(false),
            metadata: None,
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

        let cfg = Config::from_url(&redis_url);
        let pool = Arc::new(
            cfg.builder()
                .expect("Failed to create pool builder")
                .max_size(16)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build Redis pool"),
        );

        // Create RedisConnections with same pool for both primary and reader (for testing)
        let connections = Arc::new(RedisConnections::new_single_pool(pool));

        let random_id = Uuid::new_v4().to_string();
        let key_prefix = format!("test_prefix:{random_id}");

        RedisTransactionRepository::new(connections, key_prefix)
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
        let cfg = Config::from_url(&redis_url);
        let pool = Arc::new(
            cfg.builder()
                .expect("Failed to create pool builder")
                .max_size(16)
                .runtime(Runtime::Tokio1)
                .build()
                .expect("Failed to build Redis pool"),
        );
        let connections = Arc::new(RedisConnections::new_single_pool(pool));

        let result = RedisTransactionRepository::new(connections, "".to_string());
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
    async fn test_find_by_relayer_id_sorted_by_created_at_newest_first() {
        let repo = setup_test_repo().await;
        let relayer_id = Uuid::new_v4().to_string();

        // Create transactions with different created_at timestamps
        let mut tx1 = create_test_transaction_with_relayer("test-1", &relayer_id);
        tx1.created_at = "2025-01-27T10:00:00.000000+00:00".to_string(); // Oldest

        let mut tx2 = create_test_transaction_with_relayer("test-2", &relayer_id);
        tx2.created_at = "2025-01-27T12:00:00.000000+00:00".to_string(); // Middle

        let mut tx3 = create_test_transaction_with_relayer("test-3", &relayer_id);
        tx3.created_at = "2025-01-27T14:00:00.000000+00:00".to_string(); // Newest

        // Create transactions in non-chronological order to ensure sorting works
        repo.create(tx2.clone()).await.unwrap(); // Middle first
        repo.create(tx1.clone()).await.unwrap(); // Oldest second
        repo.create(tx3.clone()).await.unwrap(); // Newest last

        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = repo.find_by_relayer_id(&relayer_id, query).await.unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.items.len(), 3);

        // Verify transactions are sorted by created_at descending (newest first)
        assert_eq!(
            result.items[0].id, "test-3",
            "First item should be newest (test-3)"
        );
        assert_eq!(
            result.items[0].created_at,
            "2025-01-27T14:00:00.000000+00:00"
        );

        assert_eq!(
            result.items[1].id, "test-2",
            "Second item should be middle (test-2)"
        );
        assert_eq!(
            result.items[1].created_at,
            "2025-01-27T12:00:00.000000+00:00"
        );

        assert_eq!(
            result.items[2].id, "test-1",
            "Third item should be oldest (test-1)"
        );
        assert_eq!(
            result.items[2].created_at,
            "2025-01-27T10:00:00.000000+00:00"
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_find_by_relayer_id_migration_from_old_index() {
        let repo = setup_test_repo().await;
        let relayer_id = Uuid::new_v4().to_string();

        // Create transactions with different created_at timestamps
        let mut tx1 = create_test_transaction_with_relayer("migrate-test-1", &relayer_id);
        tx1.created_at = "2025-01-27T10:00:00.000000+00:00".to_string(); // Oldest

        let mut tx2 = create_test_transaction_with_relayer("migrate-test-2", &relayer_id);
        tx2.created_at = "2025-01-27T12:00:00.000000+00:00".to_string(); // Middle

        let mut tx3 = create_test_transaction_with_relayer("migrate-test-3", &relayer_id);
        tx3.created_at = "2025-01-27T14:00:00.000000+00:00".to_string(); // Newest

        // Create transactions directly in Redis WITHOUT adding to sorted set
        // This simulates old transactions created before the sorted set index existed
        let mut conn = repo.connections.primary().get().await.unwrap();
        let relayer_list_key = repo.relayer_list_key();
        let _: () = conn.sadd(&relayer_list_key, &relayer_id).await.unwrap();

        for tx in &[&tx1, &tx2, &tx3] {
            let key = repo.tx_key(&tx.relayer_id, &tx.id);
            let reverse_key = repo.tx_to_relayer_key(&tx.id);
            let value = repo.serialize_entity(tx, |t| &t.id, "transaction").unwrap();

            let mut pipe = redis::pipe();
            pipe.atomic();
            pipe.set(&key, &value);
            pipe.set(&reverse_key, &tx.relayer_id);

            // Add to status index (but NOT to sorted set)
            let status_key = repo.relayer_status_key(&tx.relayer_id, &tx.status);
            pipe.sadd(&status_key, &tx.id);

            pipe.exec_async(&mut conn).await.unwrap();
        }

        // Verify sorted set is empty (transactions were created without sorted set index)
        let relayer_sorted_key = repo.relayer_tx_by_created_at_key(&relayer_id);
        let count: u64 = conn.zcard(&relayer_sorted_key).await.unwrap();
        assert_eq!(count, 0, "Sorted set should be empty for old transactions");

        // Call find_by_relayer_id - this should trigger migration
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = repo
            .find_by_relayer_id(&relayer_id, query.clone())
            .await
            .unwrap();

        // Verify migration happened - sorted set should now have entries
        let count_after: u64 = conn.zcard(&relayer_sorted_key).await.unwrap();
        assert_eq!(
            count_after, 3,
            "Sorted set should be populated after migration"
        );

        // Verify results are correct and sorted (newest first)
        assert_eq!(result.total, 3);
        assert_eq!(result.items.len(), 3);

        assert_eq!(
            result.items[0].id, "migrate-test-3",
            "First item should be newest after migration"
        );
        assert_eq!(
            result.items[0].created_at,
            "2025-01-27T14:00:00.000000+00:00"
        );

        assert_eq!(
            result.items[1].id, "migrate-test-2",
            "Second item should be middle after migration"
        );
        assert_eq!(
            result.items[1].created_at,
            "2025-01-27T12:00:00.000000+00:00"
        );

        assert_eq!(
            result.items[2].id, "migrate-test-1",
            "Third item should be oldest after migration"
        );
        assert_eq!(
            result.items[2].created_at,
            "2025-01-27T10:00:00.000000+00:00"
        );

        // Verify second call uses sorted set (no migration needed)
        let result2 = repo.find_by_relayer_id(&relayer_id, query).await.unwrap();
        assert_eq!(result2.total, 3);
        assert_eq!(result2.items.len(), 3);
        // Results should be identical since sorted set is now populated
        assert_eq!(result.items[0].id, result2.items[0].id);
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
    async fn test_find_by_status_paginated() {
        let repo = setup_test_repo().await;
        let relayer_id = Uuid::new_v4().to_string();

        // Create 5 pending transactions with different timestamps
        for i in 1..=5 {
            let tx_id = Uuid::new_v4().to_string();
            let mut tx = create_test_transaction_with_status(
                &tx_id,
                &relayer_id,
                TransactionStatus::Pending,
            );
            tx.created_at = format!("2025-01-27T{:02}:00:00.000000+00:00", 10 + i);
            repo.create(tx).await.unwrap();
        }

        // Create 2 confirmed transactions
        for i in 6..=7 {
            let tx_id = Uuid::new_v4().to_string();
            let mut tx = create_test_transaction_with_status(
                &tx_id,
                &relayer_id,
                TransactionStatus::Confirmed,
            );
            tx.created_at = format!("2025-01-27T{:02}:00:00.000000+00:00", 10 + i);
            repo.create(tx).await.unwrap();
        }

        // Test first page (2 items per page)
        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };
        let result = repo
            .find_by_status_paginated(&relayer_id, &[TransactionStatus::Pending], query, false)
            .await
            .unwrap();

        assert_eq!(result.total, 5);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.page, 1);
        assert_eq!(result.per_page, 2);

        // Test second page
        let query = PaginationQuery {
            page: 2,
            per_page: 2,
        };
        let result = repo
            .find_by_status_paginated(&relayer_id, &[TransactionStatus::Pending], query, false)
            .await
            .unwrap();

        assert_eq!(result.total, 5);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.page, 2);

        // Test last page (partial)
        let query = PaginationQuery {
            page: 3,
            per_page: 2,
        };
        let result = repo
            .find_by_status_paginated(&relayer_id, &[TransactionStatus::Pending], query, false)
            .await
            .unwrap();

        assert_eq!(result.total, 5);
        assert_eq!(result.items.len(), 1);

        // Test multiple statuses
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = repo
            .find_by_status_paginated(
                &relayer_id,
                &[TransactionStatus::Pending, TransactionStatus::Confirmed],
                query,
                false,
            )
            .await
            .unwrap();

        assert_eq!(result.total, 7);
        assert_eq!(result.items.len(), 7);

        // Test empty result
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = repo
            .find_by_status_paginated(&relayer_id, &[TransactionStatus::Failed], query, false)
            .await
            .unwrap();

        assert_eq!(result.total, 0);
        assert_eq!(result.items.len(), 0);
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_find_by_status_paginated_oldest_first() {
        let repo = setup_test_repo().await;
        let relayer_id = Uuid::new_v4().to_string();

        // Create 5 pending transactions with ascending timestamps
        for i in 1..=5 {
            let tx_id = format!("tx{}-{}", i, Uuid::new_v4());
            let mut tx = create_test_transaction(&tx_id);
            tx.relayer_id = relayer_id.clone();
            tx.status = TransactionStatus::Pending;
            tx.created_at = format!("2025-01-27T{:02}:00:00.000000+00:00", 10 + i);
            repo.create(tx).await.unwrap();
        }

        // Test oldest_first: true - should return oldest transactions first
        let query = PaginationQuery {
            page: 1,
            per_page: 3,
        };
        let result = repo
            .find_by_status_paginated(
                &relayer_id,
                &[TransactionStatus::Pending],
                query.clone(),
                true,
            )
            .await
            .unwrap();

        assert_eq!(result.total, 5);
        assert_eq!(result.items.len(), 3);
        // Verify ordering: oldest first (11:00, 12:00, 13:00)
        assert!(
            result.items[0].created_at < result.items[1].created_at,
            "First item should be older than second"
        );
        assert!(
            result.items[1].created_at < result.items[2].created_at,
            "Second item should be older than third"
        );

        // Contrast with oldest_first: false - should return newest first
        let result_newest = repo
            .find_by_status_paginated(&relayer_id, &[TransactionStatus::Pending], query, false)
            .await
            .unwrap();

        assert_eq!(result_newest.items.len(), 3);
        // Verify ordering: newest first (15:00, 14:00, 13:00)
        assert!(
            result_newest.items[0].created_at > result_newest.items[1].created_at,
            "First item should be newer than second"
        );
        assert!(
            result_newest.items[1].created_at > result_newest.items[2].created_at,
            "Second item should be newer than third"
        );
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_find_by_status_paginated_oldest_first_single_item() {
        let repo = setup_test_repo().await;
        let relayer_id = Uuid::new_v4().to_string();

        // Create transactions with specific timestamps
        let timestamps = [
            "2025-01-27T08:00:00.000000+00:00", // oldest
            "2025-01-27T10:00:00.000000+00:00", // middle
            "2025-01-27T12:00:00.000000+00:00", // newest
        ];

        let mut oldest_id = String::new();
        let mut newest_id = String::new();

        for (i, timestamp) in timestamps.iter().enumerate() {
            let tx_id = format!("tx-{}-{}", i, Uuid::new_v4());
            if i == 0 {
                oldest_id = tx_id.clone();
            }
            if i == 2 {
                newest_id = tx_id.clone();
            }
            let mut tx = create_test_transaction(&tx_id);
            tx.relayer_id = relayer_id.clone();
            tx.status = TransactionStatus::Pending;
            tx.created_at = timestamp.to_string();
            repo.create(tx).await.unwrap();
        }

        // Request just 1 item with oldest_first: true
        let query = PaginationQuery {
            page: 1,
            per_page: 1,
        };
        let result = repo
            .find_by_status_paginated(
                &relayer_id,
                &[TransactionStatus::Pending],
                query.clone(),
                true,
            )
            .await
            .unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.items.len(), 1);
        assert_eq!(
            result.items[0].id, oldest_id,
            "With oldest_first=true and per_page=1, should return the oldest transaction"
        );

        // Contrast with oldest_first: false
        let result = repo
            .find_by_status_paginated(&relayer_id, &[TransactionStatus::Pending], query, false)
            .await
            .unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(
            result.items[0].id, newest_id,
            "With oldest_first=false and per_page=1, should return the newest transaction"
        );
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
            metadata: None,
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
        let debug_str = format!("{repo:?}");
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
                "delete_at should be set for status: {status:?}"
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
                duration_from_before >= expected_duration - tolerance
                    && duration_from_before <= expected_duration + tolerance,
                "delete_at should be approximately 6 hours from now for status: {status:?}. Duration: {duration_from_before:?}"
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
                "delete_at should NOT be set for status: {status:?}"
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
            "delete_at should be approximately 8 hours from now. Duration: {duration_from_before:?}"
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

    // Tests for delete_by_ids batch delete functionality

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_empty_list() {
        let repo = setup_test_repo().await;
        let tx_id = format!("test-empty-{}", Uuid::new_v4());

        // Create a transaction to ensure repo is not empty
        let tx = create_test_transaction(&tx_id);
        repo.create(tx).await.unwrap();

        // Delete with empty list should succeed and not affect existing data
        let result = repo.delete_by_ids(vec![]).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert!(result.failed.is_empty());

        // Original transaction should still exist
        assert!(repo.get_by_id(tx_id).await.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_single_transaction() {
        let repo = setup_test_repo().await;
        let tx_id = format!("test-single-{}", Uuid::new_v4());

        let tx = create_test_transaction(&tx_id);
        repo.create(tx).await.unwrap();

        let result = repo.delete_by_ids(vec![tx_id.clone()]).await.unwrap();

        assert_eq!(result.deleted_count, 1);
        assert!(result.failed.is_empty());

        // Verify transaction was deleted
        assert!(repo.get_by_id(tx_id).await.is_err());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_multiple_transactions() {
        let repo = setup_test_repo().await;
        let base_id = Uuid::new_v4();

        // Create multiple transactions
        let mut created_ids = Vec::new();
        for i in 1..=5 {
            let tx_id = format!("test-multi-{base_id}-{i}");
            let tx = create_test_transaction(&tx_id);
            repo.create(tx).await.unwrap();
            created_ids.push(tx_id);
        }

        // Delete 3 of them
        let ids_to_delete = vec![
            created_ids[0].clone(),
            created_ids[2].clone(),
            created_ids[4].clone(),
        ];
        let result = repo.delete_by_ids(ids_to_delete).await.unwrap();

        assert_eq!(result.deleted_count, 3);
        assert!(result.failed.is_empty());

        // Verify correct transactions were deleted
        assert!(repo.get_by_id(created_ids[0].clone()).await.is_err());
        assert!(repo.get_by_id(created_ids[1].clone()).await.is_ok()); // Not deleted
        assert!(repo.get_by_id(created_ids[2].clone()).await.is_err());
        assert!(repo.get_by_id(created_ids[3].clone()).await.is_ok()); // Not deleted
        assert!(repo.get_by_id(created_ids[4].clone()).await.is_err());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_nonexistent_transactions() {
        let repo = setup_test_repo().await;
        let base_id = Uuid::new_v4();

        // Try to delete transactions that don't exist
        let ids_to_delete = vec![
            format!("nonexistent-{}-1", base_id),
            format!("nonexistent-{}-2", base_id),
        ];
        let result = repo.delete_by_ids(ids_to_delete.clone()).await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert_eq!(result.failed.len(), 2);

        // Verify error messages contain the IDs
        let failed_ids: Vec<&String> = result.failed.iter().map(|(id, _)| id).collect();
        assert!(failed_ids.contains(&&ids_to_delete[0]));
        assert!(failed_ids.contains(&&ids_to_delete[1]));
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_mixed_existing_and_nonexistent() {
        let repo = setup_test_repo().await;
        let base_id = Uuid::new_v4();

        // Create some transactions
        let existing_ids: Vec<String> = (1..=3)
            .map(|i| format!("test-mixed-existing-{base_id}-{i}"))
            .collect();

        for id in &existing_ids {
            let tx = create_test_transaction(id);
            repo.create(tx).await.unwrap();
        }

        let nonexistent_ids: Vec<String> = (1..=2)
            .map(|i| format!("test-mixed-nonexistent-{base_id}-{i}"))
            .collect();

        // Try to delete mix of existing and non-existing
        let ids_to_delete = vec![
            existing_ids[0].clone(),
            nonexistent_ids[0].clone(),
            existing_ids[1].clone(),
            nonexistent_ids[1].clone(),
        ];
        let result = repo.delete_by_ids(ids_to_delete).await.unwrap();

        assert_eq!(result.deleted_count, 2);
        assert_eq!(result.failed.len(), 2);

        // Verify existing transactions were deleted
        assert!(repo.get_by_id(existing_ids[0].clone()).await.is_err());
        assert!(repo.get_by_id(existing_ids[1].clone()).await.is_err());

        // Verify remaining transaction still exists
        assert!(repo.get_by_id(existing_ids[2].clone()).await.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_removes_all_indexes() {
        let repo = setup_test_repo().await;
        let relayer_id = format!("relayer-{}", Uuid::new_v4());
        let tx_id = format!("test-indexes-{}", Uuid::new_v4());

        // Create a transaction with specific status
        let mut tx = create_test_transaction(&tx_id);
        tx.relayer_id = relayer_id.clone();
        tx.status = TransactionStatus::Confirmed;
        repo.create(tx).await.unwrap();

        // Verify transaction exists and is indexed
        let found = repo
            .find_by_status(&relayer_id, &[TransactionStatus::Confirmed])
            .await
            .unwrap();
        assert!(found.iter().any(|t| t.id == tx_id));

        // Delete the transaction
        let result = repo.delete_by_ids(vec![tx_id.clone()]).await.unwrap();
        assert_eq!(result.deleted_count, 1);

        // Verify transaction is no longer in status index
        let found_after = repo
            .find_by_status(&relayer_id, &[TransactionStatus::Confirmed])
            .await
            .unwrap();
        assert!(!found_after.iter().any(|t| t.id == tx_id));

        // Verify transaction cannot be found
        assert!(repo.get_by_id(tx_id).await.is_err());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_removes_nonce_index() {
        let repo = setup_test_repo().await;
        let relayer_id = format!("relayer-{}", Uuid::new_v4());
        let tx_id = format!("test-nonce-{}", Uuid::new_v4());
        let nonce = 12345u64;

        // Create a transaction with a specific nonce
        let tx = create_test_transaction_with_nonce(&tx_id, nonce, &relayer_id);
        repo.create(tx).await.unwrap();

        // Verify nonce index works
        let found = repo.find_by_nonce(&relayer_id, nonce).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, tx_id);

        // Delete the transaction
        let result = repo.delete_by_ids(vec![tx_id.clone()]).await.unwrap();
        assert_eq!(result.deleted_count, 1);

        // Verify nonce index was cleaned up
        let found_after = repo.find_by_nonce(&relayer_id, nonce).await.unwrap();
        assert!(found_after.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_large_batch() {
        let repo = setup_test_repo().await;
        let base_id = Uuid::new_v4();

        // Create many transactions to test batch performance
        let count = 50;
        let mut created_ids = Vec::new();

        for i in 0..count {
            let tx_id = format!("test-large-{base_id}-{i}");
            let tx = create_test_transaction(&tx_id);
            repo.create(tx).await.unwrap();
            created_ids.push(tx_id);
        }

        // Delete all of them in one batch
        let result = repo.delete_by_ids(created_ids.clone()).await.unwrap();

        assert_eq!(result.deleted_count, count);
        assert!(result.failed.is_empty());

        // Verify all were deleted
        for id in created_ids {
            assert!(repo.get_by_id(id).await.is_err());
        }
    }

    #[tokio::test]
    #[ignore = "Requires active Redis instance"]
    async fn test_delete_by_ids_preserves_other_relayer_transactions() {
        let repo = setup_test_repo().await;
        let relayer_1 = format!("relayer-1-{}", Uuid::new_v4());
        let relayer_2 = format!("relayer-2-{}", Uuid::new_v4());
        let tx_id_1 = format!("tx-relayer-1-{}", Uuid::new_v4());
        let tx_id_2 = format!("tx-relayer-2-{}", Uuid::new_v4());

        // Create transactions for different relayers
        let tx1 = create_test_transaction_with_relayer(&tx_id_1, &relayer_1);
        let tx2 = create_test_transaction_with_relayer(&tx_id_2, &relayer_2);

        repo.create(tx1).await.unwrap();
        repo.create(tx2).await.unwrap();

        // Delete only relayer-1's transaction
        let result = repo.delete_by_ids(vec![tx_id_1.clone()]).await.unwrap();

        assert_eq!(result.deleted_count, 1);

        // relayer-1's transaction should be deleted
        assert!(repo.get_by_id(tx_id_1).await.is_err());

        // relayer-2's transaction should still exist
        let remaining = repo.get_by_id(tx_id_2).await.unwrap();
        assert_eq!(remaining.relayer_id, relayer_2);
    }
}
