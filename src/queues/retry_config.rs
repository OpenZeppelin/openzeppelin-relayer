use crate::models::NetworkType;

#[derive(Debug, Clone, Copy)]
pub struct RetryBackoffConfig {
    pub initial_ms: u64,
    pub max_ms: u64,
    pub jitter: f64,
}

pub const TX_REQUEST_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 500,
    max_ms: 5000,
    jitter: 0.99,
};
pub const TX_SUBMISSION_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 500,
    max_ms: 2000,
    jitter: 0.99,
};
pub const STATUS_GENERIC_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 8000,
    jitter: 0.99,
};
pub const STATUS_EVM_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 8000,
    max_ms: 12000,
    jitter: 0.99,
};
pub const STATUS_STELLAR_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 3000,
    jitter: 0.99,
};
pub const NOTIFICATION_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 8000,
    jitter: 0.99,
};
pub const TOKEN_SWAP_REQUEST_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 20000,
    jitter: 0.99,
};
pub const TX_CLEANUP_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 20000,
    jitter: 0.99,
};
pub const SYSTEM_CLEANUP_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 5000,
    max_ms: 20000,
    jitter: 0.99,
};
pub const RELAYER_HEALTH_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 10000,
    jitter: 0.99,
};
pub const TOKEN_SWAP_CRON_BACKOFF: RetryBackoffConfig = RetryBackoffConfig {
    initial_ms: 2000,
    max_ms: 5000,
    jitter: 0.99,
};

pub fn status_backoff_config(network_type: Option<NetworkType>) -> RetryBackoffConfig {
    match network_type {
        Some(NetworkType::Evm) => STATUS_EVM_BACKOFF,
        Some(NetworkType::Stellar) => STATUS_STELLAR_BACKOFF,
        Some(NetworkType::Solana) | None => STATUS_GENERIC_BACKOFF,
    }
}

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
