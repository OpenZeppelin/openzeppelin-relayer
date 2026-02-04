//! Transaction cleanup worker implementation.
//!
//! This module implements the transaction cleanup worker that processes
//! expired transactions marked for deletion. It runs as a cron job to
//! automatically clean up transactions that have passed their delete_at timestamp.
//!
//! ## Distributed Lock
//!
//! Since this runs on multiple service instances simultaneously (each with its own
//! CronStream), a distributed lock is used to ensure only one instance processes
//! the cleanup at a time. The lock has a 9-minute TTL (the cron runs every 10 minutes),
//! ensuring the lock expires before the next scheduled run.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, BoxDynError, Data};
use apalis_cron::Tick;
use chrono::{DateTime, Utc};
use eyre::Result;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

use crate::{
    constants::{FINAL_TRANSACTION_STATUSES, WORKER_TRANSACTION_CLEANUP_RETRIES},
    jobs::handle_result,
    models::{
        DefaultAppState, NetworkTransactionData, PaginationQuery, RelayerRepoModel,
        TransactionRepoModel,
    },
    repositories::{PaginatedResult, Repository, TransactionDeleteRequest, TransactionRepository},
    utils::DistributedLock,
};

/// Maximum number of relayers to process concurrently
const MAX_CONCURRENT_RELAYERS: usize = 10;

/// Number of transactions to fetch per page during cleanup
const CLEANUP_PAGE_SIZE: u32 = 100;

/// Maximum number of transactions to delete in a single batch operation.
/// This prevents overwhelming Redis with very large pipelines.
const DELETE_BATCH_SIZE: usize = 100;

/// Distributed lock name for transaction cleanup.
/// Only one instance across the cluster should run cleanup at a time.
const CLEANUP_LOCK_NAME: &str = "transaction_cleanup";

/// TTL for the distributed lock (9 minutes).
///
/// This value should be:
/// 1. Greater than the worst-case cleanup runtime to prevent concurrent execution
/// 2. Less than the cron interval (10 minutes) to ensure availability for the next run
///
/// If cleanup consistently takes longer than this TTL, another instance may acquire
/// the lock and run concurrently. In that case, consider:
/// - Increasing the TTL (and cron interval accordingly)
/// - Optimizing the cleanup process
/// - Implementing lock refresh during long-running operations
///
/// The lock is automatically released when processing completes (via Drop),
/// so the TTL primarily serves as a safety net for crashed instances.
const CLEANUP_LOCK_TTL_SECS: u64 = 9 * 60;

/// Handles periodic transaction cleanup jobs from the queue.
///
/// This function processes expired transactions by:
/// 1. Fetching all relayers from the system
/// 2. For each relayer, finding transactions with final statuses
/// 3. Checking if their delete_at timestamp has passed
/// 4. Validating transactions are in final states before deletion
/// 5. Deleting transactions that have expired (in parallel)
///
/// # Arguments
/// * `job` - The cron reminder job triggering the cleanup
/// * `data` - Application state containing repositories
/// * `attempt` - Current attempt number for retry logic
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of cleanup processing
#[instrument(
    level = "debug",
    skip(job, data),
    fields(
        job_type = "transaction_cleanup",
        attempt = %attempt.current(),
    ),
    err
)]
pub async fn transaction_cleanup_handler(
    job: Tick<Utc>,
    data: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), BoxDynError> {
    let result = handle_request(job, data, attempt.clone()).await;

    handle_result(
        result,
        attempt,
        "TransactionCleanup",
        WORKER_TRANSACTION_CLEANUP_RETRIES,
    )
}

