use crate::models::{NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Sent,
    Submitted,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransactionRepoModel {
    pub id: String,
    pub relayer_id: String,
    pub hash: String,
    pub status: TransactionStatus,
    pub created_at: u64,
    pub sent_at: u64,
    pub confirmed_at: u64,
    pub network_data: NetworkTransactionData,
    pub network_type: NetworkType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "network_data", content = "data")]
pub enum NetworkTransactionData {
    Evm(EvmTransactionData),
    Solana(SolanaTransactionData),
    Stellar(StellarTransactionData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionData {
    pub gas_price: u128,
    pub gas_limit: u128,
    pub nonce: u64,
    pub value: u64,
    pub data: String,
    pub from: String,
    pub to: String,
    pub chain_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaTransactionData {
    pub recent_blockhash: String,
    pub fee_payer: String,
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarTransactionData {
    pub source_account: String,
    pub fee: u128,
    pub sequence_number: u64,
    pub operations: Vec<String>,
}

impl TryFrom<(&NetworkTransactionRequest, &RelayerRepoModel)> for TransactionRepoModel {
    type Error = RelayerError;

    fn try_from(
        (request, relayer_model): (&NetworkTransactionRequest, &RelayerRepoModel),
    ) -> Result<Self, Self::Error> {
        let now = Utc::now().timestamp() as u64;

        match request {
            NetworkTransactionRequest::Evm(evm_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                hash: "0x".to_string(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: now,
                confirmed_at: 0,
                network_type: NetworkType::Evm,
                network_data: NetworkTransactionData::Evm(EvmTransactionData {
                    gas_price: evm_request.gas_price,
                    gas_limit: evm_request.gas_limit,
                    nonce: 0, // TODO
                    value: evm_request.value,
                    data: evm_request.data.clone(),
                    from: "0x".to_string(), // TODO
                    to: evm_request.to.clone(),
                    chain_id: 1, // TODO
                }),
            }),
            NetworkTransactionRequest::Solana(solana_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                hash: "0x".to_string(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: now,
                confirmed_at: 0,
                network_type: NetworkType::Solana,
                network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                    recent_blockhash: solana_request.recent_blockhash.clone(),
                    fee_payer: "0x".to_string(), // TODO
                    instructions: vec![],        // TODO
                }),
            }),
            NetworkTransactionRequest::Stellar(stellar_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                hash: "0x".to_string(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: now,
                confirmed_at: 0,
                network_type: NetworkType::Stellar,
                network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                    source_account: stellar_request.source_account.clone(),
                    fee: stellar_request.fee,
                    sequence_number: 0, // TODO
                    operations: vec![], // TODO
                }),
            }),
        }
    }
}
