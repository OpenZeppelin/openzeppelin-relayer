//! Stellar relayer utility functions.
//!
//! Generic helpers for ledger math, fee slippage, and other reusable logic
//! shared across gas abstraction and related code.

use crate::constants::STELLAR_LEDGER_TIME_SECONDS;
use crate::models::RelayerError;
use crate::services::provider::StellarProviderTrait;

/// Default slippage tolerance for max_fee_amount in basis points (500 = 5%).
/// Allows fee fluctuation between quote and execution time.
pub const DEFAULT_SOROBAN_MAX_FEE_SLIPPAGE_BPS: u64 = 500;

/// Apply slippage tolerance to max_fee_amount for FeeForwarder.
///
/// The FeeForwarder contract has separate `fee_amount` (what relayer charges at execution)
/// and `max_fee_amount` (user's authorized ceiling). Setting them equal means no room for
/// fee fluctuation between quote and execution. This function applies a slippage buffer
/// to allow for price movement.
///
/// # Arguments
/// * `fee_in_token` - The calculated fee amount in token units
/// * `slippage_bps` - Slippage in basis points (default: [`DEFAULT_SOROBAN_MAX_FEE_SLIPPAGE_BPS`])
///
/// # Returns
/// The max_fee_amount with slippage buffer applied as i128
pub fn apply_max_fee_slippage_with_bps(fee_in_token: u64, slippage_bps: u64) -> i128 {
    let fee_with_slippage = (fee_in_token as u128) * (10000 + slippage_bps as u128) / 10000;
    fee_with_slippage as i128
}

/// Apply default slippage to max_fee_amount (uses [`DEFAULT_SOROBAN_MAX_FEE_SLIPPAGE_BPS`]).
pub fn apply_max_fee_slippage(fee_in_token: u64) -> i128 {
    apply_max_fee_slippage_with_bps(fee_in_token, DEFAULT_SOROBAN_MAX_FEE_SLIPPAGE_BPS)
}

