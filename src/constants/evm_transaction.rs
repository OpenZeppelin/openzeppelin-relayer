use crate::models::evm::Speed;
use chrono::Duration;

pub const DEFAULT_TX_VALID_TIMESPAN: i64 = 8 * 60 * 60 * 1000; // 8 hours in milliseconds

pub const DEFAULT_TRANSACTION_SPEED: Speed = Speed::Fast;

pub const DEFAULT_GAS_LIMIT: u64 = 21000;
pub const ERC20_TRANSFER_GAS_LIMIT: u64 = 65_000;
pub const ERC721_TRANSFER_GAS_LIMIT: u64 = 80_000;
pub const COMPLEX_GAS_LIMIT: u64 = 200_000;
pub const GAS_TX_CREATE_CONTRACT: u64 = 53000;

pub const GAS_TX_DATA_ZERO: u64 = 4; // Cost per zero byte in data
pub const GAS_TX_DATA_NONZERO: u64 = 16; // Cost per non-zero byte in data

/// Gas limit buffer multiplier for automatic gas limit estimation, 10% increase
pub const GAS_LIMIT_BUFFER_MULTIPLIER: u64 = 110;

/// Minimum gas price bump factor for transaction replacements (10% increase)
pub const MIN_BUMP_FACTOR: f64 = 1.1;

// Maximum number of transaction attempts before considering a NOOP
pub const MAXIMUM_TX_ATTEMPTS: usize = 50;
// Maximum number of NOOP transactions to attempt
pub const MAXIMUM_NOOP_RETRY_ATTEMPTS: u32 = 50;

/// Time to resubmit for Arbitrum networks
pub const ARBITRUM_TIME_TO_RESUBMIT: i64 = 20_000;

// Gas limit for Arbitrum networks (mainly used for NOOP transactions (with no data), covers L1 + L2 costs)
pub const ARBITRUM_GAS_LIMIT: u64 = 50_000;

/// Gas price cache refresh timeout in seconds (5 minutes)
/// Used to cleanup stuck refresh operations that may have failed to complete
pub const GAS_PRICE_CACHE_REFRESH_TIMEOUT_SECS: u64 = 300;

/// Number of historical blocks to fetch for fee history analysis
pub const HISTORICAL_BLOCKS: u64 = 4;

// EVM Status check and timeout constants

/// Initial delay before first status check (in seconds)
pub const EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS: i64 = 8;

/// Minimum age of transaction before allowing resubmission and timeout checks (in seconds)
/// Transactions younger than this will still get status updates from blockchain,
/// but resubmission logic and timeout checks are deferred to prevent premature actions.
pub const EVM_MIN_AGE_FOR_RESUBMIT_SECONDS: i64 = 20;

/// Timeout for preparation phase: Pending → Sent (in minutes)
/// Increased from 1 to 2 minutes to provide wider recovery window
pub const EVM_PREPARE_TIMEOUT_MINUTES: i64 = 2;

/// Timeout for submission phase: Sent → Submitted (in minutes)
pub const EVM_SUBMIT_TIMEOUT_MINUTES: i64 = 5;

/// Timeout for resend phase: Sent → Submitted (in seconds)
pub const EVM_RESEND_TIMEOUT_SECONDS: i64 = 25;

/// Trigger recovery for stuck Pending transactions (in seconds)
pub const EVM_PENDING_RECOVERY_TRIGGER_SECONDS: i64 = 20;

/// Minimum age before attempting hash recovery for transactions (in minutes)
pub const EVM_MIN_AGE_FOR_HASH_RECOVERY_MINUTES: i64 = 2;

/// Minimum number of hashes required before attempting hash recovery
pub const EVM_MIN_HASHES_FOR_RECOVERY: usize = 3;

/// Get preparation timeout duration
pub fn get_evm_prepare_timeout() -> Duration {
    Duration::minutes(EVM_PREPARE_TIMEOUT_MINUTES)
}

/// Get submission timeout duration
pub fn get_evm_submit_timeout() -> Duration {
    Duration::minutes(EVM_SUBMIT_TIMEOUT_MINUTES)
}

/// Get resend timeout duration
pub fn get_evm_resend_timeout() -> Duration {
    Duration::seconds(EVM_RESEND_TIMEOUT_SECONDS)
}

/// Get pending recovery trigger duration
pub fn get_evm_pending_recovery_trigger_timeout() -> Duration {
    Duration::seconds(EVM_PENDING_RECOVERY_TRIGGER_SECONDS)
}

/// Get status check initial delay duration
pub fn get_evm_status_check_initial_delay() -> Duration {
    Duration::seconds(EVM_STATUS_CHECK_INITIAL_DELAY_SECONDS)
}

