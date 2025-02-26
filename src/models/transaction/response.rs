use crate::{
    models::{NetworkTransactionData, TransactionRepoModel, TransactionStatus, U256},
    utils::{
        deserialize_optional_u128, deserialize_optional_u64, deserialize_u128, deserialize_u64,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
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
    #[serde(deserialize_with = "deserialize_optional_u128", default)]
    pub gas_price: Option<u128>,
    #[serde(deserialize_with = "deserialize_u64")]
    pub gas_limit: u64,
    #[serde(deserialize_with = "deserialize_optional_u64", default)]
    pub nonce: Option<u64>,
    pub value: U256,
    pub from: String,
    pub to: Option<String>,
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
    #[serde(deserialize_with = "deserialize_u128")]
    pub fee: u128,
    pub sequence_number: u64,
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
                    recent_blockhash: solana_data.recent_blockhash.unwrap_or_default(),
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
