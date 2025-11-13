//! Validation logic for Stellar transactions
//!
//! This module focuses on business logic validations that aren't
//! already handled by XDR parsing or the type system.

use crate::domain::relayer::xdr_utils::{extract_operations, muxed_account_to_string};
use crate::models::RelayerStellarPolicy;
use crate::models::{MemoSpec, OperationSpec, StellarValidationError, TransactionError};
use soroban_rs::xdr::{Asset, OperationBody, PaymentOp, TransactionEnvelope};
use stellar_strkey::ed25519::PublicKey;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StellarTransactionValidationError {
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
    #[error("Invalid asset identifier: {0}")]
    InvalidAssetIdentifier(String),
    #[error("Token not allowed: {0}")]
    TokenNotAllowed(String),
    #[error("Insufficient token payment: expected {0}, got {1}")]
    InsufficientTokenPayment(u64, u64),
    #[error("Max fee exceeded: {0}")]
    MaxFeeExceeded(u64),
}

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

/// Validator for Stellar transactions and policies
pub struct StellarTransactionValidator;

impl StellarTransactionValidator {
    /// Validate that an asset identifier is in the allowed tokens list
    pub fn validate_allowed_token(
        asset: &str,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        let allowed_tokens = policy.get_allowed_tokens();

        if allowed_tokens.is_empty() {
            // If no allowed tokens specified, all tokens are allowed
            return Ok(());
        }

        // Check if native XLM is allowed
        if asset == "native" || asset.is_empty() {
            let native_allowed = allowed_tokens
                .iter()
                .any(|token| token.asset == "native" || token.asset.is_empty());
            if !native_allowed {
                return Err(StellarTransactionValidationError::TokenNotAllowed(
                    "Native XLM not in allowed tokens list".to_string(),
                ));
            }
            return Ok(());
        }

        // Check if the asset is in the allowed list
        let is_allowed = allowed_tokens.iter().any(|token| token.asset == asset);

        if !is_allowed {
            return Err(StellarTransactionValidationError::TokenNotAllowed(format!(
                "Token {} not in allowed tokens list",
                asset
            )));
        }

        Ok(())
    }

    /// Validate that a fee amount doesn't exceed the maximum allowed fee
    pub fn validate_max_fee(
        fee: u64,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        if let Some(max_fee) = policy.max_fee {
            if fee > max_fee as u64 {
                return Err(StellarTransactionValidationError::MaxFeeExceeded(fee));
            }
        }

        Ok(())
    }

    /// Validate that a specific token's max_allowed_fee is not exceeded
    pub fn validate_token_max_fee(
        asset_id: &str,
        fee: u64,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        if let Some(token_entry) = policy.get_allowed_token_entry(asset_id) {
            if let Some(max_allowed_fee) = token_entry.max_allowed_fee {
                if fee > max_allowed_fee {
                    return Err(StellarTransactionValidationError::MaxFeeExceeded(fee));
                }
            }
        }

        Ok(())
    }

    /// Extract payment operations from a transaction envelope that pay to the relayer
    ///
    /// Returns a vector of (asset_id, amount) tuples for payments to the relayer
    pub fn extract_relayer_payments(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
    ) -> Result<Vec<(String, u64)>, StellarTransactionValidationError> {
        let operations = extract_operations(envelope).map_err(|e| {
            StellarTransactionValidationError::ValidationError(format!(
                "Failed to extract operations: {}",
                e
            ))
        })?;

        let mut payments = Vec::new();

        for op in operations.iter() {
            if let OperationBody::Payment(PaymentOp {
                destination,
                asset,
                amount,
            }) = &op.body
            {
                // Convert destination to string
                let dest_str = muxed_account_to_string(destination).map_err(|e| {
                    StellarTransactionValidationError::ValidationError(format!(
                        "Failed to parse destination: {}",
                        e
                    ))
                })?;

                // Check if payment is to relayer
                if dest_str == relayer_address {
                    // Convert asset to identifier string
                    let asset_id = asset_to_asset_id(asset)?;
                    // Convert amount from i64 to u64 (amounts are always positive)
                    let amount_u64 = (*amount) as u64;
                    payments.push((asset_id, amount_u64));
                }
            }
        }

        Ok(payments)
    }

