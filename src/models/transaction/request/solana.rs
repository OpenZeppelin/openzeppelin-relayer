use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SolanaTransactionRequest {
    pub fee_payer: String,
    pub instructions: Vec<String>,
}
