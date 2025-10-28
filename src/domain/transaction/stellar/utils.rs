//! Utility functions for Stellar transaction domain logic.
use crate::models::OperationSpec;
use crate::models::RelayerError;
use crate::services::provider::StellarProviderTrait;
use soroban_rs::xdr;
use tracing::info;

/// Returns true if any operation needs simulation (contract invocation, creation, or wasm upload).
pub fn needs_simulation(operations: &[OperationSpec]) -> bool {
    operations.iter().any(|op| {
        matches!(
            op,
            OperationSpec::InvokeContract { .. }
                | OperationSpec::CreateContract { .. }
                | OperationSpec::UploadWasm { .. }
        )
    })
}

pub fn next_sequence_u64(seq_num: i64) -> Result<u64, RelayerError> {
    let next_i64 = seq_num
        .checked_add(1)
        .ok_or_else(|| RelayerError::ProviderError("sequence overflow".into()))?;
    u64::try_from(next_i64)
        .map_err(|_| RelayerError::ProviderError("sequence overflows u64".into()))
}

pub fn i64_from_u64(value: u64) -> Result<i64, RelayerError> {
    i64::try_from(value).map_err(|_| RelayerError::ProviderError("u64→i64 overflow".into()))
}

/// Detects if an error is due to a bad sequence number.
/// Returns true if the error message contains indicators of sequence number mismatch.
pub fn is_bad_sequence_error(error_msg: &str) -> bool {
    let error_lower = error_msg.to_lowercase();
    error_lower.contains("txbadseq")
}

/// Fetches the current sequence number from the blockchain and calculates the next usable sequence.
/// This is a shared helper that can be used by both stellar_relayer and stellar_transaction.
///
/// # Returns
/// The next usable sequence number (on-chain sequence + 1)
pub async fn fetch_next_sequence_from_chain<P>(
    provider: &P,
    relayer_address: &str,
) -> Result<u64, String>
where
    P: StellarProviderTrait,
{
    info!(
        "Fetching sequence from chain for address: {}",
        relayer_address
    );

    // Fetch account info from chain
    let account = provider
        .get_account(relayer_address)
        .await
        .map_err(|e| format!("Failed to fetch account from chain: {}", e))?;

    let on_chain_seq = account.seq_num.0; // Extract the i64 value
    let next_usable = next_sequence_u64(on_chain_seq)
        .map_err(|e| format!("Failed to calculate next sequence: {}", e))?;

    info!(
        "Fetched sequence from chain: on-chain={}, next usable={}",
        on_chain_seq, next_usable
    );
    Ok(next_usable)
}

/// Convert a V0 transaction to V1 format for signing.
/// This is needed because the signature payload for V0 transactions uses V1 format internally.
pub fn convert_v0_to_v1_transaction(v0_tx: &xdr::TransactionV0) -> xdr::Transaction {
    xdr::Transaction {
        source_account: xdr::MuxedAccount::Ed25519(v0_tx.source_account_ed25519.clone()),
        fee: v0_tx.fee,
        seq_num: v0_tx.seq_num.clone(),
        cond: match v0_tx.time_bounds.clone() {
            Some(tb) => xdr::Preconditions::Time(tb),
            None => xdr::Preconditions::None,
        },
        memo: v0_tx.memo.clone(),
        operations: v0_tx.operations.clone(),
        ext: xdr::TransactionExt::V0,
    }
}

/// Create a signature payload for the given envelope type
pub fn create_signature_payload(
    envelope: &xdr::TransactionEnvelope,
    network_id: &xdr::Hash,
) -> Result<xdr::TransactionSignaturePayload, RelayerError> {
    let tagged_transaction = match envelope {
        xdr::TransactionEnvelope::TxV0(e) => {
            // For V0, convert to V1 transaction format for signing
            let v1_tx = convert_v0_to_v1_transaction(&e.tx);
            xdr::TransactionSignaturePayloadTaggedTransaction::Tx(v1_tx)
        }
        xdr::TransactionEnvelope::Tx(e) => {
            xdr::TransactionSignaturePayloadTaggedTransaction::Tx(e.tx.clone())
        }
        xdr::TransactionEnvelope::TxFeeBump(e) => {
            xdr::TransactionSignaturePayloadTaggedTransaction::TxFeeBump(e.tx.clone())
        }
    };

    Ok(xdr::TransactionSignaturePayload {
        network_id: network_id.clone(),
        tagged_transaction,
    })
}

/// Create signature payload for a transaction directly (for operations-based signing)
pub fn create_transaction_signature_payload(
    transaction: &xdr::Transaction,
    network_id: &xdr::Hash,
) -> xdr::TransactionSignaturePayload {
    xdr::TransactionSignaturePayload {
        network_id: network_id.clone(),
        tagged_transaction: xdr::TransactionSignaturePayloadTaggedTransaction::Tx(
            transaction.clone(),
        ),
    }
}

