use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::models::transaction::stellar_types::{MemoSpec, OperationSpec};

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