/// Calculate the expiration ledger for authorization.
///
/// Uses the provider to get the current ledger sequence and adds the
/// specified validity duration (in seconds) converted to ledger count.
pub async fn get_expiration_ledger<P>(
    provider: &P,
    validity_seconds: u64,
) -> Result<u32, RelayerError>
where
    P: StellarProviderTrait + Send + Sync,
{
    let current_ledger = provider
        .get_latest_ledger()
        .await
        .map_err(|e| RelayerError::Internal(format!("Failed to get latest ledger: {e}")))?;

    let mut ledgers_to_add = validity_seconds.div_ceil(STELLAR_LEDGER_TIME_SECONDS);
    if ledgers_to_add == 0 {
        ledgers_to_add = 1;
    }
    Ok(current_ledger
        .sequence
        .saturating_add(ledgers_to_add as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;

    use crate::services::provider::MockStellarProviderTrait;

    // ============================================================================
    // Tests for apply_max_fee_slippage
    // ============================================================================

    #[test]
    fn test_apply_max_fee_slippage_basic() {
        // 5% slippage on 10000 should give 10500
        let result = apply_max_fee_slippage(10000);
        assert_eq!(result, 10500);
    }

    #[test]
    fn test_apply_max_fee_slippage_zero() {
        let result = apply_max_fee_slippage(0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_apply_max_fee_slippage_large_value() {
        let large_fee: u64 = 1_000_000_000_000;
        let result = apply_max_fee_slippage(large_fee);
        assert_eq!(result, 1_050_000_000_000i128);
    }

    #[test]
    fn test_apply_max_fee_slippage_small_value() {
        let result = apply_max_fee_slippage(100);
        assert_eq!(result, 105);
    }

    // ============================================================================
    // Tests for apply_max_fee_slippage_with_bps (direct)
    // ============================================================================

    #[test]
    fn test_apply_max_fee_slippage_with_bps_zero_slippage() {
        // 0 BPS = no slippage
        let result = apply_max_fee_slippage_with_bps(10000, 0);
        assert_eq!(result, 10000);
    }

    #[test]
    fn test_apply_max_fee_slippage_with_bps_one_percent() {
        // 100 BPS = 1%
        let result = apply_max_fee_slippage_with_bps(10000, 100);
        assert_eq!(result, 10100);
    }

    #[test]
    fn test_apply_max_fee_slippage_with_bps_ten_percent() {
        // 1000 BPS = 10%
        let result = apply_max_fee_slippage_with_bps(10000, 1000);
        assert_eq!(result, 11000);
    }

    #[test]
    fn test_apply_max_fee_slippage_with_bps_hundred_percent() {
        // 10000 BPS = 100% (double)
        let result = apply_max_fee_slippage_with_bps(10000, 10000);
        assert_eq!(result, 20000);
    }

    #[test]
    fn test_apply_max_fee_slippage_with_bps_zero_fee() {
        let result = apply_max_fee_slippage_with_bps(0, 500);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_apply_max_fee_slippage_with_bps_large_fee() {
        let large_fee: u64 = 1_000_000_000_000;
        // 5% slippage
        let result = apply_max_fee_slippage_with_bps(large_fee, 500);
        assert_eq!(result, 1_050_000_000_000i128);
    }

    #[test]
    fn test_apply_max_fee_slippage_with_bps_small_fee_rounds_down() {
        // 1 unit with 1 BPS (0.01%) — should round down to 1
        let result = apply_max_fee_slippage_with_bps(1, 1);
        // (1 * 10001) / 10000 = 1 (integer division)
        assert_eq!(result, 1);
    }

    // ============================================================================
    // Tests for get_expiration_ledger
    // ============================================================================

    #[tokio::test]
    async fn test_get_expiration_ledger_success() {
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        let result = get_expiration_ledger(&provider, 300).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 1060); // 1000 + 60
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_zero_seconds() {
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        let result = get_expiration_ledger(&provider, 0).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 1001); // 1000 + 1 (minimum)
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_provider_error() {
        use crate::services::provider::ProviderError;

        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Err(ProviderError::Other(
                "network error".to_string(),
            ))))
        });

        let result = get_expiration_ledger(&provider, 300).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            RelayerError::Internal(msg) => {
                assert!(msg.contains("Failed to get latest ledger"));
            }
            _ => panic!("Expected Internal error"),
        }
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_non_divisible_seconds() {
        // 7 seconds / 5 seconds per ledger = 2 ledgers (div_ceil)
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 1000,
                },
            )))
        });

        let result = get_expiration_ledger(&provider, 7).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 1002); // 1000 + ceil(7/5) = 1000 + 2
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_one_second() {
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 500,
                },
            )))
        });

        let result = get_expiration_ledger(&provider, 1).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 501); // 500 + ceil(1/5) = 500 + 1
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_sequence_near_max() {
        // Test saturating_add behavior near u32::MAX
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: u32::MAX - 1,
                },
            )))
        });

        let result = get_expiration_ledger(&provider, 300).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        // saturating_add should cap at u32::MAX
        assert_eq!(expiration, u32::MAX);
    }

    #[tokio::test]
    async fn test_get_expiration_ledger_exact_ledger_time() {
        // Exactly STELLAR_LEDGER_TIME_SECONDS (5 seconds) = 1 ledger
        let mut provider = MockStellarProviderTrait::new();
        provider.expect_get_latest_ledger().returning(|| {
            Box::pin(ready(Ok(
                soroban_rs::stellar_rpc_client::GetLatestLedgerResponse {
                    id: "test".to_string(),
                    protocol_version: 20,
                    sequence: 2000,
                },
            )))
        });

        let result = get_expiration_ledger(&provider, STELLAR_LEDGER_TIME_SECONDS).await;
        assert!(result.is_ok());
        let expiration = result.unwrap();
        assert_eq!(expiration, 2001); // 2000 + 1
    }
}
