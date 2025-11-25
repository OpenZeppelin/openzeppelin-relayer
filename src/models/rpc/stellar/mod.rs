use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::domain::transaction::stellar::StellarTransactionValidator;
use crate::models::ApiError;
use crate::{
    domain::stellar::validation::validate_operations, models::transaction::stellar::OperationSpec,
};
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
#[schema(as = StellarFeeEstimateRequestParams)]
pub struct FeeEstimateRequestParams {
    /// Pre-built transaction XDR (base64 encoded, signed or unsigned)
    /// Mutually exclusive with operations field
    #[schema(nullable = true)]
    pub transaction_xdr: Option<String>,
    /// Source account address (required when operations are provided)
    /// For sponsored transactions, this should be the user's account address
    #[schema(nullable = true)]
    pub source_account: Option<String>,
    /// Operations array to build transaction from
    /// Mutually exclusive with transaction_xdr field
    #[schema(nullable = true)]
    pub operations: Option<Vec<OperationSpec>>,
    /// Asset identifier for fee token (e.g., "native" or "USDC:GA5Z...")
    pub fee_token: String,
}

impl FeeEstimateRequestParams {
    /// Validate the fee estimate request according to the rules:
    /// - Only one input type allowed (operations XOR transaction_xdr)
    /// - fee_token must be in valid format
    pub fn validate(&self) -> Result<(), crate::models::ApiError> {
        // Validate fee_token structure
        StellarTransactionValidator::validate_fee_token_structure(&self.fee_token)
            .map_err(|e| ApiError::BadRequest(format!("Invalid fee_token structure: {e}")))?;

        // Check that exactly one input type is provided
        let has_operations = self
            .operations
            .as_ref()
            .map(|ops| !ops.is_empty())
            .unwrap_or(false);
        let has_xdr = self.transaction_xdr.is_some();

        if has_operations {
            validate_operations(self.operations.as_ref().unwrap())
                .map_err(|e| ApiError::BadRequest(format!("Invalid operations: {e}")))?;
            if self.source_account.is_none() || self.source_account.as_ref().unwrap().is_empty() {
                return Err(ApiError::BadRequest(
                    "source_account is required when providing operations".to_string(),
                ));
            }
        }

        match (has_operations, has_xdr) {
            (true, true) => {
                return Err(ApiError::BadRequest(
                    "Cannot provide both transaction_xdr and operations".to_string(),
                ));
            }
            (false, false) => {
                return Err(ApiError::BadRequest(
                    "Must provide either transaction_xdr or operations".to_string(),
                ));
            }
            _ => {}
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, ToSchema)]
#[schema(as = StellarFeeEstimateResult)]
pub struct FeeEstimateResult {
    /// Estimated fee in token amount (decimal UI representation as string)
    pub fee_in_token_ui: String,
    /// Estimated fee in token amount (raw units as string)
    pub fee_in_token: String,
    /// Conversion rate from XLM to token (as string)
    pub conversion_rate: String,
}

// prepareTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
#[derive(Clone)]
#[schema(as = StellarPrepareTransactionRequestParams)]
pub struct PrepareTransactionRequestParams {
    /// Pre-built transaction XDR (base64 encoded, signed or unsigned)
    /// Mutually exclusive with operations field
    #[schema(nullable = true)]
    pub transaction_xdr: Option<String>,
    /// Operations array to build transaction from
    /// Mutually exclusive with transaction_xdr field
    #[schema(nullable = true)]
    pub operations: Option<Vec<OperationSpec>>,
    /// Source account address (required when operations are provided)
    /// For gasless transactions, this should be the user's account address
    #[schema(nullable = true)]
    pub source_account: Option<String>,
    /// Asset identifier for fee token
    pub fee_token: String,
}

impl PrepareTransactionRequestParams {
    /// Validate the prepare transaction request according to the rules:
    /// - Only one input type allowed (operations XOR transaction_xdr)
    /// - fee_token must be in valid format
    /// - source_account is required when operations are provided
    pub fn validate(&self) -> Result<(), crate::models::ApiError> {
        // Validate fee_token structure
        StellarTransactionValidator::validate_fee_token_structure(&self.fee_token)
            .map_err(|e| ApiError::BadRequest(format!("Invalid fee_token structure: {e}")))?;

        // Check that exactly one input type is provided
        let has_operations = self
            .operations
            .as_ref()
            .map(|ops| !ops.is_empty())
            .unwrap_or(false);
        let has_xdr = self.transaction_xdr.is_some();

        match (has_operations, has_xdr) {
            (true, true) => {
                return Err(ApiError::BadRequest(
                    "Cannot provide both transaction_xdr and operations".to_string(),
                ));
            }
            (false, false) => {
                return Err(ApiError::BadRequest(
                    "Must provide either transaction_xdr or operations".to_string(),
                ));
            }
            _ => {}
        }

        // Validate source_account is provided when operations are used
        if has_operations {
            validate_operations(self.operations.as_ref().unwrap())
                .map_err(|e| ApiError::BadRequest(format!("Invalid operations: {e}")))?;
            if self.source_account.is_none() || self.source_account.as_ref().unwrap().is_empty() {
                return Err(ApiError::BadRequest(
                    "source_account is required when providing operations".to_string(),
                ));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, ToSchema)]
#[schema(as = StellarPrepareTransactionResult)]
pub struct PrepareTransactionResult {
    /// Extended transaction XDR (base64 encoded)
    pub transaction: String,
    /// Fee amount in token (raw units as string)
    pub fee_in_token: String,
    /// Fee amount in token (decimal UI representation as string)
    pub fee_in_token_ui: String,
    /// Fee amount in stroops (as string)
    pub fee_in_stroops: String,
    /// Asset identifier for fee token
    pub fee_token: String,
    /// Transaction validity timestamp (ISO 8601 format)
    pub valid_until: String,
}

/// Stellar RPC method enum
pub enum StellarRpcMethod {
    Generic(String),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq, Clone)]
#[serde(untagged)]
#[schema(as = StellarRpcRequest)]
pub enum StellarRpcRequest {
    #[serde(rename = "rawRpcRequest")]
    #[schema(example = "rawRpcRequest")]
    RawRpcRequest {
        method: String,
        params: serde_json::Value,
    },
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(untagged)]
pub enum StellarRpcResult {
    /// Raw JSON-RPC response value. Covers string or structured JSON values.
    RawRpcResult(serde_json::Value),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::transaction::stellar::{asset::AssetSpec, OperationSpec};

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const VALID_FEE_TOKEN_NATIVE: &str = "native";
    const VALID_FEE_TOKEN_USDC: &str =
        "USDC:GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5";
    const INVALID_FEE_TOKEN: &str = "invalid-token";

