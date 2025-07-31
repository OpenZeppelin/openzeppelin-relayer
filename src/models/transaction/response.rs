use crate::{
    models::{
        evm::Speed, EvmTransactionDataSignature, NetworkTransactionData, TransactionRepoModel,
        TransactionStatus, U256,
    },
    utils::{deserialize_optional_u128, deserialize_optional_u64, serialize_optional_u128},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(untagged)]
pub enum TransactionResponse {
    Evm(Box<EvmTransactionResponse>),
    Solana(Box<SolanaTransactionResponse>),
    Stellar(Box<StellarTransactionResponse>),
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize, ToSchema)]
pub struct EvmTransactionResponse {
    pub id: String,
    #[schema(nullable = false)]
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub status_reason: Option<String>,
    pub created_at: String,
    #[schema(nullable = false)]
    pub sent_at: Option<String>,
    #[schema(nullable = false)]
    pub confirmed_at: Option<String>,
    #[serde(
        serialize_with = "serialize_optional_u128",
        deserialize_with = "deserialize_optional_u128",
        default
    )]
    #[schema(nullable = false)]
    pub gas_price: Option<u128>,
    #[serde(deserialize_with = "deserialize_optional_u64", default)]
    pub gas_limit: Option<u64>,
    #[serde(deserialize_with = "deserialize_optional_u64", default)]
    #[schema(nullable = false)]
    pub nonce: Option<u64>,
    #[schema(value_type = String)]
    pub value: U256,
    pub from: String,
    #[schema(nullable = false)]
    pub to: Option<String>,
    pub relayer_id: String,
    #[schema(nullable = false)]
    pub data: Option<String>,
    #[serde(
        serialize_with = "serialize_optional_u128",
        deserialize_with = "deserialize_optional_u128",
        default
    )]
    #[schema(nullable = false)]
    pub max_fee_per_gas: Option<u128>,
    #[serde(
        serialize_with = "serialize_optional_u128",
        deserialize_with = "deserialize_optional_u128",
        default
    )]
    #[schema(nullable = false)]
    pub max_priority_fee_per_gas: Option<u128>,
    pub signature: Option<EvmTransactionDataSignature>,
    pub speed: Option<Speed>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize, ToSchema)]
pub struct SolanaTransactionResponse {
    pub id: String,
    #[schema(nullable = false)]
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub status_reason: Option<String>,
    pub created_at: String,
    #[schema(nullable = false)]
    pub sent_at: Option<String>,
    #[schema(nullable = false)]
    pub confirmed_at: Option<String>,
    pub recent_blockhash: String,
    pub fee_payer: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize, ToSchema)]
pub struct StellarTransactionResponse {
    pub id: String,
    #[schema(nullable = false)]
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub status_reason: Option<String>,
    pub created_at: String,
    #[schema(nullable = false)]
    pub sent_at: Option<String>,
    #[schema(nullable = false)]
    pub confirmed_at: Option<String>,
    pub source_account: String,
    pub fee: u32,
    pub sequence_number: i64,
}

