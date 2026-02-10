use crate::models::NetworkType;

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

/// Computes status-check retry delay in seconds using capped exponential backoff.
///
/// `network_type` picks the base profile and `attempt` controls exponential growth.
/// The exponent is capped at `16` to avoid overflow (`attempt.min(16)`), delay is
/// capped at the profile `max_ms`, and the returned value is rounded up to whole
/// seconds via `div_ceil(1000)`.
pub fn status_check_retry_delay_secs(network_type: Option<NetworkType>, attempt: usize) -> i32 {
    let cfg = status_backoff_config(network_type);
    let factor = 2_u64.saturating_pow(attempt.min(16) as u32);
    let delay_ms = cfg.initial_ms.saturating_mul(factor).min(cfg.max_ms);
    delay_ms.div_ceil(1000) as i32
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
}