// ============================================================================
// Status Check Utility Functions
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AssetSpec;
    use crate::models::{AuthSpec, ContractSource, WasmSource};

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    fn payment_op(destination: &str) -> OperationSpec {
        OperationSpec::Payment {
            destination: destination.to_string(),
            amount: 100,
            asset: AssetSpec::Native,
        }
    }

    #[test]
    fn returns_false_for_only_payment_ops() {
        let ops = vec![payment_op(TEST_PK)];
        assert!(!needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_invoke_contract_ops() {
        let ops = vec![OperationSpec::InvokeContract {
            contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                .to_string(),
            function_name: "transfer".to_string(),
            args: vec![],
            auth: None,
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_upload_wasm_ops() {
        let ops = vec![OperationSpec::UploadWasm {
            wasm: WasmSource::Hex {
                hex: "deadbeef".to_string(),
            },
            auth: None,
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_create_contract_ops() {
        let ops = vec![OperationSpec::CreateContract {
            source: ContractSource::Address {
                address: TEST_PK.to_string(),
            },
            wasm_hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            salt: None,
            constructor_args: None,
            auth: None,
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_true_for_single_invoke_host_function() {
        let ops = vec![OperationSpec::InvokeContract {
            contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                .to_string(),
            function_name: "transfer".to_string(),
            args: vec![],
            auth: Some(AuthSpec::SourceAccount),
        }];
        assert!(needs_simulation(&ops));
    }

    #[test]
    fn returns_false_for_multiple_payment_ops() {
        let ops = vec![payment_op(TEST_PK), payment_op(TEST_PK)];
        assert!(!needs_simulation(&ops));
    }

    mod next_sequence_u64_tests {
        use super::*;

        #[test]
        fn test_increment() {
            assert_eq!(next_sequence_u64(0).unwrap(), 1);

            assert_eq!(next_sequence_u64(12345).unwrap(), 12346);
        }

        #[test]
        fn test_error_path_overflow_i64_max() {
            let result = next_sequence_u64(i64::MAX);
            assert!(result.is_err());
            match result.unwrap_err() {
                RelayerError::ProviderError(msg) => assert_eq!(msg, "sequence overflow"),
                _ => panic!("Unexpected error type"),
            }
        }
    }

    mod i64_from_u64_tests {
        use super::*;

        #[test]
        fn test_happy_path_conversion() {
            assert_eq!(i64_from_u64(0).unwrap(), 0);
            assert_eq!(i64_from_u64(12345).unwrap(), 12345);
            assert_eq!(i64_from_u64(i64::MAX as u64).unwrap(), i64::MAX);
        }

        #[test]
        fn test_error_path_overflow_u64_max() {
            let result = i64_from_u64(u64::MAX);
            assert!(result.is_err());
            match result.unwrap_err() {
                RelayerError::ProviderError(msg) => assert_eq!(msg, "u64→i64 overflow"),
                _ => panic!("Unexpected error type"),
            }
        }

        #[test]
        fn test_edge_case_just_above_i64_max() {
            // Smallest u64 value that will overflow i64
            let value = (i64::MAX as u64) + 1;
            let result = i64_from_u64(value);
            assert!(result.is_err());
            match result.unwrap_err() {
                RelayerError::ProviderError(msg) => assert_eq!(msg, "u64→i64 overflow"),
                _ => panic!("Unexpected error type"),
            }
        }
    }

    mod is_bad_sequence_error_tests {
        use super::*;

        #[test]
        fn test_detects_txbadseq() {
            assert!(is_bad_sequence_error(
                "Failed to send transaction: transaction submission failed: TxBadSeq"
            ));
            assert!(is_bad_sequence_error("Error: TxBadSeq"));
            assert!(is_bad_sequence_error("txbadseq"));
            assert!(is_bad_sequence_error("TXBADSEQ"));
        }

        #[test]
        fn test_returns_false_for_other_errors() {
            assert!(!is_bad_sequence_error("network timeout"));
            assert!(!is_bad_sequence_error("insufficient balance"));
            assert!(!is_bad_sequence_error("tx_insufficient_fee"));
            assert!(!is_bad_sequence_error("bad_auth"));
            assert!(!is_bad_sequence_error(""));
        }
    }

    mod status_check_utils_tests {
        use crate::models::{
            NetworkTransactionData, StellarTransactionData, TransactionError, TransactionInput,
            TransactionRepoModel,
        };
        use crate::utils::mocks::mockutils::create_mock_transaction;
        use chrono::{Duration, Utc};

        /// Helper to create a test transaction with a specific created_at timestamp
        fn create_test_tx_with_age(seconds_ago: i64) -> TransactionRepoModel {
            let created_at = (Utc::now() - Duration::seconds(seconds_ago)).to_rfc3339();
            let mut tx = create_mock_transaction();
            tx.id = format!("test-tx-{}", seconds_ago);
            tx.created_at = created_at;
            tx.network_data = NetworkTransactionData::Stellar(StellarTransactionData {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                fee: None,
                sequence_number: None,
                memo: None,
                valid_until: None,
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                signatures: vec![],
                hash: Some("test-hash-12345".to_string()),
                simulation_transaction_data: None,
                transaction_input: TransactionInput::Operations(vec![]),
                signed_envelope_xdr: None,
            });
            tx
        }

        mod get_age_since_created_tests {
            use crate::domain::transaction::util::get_age_since_created;

            use super::*;

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
            }
        }
    }

    #[test]
    fn test_create_signature_payload_functions() {
        use xdr::{
            Hash, SequenceNumber, TransactionEnvelope, TransactionV0, TransactionV0Envelope,
            Uint256,
        };

        // Test create_transaction_signature_payload
        let transaction = xdr::Transaction {
            source_account: xdr::MuxedAccount::Ed25519(Uint256([1u8; 32])),
            fee: 100,
            seq_num: SequenceNumber(123),
            cond: xdr::Preconditions::None,
            memo: xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: xdr::TransactionExt::V0,
        };
        let network_id = Hash([2u8; 32]);

        let payload = create_transaction_signature_payload(&transaction, &network_id);
        assert_eq!(payload.network_id, network_id);

        // Test create_signature_payload with V0 envelope
        let v0_tx = TransactionV0 {
            source_account_ed25519: Uint256([1u8; 32]),
            fee: 100,
            seq_num: SequenceNumber(123),
            time_bounds: None,
            memo: xdr::Memo::None,
            operations: vec![].try_into().unwrap(),
            ext: xdr::TransactionV0Ext::V0,
        };
        let v0_envelope = TransactionEnvelope::TxV0(TransactionV0Envelope {
            tx: v0_tx,
            signatures: vec![].try_into().unwrap(),
        });

        let v0_payload = create_signature_payload(&v0_envelope, &network_id).unwrap();
        assert_eq!(v0_payload.network_id, network_id);
    }

    mod convert_v0_to_v1_transaction_tests {
        use super::*;
        use xdr::{SequenceNumber, TransactionV0, Uint256};

        #[test]
        fn test_convert_v0_to_v1_transaction() {
            // Create a simple V0 transaction
            let v0_tx = TransactionV0 {
                source_account_ed25519: Uint256([1u8; 32]),
                fee: 100,
                seq_num: SequenceNumber(123),
                time_bounds: None,
                memo: xdr::Memo::None,
                operations: vec![].try_into().unwrap(),
                ext: xdr::TransactionV0Ext::V0,
            };

            // Convert to V1
            let v1_tx = convert_v0_to_v1_transaction(&v0_tx);

            // Check that conversion worked correctly
            assert_eq!(v1_tx.fee, v0_tx.fee);
            assert_eq!(v1_tx.seq_num, v0_tx.seq_num);
            assert_eq!(v1_tx.memo, v0_tx.memo);
            assert_eq!(v1_tx.operations, v0_tx.operations);
            assert!(matches!(v1_tx.ext, xdr::TransactionExt::V0));
            assert!(matches!(v1_tx.cond, xdr::Preconditions::None));

            // Check source account conversion
            match v1_tx.source_account {
                xdr::MuxedAccount::Ed25519(addr) => {
                    assert_eq!(addr, v0_tx.source_account_ed25519);
                }
                _ => panic!("Expected Ed25519 muxed account"),
            }
        }

        #[test]
        fn test_convert_v0_to_v1_transaction_with_time_bounds() {
            // Create a V0 transaction with time bounds
            let time_bounds = xdr::TimeBounds {
                min_time: xdr::TimePoint(100),
                max_time: xdr::TimePoint(200),
            };

            let v0_tx = TransactionV0 {
                source_account_ed25519: Uint256([2u8; 32]),
                fee: 200,
                seq_num: SequenceNumber(456),
                time_bounds: Some(time_bounds.clone()),
                memo: xdr::Memo::Text("test".try_into().unwrap()),
                operations: vec![].try_into().unwrap(),
                ext: xdr::TransactionV0Ext::V0,
            };

            // Convert to V1
            let v1_tx = convert_v0_to_v1_transaction(&v0_tx);

            // Check that time bounds were correctly converted to preconditions
            match v1_tx.cond {
                xdr::Preconditions::Time(tb) => {
                    assert_eq!(tb, time_bounds);
                }
                _ => panic!("Expected Time preconditions"),
            }
        }
    }
}
