use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub destination_account: String,
    pub amount: String,
    pub asset_code: String,
    #[schema(nullable = false)]
    pub asset_issuer: Option<String>,
    #[schema(nullable = false)]
    pub memo: Option<String>,
    pub fee: u128,
    pub sequence_number: String,
}
