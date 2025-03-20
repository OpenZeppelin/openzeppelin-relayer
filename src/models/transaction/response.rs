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
    pub sent_at: Option<String>,
    pub confirmed_at: Option<String>,
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
    pub sent_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub recent_blockhash: String,
    pub fee_payer: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Deserialize)]
pub struct StellarTransactionResponse {
    pub id: String,
    pub hash: Option<String>,
    pub status: TransactionStatus,
    pub created_at: String,
    pub sent_at: Option<String>,
    pub confirmed_at: Option<String>,
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
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: None,
            relayer_id: "relayer1".to_string(),
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                hash: Some("0xabc123".to_string()),
                gas_price: Some(20_000_000_000),
                gas_limit: 21000,
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
            priced_at: None,
            hashes: vec![],
            noop_count: None,
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
                assert_eq!(evm.gas_limit, 21000);
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
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: Some(now.clone()),
            relayer_id: "relayer2".to_string(),
            network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                hash: Some("solana_hash_123".to_string()),
                recent_blockhash: Some("blockhash123".to_string()),
                fee_payer: "fee_payer_pubkey".to_string(),
                instructions: vec![],
            }),
            valid_until: None,
            network_type: NetworkType::Solana,
            priced_at: None,
            hashes: vec![],
            noop_count: None,
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
            created_at: now.clone(),
            sent_at: Some(now.clone()),
            confirmed_at: Some(now.clone()),
            relayer_id: "relayer3".to_string(),
            network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                hash: Some("stellar_hash_123".to_string()),
                source_account: "source_account_id".to_string(),
                fee: 100,
                sequence_number: 12345,
                operations: vec![],
            }),
            valid_until: None,
            network_type: NetworkType::Stellar,
            priced_at: None,
            hashes: vec![],
            noop_count: None,
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
    fn test_solana_default_recent_blockhash() {
        let now = Utc::now().to_rfc3339();
        let model = TransactionRepoModel {
            id: "tx456".to_string(),
            status: TransactionStatus::Pending,
            created_at: now.clone(),
            sent_at: None,
            confirmed_at: None,
            relayer_id: "relayer2".to_string(),
            network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                hash: None,
                recent_blockhash: None, // Testing the default case
                fee_payer: "fee_payer_pubkey".to_string(),
                instructions: vec![],
            }),
            valid_until: None,
            network_type: NetworkType::Solana,
            priced_at: None,
            hashes: vec![],
            noop_count: None,
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
