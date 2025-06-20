//! Utility functions for Stellar transaction domain logic.
use crate::models::OperationSpec;
use crate::models::RelayerError;

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
}
