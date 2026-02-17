//! System cleanup worker implementation for Redis queue metadata.
//!
//! This module implements a cleanup worker that removes stale job metadata from Redis.
//! The job queue library stores job metadata in Redis that never gets automatically cleaned up.
//! When jobs complete, keys accumulate in:
//! - `{namespace}:done` - Sorted set of completed job IDs (score = timestamp)
//! - `{namespace}:data` - Hash storing job payloads (field = job_id)
//! - `{namespace}:result` - Hash storing job results (field = job_id)
//! - `{namespace}:failed` - Sorted set of failed jobs
//! - `{namespace}:dead` - Sorted set of dead-letter jobs
//!
//! This worker runs every 15 minutes to clean up this metadata and prevent Redis memory from growing
//! indefinitely.
//!
//! ## Distributed Lock
//!
//! Since this runs on multiple service instances simultaneously (each with its own
//! CronStream), a distributed lock is used to ensure only one instance processes
//! the cleanup at a time. The lock has a 14-minute TTL (the cron runs every 15 minutes),
//! ensuring the lock expires before the next scheduled run.

use actix_web::web::ThinData;
use deadpool_redis::Pool;
use eyre::Result;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

use crate::{
    config::ServerConfig,
    constants::{SYSTEM_CLEANUP_LOCK_TTL_SECS, WORKER_SYSTEM_CLEANUP_RETRIES},
    jobs::{handle_result, JobProducerTrait},
    models::DefaultAppState,
    queues::{HandlerError, QueueBackendType, WorkerContext},
    utils::DistributedLock,
};

/// Distributed lock name for queue cleanup.
/// Only one instance across the cluster should run cleanup at a time.
const SYSTEM_CLEANUP_LOCK_NAME: &str = "system_queue_cleanup";

// Note: SYSTEM_CLEANUP_LOCK_TTL_SECS is defined in crate::constants::worker

/// Age threshold for job metadata cleanup (10 minutes).
/// Jobs older than this threshold will be cleaned up.
const JOB_AGE_THRESHOLD_SECS: i64 = 10 * 60;

/// Batch size for cleanup operations.
/// Processing in batches prevents memory issues with large datasets.
const CLEANUP_BATCH_SIZE: isize = 500;

/// Queue names to clean up.
/// These are the queue namespaces used by the relayer.
const QUEUE_NAMES: &[&str] = &[
    "transaction_request_queue",
    "transaction_submission_queue",
    "transaction_status_queue",
    "transaction_status_queue_evm",
    "transaction_status_queue_stellar",
    "notification_queue",
    "token_swap_request_queue",
    "relayer_health_check_queue",
];

/// Sorted set suffixes that contain job IDs to clean up.
const SORTED_SET_SUFFIXES: &[&str] = &[":done", ":failed", ":dead"];

/// Represents a cron reminder job for triggering system cleanup operations.
#[derive(Default, Debug, Clone)]
pub struct SystemCleanupCronReminder();

/// Handles periodic queue metadata cleanup jobs.
///
/// This function processes stale job metadata by:
/// 1. Acquiring a distributed lock to prevent concurrent cleanup
/// 2. Iterating through all queue namespaces
/// 3. For each queue, finding and removing job IDs older than threshold
/// 4. Cleaning up associated data from the `:data` hash
///
/// # Arguments
/// * `job` - The cron reminder job triggering the cleanup
/// * `data` - Application state containing repositories
/// * `ctx` - Worker context with attempt number and task ID
///
/// # Returns
/// * `Result<(), HandlerError>` - Success or failure of cleanup processing
#[instrument(
    level = "debug",
    skip(job, data),
    fields(
        job_type = "system_cleanup",
        attempt = %ctx.attempt,
    ),
    err
)]
pub async fn system_cleanup_handler(
    job: SystemCleanupCronReminder,
    data: ThinData<DefaultAppState>,
    ctx: WorkerContext,
) -> Result<(), HandlerError> {
    let result = handle_cleanup_request(job, &data).await;

    handle_result(result, &ctx, "SystemCleanup", WORKER_SYSTEM_CLEANUP_RETRIES)
}

