use std::fmt;

use serde::{Deserialize, Serialize};

use crate::constants::{
    DEFAULT_CONCURRENCY_STATUS_CHECKER, DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM,
    WORKER_NOTIFICATION_SENDER_RETRIES, WORKER_RELAYER_HEALTH_CHECK_RETRIES,
    WORKER_TOKEN_SWAP_REQUEST_RETRIES, WORKER_TRANSACTION_REQUEST_RETRIES,
    WORKER_TRANSACTION_STATUS_CHECKER_RETRIES, WORKER_TRANSACTION_SUBMIT_RETRIES,
};

/// Queue types for relayer operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueType {
    TransactionRequest,
    TransactionSubmission,
    StatusCheck,
    StatusCheckEvm,
    StatusCheckStellar,
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
            Self::StatusCheckEvm => "status-check-evm",
            Self::StatusCheckStellar => "status-check-stellar",
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
            Self::StatusCheck => "relayer:transaction_status",
            Self::StatusCheckEvm => "relayer:transaction_status_evm",
            Self::StatusCheckStellar => "relayer:transaction_status_stellar",
            Self::Notification => "relayer:notification",
            Self::TokenSwapRequest => "relayer:token_swap_request",
            Self::RelayerHealthCheck => "relayer:relayer_health_check",
        }
    }

    /// Returns the maximum number of retries for this queue type.
    pub fn max_retries(&self) -> usize {
        match self {
            Self::TransactionRequest => WORKER_TRANSACTION_REQUEST_RETRIES,
            Self::TransactionSubmission => WORKER_TRANSACTION_SUBMIT_RETRIES,
            Self::StatusCheck | Self::StatusCheckEvm | Self::StatusCheckStellar => {
                WORKER_TRANSACTION_STATUS_CHECKER_RETRIES
            }
            Self::Notification => WORKER_NOTIFICATION_SENDER_RETRIES,
            Self::TokenSwapRequest => WORKER_TOKEN_SWAP_REQUEST_RETRIES,
            Self::RelayerHealthCheck => WORKER_RELAYER_HEALTH_CHECK_RETRIES,
        }
    }

    /// Returns the visibility timeout in seconds for SQS (how long a worker has to finish
    /// processing before the message becomes visible again).
    pub fn visibility_timeout_secs(&self) -> u32 {
        match self {
            Self::TransactionRequest => 30,
            Self::TransactionSubmission => 30,
            Self::StatusCheck | Self::StatusCheckEvm => 30,
            Self::StatusCheckStellar => 20,
            Self::Notification => 60,
            Self::TokenSwapRequest => 60,
            Self::RelayerHealthCheck => 60,
        }
    }

    /// Returns the worker name used for the concurrency environment variable.
    pub fn concurrency_env_key(&self) -> &'static str {
        match self {
            Self::TransactionRequest => "transaction_request",
            Self::TransactionSubmission => "transaction_sender",
            Self::StatusCheck => "transaction_status_checker",
            Self::StatusCheckEvm => "transaction_status_checker_evm",
            Self::StatusCheckStellar => "transaction_status_checker_stellar",
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
            Self::StatusCheck => DEFAULT_CONCURRENCY_STATUS_CHECKER,
            Self::StatusCheckEvm => DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM,
            Self::StatusCheckStellar => DEFAULT_CONCURRENCY_STATUS_CHECKER,
            Self::Notification => 30,
            Self::TokenSwapRequest => 10,
            Self::RelayerHealthCheck => 10,
        }
    }

    /// Returns the SQS long-poll wait time in seconds.
    /// Note: SQS `WaitTimeSeconds` maximum is 20 seconds.
    pub fn polling_interval_secs(&self) -> u64 {
        match self {
            Self::TransactionRequest => 15,
            Self::TransactionSubmission => 15,
            Self::StatusCheck | Self::StatusCheckEvm => 5,
            Self::StatusCheckStellar => 3,
            Self::Notification => 20,
            Self::TokenSwapRequest => 20,
            Self::RelayerHealthCheck => 20,
        }
    }

    /// Returns true if this is any variant of status check queue.
    pub fn is_status_check(&self) -> bool {
        matches!(
            self,
            Self::StatusCheck | Self::StatusCheckEvm | Self::StatusCheckStellar
        )
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
        assert_eq!(QueueType::TransactionRequest.polling_interval_secs(), 15);
        assert_eq!(QueueType::TransactionSubmission.polling_interval_secs(), 15);
        assert_eq!(QueueType::StatusCheck.polling_interval_secs(), 5);
        assert_eq!(QueueType::StatusCheckStellar.polling_interval_secs(), 3);
    }

    #[test]
    fn test_is_status_check() {
        assert!(QueueType::StatusCheck.is_status_check());
        assert!(QueueType::StatusCheckEvm.is_status_check());
        assert!(QueueType::StatusCheckStellar.is_status_check());
        assert!(!QueueType::TransactionRequest.is_status_check());
        assert!(!QueueType::Notification.is_status_check());
    }

    #[test]
    fn test_status_check_evm_concurrency() {
        assert_eq!(
            QueueType::StatusCheckEvm.default_concurrency(),
            DEFAULT_CONCURRENCY_STATUS_CHECKER_EVM
        );
    }

    #[test]
    fn test_all_variants_have_nonempty_queue_name() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in &all {
            assert!(!qt.queue_name().is_empty(), "{qt:?} has empty queue_name");
            assert!(
                !qt.redis_namespace().is_empty(),
                "{qt:?} has empty redis_namespace"
            );
            assert!(
                !qt.concurrency_env_key().is_empty(),
                "{qt:?} has empty concurrency_env_key"
            );
        }
    }

    #[test]
    fn test_display_matches_queue_name() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in &all {
            assert_eq!(qt.to_string(), qt.queue_name());
        }
    }

    #[test]
    fn test_max_retries_status_checkers_use_infinite() {
        // Status checkers retry until tx reaches final state
        assert_eq!(QueueType::StatusCheck.max_retries(), usize::MAX);
        assert_eq!(QueueType::StatusCheckEvm.max_retries(), usize::MAX);
        assert_eq!(QueueType::StatusCheckStellar.max_retries(), usize::MAX);
    }

    #[test]
    fn test_max_retries_bounded_queues() {
        // Non-status queues should have finite retries
        assert!(QueueType::TransactionRequest.max_retries() < usize::MAX);
        assert!(QueueType::TransactionSubmission.max_retries() < usize::MAX);
        assert!(QueueType::Notification.max_retries() < usize::MAX);
        assert!(QueueType::TokenSwapRequest.max_retries() < usize::MAX);
        assert!(QueueType::RelayerHealthCheck.max_retries() < usize::MAX);
    }

    #[test]
    fn test_polling_intervals_within_sqs_limit() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in &all {
            assert!(
                qt.polling_interval_secs() <= 20,
                "{qt:?} polling interval {} exceeds SQS max of 20s",
                qt.polling_interval_secs()
            );
        }
    }

    #[test]
    fn test_visibility_timeout_within_sqs_range() {
        // SQS allows 0..=43200 (12 hours)
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in &all {
            let vt = qt.visibility_timeout_secs();
            assert!(
                vt <= 43200,
                "{qt:?} visibility timeout {vt} exceeds SQS max"
            );
            assert!(vt > 0, "{qt:?} visibility timeout should be positive");
        }
    }

    #[test]
    fn test_default_concurrency_positive() {
        let all = [
            QueueType::TransactionRequest,
            QueueType::TransactionSubmission,
            QueueType::StatusCheck,
            QueueType::StatusCheckEvm,
            QueueType::StatusCheckStellar,
            QueueType::Notification,
            QueueType::TokenSwapRequest,
            QueueType::RelayerHealthCheck,
        ];
        for qt in &all {
            assert!(qt.default_concurrency() > 0, "{qt:?} has zero concurrency");
        }
    }
}
