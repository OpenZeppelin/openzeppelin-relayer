//! Apalis Redis queue metadata cleanup worker implementation.
//!
//! This module implements a cleanup worker that removes stale job metadata from Redis.
//! The apalis library stores job metadata in Redis that never gets automatically cleaned up.
//! When jobs complete, keys accumulate in:
//! - `{namespace}:done` - Sorted set of completed job IDs (score = timestamp)
//! - `{namespace}:data` - Hash storing job payloads (field = job_id)
//! - `{namespace}:failed` - Sorted set of failed jobs
//! - `{namespace}:dead` - Sorted set of dead-letter jobs
//!
//! This worker runs hourly to clean up this metadata and prevent Redis memory from growing
//! indefinitely.
//!
//! ## Distributed Lock
//!
//! Since this runs on multiple service instances simultaneously (each with its own
//! CronStream), a distributed lock is used to ensure only one instance processes
//! the cleanup at a time. The lock has a 55-minute TTL (the cron runs every hour),
//! ensuring the lock expires before the next scheduled run.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

use crate::{
    constants::WORKER_APALIS_CLEANUP_RETRIES, jobs::handle_result, models::DefaultAppState,
    utils::DistributedLock,
};

/// Distributed lock name for apalis queue cleanup.
/// Only one instance across the cluster should run cleanup at a time.
const APALIS_CLEANUP_LOCK_NAME: &str = "apalis_queue_cleanup";

/// TTL for the distributed lock (55 minutes).
///
/// This value should be:
/// 1. Greater than the worst-case cleanup runtime to prevent concurrent execution
/// 2. Less than the cron interval (1 hour) to ensure availability for the next run
const APALIS_CLEANUP_LOCK_TTL_SECS: u64 = 55 * 60;

/// Age threshold for job metadata cleanup (1 hour).
/// Jobs older than this threshold will be cleaned up.
const JOB_AGE_THRESHOLD_SECS: i64 = 3600;

/// Batch size for cleanup operations.
/// Processing in batches prevents memory issues with large datasets.
const CLEANUP_BATCH_SIZE: isize = 500;

/// Queue names to clean up.
/// These are the apalis queue namespaces used by the relayer.
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

/// Represents a cron reminder job for triggering apalis cleanup operations.
#[derive(Default, Debug, Clone)]
pub struct ApalisCleanupCronReminder();

/// Handles periodic apalis queue metadata cleanup jobs.
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
/// * `attempt` - Current attempt number for retry logic
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of cleanup processing
#[instrument(
    level = "debug",
    skip(job, data),
    fields(
        job_type = "apalis_cleanup",
        attempt = %attempt.current(),
    ),
    err
)]
pub async fn apalis_cleanup_handler(
    job: ApalisCleanupCronReminder,
    data: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    let result = handle_cleanup_request(job, data, attempt.clone()).await;

    handle_result(
        result,
        attempt,
        "ApalisCleanup",
        WORKER_APALIS_CLEANUP_RETRIES,
    )
}