    // FeeEstimateRequestParams tests

    #[test]
    fn test_fee_estimate_validate_with_xdr_success() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: None,
            source_account: None,
            fee_token: VALID_FEE_TOKEN_USDC.to_string(),
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_fee_estimate_validate_with_operations_success() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: None,
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: Some(TEST_PK.to_string()),
            fee_token: VALID_FEE_TOKEN_USDC.to_string(),
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_fee_estimate_validate_with_usdc_token_success() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: None,
            source_account: None,
            fee_token: VALID_FEE_TOKEN_USDC.to_string(),
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_fee_estimate_validate_invalid_fee_token() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: None,
            source_account: None,
            fee_token: INVALID_FEE_TOKEN.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid fee_token structure"));
        } else {
            panic!("Expected BadRequest error for invalid fee_token");
        }
    }

    #[test]
    fn test_fee_estimate_validate_both_xdr_and_operations() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: Some(TEST_PK.to_string()),
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Cannot provide both transaction_xdr and operations"));
        } else {
            panic!("Expected BadRequest error for both xdr and operations");
        }
    }

    #[test]
    fn test_fee_estimate_validate_neither_xdr_nor_operations() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: None,
            operations: None,
            source_account: None,
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Must provide either transaction_xdr or operations"));
        } else {
            panic!("Expected BadRequest error for missing both xdr and operations");
        }
    }

    #[test]
    fn test_fee_estimate_validate_operations_without_source_account() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: None,
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: None,
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("source_account is required when providing operations"));
        } else {
            panic!("Expected BadRequest error for missing source_account");
        }
    }

    #[test]
    fn test_fee_estimate_validate_operations_with_empty_source_account() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: None,
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: Some("".to_string()),
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("source_account is required when providing operations"));
        } else {
            panic!("Expected BadRequest error for empty source_account");
        }
    }

    #[test]
    fn test_fee_estimate_validate_empty_operations() {
        let params = FeeEstimateRequestParams {
            transaction_xdr: None,
            operations: Some(vec![]),
            source_account: Some(TEST_PK.to_string()),
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Must provide either transaction_xdr or operations"));
        } else {
            panic!("Expected BadRequest error for empty operations");
        }
    }

    // PrepareTransactionRequestParams tests

    #[test]
    fn test_prepare_transaction_validate_with_xdr_success() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: None,
            source_account: None,
            fee_token: VALID_FEE_TOKEN_USDC.to_string(),
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_prepare_transaction_validate_with_operations_success() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: None,
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: Some(TEST_PK.to_string()),
            fee_token: VALID_FEE_TOKEN_USDC.to_string(),
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_prepare_transaction_validate_with_usdc_token_success() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: None,
            source_account: None,
            fee_token: VALID_FEE_TOKEN_USDC.to_string(),
        };
        assert!(params.validate().is_ok());
    }

    #[test]
    fn test_prepare_transaction_validate_invalid_fee_token() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: None,
            source_account: None,
            fee_token: INVALID_FEE_TOKEN.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Invalid fee_token structure"));
        } else {
            panic!("Expected BadRequest error for invalid fee_token");
        }
    }

    #[test]
    fn test_prepare_transaction_validate_both_xdr_and_operations() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: Some("AAAAAgAAAAA=".to_string()),
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: Some(TEST_PK.to_string()),
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Cannot provide both transaction_xdr and operations"));
        } else {
            panic!("Expected BadRequest error for both xdr and operations");
        }
    }

    #[test]
    fn test_prepare_transaction_validate_neither_xdr_nor_operations() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: None,
            operations: None,
            source_account: None,
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Must provide either transaction_xdr or operations"));
        } else {
            panic!("Expected BadRequest error for missing both xdr and operations");
        }
    }

    #[test]
    fn test_prepare_transaction_validate_operations_without_source_account() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: None,
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: None,
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("source_account is required when providing operations"));
        } else {
            panic!("Expected BadRequest error for missing source_account");
        }
    }

    #[test]
    fn test_prepare_transaction_validate_operations_with_empty_source_account() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: None,
            operations: Some(vec![OperationSpec::Payment {
                destination: TEST_PK.to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }]),
            source_account: Some("".to_string()),
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("source_account is required when providing operations"));
        } else {
            panic!("Expected BadRequest error for empty source_account");
        }
    }

    #[test]
    fn test_prepare_transaction_validate_empty_operations() {
        let params = PrepareTransactionRequestParams {
            transaction_xdr: None,
            operations: Some(vec![]),
            source_account: Some(TEST_PK.to_string()),
            fee_token: VALID_FEE_TOKEN_NATIVE.to_string(),
        };
        let result = params.validate();
        assert!(result.is_err());
        // Empty operations array is treated as "no operations provided"
        // so it falls into the "neither xdr nor operations" case
        if let Err(ApiError::BadRequest(msg)) = result {
            assert!(msg.contains("Must provide either transaction_xdr or operations"));
        } else {
            panic!("Expected BadRequest error for empty operations");
        }
    }
}