/// Handles the actual transaction cleanup request logic.
///
/// This function first attempts to acquire a distributed lock to ensure only
/// one instance processes cleanup at a time. If the lock is already held by
/// another instance, this returns early without doing any work.
///
/// # Arguments
/// * `_job` - The cron reminder job (currently unused)
/// * `data` - Application state containing repositories
/// * `_attempt` - Current attempt number (currently unused)
///
/// # Returns
/// * `Result<()>` - Success or failure of the cleanup operation
async fn handle_request(
    _job: Tick<Utc>,
    data: Data<ThinData<DefaultAppState>>,
    _attempt: Attempt,
) -> Result<()> {
    let transaction_repo = data.transaction_repository();

    // Attempt to acquire distributed lock to prevent multiple instances from
    // running cleanup simultaneously. This is necessary because CronStream
    // is local to each instance, not distributed via Redis queues.
    // The lock key includes the relayer prefix to support multi-tenant deployments.
    // Key format: {prefix}:lock:{name} (e.g., "oz-relayer:lock:transaction_cleanup")
    let lock_guard = if let Some((conn, prefix)) = transaction_repo.connection_info() {
        let lock_key = format!("{prefix}:lock:{CLEANUP_LOCK_NAME}");
        let lock =
            DistributedLock::new(conn, &lock_key, Duration::from_secs(CLEANUP_LOCK_TTL_SECS));

        match lock.try_acquire().await {
            Ok(Some(guard)) => {
                debug!(lock_key = %lock_key, "acquired distributed lock for transaction cleanup");
                Some(guard)
            }
            Ok(None) => {
                info!(lock_key = %lock_key, "transaction cleanup skipped - another instance is processing");
                return Ok(());
            }
            Err(e) => {
                // If we can't communicate with Redis for locking, log warning but proceed
                // This maintains backwards compatibility and handles Redis connection issues
                warn!(
                    error = %e,
                    lock_key = %lock_key,
                    "failed to acquire distributed lock, proceeding with cleanup anyway"
                );
                None
            }
        }
    } else {
        debug!("in-memory repository detected, skipping distributed lock");
        None
    };

    let now = Utc::now();
    info!(
        timestamp = %now.to_rfc3339(),
        "executing transaction cleanup from storage"
    );

    let relayer_repo = data.relayer_repository();

    // Fetch all relayers
    let relayers = relayer_repo.list_all().await.map_err(|e| {
        error!(
            error = %e,
            "failed to fetch relayers for cleanup"
        );
        eyre::eyre!("Failed to fetch relayers: {}", e)
    })?;

    info!(
        relayer_count = relayers.len(),
        "found relayers to process for cleanup"
    );

    // Process relayers in parallel batches
    let cleanup_results = process_relayers_in_batches(relayers, transaction_repo, now).await;

    // Aggregate and report results
    let result = report_cleanup_results(cleanup_results).await;

    // Lock guard is automatically released when dropped (via Drop impl).
    // This happens regardless of whether we exit normally or via early return/error.
    drop(lock_guard);

    result
}

/// Processes multiple relayers in parallel batches for cleanup.
///
/// # Arguments
/// * `relayers` - List of relayers to process
/// * `transaction_repo` - Reference to the transaction repository
/// * `now` - Current UTC timestamp for comparison
///
/// # Returns
/// * `Vec<RelayerCleanupResult>` - Results from processing each relayer
async fn process_relayers_in_batches(
    relayers: Vec<RelayerRepoModel>,
    transaction_repo: Arc<impl TransactionRepository>,
    now: DateTime<Utc>,
) -> Vec<RelayerCleanupResult> {
    use futures::stream::{self, StreamExt};

    // Process relayers with limited concurrency to avoid overwhelming the system
    let results: Vec<RelayerCleanupResult> = stream::iter(relayers)
        .map(|relayer| {
            let repo_clone = Arc::clone(&transaction_repo);
            async move { process_single_relayer(relayer, repo_clone, now).await }
        })
        .buffer_unordered(MAX_CONCURRENT_RELAYERS)
        .collect()
        .await;

    results
}

/// Result of processing a single relayer's transactions.
#[derive(Debug)]
struct RelayerCleanupResult {
    relayer_id: String,
    cleaned_count: usize,
    error: Option<String>,
}

