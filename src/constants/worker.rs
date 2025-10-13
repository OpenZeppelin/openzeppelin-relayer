use std::time::Duration;

pub const WORKER_DEFAULT_MAXIMUM_RETRIES: usize = 5;

// Number of retries for the transaction request job
pub const WORKER_TRANSACTION_REQUEST_RETRIES: usize = 5;

// Transaction submission retry counts per command type
pub const WORKER_TRANSACTION_SUBMIT_RETRIES: usize = 3; // Fresh transaction submission
pub const WORKER_TRANSACTION_RESUBMIT_RETRIES: usize = 3; // Gas price bump (status checker will retry)
pub const WORKER_TRANSACTION_CANCEL_RETRIES: usize = 2; // Cancel/replacement (status checker will retry)
pub const WORKER_TRANSACTION_RESEND_RETRIES: usize = 3; // Resend same transaction (status checker will retry)

// Number of retries for the transaction status checker job
// Maximum retries for the transaction status checker job until tx is in final state
pub const WORKER_TRANSACTION_STATUS_CHECKER_RETRIES: usize = usize::MAX;

// Number of retries for the notification sender job
pub const WORKER_NOTIFICATION_SENDER_RETRIES: usize = 5;

// Number of retries for the solana token swap request job
pub const WORKER_SOLANA_TOKEN_SWAP_REQUEST_RETRIES: usize = 2;

// Number of retries for the transaction cleanup job
pub const WORKER_TRANSACTION_CLEANUP_RETRIES: usize = 5;

// Number of retries for the relayer health check job
pub const WORKER_RELAYER_HEALTH_CHECK_RETRIES: usize = 2;

// Default concurrency for the workers
pub const DEFAULT_CONCURRENCY: usize = 1500;

// Default rate limit for the workers
pub const DEFAULT_RATE_LIMIT: u64 = 3000;

// Default rate limit duration for the workers
pub const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);
