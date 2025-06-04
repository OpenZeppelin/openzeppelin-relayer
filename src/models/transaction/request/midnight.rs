use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// TODO: Implement MidnightTransactionRequest
#[derive(Deserialize, Serialize, ToSchema)]
pub struct MidnightTransactionRequest {}
