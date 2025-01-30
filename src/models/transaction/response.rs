use crate::models::{NetworkTransactionData, TransactionRepoModel, TransactionStatus};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum TransactionResponse {
    Evm(EvmTransactionResponse),
    Solana(SolanaTransactionResponse),
    Stellar(StellarTransactionResponse),
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize)]
pub struct EvmTransactionResponse {
    pub id: String,
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub created_at: String,
    pub sent_at: String,
    pub confirmed_at: String,
    pub gas_price: u128,
    pub gas_limit: u128,
    pub nonce: u64,
    pub value: u64,
    pub from: String,
    pub to: String,
    pub relayer_id: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize)]
pub struct SolanaTransactionResponse {
    pub id: String,
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub created_at: String,
    pub sent_at: String,
    pub confirmed_at: String,
    pub recent_blockhash: String,
    pub fee_payer: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize)]
pub struct StellarTransactionResponse {
    pub id: String,
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub created_at: String,
    pub sent_at: String,
    pub confirmed_at: String,
    pub source_account: String,
    pub fee: u128,
    pub sequence_number: u64,
}

impl<'de> Deserialize<'de> for TransactionResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;

        // If it has "gas_price" or "gas_limit", it's likely an EVM transaction
        if value.get("gas_price").is_some()
            || value.get("gas_limit").is_some()
            || value.get("nonce").is_some()
        {
            return serde_json::from_value::<EvmTransactionResponse>(value)
                .map(TransactionResponse::Evm)
                .map_err(serde::de::Error::custom);
        }

        // If it has "recent_blockhash", it's likely a Solana transaction
        if value.get("recent_blockhash").is_some() {
            return serde_json::from_value::<SolanaTransactionResponse>(value)
                .map(TransactionResponse::Solana)
                .map_err(serde::de::Error::custom);
        }

        // If it has "fee" and "sequence_number", it's likely a Stellar transaction
        if value.get("fee").is_some() && value.get("sequence_number").is_some() {
            return serde_json::from_value::<StellarTransactionResponse>(value)
                .map(TransactionResponse::Stellar)
                .map_err(serde::de::Error::custom);
        }

        Err(serde::de::Error::custom("Unknown transaction type"))
    }
}

impl From<TransactionRepoModel> for TransactionResponse {
    fn from(model: TransactionRepoModel) -> Self {
        match model.network_data {
            NetworkTransactionData::Evm(evm_data) => {
                TransactionResponse::Evm(EvmTransactionResponse {
                    id: model.id,
                    hash: evm_data.hash,
                    status: model.status,
                    created_at: model.created_at,
                    sent_at: model.sent_at,
                    confirmed_at: model.confirmed_at,
                    gas_price: evm_data.gas_price,
                    gas_limit: evm_data.gas_limit,
                    nonce: evm_data.nonce,
                    value: evm_data.value,
                    from: evm_data.from,
                    to: evm_data.to,
                    relayer_id: model.relayer_id,
                })
            }
            NetworkTransactionData::Solana(solana_data) => {
                TransactionResponse::Solana(SolanaTransactionResponse {
                    id: model.id,
                    hash: solana_data.hash,
                    status: model.status,
                    created_at: model.created_at,
                    sent_at: model.sent_at,
                    confirmed_at: model.confirmed_at,
                    recent_blockhash: solana_data.recent_blockhash,
                    fee_payer: solana_data.fee_payer,
                })
            }
            NetworkTransactionData::Stellar(stellar_data) => {
                TransactionResponse::Stellar(StellarTransactionResponse {
                    id: model.id,
                    hash: stellar_data.hash,
                    status: model.status,
                    created_at: model.created_at,
                    sent_at: model.sent_at,
                    confirmed_at: model.confirmed_at,
                    source_account: stellar_data.source_account,
                    fee: stellar_data.fee,
                    sequence_number: stellar_data.sequence_number,
                })
            }
        }
    }
}
