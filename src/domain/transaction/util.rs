//! This module provides utility functions for handling transactions within the application.
//!
//! It includes functions to retrieve transactions by ID, create relayer transactions, and
//! handle unsupported operations for specific relayers. The module interacts with various
//! repositories and factories to perform these operations.
use actix_web::web::ThinData;
use chrono::{DateTime, Duration, Utc};

use crate::{
    domain::get_relayer_by_id,
    jobs::JobProducerTrait,
    models::{
        ApiError, DefaultAppState, NetworkRepoModel, NotificationRepoModel, RelayerRepoModel,
        SignerRepoModel, ThinDataAppState, TransactionError, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
};

use super::{NetworkTransaction, RelayerTransactionFactory};

/// Retrieves a transaction by its ID.
///
/// # Arguments
///
/// * `transaction_id` - A `String` representing the ID of the transaction to retrieve.
/// * `state` - A reference to the application state, wrapped in `ThinData`.
///
/// # Returns
///
/// A `Result` containing a `TransactionRepoModel` if successful, or an `ApiError` if an error
/// occurs.
pub async fn get_transaction_by_id<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>(
    transaction_id: String,
    state: &ThinDataAppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>,
) -> Result<TransactionRepoModel, ApiError>
where
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
{
    state
        .transaction_repository
        .get_by_id(transaction_id)
        .await
        .map_err(|e| e.into())
}

/// Creates a relayer network transaction instance based on the relayer ID.
///
/// # Arguments
///
/// * `relayer_id` - A `String` representing the ID of the relayer.
/// * `state` - A reference to the application state, wrapped in `ThinData`.
///
/// # Returns
///
/// A `Result` containing a `NetworkTransaction` if successful, or an `ApiError` if an error occurs.
pub async fn get_relayer_transaction(
    relayer_id: String,
    state: &ThinData<DefaultAppState>,
) -> Result<NetworkTransaction, ApiError> {
    let relayer_model = get_relayer_by_id(relayer_id, state).await?;
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerTransactionFactory::create_transaction(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.network_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
    .await
    .map_err(|e| e.into())
}

/// Creates a relayer network transaction using a relayer model.
///
/// # Arguments
///
/// * `relayer_model` - A `RelayerRepoModel` representing the relayer.
/// * `state` - A reference to the application state, wrapped in `ThinData`.
///
/// # Returns
///
/// A `Result` containing a `NetworkTransaction` if successful, or an `ApiError` if an error occurs.
pub async fn get_relayer_transaction_by_model(
    relayer_model: RelayerRepoModel,
    state: &ThinData<DefaultAppState>,
) -> Result<NetworkTransaction, ApiError> {
    let signer_model = state
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    RelayerTransactionFactory::create_transaction(
        relayer_model,
        signer_model,
        state.relayer_repository(),
        state.network_repository(),
        state.transaction_repository(),
        state.transaction_counter_store(),
        state.job_producer(),
    )
    .await
    .map_err(|e| e.into())
}

/// Returns an error indicating that Solana relayers are not supported.
///
/// # Returns
///
/// A `Result` that always contains a `TransactionError::NotSupported` error.
pub fn solana_not_supported_transaction<T>() -> Result<T, TransactionError> {
    Err(TransactionError::NotSupported(
        "Endpoint is not supported for Solana relayers".to_string(),
    ))
}

/// Gets the age of a transaction since it was created.
///
/// # Arguments
///
/// * `tx` - The transaction repository model
///
/// # Returns
///
/// A `Result` containing the `Duration` since the transaction was created,
/// or a `TransactionError` if the created_at timestamp cannot be parsed.
pub fn get_age_since_created(tx: &TransactionRepoModel) -> Result<Duration, TransactionError> {
    let created = DateTime::parse_from_rfc3339(&tx.created_at)
        .map_err(|e| {
            TransactionError::UnexpectedError(format!("Invalid created_at timestamp: {e}"))
        })?
        .with_timezone(&Utc);
    Ok(Utc::now().signed_duration_since(created))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::mocks::mockutils::create_mock_transaction;

    mod get_age_since_created_tests {
        use super::*;

        /// Helper to create a test transaction with a specific created_at timestamp
        fn create_test_tx_with_age(seconds_ago: i64) -> TransactionRepoModel {
            let created_at = (Utc::now() - Duration::seconds(seconds_ago)).to_rfc3339();
            let mut tx = create_mock_transaction();
            tx.created_at = created_at;
            tx
        }

        #[test]
        fn test_returns_correct_age_for_recent_transaction() {
            let tx = create_test_tx_with_age(30); // 30 seconds ago
            let age = get_age_since_created(&tx).unwrap();

            // Allow for small timing differences (within 1 second)
            assert!(age.num_seconds() >= 29 && age.num_seconds() <= 31);
        }

        #[test]
        fn test_returns_correct_age_for_old_transaction() {
            let tx = create_test_tx_with_age(3600); // 1 hour ago
            let age = get_age_since_created(&tx).unwrap();

            // Allow for small timing differences
            assert!(age.num_seconds() >= 3599 && age.num_seconds() <= 3601);
        }

        #[test]
        fn test_returns_zero_age_for_just_created_transaction() {
            let tx = create_test_tx_with_age(0); // Just now
            let age = get_age_since_created(&tx).unwrap();

            // Should be very close to 0
            assert!(age.num_seconds() >= 0 && age.num_seconds() <= 1);
        }

        #[test]
        fn test_handles_negative_age_gracefully() {
            // Create transaction with future timestamp (clock skew scenario)
            let created_at = (Utc::now() + Duration::seconds(10)).to_rfc3339();
            let mut tx = create_mock_transaction();
            tx.created_at = created_at;

            let age = get_age_since_created(&tx).unwrap();

            // Age should be negative
            assert!(age.num_seconds() < 0);
        }

        #[test]
        fn test_returns_error_for_invalid_created_at() {
            let mut tx = create_mock_transaction();
            tx.created_at = "invalid-timestamp".to_string();

            let result = get_age_since_created(&tx);
            assert!(result.is_err());

            match result.unwrap_err() {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Invalid created_at timestamp"));
                }
                _ => panic!("Expected UnexpectedError"),
            }
        }

        #[test]
        fn test_returns_error_for_empty_created_at() {
            let mut tx = create_mock_transaction();
            tx.created_at = "".to_string();

            let result = get_age_since_created(&tx);
            assert!(result.is_err());

            match result.unwrap_err() {
                TransactionError::UnexpectedError(msg) => {
                    assert!(msg.contains("Invalid created_at timestamp"));
                }
                _ => panic!("Expected UnexpectedError"),
            }
        }

        #[test]
        fn test_handles_various_rfc3339_formats() {
            let mut tx = create_mock_transaction();

            // Test with UTC timezone
            tx.created_at = "2025-01-01T12:00:00Z".to_string();
            assert!(get_age_since_created(&tx).is_ok());

            // Test with offset timezone
            tx.created_at = "2025-01-01T12:00:00+00:00".to_string();
            assert!(get_age_since_created(&tx).is_ok());

            // Test with milliseconds
            tx.created_at = "2025-01-01T12:00:00.123Z".to_string();
            assert!(get_age_since_created(&tx).is_ok());

            // Test with microseconds
            tx.created_at = "2025-01-01T12:00:00.123456Z".to_string();
            assert!(get_age_since_created(&tx).is_ok());
        }

        #[test]
        fn test_handles_different_timezones() {
            let mut tx = create_mock_transaction();

            // Test with positive offset
            tx.created_at = "2025-01-01T12:00:00+05:30".to_string();
            assert!(get_age_since_created(&tx).is_ok());

            // Test with negative offset
            tx.created_at = "2025-01-01T12:00:00-08:00".to_string();
            assert!(get_age_since_created(&tx).is_ok());
        }

        #[test]
        fn test_age_calculation_is_consistent() {
            let tx = create_test_tx_with_age(60); // 1 minute ago

            // Call multiple times in quick succession
            let age1 = get_age_since_created(&tx).unwrap();
            let age2 = get_age_since_created(&tx).unwrap();
            let age3 = get_age_since_created(&tx).unwrap();

            // All should be very close (within 1 second of each other)
            let diff1 = (age2.num_seconds() - age1.num_seconds()).abs();
            let diff2 = (age3.num_seconds() - age2.num_seconds()).abs();

            assert!(diff1 <= 1);
            assert!(diff2 <= 1);
        }

        #[test]
        fn test_returns_error_for_malformed_timestamp() {
            let mut tx = create_mock_transaction();

            // Various malformed timestamps
            let invalid_timestamps = vec![
                "2025-13-01T12:00:00Z", // Invalid month
                "2025-01-32T12:00:00Z", // Invalid day
                "2025-01-01T25:00:00Z", // Invalid hour
                "2025-01-01T12:60:00Z", // Invalid minute
                "not-a-date",
                "2025/01/01",
                "12:00:00",
                "just some text",
                "2025-01-01", // Missing time
                "12:00:00Z",  // Missing date
            ];

            for invalid_ts in invalid_timestamps {
                tx.created_at = invalid_ts.to_string();
                let result = get_age_since_created(&tx);
                assert!(result.is_err(), "Expected error for: {}", invalid_ts);
            }
        }
    }

    mod solana_not_supported_transaction_tests {
        use super::*;

        #[test]
        fn test_returns_not_supported_error() {
            let result: Result<(), TransactionError> = solana_not_supported_transaction();

            assert!(result.is_err());
            match result.unwrap_err() {
                TransactionError::NotSupported(msg) => {
                    assert_eq!(msg, "Endpoint is not supported for Solana relayers");
                }
                _ => panic!("Expected NotSupported error"),
            }
        }

        #[test]
        fn test_works_with_different_return_types() {
            // Test with String return type
            let result: Result<String, TransactionError> = solana_not_supported_transaction();
            assert!(result.is_err());

            // Test with i32 return type
            let result: Result<i32, TransactionError> = solana_not_supported_transaction();
            assert!(result.is_err());

            // Test with TransactionRepoModel return type
            let result: Result<TransactionRepoModel, TransactionError> =
                solana_not_supported_transaction();
            assert!(result.is_err());
        }

        #[test]
        fn test_error_message_is_descriptive() {
            let result: Result<(), TransactionError> = solana_not_supported_transaction();

            let error = result.unwrap_err();
            let error_msg = error.to_string();

            assert!(error_msg.contains("Solana"));
            assert!(error_msg.contains("not supported"));
        }

        #[test]
        fn test_multiple_calls_return_same_error() {
            let result1: Result<(), TransactionError> = solana_not_supported_transaction();
            let result2: Result<(), TransactionError> = solana_not_supported_transaction();
            let result3: Result<(), TransactionError> = solana_not_supported_transaction();

            // All should be errors
            assert!(result1.is_err());
            assert!(result2.is_err());
            assert!(result3.is_err());

            // All should have the same message
            let msg1 = match result1.unwrap_err() {
                TransactionError::NotSupported(m) => m,
                _ => panic!("Wrong error type"),
            };
            let msg2 = match result2.unwrap_err() {
                TransactionError::NotSupported(m) => m,
                _ => panic!("Wrong error type"),
            };
            let msg3 = match result3.unwrap_err() {
                TransactionError::NotSupported(m) => m,
                _ => panic!("Wrong error type"),
            };

            assert_eq!(msg1, msg2);
            assert_eq!(msg2, msg3);
        }
    }
}