/// Handles the actual system cleanup request logic.
///
/// This function first attempts to acquire a distributed lock to ensure only
/// one instance processes cleanup at a time. If the lock is already held by
/// another instance, this returns early without doing any work.
///
/// Note: Queue metadata cleanup only runs when using Redis queue backend.
/// SQS backend and in-memory mode skip cleanup since they don't use Redis queues.
async fn handle_cleanup_request(
    _job: SystemCleanupCronReminder,
    data: &ThinData<DefaultAppState>,
) -> Result<()> {
    // Skip cleanup if not using Redis queue backend
    let backend_type = data.job_producer().backend_type();
    if backend_type != QueueBackendType::Redis {
        debug!(
            backend = %backend_type,
            "Skipping queue metadata cleanup - not using Redis queue backend"
        );
        return Ok(());
    }

    let transaction_repo = data.transaction_repository();
    let (redis_connections, key_prefix) =
        match crate::repositories::TransactionRepository::connection_info(transaction_repo.as_ref())
        {
            Some((connections, prefix)) => (connections, prefix),
            None => {
                debug!("in-memory repository detected, skipping system cleanup");
                return Ok(());
            }
        };
    let pool = redis_connections.primary().clone();

    // In distributed mode, acquire a lock to prevent multiple instances from
    // running cleanup simultaneously. In single-instance mode, skip locking.
    let _lock_guard = if ServerConfig::get_distributed_mode() {
        let lock_key = format!("{key_prefix}:lock:{SYSTEM_CLEANUP_LOCK_NAME}");
        let lock = DistributedLock::new(
            pool.clone(),
            &lock_key,
            Duration::from_secs(SYSTEM_CLEANUP_LOCK_TTL_SECS),
        );

        match lock.try_acquire().await {
            Ok(Some(guard)) => {
                debug!(lock_key = %lock_key, "acquired distributed lock for system cleanup");
                Some(guard)
            }
            Ok(None) => {
                info!(lock_key = %lock_key, "system cleanup skipped - another instance is processing");
                return Ok(());
            }
            Err(e) => {
                // Fail closed: skip cleanup if we can't communicate with Redis for locking,
                // to prevent concurrent execution across multiple instances
                warn!(
                    error = %e,
                    lock_key = %lock_key,
                    "failed to acquire distributed lock, skipping cleanup"
                );
                return Ok(());
            }
        }
    } else {
        debug!("distributed mode disabled, skipping lock acquisition");
        None
    };

    info!("executing queue metadata cleanup");

    // Queue keys use REDIS_KEY_PREFIX if set, with ":queue:" suffix
    // Format: {REDIS_KEY_PREFIX}:queue:{queue_name}:done, etc.
    // If REDIS_KEY_PREFIX is not set, keys are just {queue_name}:done, etc.
    let redis_key_prefix = env::var("REDIS_KEY_PREFIX")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|value| format!("{value}:queue:"))
        .unwrap_or_default();

    let cutoff_timestamp = chrono::Utc::now().timestamp() - JOB_AGE_THRESHOLD_SECS;

    let mut total_cleaned = 0usize;
    let mut total_errors = 0usize;

    // Process each queue
    for queue_name in QUEUE_NAMES {
        let namespace = format!("{redis_key_prefix}{queue_name}");

        match cleanup_queue(&pool, &namespace, cutoff_timestamp).await {
            Ok(cleaned) => {
                if cleaned > 0 {
                    debug!(
                        queue = %queue_name,
                        cleaned_count = cleaned,
                        "cleaned up stale job metadata"
                    );
                }
                total_cleaned += cleaned;
            }
            Err(e) => {
                error!(
                    queue = %queue_name,
                    error = %e,
                    "failed to cleanup queue"
                );
                total_errors += 1;
            }
        }
    }

    info!(
        total_cleaned,
        total_errors,
        queues_processed = QUEUE_NAMES.len(),
        "system cleanup completed"
    );

    if total_errors > 0 {
        Err(eyre::eyre!(
            "System cleanup completed with {} errors",
            total_errors
        ))
    } else {
        Ok(())
    }
}