/// Processes cleanup for a single relayer using pagination.
///
/// Iterates through pages of final transactions to avoid loading all
/// transactions into memory at once.
///
/// # Arguments
/// * `relayer` - The relayer to process
/// * `transaction_repo` - Reference to the transaction repository
/// * `now` - Current UTC timestamp for comparison
///
/// # Returns
/// * `RelayerCleanupResult` - Result of processing this relayer
async fn process_single_relayer(
    relayer: RelayerRepoModel,
    transaction_repo: Arc<impl TransactionRepository>,
    now: DateTime<Utc>,
) -> RelayerCleanupResult {
    debug!(
        relayer_id = %relayer.id,
        "processing cleanup for relayer"
    );

    let mut total_cleaned = 0usize;
    let mut current_page = 1u32;

    loop {
        let query = PaginationQuery {
            page: current_page,
            per_page: CLEANUP_PAGE_SIZE,
        };

        match fetch_final_transactions_paginated(&relayer.id, &transaction_repo, query).await {
            Ok(page_result) => {
                let page_count = page_result.items.len();

                if page_count == 0 {
                    // No more transactions to process
                    break;
                }

                debug!(
                    page = current_page,
                    page_count = page_count,
                    total = page_result.total,
                    relayer_id = %relayer.id,
                    "processing page of final transactions"
                );

                let cleaned_count = process_transactions_for_cleanup(
                    page_result.items,
                    &transaction_repo,
                    &relayer.id,
                    now,
                )
                .await;

                total_cleaned += cleaned_count;

                // Check if we've processed all pages
                let total_pages =
                    (page_result.total as f64 / CLEANUP_PAGE_SIZE as f64).ceil() as u32;
                if current_page >= total_pages {
                    break;
                }

                current_page += 1;
            }
            Err(e) => {
                error!(
                    error = %e,
                    relayer_id = %relayer.id,
                    page = current_page,
                    "failed to fetch final transactions page"
                );
                return RelayerCleanupResult {
                    relayer_id: relayer.id,
                    cleaned_count: total_cleaned,
                    error: Some(e.to_string()),
                };
            }
        }
    }

    if total_cleaned > 0 {
        info!(
            cleaned_count = total_cleaned,
            relayer_id = %relayer.id,
            "cleaned up expired transactions"
        );
    }

    RelayerCleanupResult {
        relayer_id: relayer.id,
        cleaned_count: total_cleaned,
        error: None,
    }
}

/// Fetches a page of transactions with final statuses for a specific relayer.
///
/// # Arguments
/// * `relayer_id` - ID of the relayer
/// * `transaction_repo` - Reference to the transaction repository
/// * `query` - Pagination query specifying page and page size
///
/// # Returns
/// * `Result<PaginatedResult<TransactionRepoModel>>` - Paginated transactions with final statuses
async fn fetch_final_transactions_paginated(
    relayer_id: &str,
    transaction_repo: &Arc<impl TransactionRepository>,
    query: PaginationQuery,
) -> Result<PaginatedResult<TransactionRepoModel>> {
    transaction_repo
        .find_by_status_paginated(relayer_id, FINAL_TRANSACTION_STATUSES, query, true)
        .await
        .map_err(|e| {
            eyre::eyre!(
                "Failed to fetch final transactions for relayer {}: {}",
                relayer_id,
                e
            )
        })
}

