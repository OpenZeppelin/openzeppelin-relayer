use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SolanaTransactionStatus {
    Processed,
    Confirmed,
    Finalized,
    Failed,
}