/// Get minimum age for hash recovery duration
pub fn get_evm_min_age_for_hash_recovery() -> Duration {
    Duration::minutes(EVM_MIN_AGE_FOR_HASH_RECOVERY_MINUTES)
}

/// Error message patterns indicating a transaction was already submitted or its nonce consumed.
/// Shared between the retry layer (`is_non_retriable_transaction_rpc_message`) and
/// the domain layer (`is_already_submitted_error`).
///
/// Each entry is a lowercased substring to match against the RPC error message.
pub const ALREADY_SUBMITTED_PATTERNS: &[&str] = &[
    "nonce too low",
    "nonce is too low",
    "already known",
    "replacement transaction underpriced",
    "same hash was already imported",
];

/// Error message patterns indicating the transaction nonce is ahead of the expected on-chain nonce.
/// This can be transient (burst ordering: tx N+1 arrives before N) or persistent (counter drift).
///
/// Checked **after** `ALREADY_SUBMITTED_PATTERNS` in `classify_submission_error` to avoid
/// ambiguity. Each entry is a lowercased substring to match against the RPC error message.
pub const NONCE_TOO_HIGH_PATTERNS: &[&str] = &[
    "nonce too high",              // Geth, Erigon, Hardhat, Anvil
    "nonce is too high",           // Geth, Erigon, Hardhat, Anvil
    "nonce too far in the future", // Besu
    "exceeds next nonce",          // Nethermind
    "nonce out of range",          // Arbitrum, Optimism, specialized RPCs
    "tx-nonce-too-high",           // Certain SaaS RPC providers (e.g. Alchemy/Infura internal)
];

/// Maximum number of "nonce too high" retries before escalating to a nonce health job.
/// With ~25s between retries (driven by status checker resend timeout), this means
/// escalation happens within ~75s — enough time for transient burst ordering to resolve.
pub const MAX_NONCE_TOO_HIGH_RETRIES: u32 = 3;

/// Maximum number of nonces to scan when detecting gaps between on-chain and local counter.
/// Gaps beyond this range are logged for operator investigation rather than automated recovery.
pub const MAX_GAP_SCAN_RANGE: u64 = 100;

/// Metadata key used in `RelayerHealthCheck` to indicate a targeted health action.
pub const HEALTH_CHECK_ACTION_KEY: &str = "health_check_action";

/// Value for `HEALTH_CHECK_ACTION_KEY` that triggers nonce gap detection and resolution.
pub const HEALTH_CHECK_ACTION_NONCE_HEALTH: &str = "nonce_health";

/// Optional metadata key carrying a nonce hint for the health action.
/// When present, `resolve_nonce_gaps` ensures the counter covers at least `hint + 1`
/// so the scan range includes the hinted nonce. This handles the case where the
/// counter was reset (e.g., after a restart) but a tx at a higher nonce still exists.
pub const HEALTH_CHECK_NONCE_HINT_KEY: &str = "nonce_hint";
/// Checks if a lowercased message matches "known transaction" without matching
/// "unknown transaction" (substring false positive).
pub fn matches_known_transaction(msg_lower: &str) -> bool {
    if let Some(pos) = msg_lower.find("known transaction") {
        // Reject if preceded by "un" (i.e. "unknown transaction")
        if msg_lower[..pos].ends_with("un") {
            return false;
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_too_high_patterns_match_expected_strings() {
        let cases = [
            "nonce too high",
            "nonce is too high",
            "nonce too far in the future",
            "exceeds next nonce",
            "nonce out of range",
        ];
        for case in &cases {
            let msg_lower = case.to_lowercase();
            assert!(
                NONCE_TOO_HIGH_PATTERNS
                    .iter()
                    .any(|p| msg_lower.contains(p)),
                "Expected NONCE_TOO_HIGH_PATTERNS to match: {case}"
            );
        }
    }

    #[test]
    fn test_matches_known_transaction_does_not_match_nonce_too_high() {
        let nonce_too_high_msgs = [
            "nonce too high",
            "nonce is too high",
            "nonce too far in the future",
            "exceeds next nonce",
            "nonce out of range",
        ];
        for msg in &nonce_too_high_msgs {
            assert!(
                !matches_known_transaction(&msg.to_lowercase()),
                "matches_known_transaction should NOT match nonce-too-high message: {msg}"
            );
        }
    }

    #[test]
    fn test_matches_known_transaction_matches_known_transaction() {
        assert!(matches_known_transaction("known transaction"));
        assert!(matches_known_transaction("already known transaction here"));
    }

    #[test]
    fn test_matches_known_transaction_does_not_match_unknown_transaction() {
        assert!(!matches_known_transaction("unknown transaction"));
        assert!(!matches_known_transaction("unknown transaction status"));
    }
}