/// Cleans up stale job metadata for a single queue namespace.
///
/// # Arguments
/// * `pool` - Redis connection pool
/// * `namespace` - The queue namespace (e.g., "oz-relayer:queue:transaction_request_queue")
/// * `cutoff_timestamp` - Unix timestamp; jobs older than this will be cleaned up
///
/// # Returns
/// * `Result<usize>` - Number of jobs cleaned up
async fn cleanup_queue(pool: &Arc<Pool>, namespace: &str, cutoff_timestamp: i64) -> Result<usize> {
    let mut total_cleaned = 0usize;
    let data_key = format!("{namespace}:data");
    let result_key = format!("{data_key}::result");

    // Clean up each sorted set (done, failed, dead) and associated hash entries
    for suffix in SORTED_SET_SUFFIXES {
        let sorted_set_key = format!("{namespace}{suffix}");
        let cleaned = cleanup_sorted_set_and_hashes(
            pool,
            &sorted_set_key,
            &data_key,
            &result_key,
            cutoff_timestamp,
        )
        .await?;
        total_cleaned += cleaned;
    }

    Ok(total_cleaned)
}

/// Cleans up job IDs from a sorted set and their associated data/result hashes.
///
/// Uses ZRANGEBYSCORE to find old job IDs, then removes them from the
/// sorted set and both the data and result hashes in a pipeline for efficiency.
///
/// # Arguments
/// * `pool` - Redis connection pool
/// * `sorted_set_key` - Key of the sorted set (e.g., "queue:transaction_request_queue:done")
/// * `data_key` - Key of the data hash (e.g., "queue:transaction_request_queue:data")
/// * `result_key` - Key of the result hash (e.g., "queue:transaction_request_queue:result")
/// * `cutoff_timestamp` - Unix timestamp; jobs with score older than this will be cleaned up
///
/// # Returns
/// * `Result<usize>` - Number of jobs cleaned up
async fn cleanup_sorted_set_and_hashes(
    pool: &Arc<Pool>,
    sorted_set_key: &str,
    data_key: &str,
    result_key: &str,
    cutoff_timestamp: i64,
) -> Result<usize> {
    let mut total_cleaned = 0usize;
    let mut conn = pool.get().await?;

    loop {
        // Get batch of old job IDs from sorted set
        // ZRANGEBYSCORE key -inf cutoff LIMIT 0 batch_size
        let job_ids: Vec<String> = redis::cmd("ZRANGEBYSCORE")
            .arg(sorted_set_key)
            .arg("-inf")
            .arg(cutoff_timestamp)
            .arg("LIMIT")
            .arg(0)
            .arg(CLEANUP_BATCH_SIZE)
            .query_async(&mut conn)
            .await?;

        if job_ids.is_empty() {
            break;
        }

        let batch_size = job_ids.len();

        // Use pipeline to remove from sorted set and both hashes atomically
        let mut pipe = redis::pipe();

        // ZREM sorted_set_key job_id1 job_id2 ...
        pipe.cmd("ZREM").arg(sorted_set_key);
        for job_id in &job_ids {
            pipe.arg(job_id);
        }
        pipe.ignore();

        // HDEL data_key job_id1 job_id2 ...
        pipe.cmd("HDEL").arg(data_key);
        for job_id in &job_ids {
            pipe.arg(job_id);
        }
        pipe.ignore();

        // HDEL result_key job_id1 job_id2 ...
        pipe.cmd("HDEL").arg(result_key);
        for job_id in &job_ids {
            pipe.arg(job_id);
        }
        pipe.ignore();

        pipe.query_async::<()>(&mut conn).await?;

        total_cleaned += batch_size;

        // If we got fewer than batch size, we're done
        if batch_size < CLEANUP_BATCH_SIZE as usize {
            break;
        }
    }

    Ok(total_cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_names_not_empty() {
        assert!(!QUEUE_NAMES.is_empty());
    }

    #[test]
    fn test_sorted_set_suffixes() {
        assert!(SORTED_SET_SUFFIXES.contains(&":done"));
        assert!(SORTED_SET_SUFFIXES.contains(&":failed"));
        assert!(SORTED_SET_SUFFIXES.contains(&":dead"));
    }

    #[test]
    fn test_constants() {
        assert_eq!(SYSTEM_CLEANUP_LOCK_TTL_SECS, 14 * 60); // 14 minutes
        assert_eq!(JOB_AGE_THRESHOLD_SECS, 10 * 60); // 10 minutes
        assert_eq!(CLEANUP_BATCH_SIZE, 500);
    }

    #[test]
    fn test_namespace_format_without_prefix() {
        // When REDIS_KEY_PREFIX is not set, queue keys are at root level
        let redis_key_prefix = "";
        let queue_name = "transaction_request_queue";
        let namespace = format!("{redis_key_prefix}{queue_name}");
        assert_eq!(namespace, "transaction_request_queue");
    }

    #[test]
    fn test_namespace_format_with_prefix() {
        // When REDIS_KEY_PREFIX is set, queue keys include the prefix
        let redis_key_prefix = "oz-relayer:queue:";
        let queue_name = "transaction_request_queue";
        let namespace = format!("{redis_key_prefix}{queue_name}");
        assert_eq!(namespace, "oz-relayer:queue:transaction_request_queue");
    }

    #[test]
    fn test_sorted_set_key_format() {
        // Without prefix
        let namespace = "transaction_request_queue";
        let sorted_set_key = format!("{namespace}:done");
        assert_eq!(sorted_set_key, "transaction_request_queue:done");

        // With prefix
        let namespace_with_prefix = "oz-relayer:queue:transaction_request_queue";
        let sorted_set_key_with_prefix = format!("{namespace_with_prefix}:done");
        assert_eq!(
            sorted_set_key_with_prefix,
            "oz-relayer:queue:transaction_request_queue:done"
        );
    }

    #[test]
    fn test_data_key_format() {
        // Without prefix
        let namespace = "transaction_request_queue";
        let data_key = format!("{namespace}:data");
        assert_eq!(data_key, "transaction_request_queue:data");

        // With prefix
        let namespace_with_prefix = "oz-relayer:queue:transaction_request_queue";
        let data_key_with_prefix = format!("{namespace_with_prefix}:data");
        assert_eq!(
            data_key_with_prefix,
            "oz-relayer:queue:transaction_request_queue:data"
        );
    }

    #[test]
    fn test_result_key_format() {
        // Without prefix
        let namespace = "transaction_request_queue";
        let result_key = format!("{namespace}:result");
        assert_eq!(result_key, "transaction_request_queue:result");

        // With prefix
        let namespace_with_prefix = "oz-relayer:queue:transaction_request_queue";
        let result_key_with_prefix = format!("{namespace_with_prefix}:result");
        assert_eq!(
            result_key_with_prefix,
            "oz-relayer:queue:transaction_request_queue:result"
        );
    }

    #[test]
    fn test_lock_key_format() {
        let prefix = "oz-relayer";
        let lock_key = format!("{prefix}:lock:{SYSTEM_CLEANUP_LOCK_NAME}");
        assert_eq!(lock_key, "oz-relayer:lock:system_queue_cleanup");
    }

    #[test]
    fn test_cutoff_timestamp_calculation() {
        let now = chrono::Utc::now().timestamp();
        let cutoff = now - JOB_AGE_THRESHOLD_SECS;
        assert!(cutoff < now);
        assert_eq!(now - cutoff, JOB_AGE_THRESHOLD_SECS);
    }
}
