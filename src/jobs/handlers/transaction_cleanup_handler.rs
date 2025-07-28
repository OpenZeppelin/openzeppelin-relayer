//! Transaction cleanup worker implementation.
//!
//! This module implements the transaction cleanup worker that processes
//! expired transactions marked for deletion. It runs as a cron job to
//! automatically clean up transactions that have passed their delete_at timestamp.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use chrono::{DateTime, Utc};
use eyre::Result;
use log::{debug, error, info, warn};
use std::sync::Arc;

use crate::{
    constants::{FINAL_TRANSACTION_STATUSES, WORKER_DEFAULT_MAXIMUM_RETRIES},
    jobs::handle_result,
    models::{DefaultAppState, RelayerRepoModel, TransactionRepoModel},
    repositories::{Repository, TransactionRepository},
};

/// Maximum number of relayers to process concurrently
const MAX_CONCURRENT_RELAYERS: usize = 10;

/// Maximum number of transactions to process concurrently per relayer
const MAX_CONCURRENT_TRANSACTIONS_PER_RELAYER: usize = 50;

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
pub async fn transaction_cleanup_handler(
    job: TransactionCleanupCronReminder,
    data: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    let result = handle_request(job, data, attempt.clone()).await;

    handle_result(
        result,
        attempt,
        "TransactionCleanup",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

/// Represents a cron reminder job for triggering cleanup operations.
#[derive(Default, Debug, Clone)]
pub struct TransactionCleanupCronReminder();

/// Handles the actual transaction cleanup request logic.
///
/// # Arguments
/// * `_job` - The cron reminder job (currently unused)
/// * `data` - Application state containing repositories
/// * `_attempt` - Current attempt number (currently unused)
///
/// # Returns
/// * `Result<()>` - Success or failure of the cleanup operation
async fn handle_request(
    _job: TransactionCleanupCronReminder,
    data: Data<ThinData<DefaultAppState>>,
    _attempt: Attempt,
) -> Result<()> {
    let now = Utc::now();
    info!(
        "Executing transaction cleanup from storage at: {}",
        now.to_rfc3339()
    );

    let transaction_repo = data.transaction_repository();
    let relayer_repo = data.relayer_repository();

    // Fetch all relayers
    let relayers = relayer_repo.list_all().await.map_err(|e| {
        error!("Failed to fetch relayers for cleanup: {}", e);
        eyre::eyre!("Failed to fetch relayers: {}", e)
    })?;

    info!("Found {} relayers to process for cleanup", relayers.len());

    // Process relayers in parallel batches
    let cleanup_results = process_relayers_in_batches(relayers, transaction_repo, now).await;

    // Aggregate and report results
    report_cleanup_results(cleanup_results).await
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

/// Processes cleanup for a single relayer.
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
    debug!("Processing cleanup for relayer: {}", relayer.id);

    match fetch_final_transactions(&relayer.id, &transaction_repo).await {
        Ok(final_transactions) => {
            debug!(
                "Found {} transactions with final statuses for relayer: {}",
                final_transactions.len(),
                relayer.id
            );

            let cleaned_count = process_transactions_for_cleanup(
                final_transactions,
                &transaction_repo,
                &relayer.id,
                now,
            )
            .await;

            if cleaned_count > 0 {
                info!(
                    "Cleaned up {} expired transactions for relayer: {}",
                    cleaned_count, relayer.id
                );
            }

            RelayerCleanupResult {
                relayer_id: relayer.id,
                cleaned_count,
                error: None,
            }
        }
        Err(e) => {
            error!(
                "Failed to fetch final transactions for relayer {}: {}",
                relayer.id, e
            );
            RelayerCleanupResult {
                relayer_id: relayer.id,
                cleaned_count: 0,
                error: Some(e.to_string()),
            }
        }
    }
}

/// Fetches all transactions with final statuses for a specific relayer.
///
/// # Arguments
/// * `relayer_id` - ID of the relayer
/// * `transaction_repo` - Reference to the transaction repository
///
/// # Returns
/// * `Result<Vec<TransactionRepoModel>>` - List of transactions with final statuses or error
async fn fetch_final_transactions(
    relayer_id: &str,
    transaction_repo: &Arc<impl TransactionRepository>,
) -> Result<Vec<TransactionRepoModel>> {
    transaction_repo
        .find_by_status(relayer_id, FINAL_TRANSACTION_STATUSES)
        .await
        .map_err(|e| {
            eyre::eyre!(
                "Failed to fetch final transactions for relayer {}: {}",
                relayer_id,
                e
            )
        })
}

/// Processes a list of transactions for cleanup in parallel, deleting expired ones.
///
/// This function validates that transactions are in final states before deletion,
/// ensuring data integrity by preventing accidental deletion of active transactions.
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
    transaction_repo: &Arc<impl Repository<TransactionRepoModel, String>>,
    relayer_id: &str,
    now: DateTime<Utc>,
) -> usize {
    use futures::stream::{self, StreamExt};

    if transactions.is_empty() {
        return 0;
    }

    debug!(
        "Processing {} transactions in parallel for relayer: {}",
        transactions.len(),
        relayer_id
    );

    // Filter expired transactions first (this is fast and synchronous)
    let expired_transactions: Vec<TransactionRepoModel> = transactions
        .into_iter()
        .filter(|tx| should_delete_transaction(tx, now))
        .collect();

    if expired_transactions.is_empty() {
        debug!("No expired transactions found for relayer: {}", relayer_id);
        return 0;
    }

    debug!(
        "Found {} expired transactions to delete for relayer: {}",
        expired_transactions.len(),
        relayer_id
    );

    // Process deletions in parallel with limited concurrency
    let deletion_results: Vec<bool> = stream::iter(expired_transactions)
        .map(|transaction| {
            let repo_clone = Arc::clone(transaction_repo);
            let relayer_id = relayer_id.to_string();
            async move {
                match delete_expired_transaction(&transaction, &repo_clone, &relayer_id).await {
                    Ok(()) => true,
                    Err(e) => {
                        error!(
                            "Failed to delete expired transaction {}: {}",
                            transaction.id, e
                        );
                        false
                    }
                }
            }
        })
        .buffer_unordered(MAX_CONCURRENT_TRANSACTIONS_PER_RELAYER)
        .collect()
        .await;

    // Count successful deletions
    let cleaned_count = deletion_results.iter().filter(|&&success| success).count();

    debug!(
        "Successfully deleted {}/{} expired transactions for relayer: {}",
        cleaned_count,
        deletion_results.len(),
        relayer_id
    );

    cleaned_count
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
                    "Transaction {} is expired (expired at: {})",
                    transaction.id,
                    delete_at.to_rfc3339()
                );
            }
            is_expired
        })
        .unwrap_or_else(|| {
            if transaction.delete_at.is_some() {
                warn!(
                    "Transaction {} has invalid delete_at timestamp",
                    transaction.id
                );
            }
            false
        })
}

