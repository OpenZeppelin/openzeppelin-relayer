use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::models::transaction::stellar::{MemoSpec, OperationSpec};

#[derive(Deserialize, Serialize, ToSchema)]
pub struct StellarTransactionRequest {
    #[schema(nullable = true)]
    pub source_account: Option<String>,
    pub network: String,
    #[schema(max_length = 100, nullable = true)]
    pub operations: Option<Vec<OperationSpec>>,
    #[schema(nullable = true)]
    pub memo: Option<MemoSpec>,
    #[schema(nullable = true)]
    pub valid_until: Option<String>,
    /// Pre-built transaction XDR (base64 encoded, signed or unsigned)
    /// Mutually exclusive with operations field
    #[schema(nullable = true)]
    pub transaction_xdr: Option<String>,
    /// Explicitly request fee-bump wrapper
    /// Only valid when transaction_xdr contains a signed transaction
    #[schema(nullable = true)]
    pub fee_bump: Option<bool>,
    /// Maximum fee in stroops (defaults to 0.1 XLM = 1,000,000 stroops)
    #[schema(nullable = true)]
    pub max_fee: Option<i64>,
}

impl StellarTransactionRequest {
    /// Validate the transaction request according to the rules:
    /// - Only one input type allowed (operations XOR transaction_xdr)
    /// - If fee_bump is true, transaction_xdr must be provided
    /// - Operations mode cannot use fee_bump
    pub fn validate(&self) -> Result<(), crate::models::ApiError> {
        use crate::models::ApiError;

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
                    "Cannot provide both operations and transaction_xdr".to_string(),
                ));
            }
            (false, false) => {
                return Err(ApiError::BadRequest(
                    "Must provide either operations or transaction_xdr".to_string(),
                ));
            }
            _ => {}
        }

        // Validate fee_bump flag usage
        if self.fee_bump == Some(true) && has_operations {
            return Err(ApiError::BadRequest(
                "Cannot request fee_bump with operations mode".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_serde_operations_mode() {
        let json = r#"{
            "source_account": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "network": "testnet",
            "operations": []
        }"#;

        let req: StellarTransactionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req.source_account,
            Some("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string())
        );
        assert_eq!(req.operations.as_ref().map(|ops| ops.len()), Some(0));
        assert_eq!(req.network, "testnet");
    }

    #[test]
    fn test_validate_operations_and_xdr() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: Some(vec![OperationSpec::Payment {
                destination: "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string(),
                amount: 1000000,
                asset: crate::models::transaction::stellar::AssetSpec::Native,
            }]),
            memo: None,
            valid_until: None,
            transaction_xdr: Some("AAAAA...".to_string()),
            fee_bump: None,
            max_fee: None,
        };

        let result = req.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot provide both"));
    }

    #[test]
    fn test_validate_neither_operations_nor_xdr() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: Some(vec![]),
            memo: None,
            valid_until: None,
            transaction_xdr: None,
            fee_bump: None,
            max_fee: None,
        };

        let result = req.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Must provide either"));
    }

    #[test]
    fn test_validate_fee_bump_with_operations() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: Some(vec![OperationSpec::Payment {
                destination: "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string(),
                amount: 1000000,
                asset: crate::models::transaction::stellar::AssetSpec::Native,
            }]),
            memo: None,
            valid_until: None,
            transaction_xdr: None,
            fee_bump: Some(true),
            max_fee: None,
        };

        let result = req.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot request fee_bump with operations"));
    }

    #[test]
    fn test_validate_fee_bump_with_xdr() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: None,
            memo: None,
            valid_until: None,
            transaction_xdr: Some("AAAAA...".to_string()),
            fee_bump: Some(true),
            max_fee: Some(10000000),
        };

        let result = req.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_valid_operations_mode() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: Some(vec![OperationSpec::Payment {
                destination: "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string(),
                amount: 1000000,
                asset: crate::models::transaction::stellar::AssetSpec::Native,
            }]),
            memo: None,
            valid_until: None,
            transaction_xdr: None,
            fee_bump: None,
            max_fee: None,
        };

        let result = req.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_valid_xdr_mode() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: None,
            memo: None,
            valid_until: None,
            transaction_xdr: Some("AAAAA...".to_string()),
            fee_bump: None,
            max_fee: None,
        };

        let result = req.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_default_structure() {
        let req = StellarTransactionRequest {
            source_account: Some(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ),
            network: "testnet".to_string(),
            operations: Some(vec![]),
            memo: None,
            valid_until: None,
            transaction_xdr: None,
            fee_bump: None,
            max_fee: None,
        };

        assert_eq!(
            req.source_account,
            Some("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string())
        );
        assert_eq!(req.operations.as_ref().map(|ops| ops.len()), Some(0));
        assert_eq!(req.network, "testnet");
        assert!(req.memo.is_none());
        assert!(req.valid_until.is_none());
        assert!(req.transaction_xdr.is_none());
        assert!(req.fee_bump.is_none());
        assert!(req.max_fee.is_none());
    }

    #[test]
    fn test_xdr_mode() {
        let json = r#"{
            "source_account": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "network": "testnet",
            "operations": [],
            "transaction_xdr": "AAAAAgAAAABjc+mbXCnvmVk4lxqVl7s0LAz5slXqmkHBg8PpH7p3DgAAAGQABpK0AAAACQAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAGN0qQBW8x3mfbwGGYndt2uq4O4sZPUrDx5HlwuQke9zAAAAAAAAAAAAAA9CAAAAAA==",
            "fee_bump": true,
            "max_fee": 10000000
        }"#;

        let req: StellarTransactionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req.source_account,
            Some("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string())
        );
        assert!(req.transaction_xdr.is_some());
        assert_eq!(req.fee_bump, Some(true));
        assert_eq!(req.max_fee, Some(10000000));
        assert_eq!(
            req.operations.as_ref().map(|ops| ops.is_empty()),
            Some(true)
        );
    }

    #[test]
    fn test_operations_with_fee_bump_is_invalid() {
        // This test documents that operations and fee_bump together should be invalid
        // The actual validation will happen in the request processing logic
        let json = r#"{
            "source_account": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "network": "testnet",
            "operations": [],
            "fee_bump": true
        }"#;

        // This should parse successfully (validation happens later)
        let req: StellarTransactionRequest = serde_json::from_str(json).unwrap();
        assert!(req.fee_bump == Some(true));
        assert_eq!(
            req.operations.as_ref().map(|ops| ops.is_empty()),
            Some(true)
        );
    }

    #[test]
    fn test_xdr_mode_without_operations_field() {
        // Test that we can deserialize without operations field
        let json = r#"{
            "network": "testnet",
            "fee": 1,
            "transaction_xdr": "AAAAAgAAAACige4lTdwSB/sto4SniEdJ2kOa2X65s5bqkd40J4DjSwAAAAEAAHAkAAAADwAAAAAAAAAAAAAAAQAAAAAAAAABAAAAAKKB7iVN3BIH+y2jhKeIR0naQ5rZfrmzluqR3jQngONLAAAAAAAAAAAAD0JAAAAAAAAAAAA="
        }"#;

        let req: StellarTransactionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.network, "testnet");
        assert!(req.transaction_xdr.is_some());
        assert!(req.operations.is_none());

        // Validate should pass
        assert!(req.validate().is_ok());
    }
}
