use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct StellarTransactionRequest {
    pub source_account: String,
    pub destination_account: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub memo: Option<String>,
    pub fee: u128,
    pub sequence_number: String,
}
