//! Queue backend type definitions.
//!
//! This module defines types for the queue backend abstraction layer:
//! - QueueType: Enum representing different queue types
//! - QueueBackendError: Error types for queue operations
//! - WorkerHandle: Handle to running worker tasks
//! - QueueHealth: Queue health status information

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use thiserror::Error;

use crate::models::{NetworkType, RelayerNetworkPolicy, RelayerRepoModel};
use tracing::debug;

/// Queue types for relayer operations.
///
/// Each variant represents a specific job processing queue with its own
/// configuration, concurrency limits, and retry behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueType {
    /// Transaction request queue - initial validation and preparation
    TransactionRequest,
    /// Transaction submission queue - network submission (Submit/Resubmit/Cancel/Resend)
    TransactionSubmission,
    /// Status check queue - high-frequency status monitoring
    StatusCheck,
    /// Notification queue - webhook delivery
    Notification,
    /// Token swap request queue
    TokenSwapRequest,
    /// Relayer health check queue
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
    ///
    /// - Transaction request: 5 retries (validation can be retried)
    /// - Transaction submission: 1 retry (avoid duplicate submissions)
    /// - Status check: usize::MAX (circuit breaker handles termination)
    /// - Notification: 5 retries (webhook delivery can be retried)
    pub fn max_retries(&self) -> usize {
        match self {
            Self::TransactionRequest => 5,
            Self::TransactionSubmission => 1,
            Self::StatusCheck => usize::MAX, // Circuit breaker terminates
            Self::Notification => 5,
            Self::TokenSwapRequest => 5,
            Self::RelayerHealthCheck => 5,
        }
    }

    /// Returns the visibility timeout in seconds for SQS (how long worker has to process).
    pub fn visibility_timeout_secs(&self) -> u32 {
        match self {
            Self::TransactionRequest => 300,    // 5 minutes
            Self::TransactionSubmission => 120, // 2 minutes
            Self::StatusCheck => 300,           // 5 minutes
            Self::Notification => 180,          // 3 minutes
            Self::TokenSwapRequest => 300,      // 5 minutes
            Self::RelayerHealthCheck => 300,    // 5 minutes
        }
    }

    /// Returns the polling interval in seconds (how often to check for new messages).
    pub fn polling_interval_secs(&self) -> u64 {
        match self {
            Self::TransactionRequest => 20,
            Self::TransactionSubmission => 20,
            Self::StatusCheck => 2, // High frequency for status checks
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

/// Returns the retry delay in seconds for status check re-enqueue based on network type.
///
/// These values approximate the center of the Apalis exponential backoff ranges
/// used in the Redis backend, ensuring consistent status polling intervals
/// regardless of queue backend.
pub fn status_check_retry_delay_secs(network_type: Option<NetworkType>) -> i32 {
    match network_type {
        Some(NetworkType::Evm) => 10,
        Some(NetworkType::Stellar) => 3,
        Some(NetworkType::Solana) => 7,
        None => 5,
    }
}

/// Errors that can occur during queue backend operations.
#[derive(Debug, Error, Serialize, Clone)]
pub enum QueueBackendError {
    /// Redis-specific error (from Apalis or redis crate)
    #[error("Redis error: {0}")]
    RedisError(String),

    /// SQS-specific error (from AWS SDK)
    #[error("SQS error: {0}")]
    SqsError(String),

    /// Job serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Configuration error (missing env vars, invalid config)
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Queue not found or not initialized
    #[error("Queue not found: {0}")]
    QueueNotFound(String),

    /// Worker initialization error
    #[error("Worker initialization error: {0}")]
    WorkerInitError(String),

    /// Generic queue error
    #[error("Queue error: {0}")]
    QueueError(String),
}

/// Handle to a running worker task.
///
/// Workers process jobs from queues. The handle can be used to:
/// - Monitor worker health
/// - Gracefully shutdown workers
/// - Await worker completion
#[derive(Debug)]
pub enum WorkerHandle {
    /// Apalis worker handle (from Redis backend)
    Apalis(Box<dyn std::any::Any + Send>),
    /// Tokio task handle (from SQS backend)
    Tokio(tokio::task::JoinHandle<()>),
}

/// Queue health status information.
///
/// Used for health checks and monitoring queue depths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueHealth {
    /// Queue type being checked
    pub queue_type: QueueType,
    /// Number of messages visible in queue (ready to process)
    pub messages_visible: u64,
    /// Number of messages in flight (being processed)
    pub messages_in_flight: u64,
    /// Number of messages in dead-letter queue (failed after max retries)
    pub messages_dlq: u64,
    /// Backend type (redis or sqs)
    pub backend: String,
    /// Is the queue healthy? (defined by backend-specific criteria)
    pub is_healthy: bool,
}

/// Backend-neutral context passed to all job handlers.
///
/// Replaces direct use of Apalis `Attempt` and `TaskId` in handler signatures,
/// allowing handlers to be backend-agnostic (works with both Redis/Apalis and SQS).
#[derive(Debug, Clone)]
pub struct WorkerContext {
    /// Current retry attempt (0-indexed)
    pub attempt: usize,
    /// Unique task identifier for tracing
    pub task_id: String,
}

impl WorkerContext {
    pub fn new(attempt: usize, task_id: String) -> Self {
        Self { attempt, task_id }
    }
}

/// Backend-neutral handler error for retry control.
///
/// Replaces direct use of `apalis::prelude::Error` in handler return types.
/// Each queue backend maps this to its own retry mechanism:
/// - Redis/Apalis: `Retry` → `Error::Failed`, `Abort` → `Error::Abort`
/// - SQS: `Retry` → message returns via visibility timeout, `Abort` → message deleted
#[derive(Debug)]
pub enum HandlerError {
    /// Retryable failure — backend will reschedule the job
    Retry(String),
    /// Permanent failure — backend will move job to dead-letter / abort
    Abort(String),
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retry(msg) => write!(f, "Retry: {msg}"),
            Self::Abort(msg) => write!(f, "Abort: {msg}"),
        }
    }
}

