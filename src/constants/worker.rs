pub const WORKER_DEFAULT_MAXIMUM_RETRIES: usize = 5;

// Number of retries for the transaction request job
pub const WORKER_TRANSACTION_REQUEST_RETRIES: usize = 5;

// Transaction submission retry counts per command type
pub const WORKER_TRANSACTION_SUBMIT_RETRIES: usize = 1; // Fresh transaction submission
pub const WORKER_TRANSACTION_RESUBMIT_RETRIES: usize = 1; // Gas price bump (status checker will retry)
pub const WORKER_TRANSACTION_CANCEL_RETRIES: usize = 1; // Cancel/replacement (status checker will retry)
pub const WORKER_TRANSACTION_RESEND_RETRIES: usize = 1; // Resend same transaction (status checker will retry)

// Number of retries for the transaction status checker job
// Maximum retries for the transaction status checker job until tx is in final state
pub const WORKER_TRANSACTION_STATUS_CHECKER_RETRIES: usize = usize::MAX;

// Number of retries for the notification sender job
pub const WORKER_NOTIFICATION_SENDER_RETRIES: usize = 5;

// Number of retries for the token swap request job
pub const WORKER_TOKEN_SWAP_REQUEST_RETRIES: usize = 2;

// Number of retries for the transaction cleanup job
pub const WORKER_TRANSACTION_CLEANUP_RETRIES: usize = 5;

// Number of retries for the relayer health check job
pub const WORKER_RELAYER_HEALTH_CHECK_RETRIES: usize = 2;

// Default concurrency for the workers (fallback)
pub const DEFAULT_CONCURRENCY: usize = 100;

// Optimized per-worker defaults based on typical workload
pub const DEFAULT_CONCURRENCY_TRANSACTION_REQUEST: usize = 50;
pub const DEFAULT_CONCURRENCY_TRANSACTION_SENDER: usize = 75;
pub const DEFAULT_CONCURRENCY_STATUS_CHECKER: usize = 50; // Generic/Solana
pub const DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM: usize = 100; // Highest volume (75% of jobs)
pub const DEFAULT_CONCURRENCY_STATUS_CHECKER_STELLAR: usize = 50;
pub const DEFAULT_CONCURRENCY_NOTIFICATION: usize = 30;
pub const DEFAULT_CONCURRENCY_TOKEN_SWAP: usize = 10; // Low volume
pub const DEFAULT_CONCURRENCY_HEALTH_CHECK: usize = 10; // Low volume
