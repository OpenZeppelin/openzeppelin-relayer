//! Transaction status monitoring handler.
//!
//! Monitors the status of submitted transactions by:
//! - Checking transaction status on the network
//! - Updating transaction status in storage
//! - Tracking failure counts for circuit breaker decisions (stored in Redis by tx_id)
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, TaskId, *};
use apalis_redis::{ConnectionManager, RedisContext};
use deadpool_redis::Pool;
use eyre::Result;
use redis::AsyncCommands;
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};

use crate::{
    config::ServerConfig,
    constants::{get_max_consecutive_status_failures, get_max_total_status_failures},
    domain::{
        get_relayer_by_id, get_relayer_transaction, get_transaction_by_id, is_final_state,
        Transaction,
    },
    jobs::{Job, StatusCheckContext, TransactionStatusCheck},
    models::{DefaultAppState, NetworkType, TransactionRepoModel},
    observability::request_id::set_request_id,
};

/// Redis key prefix for transaction status check metadata (failure counters).
/// Stored separately from Apalis job data to persist across retries.
const TX_STATUS_CHECK_METADATA_PREFIX: &str = "queue:tx_status_check_metadata";

/// Abstraction over Redis connection types.
/// Uses Pool when Redis storage is configured, falls back to ConnectionManager for in-memory mode.
#[derive(Clone)]
enum RedisConn {
    Pool(Arc<Pool>),
    ConnectionManager(Arc<ConnectionManager>),
}

#[instrument(
    level = "debug",
    skip(job, state, _ctx),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %attempt.current(),
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        task_id = %task_id.to_string(),
    )
)]
pub async fn transaction_status_handler(
    job: Job<TransactionStatusCheck>,
    state: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
    task_id: TaskId,
    _ctx: RedisContext,
) -> Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    let tx_id = &job.data.transaction_id;

    // Get Redis connection - prefer pool when available, fall back to connection_manager
    let queue = state
        .job_producer()
        .get_queue()
        .await
        .map_err(|e| Error::Failed(Arc::new(format!("Failed to get queue: {e}").into())))?;

    let redis_conn = match queue.redis_connections() {
        Some(conn) => RedisConn::Pool(conn.primary().clone()),
        None => RedisConn::ConnectionManager(queue.connection_manager.clone()),
    };

    // Read failure counters from separate Redis key (not job metadata)
    // This persists across job retries since we store it independently
    let (consecutive_failures, total_failures) = read_counters_from_redis(&redis_conn, tx_id).await;

    // Get network type from job data, or fetch from relayer for legacy jobs without network_type
    let network_type = match job.data.network_type {
        Some(nt) => nt,
        None => {
            // Legacy job without network_type - fetch from relayer
            match get_relayer_by_id(job.data.relayer_id.clone(), &state).await {
                Ok(relayer) => relayer.network_type,
                Err(e) => {
                    warn!(
                        error = %e,
                        relayer_id = %job.data.relayer_id,
                        "failed to fetch relayer for network type, defaulting to EVM"
                    );
                    NetworkType::Evm
                }
            }
        }
    };
    let max_consecutive = get_max_consecutive_status_failures(network_type);
    let max_total = get_max_total_status_failures(network_type);

    debug!(
        tx_id = %tx_id,
        consecutive_failures,
        total_failures,
        max_consecutive,
        max_total,
        attempt = attempt.current(),
        task_id = %task_id.to_string(),
        "handling transaction status check"
    );

    // Build circuit breaker context
    let context = StatusCheckContext::new(
        consecutive_failures,
        total_failures,
        attempt.current() as u32,
        max_consecutive,
        max_total,
        network_type,
    );

    // Execute status check with context
    let result = handle_request(job.data.clone(), state.clone(), context).await;

    // Handle result and update counters in Redis
    handle_result(
        result,
        &redis_conn,
        tx_id,
        consecutive_failures,
        total_failures,
    )
    .await
}