impl From<TransactionRepoModel> for TransactionResponse {
    fn from(model: TransactionRepoModel) -> Self {
        match model.network_data {
            NetworkTransactionData::Evm(evm_data) => {
                TransactionResponse::Evm(Box::new(EvmTransactionResponse {
                    id: model.id,
                    hash: evm_data.hash,
                    status: model.status,
                    status_reason: model.status_reason,
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
                    data: evm_data.data,
                    max_fee_per_gas: evm_data.max_fee_per_gas,
                    max_priority_fee_per_gas: evm_data.max_priority_fee_per_gas,
                    signature: evm_data.signature,
                    speed: evm_data.speed,
                }))
            }
            NetworkTransactionData::Solana(solana_data) => {
                TransactionResponse::Solana(Box::new(SolanaTransactionResponse {
                    id: model.id,
                    hash: solana_data.hash,
                    status: model.status,
                    status_reason: model.status_reason,
                    created_at: model.created_at,
                    sent_at: model.sent_at,
                    confirmed_at: model.confirmed_at,
                    recent_blockhash: solana_data.recent_blockhash.unwrap_or_default(),
                    fee_payer: solana_data.fee_payer,
                }))
            }
            NetworkTransactionData::Stellar(stellar_data) => {
                TransactionResponse::Stellar(Box::new(StellarTransactionResponse {
                    id: model.id,
                    hash: stellar_data.hash,
                    status: model.status,
                    status_reason: model.status_reason,
                    created_at: model.created_at,
                    sent_at: model.sent_at,
                    confirmed_at: model.confirmed_at,
                    source_account: stellar_data.source_account,
                    fee: stellar_data.fee.unwrap_or(0),
                    sequence_number: stellar_data.sequence_number.unwrap_or(0),
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionData, NetworkType, SolanaTransactionData, StellarTransactionData,
        TransactionRepoModel,
    };
    use chrono::Utc;

    #[test]
    fn test_from_transaction_repo_model_evm() {
        let now = Utc::now().to_rfc3339();
        let model = TransactionRepoModel {
            id: "tx123".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: None,
            relayer_id: "relayer1".to_string(),
            priced_at: None,
            hashes: vec![],
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                hash: Some("0xabc123".to_string()),
                gas_price: Some(20_000_000_000),
                gas_limit: Some(21000),
                nonce: Some(5),
                value: U256::from(1000000000000000000u128), // 1 ETH
                from: "0xsender".to_string(),
                to: Some("0xrecipient".to_string()),
                data: None,
                chain_id: 1,
                signature: None,
                speed: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                raw: None,
            }),
            valid_until: None,
            network_type: NetworkType::Evm,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        };

        let response = TransactionResponse::from(model.clone());

        match response {
            TransactionResponse::Evm(evm) => {
                assert_eq!(evm.id, model.id);
                assert_eq!(evm.hash, Some("0xabc123".to_string()));
                assert_eq!(evm.status, TransactionStatus::Pending);
                assert_eq!(evm.created_at, now);
                assert_eq!(evm.sent_at, Some(now.clone()));
                assert_eq!(evm.confirmed_at, None);
                assert_eq!(evm.gas_price, Some(20_000_000_000));
                assert_eq!(evm.gas_limit, Some(21000));
                assert_eq!(evm.nonce, Some(5));
                assert_eq!(evm.value, U256::from(1000000000000000000u128));
                assert_eq!(evm.from, "0xsender");
                assert_eq!(evm.to, Some("0xrecipient".to_string()));
                assert_eq!(evm.relayer_id, "relayer1");
            }
            _ => panic!("Expected EvmTransactionResponse"),
        }
    }

    #[test]
    fn test_from_transaction_repo_model_solana() {
        let now = Utc::now().to_rfc3339();
        let model = TransactionRepoModel {
            id: "tx456".to_string(),
            status: TransactionStatus::Confirmed,
            status_reason: None,
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: Some(now.clone()),
            relayer_id: "relayer2".to_string(),
            priced_at: None,
            hashes: vec![],
            network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                hash: Some("solana_hash_123".to_string()),
                recent_blockhash: Some("blockhash123".to_string()),
                fee_payer: "fee_payer_pubkey".to_string(),
                instructions: vec![],
            }),
            valid_until: None,
            network_type: NetworkType::Solana,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        };

        let response = TransactionResponse::from(model.clone());

        match response {
            TransactionResponse::Solana(solana) => {
                assert_eq!(solana.id, model.id);
                assert_eq!(solana.hash, Some("solana_hash_123".to_string()));
                assert_eq!(solana.status, TransactionStatus::Confirmed);
                assert_eq!(solana.created_at, now);
                assert_eq!(solana.sent_at, Some(now.clone()));
                assert_eq!(solana.confirmed_at, Some(now.clone()));
                assert_eq!(solana.recent_blockhash, "blockhash123");
                assert_eq!(solana.fee_payer, "fee_payer_pubkey");
            }
            _ => panic!("Expected SolanaTransactionResponse"),
        }
    }

    #[test]
    fn test_from_transaction_repo_model_stellar() {
        let now = Utc::now().to_rfc3339();
        let model = TransactionRepoModel {
            id: "tx789".to_string(),
            status: TransactionStatus::Failed,
            status_reason: None,
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: Some(now.clone()),
            relayer_id: "relayer3".to_string(),
            priced_at: None,
            hashes: vec![],
            network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                hash: Some("stellar_hash_123".to_string()),
                source_account: "source_account_id".to_string(),
                fee: Some(100),
                sequence_number: Some(12345),
                transaction_input: crate::models::TransactionInput::Operations(vec![]),
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                memo: None,
                valid_until: None,
                signatures: Vec::new(),
                simulation_transaction_data: None,
                signed_envelope_xdr: None,
            }),
            valid_until: None,
            network_type: NetworkType::Stellar,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        };

