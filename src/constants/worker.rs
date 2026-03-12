pub const WORKER_DEFAULT_MAXIMUM_RETRIES: usize = 5;

// Number of retries for the transaction request job
pub const WORKER_TRANSACTION_REQUEST_RETRIES: usize = 5;

// Transaction submission retry counts per command type
pub const WORKER_TRANSACTION_SUBMIT_RETRIES: usize = 0; // No retries — status checker handles recovery
pub const WORKER_TRANSACTION_RESUBMIT_RETRIES: usize = 0; // No retries — status checker handles recovery
pub const WORKER_TRANSACTION_CANCEL_RETRIES: usize = 0; // No retries — Cancel/replacement (status checker will retry)
pub const WORKER_TRANSACTION_RESEND_RETRIES: usize = 0; // No retries — Resend same transaction (status checker will retry)

// Number of retries for the transaction status checker job
// Maximum retries for the transaction status checker job until tx is in final state
pub const WORKER_TRANSACTION_STATUS_CHECKER_RETRIES: usize = usize::MAX;

// Number of retries for the notification sender job
pub const WORKER_NOTIFICATION_SENDER_RETRIES: usize = 5;

// Number of retries for the token swap request job
pub const WORKER_TOKEN_SWAP_REQUEST_RETRIES: usize = 0;

// Number of retries for the transaction cleanup job
pub const WORKER_TRANSACTION_CLEANUP_RETRIES: usize = 5;

// Number of retries for the relayer health check job
pub const WORKER_RELAYER_HEALTH_CHECK_RETRIES: usize = 2;

// Number of retries for the system queue cleanup job
pub const WORKER_SYSTEM_CLEANUP_RETRIES: usize = 3;

// Default concurrency for the workers (fallback)
pub const DEFAULT_CONCURRENCY: usize = 100;

// Redis-only worker concurrency defaults (queues not represented in QueueType).
// For QueueType-mapped queues, defaults are in QueueType::default_concurrency().
pub const DEFAULT_CONCURRENCY_STATUS_CHECKER: usize = 50; // Generic/Solana
pub const DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM: usize = 100; // Highest volume (75% of jobs)

// Cron schedule configurations (shared between Redis and SQS backends)
/// Cron expression for transaction cleanup: runs every 10 minutes
pub const TRANSACTION_CLEANUP_CRON_SCHEDULE: &str = "0 */10 * * * *";

/// TTL for the transaction cleanup distributed lock (9 minutes).
///
/// This value should be:
/// 1. Greater than the worst-case cleanup runtime to prevent concurrent execution
/// 2. Less than the cron interval (10 minutes) to ensure availability for the next run
pub const TRANSACTION_CLEANUP_LOCK_TTL_SECS: u64 = 9 * 60;

/// Cron expression for system cleanup: runs every 15 minutes
pub const SYSTEM_CLEANUP_CRON_SCHEDULE: &str = "0 */15 * * * *";

/// TTL for the system cleanup distributed lock (14 minutes).
///
/// This value should be:
/// 1. Greater than the worst-case cleanup runtime to prevent concurrent execution
/// 2. Less than the cron interval (15 minutes) to ensure availability for the next run
pub const SYSTEM_CLEANUP_LOCK_TTL_SECS: u64 = 14 * 60;

/// Fallback TTL for token-swap cron distributed locks (4 minutes).
///
/// This fallback is used when the cron interval cannot be derived from the
/// schedule expression (for example, parse failures). In normal operation,
/// token-swap lock TTL is derived from the cron interval in SQS cron scheduler.
pub const TOKEN_SWAP_CRON_LOCK_TTL_SECS: u64 = 4 * 60;
