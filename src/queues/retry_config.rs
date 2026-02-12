use crate::models::NetworkType;

use super::QueueType;

/// Exponential backoff configuration values in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct RetryBackoffConfig {
    /// Initial retry delay in milliseconds before exponential growth.
    pub initial_ms: u64,
    /// Maximum retry delay in milliseconds after capping.
    pub max_ms: u64,
    /// Jitter factor applied by worker retry policies using this config.
    pub jitter: f64,
}

/// Backoff profile for transaction-request retries.
pub const TX_REQUEST_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 500,
    max_ms: 5000,
    jitter: 0.99,
};
/// Backoff profile for transaction-submission retries.
pub const TX_SUBMISSION_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 500,
    max_ms: 2000,
    jitter: 0.99,
};
/// Backoff profile for generic status-check retries (Solana/default).
pub const STATUS_GENERIC_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 8000,
    jitter: 0.99,
};
/// Backoff profile for EVM status-check retries.
pub const STATUS_EVM_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 8000,
    max_ms: 12000,
    jitter: 0.99,
};
/// Backoff profile for Stellar status-check retries.
pub const STATUS_STELLAR_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 3000,
    jitter: 0.99,
};
/// Backoff profile for notification delivery retries.
pub const NOTIFICATION_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 8000,
    jitter: 0.99,
};
/// Backoff profile for token-swap request retries.
pub const TOKEN_SWAP_REQUEST_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 20000,
    jitter: 0.99,
};
/// Backoff profile for transaction-cleanup retries.
pub const TX_CLEANUP_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 20000,
    jitter: 0.99,
};
/// Backoff profile for system-cleanup retries.
pub const SYSTEM_CLEANUP_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 20000,
    jitter: 0.99,
};
/// Backoff profile for relayer-health-check retries.
pub const RELAYER_HEALTH_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 10000,
    jitter: 0.99,
};
/// Backoff profile for token-swap cron retries.
pub const TOKEN_SWAP_CRON_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 5000,
    jitter: 0.99,
};

/// Returns status-check backoff config for a network type.
///
/// `network_type` selects network-specific status timing:
/// EVM -> `STATUS_EVM_BACKOFF`, Stellar -> `STATUS_STELLAR_BACKOFF`,
/// Solana/`None` -> `STATUS_GENERIC_BACKOFF`.
pub fn status_backoff_config(network_type: Option<NetworkType>) -> RetryBackoffConfig {
    match network_type {
        Some(NetworkType::Evm) => STATUS_EVM_BACKOFF,
        Some(NetworkType::Stellar) => STATUS_STELLAR_BACKOFF,
        Some(NetworkType::Solana) | None => STATUS_GENERIC_BACKOFF,
    }
}

/// Computes retry delay in seconds from any backoff config + attempt.
///
/// Uses capped exponential backoff: `initial_ms * 2^attempt`, capped at `max_ms`.
/// The exponent is clamped at `16` to avoid overflow, and the result is rounded
/// up to whole seconds via `div_ceil(1000)`.
pub fn retry_delay_secs(config: RetryBackoffConfig, attempt: usize) -> i32 {
    let factor = 2_u64.saturating_pow(attempt.min(16) as u32);
    let delay_ms = config.initial_ms.saturating_mul(factor).min(config.max_ms);
    delay_ms.div_ceil(1000) as i32
}

/// Computes status-check retry delay in seconds using capped exponential backoff.
///
/// Delegates to [`retry_delay_secs`] with the network-specific backoff profile.
pub fn status_check_retry_delay_secs(network_type: Option<NetworkType>, attempt: usize) -> i32 {
    retry_delay_secs(status_backoff_config(network_type), attempt)
}