        let response = TransactionResponse::from(model.clone());

        match response {
            TransactionResponse::Stellar(stellar) => {
                assert_eq!(stellar.id, model.id);
                assert_eq!(stellar.hash, Some("stellar_hash_123".to_string()));
                assert_eq!(stellar.status, TransactionStatus::Failed);
                assert_eq!(stellar.created_at, now);
                assert_eq!(stellar.sent_at, Some(now.clone()));
                assert_eq!(stellar.confirmed_at, Some(now.clone()));
                assert_eq!(stellar.source_account, "source_account_id");
                assert_eq!(stellar.fee, 100);
                assert_eq!(stellar.sequence_number, 12345);
            }
            _ => panic!("Expected StellarTransactionResponse"),
        }
    }

    #[test]
    fn test_stellar_fee_bump_transaction_response() {
        let now = Utc::now().to_rfc3339();
        let model = TransactionRepoModel {
            id: "tx-fee-bump".to_string(),
            status: TransactionStatus::Confirmed,
            status_reason: None,
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: Some(now.clone()),
            relayer_id: "relayer3".to_string(),
            priced_at: None,
            hashes: vec!["fee_bump_hash_456".to_string()],
            network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                hash: Some("fee_bump_hash_456".to_string()),
                source_account: "fee_source_account".to_string(),
                fee: Some(200),
                sequence_number: Some(54321),
                transaction_input: crate::models::TransactionInput::SignedXdr {
                    xdr: "dummy_xdr".to_string(),
                    max_fee: 1_000_000,
                },
                network_passphrase: "Test SDF Network ; September 2015".to_string(),
                memo: None,
                valid_until: None,
                signatures: Vec::new(),
                simulation_transaction_data: None,
                signed_envelope_xdr: None,
            }),
            valid_until: None,
            network_type: NetworkType::Stellar,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        };

        let response = TransactionResponse::from(model.clone());

        match response {
            TransactionResponse::Stellar(stellar) => {
                assert_eq!(stellar.id, model.id);
                assert_eq!(stellar.hash, Some("fee_bump_hash_456".to_string()));
                assert_eq!(stellar.status, TransactionStatus::Confirmed);
                assert_eq!(stellar.created_at, now);
                assert_eq!(stellar.sent_at, Some(now.clone()));
                assert_eq!(stellar.confirmed_at, Some(now.clone()));
                assert_eq!(stellar.source_account, "fee_source_account");
                assert_eq!(stellar.fee, 200);
                assert_eq!(stellar.sequence_number, 54321);
            }
            _ => panic!("Expected StellarTransactionResponse"),
        }
    }

    #[test]
    fn test_solana_default_recent_blockhash() {
        let now = Utc::now().to_rfc3339();
        let model = TransactionRepoModel {
            id: "tx456".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: now.clone(),
            sent_at: None,
            confirmed_at: None,
            relayer_id: "relayer2".to_string(),
            priced_at: None,
            hashes: vec![],
            network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                hash: None,
                recent_blockhash: None, // Testing the default case
                fee_payer: "fee_payer_pubkey".to_string(),
                instructions: vec![],
            }),
            valid_until: None,
            network_type: NetworkType::Solana,
            noop_count: None,
            is_canceled: Some(false),
            delete_at: None,
        };

        let response = TransactionResponse::from(model);

        match response {
            TransactionResponse::Solana(solana) => {
                assert_eq!(solana.recent_blockhash, ""); // Should be default empty string
            }
            _ => panic!("Expected SolanaTransactionResponse"),
        }
    }
}