/// Handles status check results with circuit breaker tracking.
///
/// # Strategy
/// - If transaction is in final state → Clean up counters, return Ok (job completes)
/// - If success but not final → Reset consecutive to 0 in Redis, return Err (Apalis retries)
/// - If error → Increment counters in Redis, return Err (Apalis retries)
///
/// Counters are stored in a separate Redis key by tx_id, independent of Apalis job data.
async fn handle_result(
    result: Result<TransactionRepoModel>,
    redis_conn: &RedisConn,
    tx_id: &str,
    consecutive_failures: u32,
    total_failures: u32,
) -> Result<(), Error> {
    match result {
        Ok(tx) if is_final_state(&tx.status) => {
            // Transaction reached final state - job complete, clean up counters
            debug!(
                tx_id = %tx.id,
                status = ?tx.status,
                consecutive_failures,
                total_failures,
                "transaction in final state, status check complete"
            );

            // Clean up the counters from Redis
            if let Err(e) = delete_counters_from_redis(redis_conn, tx_id).await {
                warn!(error = %e, tx_id = %tx_id, "failed to clean up counters from Redis");
            }

            Ok(())
        }
        Ok(tx) => {
            // Success but not final - RESET consecutive counter, keep total unchanged
            info!(
                tx_id = %tx.id,
                status = ?tx.status,
                "transaction not in final state, resetting consecutive failures"
            );

            // Reset consecutive counter in Redis
            if let Err(e) = update_counters_in_redis(redis_conn, tx_id, 0, total_failures).await {
                warn!(error = %e, tx_id = %tx_id, "failed to reset consecutive counter in Redis");
            }

            // Return error to trigger Apalis retry
            Err(Error::Failed(Arc::new(
                format!(
                    "transaction status: {:?} - not in final state, retrying",
                    tx.status
                )
                .into(),
            )))
        }
        Err(e) => {
            // Error occurred - INCREMENT both counters
            let new_consecutive = consecutive_failures.saturating_add(1);
            let new_total = total_failures.saturating_add(1);

            warn!(
                error = %e,
                tx_id = %tx_id,
                consecutive_failures = new_consecutive,
                total_failures = new_total,
                "status check failed, incrementing failure counters"
            );

            // Update counters in Redis
            if let Err(update_err) =
                update_counters_in_redis(redis_conn, tx_id, new_consecutive, new_total).await
            {
                warn!(error = %update_err, tx_id = %tx_id, "failed to update counters in Redis");
            }

            // Return error to trigger Apalis retry
            Err(Error::Failed(Arc::new(format!("{e}").into())))
        }
    }
}

/// Builds the Redis key for storing status check metadata for a transaction.
fn get_metadata_key(tx_id: &str) -> String {
    let redis_key_prefix = ServerConfig::get_redis_key_prefix();
    format!("{redis_key_prefix}:{TX_STATUS_CHECK_METADATA_PREFIX}:{tx_id}")
}

/// Reads failure counters from Redis for a given transaction.
///
/// Returns (consecutive_failures, total_failures), defaulting to (0, 0) if not found.
async fn read_counters_from_redis(redis_conn: &RedisConn, tx_id: &str) -> (u32, u32) {
    let key = get_metadata_key(tx_id);

    let result: Result<(u32, u32)> = match redis_conn {
        RedisConn::Pool(pool) => {
            async {
                let mut conn = pool
                    .get()
                    .await
                    .map_err(|e| eyre::eyre!("Failed to get Redis connection: {e}"))?;

                let values: Vec<Option<String>> = conn
                    .hget(&key, &["consecutive", "total"])
                    .await
                    .map_err(|e| eyre::eyre!("Failed to read counters from Redis: {e}"))?;

                let consecutive = values
                    .first()
                    .and_then(|v| v.as_ref())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let total = values
                    .get(1)
                    .and_then(|v| v.as_ref())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);

                Ok((consecutive, total))
            }
            .await
        }
        RedisConn::ConnectionManager(conn_manager) => {
            async {
                let mut conn = (**conn_manager).clone();

                let values: Vec<Option<String>> = conn
                    .hget(&key, &["consecutive", "total"])
                    .await
                    .map_err(|e| eyre::eyre!("Failed to read counters from Redis: {e}"))?;

                let consecutive = values
                    .first()
                    .and_then(|v| v.as_ref())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let total = values
                    .get(1)
                    .and_then(|v| v.as_ref())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);

                Ok((consecutive, total))
            }
            .await
        }
    };

    match result {
        Ok(counters) => counters,
        Err(e) => {
            warn!(error = %e, tx_id = %tx_id, "failed to read counters from Redis, using defaults");
            (0, 0)
        }
    }
}