/// Processes a list of transactions for cleanup using batch delete, deleting expired ones.
///
/// This function validates that transactions are in final states before deletion,
/// ensuring data integrity by preventing accidental deletion of active transactions.
/// Uses batch deletion for improved performance with large numbers of transactions.
///
/// # Arguments
/// * `transactions` - List of transactions to process
/// * `transaction_repo` - Reference to the transaction repository
/// * `relayer_id` - ID of the relayer (for logging)
/// * `now` - Current UTC timestamp for comparison
///
/// # Returns
/// * `usize` - Number of transactions successfully cleaned up
async fn process_transactions_for_cleanup(
    transactions: Vec<TransactionRepoModel>,
    transaction_repo: &Arc<impl TransactionRepository>,
    relayer_id: &str,
    now: DateTime<Utc>,
) -> usize {
    if transactions.is_empty() {
        return 0;
    }

    debug!(
        transaction_count = transactions.len(),
        relayer_id = %relayer_id,
        "processing transactions for cleanup"
    );

    // Filter expired transactions and validate they are in final states,
    // then convert to delete requests with pre-extracted data
    let delete_requests: Vec<TransactionDeleteRequest> = transactions
        .into_iter()
        .filter(|tx| {
            // Must be in a final state
            if !FINAL_TRANSACTION_STATUSES.contains(&tx.status) {
                warn!(
                    tx_id = %tx.id,
                    status = ?tx.status,
                    "skipping transaction not in final state"
                );
                return false;
            }
            // Must be expired
            should_delete_transaction(tx, now)
        })
        .map(|tx| {
            // Extract nonce from network data for index cleanup
            let nonce = extract_nonce_from_network_data(&tx.network_data);
            TransactionDeleteRequest::new(tx.id, tx.relayer_id, nonce)
        })
        .collect();

    if delete_requests.is_empty() {
        debug!(
            relayer_id = %relayer_id,
            "no expired transactions found"
        );
        return 0;
    }

    let total_expired = delete_requests.len();
    debug!(
        expired_count = total_expired,
        relayer_id = %relayer_id,
        "found expired transactions to delete"
    );

    // Process deletions in batches to avoid overwhelming Redis with large pipelines
    let mut total_deleted = 0;
    let mut total_failed = 0;

    for (batch_idx, batch) in delete_requests.chunks(DELETE_BATCH_SIZE).enumerate() {
        let batch_requests: Vec<TransactionDeleteRequest> = batch.to_vec();
        let batch_size = batch_requests.len();

        debug!(
            batch = batch_idx + 1,
            batch_size = batch_size,
            relayer_id = %relayer_id,
            "processing delete batch"
        );

        match transaction_repo.delete_by_requests(batch_requests).await {
            Ok(result) => {
                if !result.failed.is_empty() {
                    for (id, error) in &result.failed {
                        error!(
                            tx_id = %id,
                            error = %error,
                            relayer_id = %relayer_id,
                            "failed to delete expired transaction in batch"
                        );
                    }
                }

                total_deleted += result.deleted_count;
                total_failed += result.failed.len();
            }
            Err(e) => {
                error!(
                    error = %e,
                    relayer_id = %relayer_id,
                    batch = batch_idx + 1,
                    batch_size = batch_size,
                    "batch delete failed completely"
                );
                total_failed += batch_size;
            }
        }
    }

    debug!(
        total_deleted,
        total_failed,
        total_expired,
        relayer_id = %relayer_id,
        "batch delete completed"
    );

    total_deleted
}

/// Extracts the nonce from network transaction data if available.
/// This is used for cleaning up nonce indexes during deletion.
fn extract_nonce_from_network_data(network_data: &NetworkTransactionData) -> Option<u64> {
    match network_data {
        NetworkTransactionData::Evm(evm_data) => evm_data.nonce,
        _ => None,
    }
}

/// Determines if a transaction should be deleted based on its delete_at timestamp.
///
/// # Arguments
/// * `transaction` - The transaction to check
/// * `now` - Current UTC timestamp for comparison
///
/// # Returns
/// * `bool` - True if the transaction should be deleted, false otherwise
fn should_delete_transaction(transaction: &TransactionRepoModel, now: DateTime<Utc>) -> bool {
    transaction
        .delete_at
        .as_ref()
        .and_then(|delete_at_str| DateTime::parse_from_rfc3339(delete_at_str).ok())
        .map(|delete_at| {
            let is_expired = now >= delete_at.with_timezone(&Utc);
            if is_expired {
                debug!(
                    tx_id = %transaction.id,
                    expired_at = %delete_at.to_rfc3339(),
                    "transaction is expired"
                );
            }
            is_expired
        })
        .unwrap_or_else(|| {
            if transaction.delete_at.is_some() {
                warn!(
                    tx_id = %transaction.id,
                    "transaction has invalid delete_at timestamp"
                );
            }
            false
        })
}