/// Returns the backoff config for a given queue type.
///
/// Status-check queues return [`STATUS_GENERIC_BACKOFF`] here; for network-specific
/// status timing use [`status_backoff_config`] instead.
pub fn backoff_config_for_queue(queue_type: QueueType) -> RetryBackoffConfig {
    match queue_type {
        QueueType::TransactionRequest => TX_REQUEST_BACKOFF,
        QueueType::TransactionSubmission => TX_SUBMISSION_BACKOFF,
        QueueType::Notification => NOTIFICATION_BACKOFF,
        QueueType::TokenSwapRequest => TOKEN_SWAP_REQUEST_BACKOFF,
        QueueType::RelayerHealthCheck => RELAYER_HEALTH_BACKOFF,
        QueueType::StatusCheck | QueueType::StatusCheckEvm | QueueType::StatusCheckStellar => {
            STATUS_GENERIC_BACKOFF
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_check_retry_delay_secs_caps() {
        assert_eq!(status_check_retry_delay_secs(Some(NetworkType::Evm), 0), 8);
        assert_eq!(status_check_retry_delay_secs(Some(NetworkType::Evm), 1), 12);
        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Evm), 10),
            12
        );

        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Stellar), 0),
            2
        );
        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Stellar), 1),
            3
        );
        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Stellar), 10),
            3
        );

        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Solana), 0),
            5
        );
        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Solana), 1),
            8
        );
        assert_eq!(
            status_check_retry_delay_secs(Some(NetworkType::Solana), 10),
            8
        );
    }

    #[test]
    fn test_status_backoff_config_selects_correct_profile() {
        let evm = status_backoff_config(Some(NetworkType::Evm));
        assert_eq!(evm.initial_ms, STATUS_EVM_BACKOFF.initial_ms);
        assert_eq!(evm.max_ms, STATUS_EVM_BACKOFF.max_ms);

        let stellar = status_backoff_config(Some(NetworkType::Stellar));
        assert_eq!(stellar.initial_ms, STATUS_STELLAR_BACKOFF.initial_ms);

        let solana = status_backoff_config(Some(NetworkType::Solana));
        assert_eq!(solana.initial_ms, STATUS_GENERIC_BACKOFF.initial_ms);
    }

    #[test]
    fn test_status_backoff_config_none_uses_generic() {
        let none_cfg = status_backoff_config(None);
        let solana_cfg = status_backoff_config(Some(NetworkType::Solana));
        assert_eq!(none_cfg.initial_ms, solana_cfg.initial_ms);
        assert_eq!(none_cfg.max_ms, solana_cfg.max_ms);
    }

    #[test]
    fn test_retry_delay_secs_basic() {
        // TX_REQUEST_BACKOFF: initial_ms=500, max_ms=5000
        assert_eq!(retry_delay_secs(TX_REQUEST_BACKOFF, 0), 1); // 500ms -> 1s
        assert_eq!(retry_delay_secs(TX_REQUEST_BACKOFF, 1), 1); // 1000ms -> 1s
        assert_eq!(retry_delay_secs(TX_REQUEST_BACKOFF, 2), 2); // 2000ms -> 2s
        assert_eq!(retry_delay_secs(TX_REQUEST_BACKOFF, 3), 4); // 4000ms -> 4s
        assert_eq!(retry_delay_secs(TX_REQUEST_BACKOFF, 4), 5); // capped at 5000ms -> 5s
        assert_eq!(retry_delay_secs(TX_REQUEST_BACKOFF, 10), 5); // stays capped
    }

    #[test]
    fn test_retry_delay_secs_never_exceeds_max() {
        let configs = [
            TX_REQUEST_BACKOFF,
            TX_SUBMISSION_BACKOFF,
            NOTIFICATION_BACKOFF,
            TOKEN_SWAP_REQUEST_BACKOFF,
            RELAYER_HEALTH_BACKOFF,
        ];
        for config in configs {
            for attempt in 0..20 {
                let delay = retry_delay_secs(config, attempt);
                assert!(
                    delay <= config.max_ms.div_ceil(1000) as i32,
                    "attempt {attempt} delay {delay}s exceeds max {}ms",
                    config.max_ms
                );
            }
        }
    }

    #[test]
    fn test_retry_delay_secs_delegates_correctly() {
        // Verify status_check_retry_delay_secs matches retry_delay_secs with same config
        for attempt in 0..10 {
            assert_eq!(
                status_check_retry_delay_secs(Some(NetworkType::Evm), attempt),
                retry_delay_secs(STATUS_EVM_BACKOFF, attempt),
            );
        }
    }

    #[test]
    fn test_backoff_config_for_queue_maps_correctly() {
        assert_eq!(
            backoff_config_for_queue(QueueType::TransactionRequest).initial_ms,
            TX_REQUEST_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::TransactionSubmission).initial_ms,
            TX_SUBMISSION_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::Notification).initial_ms,
            NOTIFICATION_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::TokenSwapRequest).initial_ms,
            TOKEN_SWAP_REQUEST_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::RelayerHealthCheck).initial_ms,
            RELAYER_HEALTH_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::StatusCheck).initial_ms,
            STATUS_GENERIC_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::StatusCheckEvm).initial_ms,
            STATUS_GENERIC_BACKOFF.initial_ms
        );
        assert_eq!(
            backoff_config_for_queue(QueueType::StatusCheckStellar).initial_ms,
            STATUS_GENERIC_BACKOFF.initial_ms
        );
    }

    #[test]
    fn test_status_check_retry_delay_never_exceeds_max() {
        for attempt in 0..20 {
            let evm_delay = status_check_retry_delay_secs(Some(NetworkType::Evm), attempt);
            assert!(
                evm_delay <= STATUS_EVM_BACKOFF.max_ms.div_ceil(1000) as i32,
                "EVM attempt {attempt} delay {evm_delay}s exceeds max"
            );

            let stellar_delay = status_check_retry_delay_secs(Some(NetworkType::Stellar), attempt);
            assert!(
                stellar_delay <= STATUS_STELLAR_BACKOFF.max_ms.div_ceil(1000) as i32,
                "Stellar attempt {attempt} delay {stellar_delay}s exceeds max"
            );
        }
    }
}