/// Deletes an expired transaction from the repository.
///
/// # Arguments
/// * `transaction` - The transaction to delete
/// * `transaction_repo` - Reference to the transaction repository
/// * `relayer_id` - ID of the relayer (for logging)
///
/// # Returns
/// * `Result<()>` - Success or failure of the deletion
async fn delete_expired_transaction(
    transaction: &TransactionRepoModel,
    transaction_repo: &Arc<impl Repository<TransactionRepoModel, String>>,
    relayer_id: &str,
) -> Result<()> {
    // Validate that the transaction is in a final state before deletion
    if !FINAL_TRANSACTION_STATUSES.contains(&transaction.status) {
        return Err(eyre::eyre!(
            "Transaction {} is not in a final state (current: {:?})",
            transaction.id,
            transaction.status
        ));
    }

    debug!(
        "Deleting expired transaction {} (status: {:?}) for relayer: {}",
        transaction.id, transaction.status, relayer_id
    );

    transaction_repo
        .delete_by_id(transaction.id.clone())
        .await
        .map_err(|e| eyre::eyre!("Failed to delete transaction {}: {}", transaction.id, e))?;

    info!(
        "Successfully deleted expired transaction: {} (status: {:?}) for relayer: {}",
        transaction.id, transaction.status, relayer_id
    );

    Ok(())
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
                "Failed to cleanup transactions for relayer {}: {}",
                result.relayer_id, error
            );
        }
    }

    if total_errors > 0 {
        warn!(
            "Transaction cleanup completed with {} errors out of {} relayers. Successfully cleaned {} transactions.",
            total_errors, total_relayers, total_cleaned
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
            "Transaction cleanup completed successfully. Cleaned {} transactions from {} relayers.",
            total_cleaned, total_relayers
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
    async fn test_delete_expired_transaction() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        let transaction = create_test_transaction(
            "test-tx",
            relayer_id,
            TransactionStatus::Confirmed, // Final status
            Some(Utc::now().to_rfc3339()),
        );

        // Store transaction
        transaction_repo.create(transaction.clone()).await.unwrap();

        // Verify it exists
        assert!(transaction_repo
            .get_by_id("test-tx".to_string())
            .await
            .is_ok());

        // Delete it
        let result = delete_expired_transaction(&transaction, &transaction_repo, relayer_id).await;
        assert!(result.is_ok());

        // Verify it was deleted
        assert!(transaction_repo
            .get_by_id("test-tx".to_string())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_delete_expired_transaction_validates_final_status() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        let transaction = create_test_transaction(
            "test-tx",
            relayer_id,
            TransactionStatus::Pending, // Non-final status
            Some(Utc::now().to_rfc3339()),
        );

        // Store transaction
        transaction_repo.create(transaction.clone()).await.unwrap();

        // Verify it exists
        assert!(transaction_repo
            .get_by_id("test-tx".to_string())
            .await
            .is_ok());

        // Try to delete it - should fail due to validation
        let result = delete_expired_transaction(&transaction, &transaction_repo, relayer_id).await;
        assert!(result.is_err());

        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("is not in a final state"));
        assert!(error_message.contains("Pending"));

        // Verify it still exists (wasn't deleted)
        assert!(transaction_repo
            .get_by_id("test-tx".to_string())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_delete_expired_transaction_validates_all_final_statuses() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        // Test each final status to ensure they all pass validation
        let final_statuses = [
            TransactionStatus::Confirmed,
            TransactionStatus::Failed,
            TransactionStatus::Canceled,
            TransactionStatus::Expired,
        ];

        for (i, status) in final_statuses.iter().enumerate() {
            let tx_id = format!("test-tx-{}", i);
            let transaction = create_test_transaction(
                &tx_id,
                relayer_id,
                status.clone(),
                Some(Utc::now().to_rfc3339()),
            );

            // Store transaction
            transaction_repo.create(transaction.clone()).await.unwrap();

            // Delete it - should succeed for all final statuses
            let result =
                delete_expired_transaction(&transaction, &transaction_repo, relayer_id).await;
            assert!(
                result.is_ok(),
                "Failed to delete transaction with status: {:?}",
                status
            );

            // Verify it was deleted
            assert!(transaction_repo.get_by_id(tx_id).await.is_err());
        }
    }

    #[tokio::test]
    async fn test_fetch_final_transactions() {
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

        // Fetch final transactions
        let final_transactions = fetch_final_transactions(relayer_id, &transaction_repo)
            .await
            .unwrap();

        // Should only return transactions with final statuses (Confirmed, Failed)
        assert_eq!(final_transactions.len(), 2);
        let final_ids: Vec<&String> = final_transactions.iter().map(|tx| &tx.id).collect();
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
    async fn test_delete_expired_transaction_repository_error() {
        let transaction_repo = Arc::new(InMemoryTransactionRepository::new());
        let relayer_id = "test-relayer";

        let transaction = create_test_transaction(
            "nonexistent-tx",
            relayer_id,
            TransactionStatus::Confirmed,
            Some(Utc::now().to_rfc3339()),
        );

        // Don't store the transaction, so delete will fail with NotFound
        let result = delete_expired_transaction(&transaction, &transaction_repo, relayer_id).await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("Failed to delete transaction"));
    }

    #[tokio::test]
    async fn test_report_cleanup_results_empty() {
        let results = vec![];
        let result = report_cleanup_results(results).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fetch_final_transactions_with_mixed_statuses() {
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

        // Fetch final transactions
        let final_transactions = fetch_final_transactions(relayer_id, &transaction_repo)
            .await
            .unwrap();

        // Should only return the 4 final status transactions
        assert_eq!(final_transactions.len(), 4);
        let final_ids: Vec<&String> = final_transactions.iter().map(|tx| &tx.id).collect();
        assert!(final_ids.contains(&&"confirmed-tx".to_string()));
        assert!(final_ids.contains(&&"failed-tx".to_string()));
        assert!(final_ids.contains(&&"canceled-tx".to_string()));
        assert!(final_ids.contains(&&"expired-tx".to_string()));
        assert!(!final_ids.contains(&&"pending-tx".to_string()));
        assert!(!final_ids.contains(&&"sent-tx".to_string()));
    }
}
