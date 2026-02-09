use serde::{Deserialize, Serialize};
use std::fmt;

/// Queue types for relayer operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueType {
    TransactionRequest,
    TransactionSubmission,
    StatusCheck,
    Notification,
    TokenSwapRequest,
    RelayerHealthCheck,
}

impl QueueType {
    /// Returns the queue name for logging and identification purposes.
    pub fn queue_name(&self) -> &'static str {
        match self {
            Self::TransactionRequest => "transaction-request",
            Self::TransactionSubmission => "transaction-submission",
            Self::StatusCheck => "status-check",
            Self::Notification => "notification",
            Self::TokenSwapRequest => "token-swap-request",
            Self::RelayerHealthCheck => "relayer-health-check",
        }
    }

    /// Returns the Redis namespace for this queue type (Apalis format).
    pub fn redis_namespace(&self) -> &'static str {
        match self {
            Self::TransactionRequest => "relayer:transaction_request",
            Self::TransactionSubmission => "relayer:transaction_submission",
            Self::StatusCheck => "relayer:transaction_status_stellar",
            Self::Notification => "relayer:notification",
            Self::TokenSwapRequest => "relayer:token_swap_request",
            Self::RelayerHealthCheck => "relayer:relayer_health_check",
        }
    }

    /// Returns the maximum number of retries for this queue type.
    pub fn max_retries(&self) -> usize {
        // TODO review
        match self {
            Self::TransactionRequest => 5,
            Self::TransactionSubmission => 1,
            Self::StatusCheck => usize::MAX,
            Self::Notification => 5,
            Self::TokenSwapRequest => 5,
            Self::RelayerHealthCheck => 5,
        }
    }

    /// Returns the visibility timeout in seconds for SQS (how long worker has to process).
    pub fn visibility_timeout_secs(&self) -> u32 {
        // TODO review
        match self {
            Self::TransactionRequest => 300,
            Self::TransactionSubmission => 120,
            Self::StatusCheck => 300,
            Self::Notification => 180,
            Self::TokenSwapRequest => 300,
            Self::RelayerHealthCheck => 300,
        }
    }

    /// Returns the worker name used for the concurrency environment variable.
    pub fn concurrency_env_key(&self) -> &'static str {
        match self {
            Self::TransactionRequest => "transaction_request",
            Self::TransactionSubmission => "transaction_sender",
            Self::StatusCheck => "transaction_status_checker",
            Self::Notification => "notification_sender",
            Self::TokenSwapRequest => "token_swap_request",
            Self::RelayerHealthCheck => "relayer_health_check",
        }
    }

    /// Returns the default concurrency for this queue type.
    pub fn default_concurrency(&self) -> usize {
        match self {
            Self::TransactionRequest => 50,
            Self::TransactionSubmission => 75,
            Self::StatusCheck => 50,
            Self::Notification => 30,
            Self::TokenSwapRequest => 10,
            Self::RelayerHealthCheck => 10,
        }
    }

    /// Returns the SQS long-poll wait time in seconds.
    pub fn polling_interval_secs(&self) -> u64 {
        match self {
            Self::TransactionRequest => 5,
            Self::TransactionSubmission => 5,
            Self::StatusCheck => 2,
            Self::Notification => 20,
            Self::TokenSwapRequest => 20,
            Self::RelayerHealthCheck => 20,
        }
    }
}

impl fmt::Display for QueueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.queue_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polling_interval_defaults() {
        assert_eq!(QueueType::TransactionRequest.polling_interval_secs(), 5);
        assert_eq!(QueueType::TransactionSubmission.polling_interval_secs(), 5);
        assert_eq!(QueueType::StatusCheck.polling_interval_secs(), 2);
    }
}
