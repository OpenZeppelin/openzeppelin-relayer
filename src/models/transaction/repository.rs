use crate::{
    constants::ZERO_ADDRESS,
    models::{
        NetworkTransactionRequest, NetworkType, RelayerError, RelayerNetworkPolicy,
        RelayerRepoModel, TransactionError,
    },
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use uuid::Uuid;

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

// TODO support legacy and eip1559 transactions models
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
    pub hash: Option<String>,
    pub signature: Option<EvmTransactionDataSignature>,
}

impl EvmTransactionData {
    pub fn validate(&self, relayer: &RelayerRepoModel) -> Result<(), TransactionError> {
        if let RelayerNetworkPolicy::Evm(evm_policy) = &relayer.policies {
            if let Some(whitelist) = &evm_policy.whitelist_receivers {
                let target_address = self.to.to_lowercase();
                let mut allowed_addresses: Vec<String> =
                    whitelist.iter().map(|addr| addr.to_lowercase()).collect();
                allowed_addresses.push(ZERO_ADDRESS.to_string());
                allowed_addresses.push(relayer.address.to_lowercase());

                if !allowed_addresses.contains(&target_address) {
                    return Err(TransactionError::ValidationError(
                        "Transaction target address is not whitelisted".to_string(),
                    ));
                }
            }
        }
        Ok(())
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
            NetworkTransactionRequest::Evm(evm_request) => Ok(Self {
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
                    from: "0x".to_string(), // TODO
                    to: evm_request.to.clone(),
                    chain_id: 1, // TODO
                    hash: Some("0x".to_string()),
                    signature: None,
                }),
            }),
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
    pub fn validate(&self, relayer: &RelayerRepoModel) -> Result<(), TransactionError> {
        match &self.network_data {
            NetworkTransactionData::Evm(evm_data) => {
                evm_data.validate(relayer)?;
            }
            NetworkTransactionData::Solana(_) => {}
            NetworkTransactionData::Stellar(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::models::RelayerEvmPolicy;

    use super::*;

    fn create_test_evm_transaction(to: &str) -> TransactionRepoModel {
        TransactionRepoModel {
            id: "test_id".to_string(),
            relayer_id: "test_relayer".to_string(),
            status: TransactionStatus::Pending,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            sent_at: "".to_string(),
            confirmed_at: "".to_string(),
            network_type: NetworkType::Evm,
            network_data: NetworkTransactionData::Evm(EvmTransactionData {
                gas_price: 1000,
                gas_limit: 21000,
                nonce: 0,
                value: 0,
                data: "0x".to_string(),
                from: "0x123".to_string(),
                to: to.to_string(),
                chain_id: 1,
                hash: None,
                signature: None,
            }),
        }
    }

    fn create_test_relayer(whitelist: Option<Vec<String>>) -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "test_network".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            signer_id: "test_signer".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                whitelist_receivers: whitelist,
                ..RelayerEvmPolicy::default()
            }),
            address: "0xrelayer".to_string(),
            notification_id: None,
            system_disabled: false,
        }
    }

    #[test]
    fn test_validate_whitelisted_address() {
        let whitelist = vec!["0xwhitelisted".to_string()];
        let relayer = create_test_relayer(Some(whitelist));
        let tx = create_test_evm_transaction("0xwhitelisted");

        assert!(tx.validate(&relayer).is_ok());
    }

    #[test]
    fn test_validate_zero_address() {
        let relayer = create_test_relayer(Some(vec![]));
        let tx = create_test_evm_transaction(ZERO_ADDRESS);

        assert!(tx.validate(&relayer).is_ok());
    }

    #[test]
    fn test_validate_relayer_address() {
        let relayer = create_test_relayer(Some(vec![]));
        let tx = create_test_evm_transaction(&relayer.address);

        assert!(tx.validate(&relayer).is_ok());
    }

    #[test]
    fn test_validate_non_whitelisted_address() {
        let whitelist = vec!["0xwhitelisted".to_string()];
        let relayer = create_test_relayer(Some(whitelist));
        let tx = create_test_evm_transaction("0xnonwhitelisted");

        assert!(matches!(
            tx.validate(&relayer),
            Err(TransactionError::ValidationError(_))
        ));
    }

    #[test]
    fn test_validate_no_whitelist_policy() {
        let relayer = create_test_relayer(None);
        let tx = create_test_evm_transaction("0xany");

        assert!(tx.validate(&relayer).is_ok());
    }
}