/// Reports the aggregated results of the cleanup operation.
///
/// # Arguments
/// * `cleanup_results` - Results from processing all relayers
///
/// # Returns
/// * `Result<()>` - Success if all went well, error if there were failures
async fn report_cleanup_results(cleanup_results: Vec<RelayerCleanupResult>) -> Result<()> {
    let total_cleaned: usize = cleanup_results.iter().map(|r| r.cleaned_count).sum();
    let total_errors = cleanup_results.iter().filter(|r| r.error.is_some()).count();
    let total_relayers = cleanup_results.len();

    // Log detailed results for relayers with errors
    for result in &cleanup_results {
        if let Some(error) = &result.error {
            error!(
                relayer_id = %result.relayer_id,
                error = %error,
                "failed to cleanup transactions for relayer"
            );
        }
    }

    if total_errors > 0 {
        warn!(
            total_errors,
            total_relayers, total_cleaned, "transaction cleanup completed with errors"
        );

        // Return error if there were failures, but don't fail the entire job
        // This allows for partial success and retry of failed relayers
        Err(eyre::eyre!(
            "Cleanup completed with {} errors out of {} relayers",
            total_errors,
            total_relayers
        ))
    } else {
        info!(
            total_cleaned,
            total_relayers, "transaction cleanup completed successfully"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        models::{
            NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel,
            TransactionRepoModel, TransactionStatus,
        },
        repositories::{InMemoryTransactionRepository, Repository},
        utils::mocks::mockutils::create_mock_transaction,
    };
    use chrono::{Duration, Utc};

    fn create_test_transaction(
        id: &str,
        relayer_id: &str,
        status: TransactionStatus,
        delete_at: Option<String>,
    ) -> TransactionRepoModel {
        let mut tx = create_mock_transaction();
        tx.id = id.to_string();
        tx.relayer_id = relayer_id.to_string();
        tx.status = status;
        tx.delete_at = delete_at;
        tx
    }

    #[tokio::test]
    async fn test_should_delete_transaction_expired() {
        let now = Utc::now();
        let expired_delete_at = (now - Duration::hours(1)).to_rfc3339();

        let transaction = create_test_transaction(
            "test-tx",
            "test-relayer",
            TransactionStatus::Confirmed,
            Some(expired_delete_at),
        );

        assert!(should_delete_transaction(&transaction, now));
    }

    #[tokio::test]
    async fn test_should_delete_transaction_not_expired() {
        let now = Utc::now();
        let future_delete_at = (now + Duration::hours(1)).to_rfc3339();

        let transaction = create_test_transaction(
            "test-tx",
            "test-relayer",
            TransactionStatus::Confirmed,
            Some(future_delete_at),
        );

        assert!(!should_delete_transaction(&transaction, now));
    }

    #[tokio::test]
    async fn test_should_delete_transaction_no_delete_at() {
        let now = Utc::now();

        let transaction = create_test_transaction(
            "test-tx",
            "test-relayer",
            TransactionStatus::Confirmed,
            None,
        );

        assert!(!should_delete_transaction(&transaction, now));
    }

    #[tokio::test]
    async fn test_should_delete_transaction_invalid_timestamp() {
        let now = Utc::now();

        let transaction = create_test_transaction(
            "test-tx",
            "test-relayer",
            TransactionStatus::Confirmed,
            Some("invalid-timestamp".to_string()),
        );

        assert!(!should_delete_transaction(&transaction, now));
    }

    #[tokio::test]
    async fn test_process_transactions_for_cleanup_parallel() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";
        let now = Utc::now();

        // Create test transactions
        let expired_delete_at = (now - Duration::hours(1)).to_rfc3339();
        let future_delete_at = (now + Duration::hours(1)).to_rfc3339();

        let expired_tx = create_test_transaction(
            "expired-tx",
            relayer_id,
            TransactionStatus::Confirmed,
            Some(expired_delete_at),
        );
        let future_tx = create_test_transaction(
            "future-tx",
            relayer_id,
            TransactionStatus::Failed,
            Some(future_delete_at),
        );
        let no_delete_tx = create_test_transaction(
            "no-delete-tx",
            relayer_id,
            TransactionStatus::Canceled,
            None,
        );

        // Store transactions
        transaction_repo.create(expired_tx.clone()).await.unwrap();
        transaction_repo.create(future_tx.clone()).await.unwrap();
        transaction_repo.create(no_delete_tx.clone()).await.unwrap();

        let transactions = vec![expired_tx, future_tx, no_delete_tx];

        // Process transactions
        let cleaned_count =
            process_transactions_for_cleanup(transactions, &transaction_repo, relayer_id, now)
                .await;

        // Should have cleaned up 1 expired transaction
        assert_eq!(cleaned_count, 1);

        // Verify expired transaction was deleted
        assert!(transaction_repo
            .get_by_id("expired-tx".to_string())
            .await
            .is_err());

        // Verify non-expired transactions still exist
        assert!(transaction_repo
            .get_by_id("future-tx".to_string())
            .await
            .is_ok());
        assert!(transaction_repo
            .get_by_id("no-delete-tx".to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_batch_delete_expired_transactions() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";
        let now = Utc::now();

        // Create multiple expired transactions
        let expired_delete_at = (now - Duration::hours(1)).to_rfc3339();

        for i in 0..5 {
            let tx = create_test_transaction(
                &format!("expired-tx-{}", i),
                relayer_id,
                TransactionStatus::Confirmed,
                Some(expired_delete_at.clone()),
            );
            transaction_repo.create(tx).await.unwrap();
        }

        // Verify they exist
        assert_eq!(transaction_repo.count().await.unwrap(), 5);

        // Delete them using batch delete
        let ids: Vec<String> = (0..5).map(|i| format!("expired-tx-{}", i)).collect();
        let result = transaction_repo.delete_by_ids(ids).await.unwrap();

        assert_eq!(result.deleted_count, 5);
        assert!(result.failed.is_empty());

        // Verify they were deleted
        assert_eq!(transaction_repo.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_batch_delete_with_nonexistent_ids() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        // Create one transaction
        let tx = create_test_transaction(
            "existing-tx",
            relayer_id,
            TransactionStatus::Confirmed,
            Some(Utc::now().to_rfc3339()),
        );
        transaction_repo.create(tx).await.unwrap();

        // Try to delete existing and non-existing transactions
        let ids = vec![
            "existing-tx".to_string(),
            "nonexistent-1".to_string(),
            "nonexistent-2".to_string(),
        ];
        let result = transaction_repo.delete_by_ids(ids).await.unwrap();

        // Should delete the existing one and report failures for the others
        assert_eq!(result.deleted_count, 1);
        assert_eq!(result.failed.len(), 2);

        // Verify the existing one was deleted
        assert!(transaction_repo
            .get_by_id("existing-tx".to_string())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_process_transactions_skips_non_final_status() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";
        let now = Utc::now();

        // Create a transaction with non-final status but expired delete_at
        let expired_delete_at = (now - Duration::hours(1)).to_rfc3339();
        let pending_tx = create_test_transaction(
            "pending-tx",
            relayer_id,
            TransactionStatus::Pending, // Non-final status
            Some(expired_delete_at),
        );
        transaction_repo.create(pending_tx.clone()).await.unwrap();

        let transactions = vec![pending_tx];

        // Process should skip non-final status transactions
        let cleaned_count =
            process_transactions_for_cleanup(transactions, &transaction_repo, relayer_id, now)
                .await;

        // Should not have cleaned any transactions
        assert_eq!(cleaned_count, 0);

        // Transaction should still exist
        assert!(transaction_repo
            .get_by_id("pending-tx".to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_fetch_final_transactions_paginated() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        // Create transactions with different statuses
        let confirmed_tx = create_test_transaction(
            "confirmed-tx",
            relayer_id,
            TransactionStatus::Confirmed,
            None,
        );
        let pending_tx =
            create_test_transaction("pending-tx", relayer_id, TransactionStatus::Pending, None);
        let failed_tx =
            create_test_transaction("failed-tx", relayer_id, TransactionStatus::Failed, None);

        // Store transactions
        transaction_repo.create(confirmed_tx).await.unwrap();
        transaction_repo.create(pending_tx).await.unwrap();
        transaction_repo.create(failed_tx).await.unwrap();

        // Fetch final transactions with pagination
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = fetch_final_transactions_paginated(relayer_id, &transaction_repo, query)
            .await
            .unwrap();

        // Should only return transactions with final statuses (Confirmed, Failed)
        assert_eq!(result.total, 2);
        assert_eq!(result.items.len(), 2);
        let final_ids: Vec<&String> = result.items.iter().map(|tx| &tx.id).collect();
        assert!(final_ids.contains(&&"confirmed-tx".to_string()));
        assert!(final_ids.contains(&&"failed-tx".to_string()));
        assert!(!final_ids.contains(&&"pending-tx".to_string()));
    }

    #[tokio::test]
    async fn test_report_cleanup_results_success() {
        let results = vec![
            RelayerCleanupResult {
                relayer_id: "relayer-1".to_string(),
                cleaned_count: 2,
                error: None,
            },
            RelayerCleanupResult {
                relayer_id: "relayer-2".to_string(),
                cleaned_count: 1,
                error: None,
            },
        ];

        let result = report_cleanup_results(results).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_report_cleanup_results_with_errors() {
        let results = vec![
            RelayerCleanupResult {
                relayer_id: "relayer-1".to_string(),
                cleaned_count: 2,
                error: None,
            },
            RelayerCleanupResult {
                relayer_id: "relayer-2".to_string(),
                cleaned_count: 0,
                error: Some("Database error".to_string()),
            },
        ];

        let result = report_cleanup_results(results).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_process_single_relayer_success() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer = RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            address: "0x1234567890123456789012345678901234567890".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        };
        let now = Utc::now();

        // Create expired and non-expired transactions
        let expired_tx = create_test_transaction(
            "expired-tx",
            &relayer.id,
            TransactionStatus::Confirmed,
            Some((now - Duration::hours(1)).to_rfc3339()),
        );
        let future_tx = create_test_transaction(
            "future-tx",
            &relayer.id,
            TransactionStatus::Failed,
            Some((now + Duration::hours(1)).to_rfc3339()),
        );

        transaction_repo.create(expired_tx).await.unwrap();
        transaction_repo.create(future_tx).await.unwrap();

        let result = process_single_relayer(relayer.clone(), transaction_repo.clone(), now).await;

        assert_eq!(result.relayer_id, relayer.id);
        assert_eq!(result.cleaned_count, 1);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_process_single_relayer_no_transactions() {
        // Create a relayer with no transactions in the repo
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer = RelayerRepoModel {
            id: "empty-relayer".to_string(),
            name: "Empty Relayer".to_string(),
            network: "ethereum".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            signer_id: "test-signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            address: "0x1234567890123456789012345678901234567890".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
            ..Default::default()
        };
        let now = Utc::now();

        // This should succeed but find no transactions
        let result = process_single_relayer(relayer.clone(), transaction_repo, now).await;

        assert_eq!(result.relayer_id, relayer.id);
        assert_eq!(result.cleaned_count, 0);
        assert!(result.error.is_none()); // No error, just no transactions found
    }

    #[tokio::test]
    async fn test_process_transactions_with_empty_list() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";
        let now = Utc::now();
        let transactions = vec![];

        let cleaned_count =
            process_transactions_for_cleanup(transactions, &transaction_repo, relayer_id, now)
                .await;

        assert_eq!(cleaned_count, 0);
    }

    #[tokio::test]
    async fn test_process_transactions_with_no_expired() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";
        let now = Utc::now();

        // Create only non-expired transactions
        let future_tx1 = create_test_transaction(
            "future-tx-1",
            relayer_id,
            TransactionStatus::Confirmed,
            Some((now + Duration::hours(1)).to_rfc3339()),
        );
        let future_tx2 = create_test_transaction(
            "future-tx-2",
            relayer_id,
            TransactionStatus::Failed,
            Some((now + Duration::hours(2)).to_rfc3339()),
        );
        let no_delete_tx = create_test_transaction(
            "no-delete-tx",
            relayer_id,
            TransactionStatus::Canceled,
            None,
        );

        let transactions = vec![future_tx1, future_tx2, no_delete_tx];

        let cleaned_count =
            process_transactions_for_cleanup(transactions, &transaction_repo, relayer_id, now)
                .await;

        assert_eq!(cleaned_count, 0);
    }

    #[tokio::test]
    async fn test_should_delete_transaction_exactly_at_expiry_time() {
        let now = Utc::now();
        let exact_expiry_time = now.to_rfc3339();

        let transaction = create_test_transaction(
            "test-tx",
            "test-relayer",
            TransactionStatus::Confirmed,
            Some(exact_expiry_time),
        );

        // Should be considered expired when exactly at expiry time
        assert!(should_delete_transaction(&transaction, now));
    }

    #[tokio::test]
    async fn test_parallel_processing_with_mixed_results() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";
        let now = Utc::now();

        // Create multiple expired transactions
        let expired_tx1 = create_test_transaction(
            "expired-tx-1",
            relayer_id,
            TransactionStatus::Confirmed,
            Some((now - Duration::hours(1)).to_rfc3339()),
        );
        let expired_tx2 = create_test_transaction(
            "expired-tx-2",
            relayer_id,
            TransactionStatus::Failed,
            Some((now - Duration::hours(2)).to_rfc3339()),
        );
        let expired_tx3 = create_test_transaction(
            "expired-tx-3",
            relayer_id,
            TransactionStatus::Canceled,
            Some((now - Duration::hours(3)).to_rfc3339()),
        );

        // Store only some transactions (others will fail deletion due to NotFound)
        transaction_repo.create(expired_tx1.clone()).await.unwrap();
        transaction_repo.create(expired_tx2.clone()).await.unwrap();
        // Don't store expired_tx3 - it will fail deletion

        let transactions = vec![expired_tx1, expired_tx2, expired_tx3];

        let cleaned_count =
            process_transactions_for_cleanup(transactions, &transaction_repo, relayer_id, now)
                .await;

        // Should have cleaned 2 out of 3 transactions (one failed due to NotFound)
        assert_eq!(cleaned_count, 2);
    }

    #[tokio::test]
    async fn test_report_cleanup_results_empty() {
        let results = vec![];
        let result = report_cleanup_results(results).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fetch_final_transactions_paginated_with_mixed_statuses() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        // Create transactions with all possible statuses
        let confirmed_tx = create_test_transaction(
            "confirmed-tx",
            relayer_id,
            TransactionStatus::Confirmed,
            None,
        );
        let failed_tx =
            create_test_transaction("failed-tx", relayer_id, TransactionStatus::Failed, None);
        let canceled_tx =
            create_test_transaction("canceled-tx", relayer_id, TransactionStatus::Canceled, None);
        let expired_tx =
            create_test_transaction("expired-tx", relayer_id, TransactionStatus::Expired, None);
        let pending_tx =
            create_test_transaction("pending-tx", relayer_id, TransactionStatus::Pending, None);
        let sent_tx = create_test_transaction("sent-tx", relayer_id, TransactionStatus::Sent, None);

        // Store all transactions
        transaction_repo.create(confirmed_tx).await.unwrap();
        transaction_repo.create(failed_tx).await.unwrap();
        transaction_repo.create(canceled_tx).await.unwrap();
        transaction_repo.create(expired_tx).await.unwrap();
        transaction_repo.create(pending_tx).await.unwrap();
        transaction_repo.create(sent_tx).await.unwrap();

        // Fetch final transactions with pagination
        let query = PaginationQuery {
            page: 1,
            per_page: 10,
        };
        let result = fetch_final_transactions_paginated(relayer_id, &transaction_repo, query)
            .await
            .unwrap();

        // Should only return the 4 final status transactions
        assert_eq!(result.total, 4);
        assert_eq!(result.items.len(), 4);
        let final_ids: Vec<&String> = result.items.iter().map(|tx| &tx.id).collect();
        assert!(final_ids.contains(&&"confirmed-tx".to_string()));
        assert!(final_ids.contains(&&"failed-tx".to_string()));
        assert!(final_ids.contains(&&"canceled-tx".to_string()));
        assert!(final_ids.contains(&&"expired-tx".to_string()));
        assert!(!final_ids.contains(&&"pending-tx".to_string()));
        assert!(!final_ids.contains(&&"sent-tx".to_string()));
    }

    #[tokio::test]
    async fn test_fetch_final_transactions_paginated_pagination() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        // Create 5 confirmed transactions
        for i in 1..=5 {
            let mut tx = create_test_transaction(
                &format!("tx-{}", i),
                relayer_id,
                TransactionStatus::Confirmed,
                None,
            );
            tx.created_at = format!("2025-01-27T{:02}:00:00.000000+00:00", 10 + i);
            transaction_repo.create(tx).await.unwrap();
        }

        // Test first page with 2 items
        let query = PaginationQuery {
            page: 1,
            per_page: 2,
        };
        let result = fetch_final_transactions_paginated(relayer_id, &transaction_repo, query)
            .await
            .unwrap();

        assert_eq!(result.total, 5);
        assert_eq!(result.items.len(), 2);
        assert_eq!(result.page, 1);

        // Test second page
        let query = PaginationQuery {
            page: 2,
            per_page: 2,
        };
        let result = fetch_final_transactions_paginated(relayer_id, &transaction_repo, query)
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
        let result = fetch_final_transactions_paginated(relayer_id, &transaction_repo, query)
            .await
            .unwrap();

        assert_eq!(result.total, 5);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.page, 3);
    }
}
