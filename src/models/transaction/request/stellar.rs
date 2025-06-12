use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::models::transaction::stellar::{MemoSpec, OperationSpec};

#[derive(Deserialize, Serialize, ToSchema)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub network: String,
    #[schema(max_length = 100)]
    pub operations: Vec<OperationSpec>,
    #[schema(nullable = true)]
    pub memo: Option<MemoSpec>,
    #[schema(nullable = true)]
    pub valid_until: Option<String>,
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
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
        assert_eq!(req.operations.len(), 0);
        assert_eq!(req.network, "testnet");
    }

    #[test]
    fn test_default_structure() {
        let req = StellarTransactionRequest {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network: "testnet".to_string(),
            operations: vec![],
            memo: None,
            valid_until: None,
        };

        assert_eq!(
            req.source_account,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
        assert_eq!(req.operations.len(), 0);
        assert_eq!(req.network, "testnet");
        assert!(req.memo.is_none());
        assert!(req.valid_until.is_none());
    }
}
