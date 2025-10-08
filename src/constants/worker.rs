use std::time::Duration;

pub const WORKER_DEFAULT_MAXIMUM_RETRIES: usize = 5;

pub const WORKER_TRANSACTION_REQUEST_RETRIES: usize = 20;

// Transaction submission retry counts per command type
pub const WORKER_TRANSACTION_SUBMIT_RETRIES: usize = 10; // Fresh transaction submission
pub const WORKER_TRANSACTION_RESUBMIT_RETRIES: usize = 2; // Gas price bump (status checker will retry)
pub const WORKER_TRANSACTION_CANCEL_RETRIES: usize = 2; // Cancel/replacement (status checker will retry)
pub const WORKER_TRANSACTION_RESEND_RETRIES: usize = 2; // Resend same transaction (status checker will retry)

pub const WORKER_TRANSACTION_SUBMISSION_RETRIES: usize = WORKER_TRANSACTION_SUBMIT_RETRIES; // Largest of all command-specific retries

pub const WORKER_TRANSACTION_STATUS_CHECKER_RETRIES: usize = usize::MAX;

pub const WORKER_NOTIFICATION_SENDER_RETRIES: usize = 10;

pub const WORKER_SOLANA_TOKEN_SWAP_REQUEST_RETRIES: usize = 2;

pub const WORKER_TRANSACTION_CLEANUP_RETRIES: usize = 10;

pub const DEFAULT_CONCURRENCY: usize = 50;

pub const DEFAULT_RATE_LIMIT: u64 = 100;

pub const DEFAULT_RATE_LIMIT_DURATION: Duration = Duration::from_secs(1);
