use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct SolanaTransactionRequest {
    pub fee_payer: String,
    pub instructions: Vec<String>,
}