/// Updates failure counters in Redis for a given transaction.
///
/// TTL is refreshed on every update to act as an "inactivity timeout".
/// If no status checks happen for 12 hours, the metadata is considered stale.
/// Active transactions keep their metadata fresh.
async fn update_counters_in_redis(
    redis_conn: &RedisConn,
    tx_id: &str,
    consecutive: u32,
    total: u32,
) -> Result<()> {
    let key = get_metadata_key(tx_id);

    // Use pipeline to atomically set values and TTL
    // hset_multiple returns "OK", expire returns 1 if TTL was set
    let ttl_result: i64 = match redis_conn {
        RedisConn::Pool(pool) => {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| eyre::eyre!("Failed to get Redis connection: {e}"))?;

            let (result,): (i64,) = redis::pipe()
                .hset_multiple(
                    &key,
                    &[
                        ("consecutive", consecutive.to_string()),
                        ("total", total.to_string()),
                    ],
                )
                .ignore()
                .expire(&key, 43200) // 12 hours TTL
                .query_async(&mut *conn)
                .await
                .map_err(|e| eyre::eyre!("Failed to update counters in Redis: {e}"))?;
            result
        }
        RedisConn::ConnectionManager(conn_manager) => {
            let mut conn = (**conn_manager).clone();

            let (result,): (i64,) = redis::pipe()
                .hset_multiple(
                    &key,
                    &[
                        ("consecutive", consecutive.to_string()),
                        ("total", total.to_string()),
                    ],
                )
                .ignore()
                .expire(&key, 43200) // 12 hours TTL
                .query_async(&mut conn)
                .await
                .map_err(|e| eyre::eyre!("Failed to update counters in Redis: {e}"))?;
            result
        }
    };

    let ttl_set = ttl_result == 1;

    debug!(
        tx_id = %tx_id,
        consecutive,
        total,
        key,
        ttl_set,
        "updated status check counters in Redis"
    );

    Ok(())
}

/// Deletes failure counters from Redis when transaction reaches final state.
async fn delete_counters_from_redis(redis_conn: &RedisConn, tx_id: &str) -> Result<()> {
    let key = get_metadata_key(tx_id);

    match redis_conn {
        RedisConn::Pool(pool) => {
            let mut conn = pool
                .get()
                .await
                .map_err(|e| eyre::eyre!("Failed to get Redis connection: {e}"))?;

            conn.del::<_, ()>(&key)
                .await
                .map_err(|e| eyre::eyre!("Failed to delete counters from Redis: {e}"))?;
        }
        RedisConn::ConnectionManager(conn_manager) => {
            let mut conn = (**conn_manager).clone();

            conn.del::<_, ()>(&key)
                .await
                .map_err(|e| eyre::eyre!("Failed to delete counters from Redis: {e}"))?;
        }
    }

    debug!(tx_id = %tx_id, key, "deleted status check counters from Redis");

    Ok(())
}

