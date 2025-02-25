use crate::{
    domain::TransactionPriceParams,
    models::{
        EvmNetwork, NetworkTransactionRequest, NetworkType, RelayerError, RelayerRepoModel,
        TransactionError,
    },
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::evm::Speed;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
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
    pub status: TransactionStatus,
    pub created_at: String,
    pub sent_at: String,
    pub confirmed_at: String,
    pub network_data: NetworkTransactionData,
    pub network_type: NetworkType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "network_data", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum NetworkTransactionData {
    Evm(EvmTransactionData),
    Solana(SolanaTransactionData),
    Stellar(StellarTransactionData),
}

impl NetworkTransactionData {
    pub fn get_evm_transaction_data(&self) -> Result<EvmTransactionData, TransactionError> {
        match self {
            NetworkTransactionData::Evm(data) => Ok(data.clone()),
            _ => Err(TransactionError::InvalidType(
                "Expected EVM transaction".to_string(),
            )),
        }
    }

    pub fn get_solana_transaction_data(&self) -> Result<SolanaTransactionData, TransactionError> {
        match self {
            NetworkTransactionData::Solana(data) => Ok(data.clone()),
            _ => Err(TransactionError::InvalidType(
                "Expected Solana transaction".to_string(),
            )),
        }
    }

    pub fn get_stellar_transaction_data(&self) -> Result<StellarTransactionData, TransactionError> {
        match self {
            NetworkTransactionData::Stellar(data) => Ok(data.clone()),
            _ => Err(TransactionError::InvalidType(
                "Expected Stellar transaction".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionDataSignature {
    pub v: u64,
    pub r: String,
    pub s: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTransactionData {
    pub gas_price: Option<u128>,
    pub gas_limit: u128,
    pub nonce: u64,
    pub value: u64,
    pub data: Option<String>,
    pub from: String,
    pub to: Option<String>,
    pub chain_id: u64,
    pub hash: Option<String>,
    pub signature: Option<EvmTransactionDataSignature>,
    pub speed: Option<Speed>,
    pub max_fee_per_gas: Option<u128>,
    pub max_priority_fee_per_gas: Option<u128>,
}

impl EvmTransactionData {
    pub fn with_price_params(mut self, price_params: TransactionPriceParams) -> Self {
        self.gas_price = price_params.gas_price.map(|price| price.to::<u128>());
        self.max_fee_per_gas = price_params.max_fee_per_gas.map(|price| price.to::<u128>());
        self.max_priority_fee_per_gas = price_params
            .max_priority_fee_per_gas
            .map(|price| price.to::<u128>());
        self
    }
}

pub trait EvmTransactionDataTrait {
    fn is_legacy(&self) -> bool;
    fn is_eip1559(&self) -> bool;
    fn is_speed(&self) -> bool;
}

impl EvmTransactionDataTrait for EvmTransactionData {
    fn is_legacy(&self) -> bool {
        self.gas_price.is_some()
    }

    fn is_eip1559(&self) -> bool {
        self.max_fee_per_gas.is_some() && self.max_priority_fee_per_gas.is_some()
    }

    fn is_speed(&self) -> bool {
        self.speed.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaTransactionData {
    pub recent_blockhash: Option<String>,
    pub fee_payer: String,
    pub instructions: Vec<String>,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StellarTransactionData {
    pub source_account: String,
    pub fee: u128,
    pub sequence_number: u64,
    pub operations: Vec<String>,
    pub hash: Option<String>,
}

impl TryFrom<(&NetworkTransactionRequest, &RelayerRepoModel)> for TransactionRepoModel {
    type Error = RelayerError;

    fn try_from(
        (request, relayer_model): (&NetworkTransactionRequest, &RelayerRepoModel),
    ) -> Result<Self, Self::Error> {
        let now = Utc::now().to_rfc3339();

        match request {
            NetworkTransactionRequest::Evm(evm_request) => {
                let named_network = relayer_model.network.clone();
                let network = EvmNetwork::from_network_str(&named_network);
                Ok(Self {
                    id: Uuid::new_v4().to_string(),
                    relayer_id: relayer_model.id.clone(),
                    status: TransactionStatus::Pending,
                    created_at: now,
                    sent_at: "".to_string(),
                    confirmed_at: "".to_string(),
                    network_type: NetworkType::Evm,
                    network_data: NetworkTransactionData::Evm(EvmTransactionData {
                        gas_price: evm_request.gas_price,
                        gas_limit: evm_request.gas_limit,
                        nonce: 0, // TODO
                        value: evm_request.value,
                        data: evm_request.data.clone(),
                        from: evm_request.from.clone(),
                        to: evm_request.to.clone(),
                        chain_id: network.unwrap().id(),
                        hash: Some("0x".to_string()),
                        signature: None,
                        speed: evm_request.speed.clone(),
                        max_fee_per_gas: evm_request.max_fee_per_gas,
                        max_priority_fee_per_gas: evm_request.max_priority_fee_per_gas,
                    }),
                })
            }
            NetworkTransactionRequest::Solana(solana_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: "".to_string(),
                confirmed_at: "".to_string(),
                network_type: NetworkType::Solana,
                network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                    recent_blockhash: None,
                    fee_payer: solana_request.fee_payer.clone(),
                    instructions: solana_request.instructions.clone(),
                    hash: None,
                }),
            }),
            NetworkTransactionRequest::Stellar(stellar_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: "".to_string(),
                confirmed_at: "".to_string(),
                network_type: NetworkType::Stellar,
                network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                    source_account: stellar_request.source_account.clone(),
                    fee: stellar_request.fee,
                    sequence_number: 0, // TODO
                    operations: vec![], // TODO
                    hash: Some("0x".to_string()),
                }),
            }),
        }
    }
}

impl TransactionRepoModel {
    pub fn validate(&self) -> Result<(), TransactionError> {
        Ok(())
    }
}
