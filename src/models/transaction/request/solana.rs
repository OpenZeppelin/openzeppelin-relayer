use crate::models::EncodedSerializedTransaction;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SolanaTransactionRequest {
    pub transaction: EncodedSerializedTransaction,
}