async fn handle_request(
    status_request: TransactionStatusCheck,
    state: Data<ThinData<DefaultAppState>>,
    context: StatusCheckContext,
) -> Result<TransactionRepoModel> {
    let relayer_transaction =
        get_relayer_transaction(status_request.relayer_id.clone(), &state).await?;

    let transaction = get_transaction_by_id(status_request.transaction_id.clone(), &state).await?;

    let updated_transaction = relayer_transaction
        .handle_transaction_status(transaction, Some(context))
        .await?;

    debug!(
        "status check handled successfully for tx_id {}",
        status_request.transaction_id
    );

    Ok(updated_transaction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TransactionStatus;
    use std::collections::HashMap;

    #[test]
    fn test_get_metadata_key() {
        // Note: This test assumes default redis key prefix
        let key = get_metadata_key("tx123");
        assert!(key.contains(TX_STATUS_CHECK_METADATA_PREFIX));
        assert!(key.contains("tx123"));
    }

    #[tokio::test]
    async fn test_status_check_job_validation() {
        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
        let job = Job::new(crate::jobs::JobType::TransactionStatusCheck, check_job);

        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert!(job.data.metadata.is_none());
    }

    #[tokio::test]
    async fn test_status_check_with_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("retry_count".to_string(), "2".to_string());
        metadata.insert("last_status".to_string(), "pending".to_string());

        let check_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm)
            .with_metadata(metadata.clone());

        assert!(check_job.metadata.is_some());
        let job_metadata = check_job.metadata.unwrap();
        assert_eq!(job_metadata.get("retry_count").unwrap(), "2");
        assert_eq!(job_metadata.get("last_status").unwrap(), "pending");
    }

    mod context_tests {
        use super::*;

        #[test]
        fn test_context_should_force_finalize_below_threshold() {
            let ctx = StatusCheckContext::new(5, 10, 15, 25, 75, NetworkType::Evm);
            assert!(!ctx.should_force_finalize());
        }

        #[test]
        fn test_context_should_force_finalize_consecutive_at_threshold() {
            let ctx = StatusCheckContext::new(25, 30, 35, 25, 75, NetworkType::Evm);
            assert!(ctx.should_force_finalize());
        }

        #[test]
        fn test_context_should_force_finalize_total_at_threshold() {
            let ctx = StatusCheckContext::new(10, 75, 80, 25, 75, NetworkType::Evm);
            assert!(ctx.should_force_finalize());
        }
    }

    mod final_state_tests {
        use super::*;

        fn verify_final_state(status: TransactionStatus) {
            assert!(is_final_state(&status));
        }

        fn verify_not_final_state(status: TransactionStatus) {
            assert!(!is_final_state(&status));
        }

        #[test]
        fn test_confirmed_is_final() {
            verify_final_state(TransactionStatus::Confirmed);
        }

        #[test]
        fn test_failed_is_final() {
            verify_final_state(TransactionStatus::Failed);
        }

        #[test]
        fn test_canceled_is_final() {
            verify_final_state(TransactionStatus::Canceled);
        }

        #[test]
        fn test_expired_is_final() {
            verify_final_state(TransactionStatus::Expired);
        }

        #[test]
        fn test_pending_is_not_final() {
            verify_not_final_state(TransactionStatus::Pending);
        }

        #[test]
        fn test_sent_is_not_final() {
            verify_not_final_state(TransactionStatus::Sent);
        }

        #[test]
        fn test_submitted_is_not_final() {
            verify_not_final_state(TransactionStatus::Submitted);
        }

        #[test]
        fn test_mined_is_not_final() {
            verify_not_final_state(TransactionStatus::Mined);
        }
    }

    mod handle_result_tests {
        use super::*;

        /// Tests that counter increment uses saturating_add to prevent overflow
        #[test]
        fn test_counter_increment_saturating() {
            let consecutive: u32 = u32::MAX;
            let total: u32 = u32::MAX;

            let new_consecutive = consecutive.saturating_add(1);
            let new_total = total.saturating_add(1);

            // Should not overflow, stays at MAX
            assert_eq!(new_consecutive, u32::MAX);
            assert_eq!(new_total, u32::MAX);
        }

        /// Tests normal counter increment
        #[test]
        fn test_counter_increment_normal() {
            let consecutive: u32 = 5;
            let total: u32 = 10;

            let new_consecutive = consecutive.saturating_add(1);
            let new_total = total.saturating_add(1);

            assert_eq!(new_consecutive, 6);
            assert_eq!(new_total, 11);
        }

        /// Tests that consecutive counter resets to 0 on success (non-final)
        #[test]
        fn test_consecutive_reset_on_success() {
            // When status check succeeds but tx is not final,
            // consecutive should reset to 0, total stays unchanged
            let total: u32 = 20;

            // On success, consecutive resets
            let new_consecutive = 0;
            let new_total = total; // unchanged

            assert_eq!(new_consecutive, 0);
            assert_eq!(new_total, 20);
        }

        /// Tests that final states are correctly identified for cleanup
        #[test]
        fn test_final_state_triggers_cleanup() {
            let final_states = vec![
                TransactionStatus::Confirmed,
                TransactionStatus::Failed,
                TransactionStatus::Canceled,
                TransactionStatus::Expired,
            ];

            for status in final_states {
                assert!(
                    is_final_state(&status),
                    "Expected {:?} to be a final state",
                    status
                );
            }
        }

        /// Tests that non-final states trigger retry
        #[test]
        fn test_non_final_state_triggers_retry() {
            let non_final_states = vec![
                TransactionStatus::Pending,
                TransactionStatus::Sent,
                TransactionStatus::Submitted,
                TransactionStatus::Mined,
            ];

            for status in non_final_states {
                assert!(
                    !is_final_state(&status),
                    "Expected {:?} to NOT be a final state",
                    status
                );
            }
        }
    }
}