impl std::error::Error for HandlerError {}

impl From<HandlerError> for apalis::prelude::Error {
    fn from(err: HandlerError) -> Self {
        match err {
            HandlerError::Retry(msg) => apalis::prelude::Error::Failed(Arc::new(msg.into())),
            HandlerError::Abort(msg) => apalis::prelude::Error::Abort(Arc::new(msg.into())),
        }
    }
}

/// Filters relayers to find those eligible for swap workers (Solana or Stellar).
///
/// Returns relayers that have:
/// 1. Solana or Stellar network type
/// 2. Swap configuration
/// 3. Cron schedule defined
pub fn filter_relayers_for_swap(relayers: Vec<RelayerRepoModel>) -> Vec<RelayerRepoModel> {
    relayers
        .into_iter()
        .filter(|relayer| {
            match &relayer.policies {
                RelayerNetworkPolicy::Solana(policy) => {
                    let swap_config = match policy.get_swap_config() {
                        Some(config) => config,
                        None => {
                            debug!(relayer_id = %relayer.id, "No Solana swap configuration specified; skipping");
                            return false;
                        }
                    };

                    if swap_config.cron_schedule.is_none() {
                        debug!(relayer_id = %relayer.id, "No cron schedule specified; skipping");
                        return false;
                    }
                    true
                }
                RelayerNetworkPolicy::Stellar(policy) => {
                    let swap_config = match policy.get_swap_config() {
                        Some(config) => config,
                        None => {
                            debug!(relayer_id = %relayer.id, "No Stellar swap configuration specified; skipping");
                            return false;
                        }
                    };

                    if swap_config.cron_schedule.is_none() {
                        debug!(relayer_id = %relayer.id, "No cron schedule specified; skipping");
                        return false;
                    }
                    true
                }
                _ => {
                    debug!(relayer_id = %relayer.id, "Network type does not support swap; skipping");
                    false
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_type_names() {
        assert_eq!(
            QueueType::TransactionRequest.queue_name(),
            "transaction-request"
        );
        assert_eq!(
            QueueType::TransactionSubmission.queue_name(),
            "transaction-submission"
        );
        assert_eq!(QueueType::StatusCheck.queue_name(), "status-check");
        assert_eq!(QueueType::Notification.queue_name(), "notification");
        assert_eq!(
            QueueType::TokenSwapRequest.queue_name(),
            "token-swap-request"
        );
        assert_eq!(
            QueueType::RelayerHealthCheck.queue_name(),
            "relayer-health-check"
        );
    }

    #[test]
    fn test_queue_type_redis_namespaces() {
        assert_eq!(
            QueueType::TransactionRequest.redis_namespace(),
            "relayer:transaction_request"
        );
        assert_eq!(
            QueueType::TransactionSubmission.redis_namespace(),
            "relayer:transaction_submission"
        );
        assert_eq!(
            QueueType::StatusCheck.redis_namespace(),
            "relayer:transaction_status_stellar"
        );
        assert_eq!(
            QueueType::Notification.redis_namespace(),
            "relayer:notification"
        );
        assert_eq!(
            QueueType::TokenSwapRequest.redis_namespace(),
            "relayer:token_swap_request"
        );
        assert_eq!(
            QueueType::RelayerHealthCheck.redis_namespace(),
            "relayer:relayer_health_check"
        );
    }

    #[test]
    fn test_queue_type_max_retries() {
        assert_eq!(QueueType::TransactionRequest.max_retries(), 5);
        assert_eq!(QueueType::TransactionSubmission.max_retries(), 1);
        assert_eq!(QueueType::StatusCheck.max_retries(), usize::MAX);
        assert_eq!(QueueType::Notification.max_retries(), 5);
        assert_eq!(QueueType::TokenSwapRequest.max_retries(), 5);
        assert_eq!(QueueType::RelayerHealthCheck.max_retries(), 5);
    }

    #[test]
    fn test_queue_type_visibility_timeout() {
        assert_eq!(QueueType::TransactionRequest.visibility_timeout_secs(), 300);
        assert_eq!(
            QueueType::TransactionSubmission.visibility_timeout_secs(),
            120
        );
        assert_eq!(QueueType::StatusCheck.visibility_timeout_secs(), 300);
        assert_eq!(QueueType::Notification.visibility_timeout_secs(), 180);
        assert_eq!(QueueType::TokenSwapRequest.visibility_timeout_secs(), 300);
        assert_eq!(QueueType::RelayerHealthCheck.visibility_timeout_secs(), 300);
    }

    #[test]
    fn test_queue_type_polling_interval() {
        assert_eq!(QueueType::TransactionRequest.polling_interval_secs(), 20);
        assert_eq!(QueueType::TransactionSubmission.polling_interval_secs(), 20);
        assert_eq!(QueueType::StatusCheck.polling_interval_secs(), 2);
        assert_eq!(QueueType::Notification.polling_interval_secs(), 20);
        assert_eq!(QueueType::TokenSwapRequest.polling_interval_secs(), 20);
        assert_eq!(QueueType::RelayerHealthCheck.polling_interval_secs(), 20);
    }

    #[test]
    fn test_status_check_retry_delay_secs() {
        assert_eq!(status_check_retry_delay_secs(Some(NetworkType::Evm)), 10);
        assert_eq!(status_check_retry_delay_secs(Some(NetworkType::Stellar)), 3);
        assert_eq!(status_check_retry_delay_secs(Some(NetworkType::Solana)), 7);
        assert_eq!(status_check_retry_delay_secs(None), 5);
    }

    // --- filter_relayers_for_swap tests ---

    use crate::models::{
        RelayerEvmPolicy, RelayerSolanaPolicy, RelayerSolanaSwapConfig, RelayerStellarPolicy,
        RelayerStellarSwapConfig, StellarFeePaymentStrategy, StellarSwapStrategy,
    };

    fn create_test_evm_relayer(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("EVM Relayer {id}"),
            network: "sepolia".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            signer_id: "test-signer".to_string(),
            address: "0x742d35Cc6634C0532925a3b8D8C2e48a73F6ba2E".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    fn create_test_solana_relayer_with_swap(
        id: &str,
        cron_schedule: Option<String>,
    ) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Solana Relayer {id}"),
            network: "mainnet-beta".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                min_balance: Some(1000000000),
                allowed_tokens: None,
                allowed_programs: None,
                max_signatures: None,
                max_tx_data_size: None,
                fee_payment_strategy: None,
                fee_margin_percentage: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: Some(RelayerSolanaSwapConfig {
                    strategy: None,
                    cron_schedule,
                    min_balance_threshold: Some(5000000000),
                    jupiter_swap_options: None,
                }),
            }),
            signer_id: "test-signer".to_string(),
            address: "5zWma6gn4QxRfC6xZk6KfpXWXXgV3Xt6VzPpXMKCMYW5".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    fn create_test_stellar_relayer_with_swap(
        id: &str,
        cron_schedule: Option<String>,
    ) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Stellar Relayer {id}"),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(1000000000),
                max_fee: None,
                timeout_seconds: None,
                concurrent_transactions: None,
                allowed_tokens: None,
                fee_payment_strategy: Some(StellarFeePaymentStrategy::User),
                slippage_percentage: None,
                fee_margin_percentage: None,
                swap_config: Some(RelayerStellarSwapConfig {
                    strategies: vec![StellarSwapStrategy::OrderBook],
                    cron_schedule,
                    min_balance_threshold: Some(5000000000),
                }),
            }),
            signer_id: "test-signer".to_string(),
            address: "GABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890ABCDEFGH".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    fn create_test_solana_relayer_without_swap_config(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Solana Relayer {id}"),
            network: "mainnet-beta".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                min_balance: Some(1000000000),
                allowed_tokens: None,
                allowed_programs: None,
                max_signatures: None,
                max_tx_data_size: None,
                fee_payment_strategy: None,
                fee_margin_percentage: None,
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
            }),
            signer_id: "test-signer".to_string(),
            address: "5zWma6gn4QxRfC6xZk6KfpXWXXgV3Xt6VzPpXMKCMYW5".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    fn create_test_stellar_relayer_without_swap_config(id: &str) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.to_string(),
            name: format!("Stellar Relayer {id}"),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                min_balance: Some(1000000000),
                max_fee: None,
                timeout_seconds: None,
                concurrent_transactions: None,
                allowed_tokens: None,
                fee_payment_strategy: Some(StellarFeePaymentStrategy::User),
                slippage_percentage: None,
                fee_margin_percentage: None,
                swap_config: None,
            }),
            signer_id: "test-signer".to_string(),
            address: "GABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890ABCDEFGH".to_string(),
            system_disabled: false,
            ..Default::default()
        }
    }

    #[test]
    fn test_filter_relayers_for_swap_with_empty_list() {
        let relayers = vec![];
        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should return empty list when no relayers provided"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_filters_non_solana_stellar() {
        let relayers = vec![
            create_test_evm_relayer("evm-1"),
            create_test_evm_relayer("evm-2"),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should filter out all non-Solana/Stellar relayers"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_filters_no_cron_schedule() {
        let relayers = vec![
            create_test_solana_relayer_with_swap("solana-1", None),
            create_test_solana_relayer_with_swap("solana-2", None),
            create_test_stellar_relayer_with_swap("stellar-1", None),
            create_test_stellar_relayer_with_swap("stellar-2", None),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should filter out Solana and Stellar relayers without cron schedule"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_includes_valid_relayers() {
        let relayers = vec![
            create_test_solana_relayer_with_swap("solana-1", Some("0 0 * * * *".to_string())),
            create_test_solana_relayer_with_swap("solana-2", Some("0 */2 * * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-1", Some("0 0 * * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-2", Some("0 */2 * * * *".to_string())),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            4,
            "Should include all Solana and Stellar relayers with cron schedule"
        );
        let ids: Vec<&str> = filtered.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"solana-1"), "Should include solana-1");
        assert!(ids.contains(&"solana-2"), "Should include solana-2");
        assert!(ids.contains(&"stellar-1"), "Should include stellar-1");
        assert!(ids.contains(&"stellar-2"), "Should include stellar-2");
    }

    #[test]
    fn test_filter_relayers_for_swap_with_mixed_relayers() {
        let relayers = vec![
            create_test_evm_relayer("evm-1"),
            create_test_solana_relayer_with_swap("solana-no-cron", None),
            create_test_solana_relayer_with_swap(
                "solana-with-cron-1",
                Some("0 0 * * * *".to_string()),
            ),
            create_test_evm_relayer("evm-2"),
            create_test_solana_relayer_with_swap(
                "solana-with-cron-2",
                Some("0 */3 * * * *".to_string()),
            ),
            create_test_stellar_relayer_with_swap("stellar-no-cron", None),
            create_test_stellar_relayer_with_swap(
                "stellar-with-cron-1",
                Some("0 0 * * * *".to_string()),
            ),
            create_test_stellar_relayer_with_swap(
                "stellar-with-cron-2",
                Some("0 */3 * * * *".to_string()),
            ),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            4,
            "Should only include Solana and Stellar relayers with cron schedule"
        );

        let ids: Vec<&str> = filtered.iter().map(|r| r.id.as_str()).collect();
        assert!(
            ids.contains(&"solana-with-cron-1"),
            "Should include solana-with-cron-1"
        );
        assert!(
            ids.contains(&"solana-with-cron-2"),
            "Should include solana-with-cron-2"
        );
        assert!(
            ids.contains(&"stellar-with-cron-1"),
            "Should include stellar-with-cron-1"
        );
        assert!(
            ids.contains(&"stellar-with-cron-2"),
            "Should include stellar-with-cron-2"
        );
        assert!(!ids.contains(&"evm-1"), "Should not include EVM relayers");
        assert!(
            !ids.contains(&"solana-no-cron"),
            "Should not include Solana without cron"
        );
        assert!(
            !ids.contains(&"stellar-no-cron"),
            "Should not include Stellar without cron"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_preserves_solana_relayer_data() {
        let cron = "0 1 * * * *".to_string();
        let relayers = vec![create_test_solana_relayer_with_swap(
            "test-relayer",
            Some(cron.clone()),
        )];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(filtered.len(), 1);

        let relayer = &filtered[0];
        assert_eq!(relayer.id, "test-relayer");
        assert_eq!(relayer.name, "Solana Relayer test-relayer");
        assert_eq!(relayer.network_type, NetworkType::Solana);

        let policy = relayer.policies.get_solana_policy();
        let swap_config = policy.get_swap_config().expect("Should have swap config");
        assert_eq!(swap_config.cron_schedule.as_ref(), Some(&cron));
    }

    #[test]
    fn test_filter_relayers_for_swap_preserves_stellar_relayer_data() {
        let cron = "0 1 * * * *".to_string();
        let relayers = vec![create_test_stellar_relayer_with_swap(
            "test-relayer",
            Some(cron.clone()),
        )];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(filtered.len(), 1);

        let relayer = &filtered[0];
        assert_eq!(relayer.id, "test-relayer");
        assert_eq!(relayer.name, "Stellar Relayer test-relayer");
        assert_eq!(relayer.network_type, NetworkType::Stellar);

        let policy = relayer.policies.get_stellar_policy();
        let swap_config = policy.get_swap_config().expect("Should have swap config");
        assert_eq!(swap_config.cron_schedule.as_ref(), Some(&cron));
    }

    #[test]
    fn test_filter_relayers_for_swap_filters_solana_without_swap_config() {
        let relayers = vec![
            create_test_solana_relayer_without_swap_config("solana-1"),
            create_test_solana_relayer_without_swap_config("solana-2"),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should filter out Solana relayers without swap config"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_filters_stellar_without_swap_config() {
        let relayers = vec![
            create_test_stellar_relayer_without_swap_config("stellar-1"),
            create_test_stellar_relayer_without_swap_config("stellar-2"),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            0,
            "Should filter out Stellar relayers without swap config"
        );
    }

    #[test]
    fn test_filter_relayers_for_swap_with_mixed_swap_configs() {
        let relayers = vec![
            create_test_solana_relayer_without_swap_config("solana-no-config"),
            create_test_solana_relayer_with_swap("solana-no-cron", None),
            create_test_solana_relayer_with_swap(
                "solana-with-cron",
                Some("0 0 * * * *".to_string()),
            ),
            create_test_stellar_relayer_without_swap_config("stellar-no-config"),
            create_test_stellar_relayer_with_swap("stellar-no-cron", None),
            create_test_stellar_relayer_with_swap(
                "stellar-with-cron",
                Some("0 0 * * * *".to_string()),
            ),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            2,
            "Should only include relayers with swap config and cron schedule"
        );

        let ids: Vec<&str> = filtered.iter().map(|r| r.id.as_str()).collect();
        assert!(
            ids.contains(&"solana-with-cron"),
            "Should include solana-with-cron"
        );
        assert!(
            ids.contains(&"stellar-with-cron"),
            "Should include stellar-with-cron"
        );
        assert!(
            !ids.contains(&"solana-no-config"),
            "Should not include solana without config"
        );
        assert!(
            !ids.contains(&"solana-no-cron"),
            "Should not include solana without cron"
        );
        assert!(
            !ids.contains(&"stellar-no-config"),
            "Should not include stellar without config"
        );
        assert!(
            !ids.contains(&"stellar-no-cron"),
            "Should not include stellar without cron"
        );
    }

    #[test]
    fn test_filter_relayers_preserves_order() {
        let relayers = vec![
            create_test_solana_relayer_with_swap("solana-1", Some("0 0 * * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-1", Some("0 0 * * * *".to_string())),
            create_test_solana_relayer_with_swap("solana-2", Some("0 0 * * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-2", Some("0 0 * * * *".to_string())),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(filtered.len(), 4);
        assert_eq!(filtered[0].id, "solana-1");
        assert_eq!(filtered[1].id, "stellar-1");
        assert_eq!(filtered[2].id, "solana-2");
        assert_eq!(filtered[3].id, "stellar-2");
    }

    #[test]
    fn test_filter_relayers_with_different_cron_formats() {
        let relayers = vec![
            create_test_solana_relayer_with_swap("solana-1", Some("0 0 * * * *".to_string())),
            create_test_solana_relayer_with_swap("solana-2", Some("*/5 * * * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-1", Some("0 0 12 * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-2", Some("0 */15 * * * *".to_string())),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(
            filtered.len(),
            4,
            "Should accept various valid cron schedule formats"
        );
    }

    #[test]
    fn test_filter_relayers_with_all_network_types() {
        let relayers = vec![
            create_test_evm_relayer("evm-1"),
            create_test_solana_relayer_with_swap("solana-1", Some("0 0 * * * *".to_string())),
            create_test_stellar_relayer_with_swap("stellar-1", Some("0 0 * * * *".to_string())),
        ];

        let filtered = filter_relayers_for_swap(relayers);

        assert_eq!(filtered.len(), 2, "Should only include Solana and Stellar");

        let network_types: Vec<NetworkType> = filtered.iter().map(|r| r.network_type).collect();
        assert!(
            network_types.contains(&NetworkType::Solana),
            "Should include Solana"
        );
        assert!(
            network_types.contains(&NetworkType::Stellar),
            "Should include Stellar"
        );
        assert!(
            !network_types.contains(&NetworkType::Evm),
            "Should not include EVM"
        );
    }
}
