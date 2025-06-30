//! Validation logic for Stellar transactions
//!
//! This module focuses on business logic validations that aren't
//! already handled by XDR parsing or the type system.

use crate::models::{MemoSpec, OperationSpec, StellarValidationError, TransactionError};

/// Validate operations for business rules
pub fn validate_operations(ops: &[OperationSpec]) -> Result<(), TransactionError> {
    // Basic sanity checks
    if ops.is_empty() {
        return Err(StellarValidationError::EmptyOperations.into());
    }

    if ops.len() > 100 {
        return Err(StellarValidationError::TooManyOperations {
            count: ops.len(),
            max: 100,
        }
        .into());
    }

    // Check Soroban exclusivity - this is a specific business rule
    validate_soroban_exclusivity(ops)?;

    Ok(())
}

/// Validate that Soroban operations are exclusive
fn validate_soroban_exclusivity(ops: &[OperationSpec]) -> Result<(), TransactionError> {
    let soroban_ops = ops.iter().filter(|op| is_soroban_operation(op)).count();

    if soroban_ops > 1 {
        return Err(StellarValidationError::MultipleSorobanOperations.into());
    }

    if soroban_ops == 1 && ops.len() > 1 {
        return Err(StellarValidationError::SorobanNotExclusive.into());
    }

    Ok(())
}

/// Check if an operation is a Soroban operation
fn is_soroban_operation(op: &OperationSpec) -> bool {
    matches!(
        op,
        OperationSpec::InvokeContract { .. }
            | OperationSpec::CreateContract { .. }
            | OperationSpec::UploadWasm { .. }
    )
}

/// Validate that Soroban operations don't have a non-None memo
pub fn validate_soroban_memo_restriction(
    ops: &[OperationSpec],
    memo: &Option<MemoSpec>,
) -> Result<(), TransactionError> {
    let has_soroban = ops.iter().any(is_soroban_operation);

    if has_soroban && memo.is_some() && !matches!(memo, Some(MemoSpec::None)) {
        return Err(StellarValidationError::SorobanWithMemo.into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AssetSpec;

    #[test]
    fn test_empty_operations_rejected() {
        let result = validate_operations(&[]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("at least one operation"));
    }

    #[test]
    fn test_too_many_operations_rejected() {
        let ops = vec![
            OperationSpec::Payment {
                destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            };
            101
        ];
        let result = validate_operations(&ops);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("maximum allowed is 100"));
    }

    #[test]
    fn test_soroban_exclusivity_enforced() {
        // Multiple Soroban operations should fail
        let ops = vec![
            OperationSpec::InvokeContract {
                contract_address: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                    .to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            },
            OperationSpec::CreateContract {
                source: crate::models::ContractSource::Address {
                    address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                },
                wasm_hash: "abc123".to_string(),
                salt: None,
                constructor_args: None,
                auth: None,
            },
        ];
        let result = validate_operations(&ops);
        assert!(result.is_err());

        // Soroban mixed with non-Soroban should fail
        let ops = vec![
            OperationSpec::InvokeContract {
                contract_address: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                    .to_string(),
                function_name: "test".to_string(),
                args: vec![],
                auth: None,
            },
            OperationSpec::Payment {
                destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            },
        ];
        let result = validate_operations(&ops);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Soroban operations must be exclusive"));
    }

    #[test]
    fn test_soroban_memo_restriction() {
        let soroban_op = vec![OperationSpec::InvokeContract {
            contract_address: "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
                .to_string(),
            function_name: "test".to_string(),
            args: vec![],
            auth: None,
        }];

        // Soroban with text memo should fail
        let result = validate_soroban_memo_restriction(
            &soroban_op,
            &Some(MemoSpec::Text {
                value: "test".to_string(),
            }),
        );
        assert!(result.is_err());

        // Soroban with MemoNone should succeed
        let result = validate_soroban_memo_restriction(&soroban_op, &Some(MemoSpec::None));
        assert!(result.is_ok());

        // Soroban with no memo should succeed
        let result = validate_soroban_memo_restriction(&soroban_op, &None);
        assert!(result.is_ok());
    }
}