    /// Validate token payment in transaction
    ///
    /// Checks that:
    /// 1. Payment operation to relayer exists
    /// 2. Token is in allowed_tokens list
    /// 3. Payment amount matches expected fee (within tolerance)
    pub fn validate_token_payment(
        envelope: &TransactionEnvelope,
        relayer_address: &str,
        expected_fee_token: &str,
        expected_fee_amount: u64,
        policy: &RelayerStellarPolicy,
    ) -> Result<(), StellarTransactionValidationError> {
        // Extract payments to relayer
        let payments = Self::extract_relayer_payments(envelope, relayer_address)?;

        if payments.is_empty() {
            return Err(StellarTransactionValidationError::ValidationError(
                "No payment operation found to relayer".to_string(),
            ));
        }

        // Find payment matching the expected token
        let matching_payment = payments
            .iter()
            .find(|(asset_id, _)| asset_id == expected_fee_token);

        match matching_payment {
            Some((asset_id, amount)) => {
                // Validate token is allowed
                Self::validate_allowed_token(asset_id, policy)?;

                // Validate amount matches expected (allow 1% tolerance for rounding)
                let tolerance = (expected_fee_amount as f64 * 0.01) as u64;
                if *amount < expected_fee_amount.saturating_sub(tolerance) {
                    return Err(StellarTransactionValidationError::InsufficientTokenPayment(
                        expected_fee_amount,
                        *amount,
                    ));
                }

                // Validate max fee
                Self::validate_token_max_fee(asset_id, *amount, policy)?;

                Ok(())
            }
            None => Err(StellarTransactionValidationError::ValidationError(format!(
                "No payment found for expected token: {}. Found payments: {:?}",
                expected_fee_token, payments
            ))),
        }
    }
}

/// Convert XDR Asset to asset identifier string
fn asset_to_asset_id(asset: &Asset) -> Result<String, StellarTransactionValidationError> {
    match asset {
        Asset::Native => Ok("native".to_string()),
        Asset::CreditAlphanum4(alpha4) => {
            // Extract code (trim null bytes)
            let code_bytes = alpha4.asset_code.0;
            let code_len = code_bytes.iter().position(|&b| b == 0).unwrap_or(4);
            let code = String::from_utf8(code_bytes[..code_len].to_vec()).map_err(|e| {
                StellarTransactionValidationError::InvalidAssetIdentifier(format!(
                    "Invalid asset code: {}",
                    e
                ))
            })?;

            // Extract issuer
            let issuer = match &alpha4.issuer.0 {
                soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(uint256) => {
                    let bytes: [u8; 32] = uint256.0;
                    let pk = PublicKey(bytes);
                    pk.to_string()
                }
            };

            Ok(format!("{}:{}", code, issuer))
        }
        Asset::CreditAlphanum12(alpha12) => {
            // Extract code (trim null bytes)
            let code_bytes = alpha12.asset_code.0;
            let code_len = code_bytes.iter().position(|&b| b == 0).unwrap_or(12);
            let code = String::from_utf8(code_bytes[..code_len].to_vec()).map_err(|e| {
                StellarTransactionValidationError::InvalidAssetIdentifier(format!(
                    "Invalid asset code: {}",
                    e
                ))
            })?;

            // Extract issuer
            let issuer = match &alpha12.issuer.0 {
                soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(uint256) => {
                    let bytes: [u8; 32] = uint256.0;
                    let pk = PublicKey(bytes);
                    pk.to_string()
                }
            };

            Ok(format!("{}:{}", code, issuer))
        }
    }
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