/// Handles the actual apalis cleanup request logic.
///
/// This function first attempts to acquire a distributed lock to ensure only
/// one instance processes cleanup at a time. If the lock is already held by
/// another instance, this returns early without doing any work.
async fn handle_cleanup_request(
    _job: ApalisCleanupCronReminder,
    data: Data<ThinData<DefaultAppState>>,
    _attempt: Attempt,
) -> Result<()> {
    let transaction_repo = data.transaction_repository();

    // Get Redis connection info - if using in-memory repo, skip cleanup
    let (conn, prefix) = match transaction_repo.connection_info() {
        Some(info) => info,
        None => {
            debug!("in-memory repository detected, skipping apalis cleanup");
            return Ok(());
        }
    };

    // Attempt to acquire distributed lock
    let lock_key = format!("{prefix}:lock:{APALIS_CLEANUP_LOCK_NAME}");
    let lock = DistributedLock::new(
        conn.clone(),
        &lock_key,
        Duration::from_secs(APALIS_CLEANUP_LOCK_TTL_SECS),
    );

    let _lock_guard = match lock.try_acquire().await {
        Ok(Some(guard)) => {
            debug!(lock_key = %lock_key, "acquired distributed lock for apalis cleanup");
            guard
        }
        Ok(None) => {
            info!(lock_key = %lock_key, "apalis cleanup skipped - another instance is processing");
            return Ok(());
        }
        Err(e) => {
            // If we can't communicate with Redis for locking, skip cleanup to avoid
            // potential concurrent execution across multiple instances
            warn!(
                error = %e,
                lock_key = %lock_key,
                "failed to acquire distributed lock, skipping cleanup"
            );
            return Ok(());
        }
    };

    info!("executing apalis queue metadata cleanup");

    // Queue keys are created by apalis directly at the root level
    // Format: {queue_name}:done, {queue_name}:data, etc.
    // No prefix is added - apalis doesn't use the REDIS_KEY_PREFIX for queue keys
    let redis_key_prefix = "";

    let cutoff_timestamp = chrono::Utc::now().timestamp() - JOB_AGE_THRESHOLD_SECS;

    let mut total_cleaned = 0usize;
    let mut total_errors = 0usize;

    // Process each queue
    for queue_name in QUEUE_NAMES {
        let namespace = format!("{redis_key_prefix}{queue_name}");

        match cleanup_queue(&conn, &namespace, cutoff_timestamp).await {
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
        "apalis cleanup completed"
    );

    if total_errors > 0 {
        Err(eyre::eyre!(
            "Apalis cleanup completed with {} errors",
            total_errors
        ))
    } else {
        Ok(())
    }
}

/// Cleans up stale job metadata for a single queue namespace.
///
/// # Arguments
/// * `conn` - Redis connection manager
/// * `namespace` - The queue namespace (e.g., "oz-relayer:queue:transaction_request_queue")
/// * `cutoff_timestamp` - Unix timestamp; jobs older than this will be cleaned up
///
/// # Returns
/// * `Result<usize>` - Number of jobs cleaned up
async fn cleanup_queue(
    conn: &Arc<ConnectionManager>,
    namespace: &str,
    cutoff_timestamp: i64,
) -> Result<usize> {
    let mut total_cleaned = 0usize;
    let data_key = format!("{namespace}:data");

    // Clean up each sorted set (done, failed, dead)
    for suffix in SORTED_SET_SUFFIXES {
        let sorted_set_key = format!("{namespace}{suffix}");
        let cleaned =
            cleanup_sorted_set_and_data(conn, &sorted_set_key, &data_key, cutoff_timestamp).await?;
        total_cleaned += cleaned;
    }

    Ok(total_cleaned)
}

/// Cleans up job IDs from a sorted set and their associated data from the hash.
///
/// Uses ZRANGEBYSCORE to find old job IDs, then removes them from both the
/// sorted set and the data hash in a pipeline for efficiency.
///
/// # Arguments
/// * `conn` - Redis connection manager
/// * `sorted_set_key` - Key of the sorted set (e.g., "queue:transaction_request_queue:done")
/// * `data_key` - Key of the data hash (e.g., "queue:transaction_request_queue:data")
/// * `cutoff_timestamp` - Unix timestamp; jobs with score older than this will be cleaned up
///
/// # Returns
/// * `Result<usize>` - Number of jobs cleaned up
async fn cleanup_sorted_set_and_data(
    conn: &Arc<ConnectionManager>,
    sorted_set_key: &str,
    data_key: &str,
    cutoff_timestamp: i64,
) -> Result<usize> {
    let mut total_cleaned = 0usize;
    let mut conn = (**conn).clone();

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

        // Use pipeline to remove from sorted set and data hash atomically
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
        assert_eq!(APALIS_CLEANUP_LOCK_TTL_SECS, 55 * 60); // 55 minutes
        assert_eq!(JOB_AGE_THRESHOLD_SECS, 3600); // 1 hour
        assert_eq!(CLEANUP_BATCH_SIZE, 500);
    }

    #[test]
    fn test_namespace_format() {
        // Queue keys are at root level without prefix
        let redis_key_prefix = "";
        let queue_name = "transaction_request_queue";
        let namespace = format!("{redis_key_prefix}{queue_name}");
        assert_eq!(namespace, "transaction_request_queue");
    }

    #[test]
    fn test_sorted_set_key_format() {
        let namespace = "transaction_request_queue";
        let sorted_set_key = format!("{namespace}:done");
        assert_eq!(sorted_set_key, "transaction_request_queue:done");
    }

    #[test]
    fn test_data_key_format() {
        let namespace = "transaction_request_queue";
        let data_key = format!("{namespace}:data");
        assert_eq!(data_key, "transaction_request_queue:data");
    }

    #[test]
    fn test_lock_key_format() {
        let prefix = "oz-relayer";
        let lock_key = format!("{prefix}:lock:{APALIS_CLEANUP_LOCK_NAME}");
        assert_eq!(lock_key, "oz-relayer:lock:apalis_queue_cleanup");
    }

    #[test]
    fn test_cutoff_timestamp_calculation() {
        let now = chrono::Utc::now().timestamp();
        let cutoff = now - JOB_AGE_THRESHOLD_SECS;
        assert!(cutoff < now);
        assert_eq!(now - cutoff, JOB_AGE_THRESHOLD_SECS);
    }
}
