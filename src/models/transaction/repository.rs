use crate::{
    domain::{PriceParams, SignTransactionResponseEvm},
    models::{
        AddressError, EvmNetwork, NetworkTransactionRequest, NetworkType, RelayerError,
        RelayerRepoModel, SignerError, TransactionError, U256,
    },
};
use alloy::{
    consensus::{TxEip1559, TxLegacy},
    primitives::{Address as AlloyAddress, Bytes, TxKind},
    rpc::types::AccessList,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, str::FromStr};
use utoipa::ToSchema;
use uuid::Uuid;

use super::evm::Speed;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TransactionStatus {
    Canceled,
    Pending,
    Sent,
    Submitted,
    Mined,
    Confirmed,
    Failed,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TransactionUpdateRequest {
    pub status: Option<TransactionStatus>,
    pub sent_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub network_data: Option<NetworkTransactionData>,
    /// Timestamp when gas price was determined
    pub priced_at: Option<String>,
    /// History of transaction hashes
    pub hashes: Option<Vec<String>>,
    /// Number of no-ops in the transaction
    pub noop_count: Option<u32>,
    /// Whether the transaction is canceled
    pub is_canceled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRepoModel {
    pub id: String,
    pub relayer_id: String,
    pub status: TransactionStatus,
    pub created_at: String,
    pub sent_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub valid_until: Option<String>,
    pub network_data: NetworkTransactionData,
    /// Timestamp when gas price was determined
    pub priced_at: Option<String>,
    /// History of transaction hashes
    pub hashes: Vec<String>,
    pub network_type: NetworkType,
    pub noop_count: Option<u32>,
    pub is_canceled: Option<bool>,
}

impl TransactionRepoModel {
    pub fn validate(&self) -> Result<(), TransactionError> {
        Ok(())
    }
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
    pub r: String,
    pub s: String,
    pub v: u8,
    pub sig: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]

pub struct EvmTransactionData {
    pub gas_price: Option<u128>,
    pub gas_limit: u64,
    pub nonce: Option<u64>,
    pub value: U256,
    pub data: Option<String>,
    pub from: String,
    pub to: Option<String>,
    pub chain_id: u64,
    pub hash: Option<String>,
    pub signature: Option<EvmTransactionDataSignature>,
    pub speed: Option<Speed>,
    pub max_fee_per_gas: Option<u128>,
    pub max_priority_fee_per_gas: Option<u128>,
    pub raw: Option<Vec<u8>>,
}

impl EvmTransactionData {
    pub fn with_price_params(mut self, price_params: PriceParams) -> Self {
        self.gas_price = price_params.gas_price;
        self.max_fee_per_gas = price_params.max_fee_per_gas;
        self.max_priority_fee_per_gas = price_params.max_priority_fee_per_gas;

        self
    }
    pub fn with_gas_estimate(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = Some(nonce);
        self
    }
    pub fn with_signed_transaction_data(mut self, sig: SignTransactionResponseEvm) -> Self {
        self.signature = Some(sig.signature);
        self.hash = Some(sig.hash);
        self.raw = Some(sig.raw);
        self
    }
}

#[cfg(test)]
impl Default for EvmTransactionData {
    fn default() -> Self {
        Self {
            from: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(), // Standard Hardhat test address
            to: Some("0x70997970C51812dc3A010C7d01b50e0d17dc79C8".to_string()), // Standard Hardhat test address
            gas_price: Some(20000000000),
            value: U256::from(1000000000000000000u128), // 1 ETH
            data: Some("0x".to_string()),
            nonce: Some(1),
            chain_id: 1,
            gas_limit: 21000,
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        }
    }
}

#[cfg(test)]
impl Default for TransactionRepoModel {
    fn default() -> Self {
        Self {
            id: "00000000-0000-0000-0000-000000000001".to_string(),
            relayer_id: "00000000-0000-0000-0000-000000000002".to_string(),
            status: TransactionStatus::Pending,
            created_at: "2023-01-01T00:00:00Z".to_string(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            network_data: NetworkTransactionData::Evm(EvmTransactionData::default()),
            network_type: NetworkType::Evm,
            priced_at: None,
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
        }
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
                    sent_at: None,
                    confirmed_at: None,
                    valid_until: evm_request.valid_until.clone(),
                    network_type: NetworkType::Evm,
                    network_data: NetworkTransactionData::Evm(EvmTransactionData {
                        gas_price: evm_request.gas_price,
                        gas_limit: evm_request.gas_limit,
                        nonce: None,
                        value: evm_request.value,
                        data: evm_request.data.clone(),
                        from: relayer_model.address.clone(),
                        to: evm_request.to.clone(),
                        chain_id: network.unwrap().id(),
                        hash: None,
                        signature: None,
                        speed: evm_request.speed.clone(),
                        max_fee_per_gas: evm_request.max_fee_per_gas,
                        max_priority_fee_per_gas: evm_request.max_priority_fee_per_gas,
                        raw: None,
                    }),
                    priced_at: None,
                    hashes: Vec::new(),
                    noop_count: None,
                    is_canceled: Some(false),
                })
            }
            NetworkTransactionRequest::Solana(solana_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: None,
                confirmed_at: None,
                valid_until: None,
                network_type: NetworkType::Solana,
                network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                    recent_blockhash: None,
                    fee_payer: solana_request.fee_payer.clone(),
                    instructions: solana_request.instructions.clone(),
                    hash: None,
                }),
                priced_at: None,
                hashes: Vec::new(),
                noop_count: None,
                is_canceled: Some(false),
            }),
            NetworkTransactionRequest::Stellar(stellar_request) => Ok(Self {
                id: Uuid::new_v4().to_string(),
                relayer_id: relayer_model.id.clone(),
                status: TransactionStatus::Pending,
                created_at: now,
                sent_at: None,
                confirmed_at: None,
                valid_until: None,
                network_type: NetworkType::Stellar,
                network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                    source_account: stellar_request.source_account.clone(),
                    fee: stellar_request.fee,
                    sequence_number: 0, // TODO
                    operations: vec![], // TODO
                    hash: Some("0x".to_string()),
                }),
                priced_at: None,
                hashes: Vec::new(),
                noop_count: None,
                is_canceled: Some(false),
            }),
        }
    }
}

impl EvmTransactionData {
    pub fn to_address(&self) -> Result<Option<AlloyAddress>, SignerError> {
        Ok(match self.to.as_deref().filter(|s| !s.is_empty()) {
            Some(addr_str) => Some(AlloyAddress::from_str(addr_str).map_err(|e| {
                AddressError::ConversionError(format!("Invalid 'to' address: {}", e))
            })?),
            None => None,
        })
    }

    pub fn data_to_bytes(&self) -> Result<Bytes, SignerError> {
        Bytes::from_str(self.data.as_deref().unwrap_or(""))
            .map_err(|e| SignerError::SigningError(format!("Invalid transaction data: {}", e)))
    }
}

impl TryFrom<NetworkTransactionData> for TxLegacy {
    type Error = SignerError;

    fn try_from(tx: NetworkTransactionData) -> Result<Self, Self::Error> {
        match tx {
            NetworkTransactionData::Evm(tx) => {
                let tx_kind = match tx.to_address()? {
                    Some(addr) => TxKind::Call(addr),
                    None => TxKind::Create,
                };

                Ok(Self {
                    chain_id: Some(tx.chain_id),
                    nonce: tx.nonce.unwrap_or(0),
                    gas_limit: tx.gas_limit,
                    gas_price: tx.gas_price.unwrap_or(0),
                    to: tx_kind,
                    value: tx.value,
                    input: tx.data_to_bytes()?,
                })
            }
            _ => Err(SignerError::SigningError(
                "Not an EVM transaction".to_string(),
            )),
        }
    }
}

impl TryFrom<NetworkTransactionData> for TxEip1559 {
    type Error = SignerError;

    fn try_from(tx: NetworkTransactionData) -> Result<Self, Self::Error> {
        match tx {
            NetworkTransactionData::Evm(tx) => {
                let tx_kind = match tx.to_address()? {
                    Some(addr) => TxKind::Call(addr),
                    None => TxKind::Create,
                };

                Ok(Self {
                    chain_id: tx.chain_id,
                    nonce: tx.nonce.unwrap_or(0),
                    gas_limit: tx.gas_limit,
                    max_fee_per_gas: tx.max_fee_per_gas.unwrap_or(0),
                    max_priority_fee_per_gas: tx.max_priority_fee_per_gas.unwrap_or(0),
                    to: tx_kind,
                    value: tx.value,
                    access_list: AccessList::default(),
                    input: tx.data_to_bytes()?,
                })
            }
            _ => Err(SignerError::SigningError(
                "Not an EVM transaction".to_string(),
            )),
        }
    }
}

impl TryFrom<&EvmTransactionData> for TxLegacy {
    type Error = SignerError;

    fn try_from(tx: &EvmTransactionData) -> Result<Self, Self::Error> {
        let tx_kind = match tx.to_address()? {
            Some(addr) => TxKind::Call(addr),
            None => TxKind::Create,
        };

        Ok(Self {
            chain_id: Some(tx.chain_id),
            nonce: tx.nonce.unwrap_or(0),
            gas_limit: tx.gas_limit,
            gas_price: tx.gas_price.unwrap_or(0),
            to: tx_kind,
            value: tx.value,
            input: tx.data_to_bytes()?,
        })
    }
}

impl TryFrom<EvmTransactionData> for TxLegacy {
    type Error = SignerError;

    fn try_from(tx: EvmTransactionData) -> Result<Self, Self::Error> {
        Self::try_from(&tx)
    }
}

impl TryFrom<&EvmTransactionData> for TxEip1559 {
    type Error = SignerError;

    fn try_from(tx: &EvmTransactionData) -> Result<Self, Self::Error> {
        let tx_kind = match tx.to_address()? {
            Some(addr) => TxKind::Call(addr),
            None => TxKind::Create,
        };

        Ok(Self {
            chain_id: tx.chain_id,
            nonce: tx.nonce.unwrap_or(0),
            gas_limit: tx.gas_limit,
            max_fee_per_gas: tx.max_fee_per_gas.unwrap_or(0),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.unwrap_or(0),
            to: tx_kind,
            value: tx.value,
            access_list: AccessList::default(),
            input: tx.data_to_bytes()?,
        })
    }
}

impl TryFrom<EvmTransactionData> for TxEip1559 {
    type Error = SignerError;

    fn try_from(tx: EvmTransactionData) -> Result<Self, Self::Error> {
        Self::try_from(&tx)
    }
}

impl From<&[u8; 65]> for EvmTransactionDataSignature {
    fn from(bytes: &[u8; 65]) -> Self {
        Self {
            r: hex::encode(&bytes[0..32]),
            s: hex::encode(&bytes[32..64]),
            v: bytes[64],
            sig: hex::encode(bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_from_bytes() {
        let test_bytes: [u8; 65] = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, // r (32 bytes)
            33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54,
            55, 56, 57, 58, 59, 60, 61, 62, 63, 64, // s (32 bytes)
            27, // v (1 byte)
        ];

        let signature = EvmTransactionDataSignature::from(&test_bytes);

        assert_eq!(signature.r.len(), 64); // 32 bytes in hex
        assert_eq!(signature.s.len(), 64); // 32 bytes in hex
        assert_eq!(signature.v, 27);
        assert_eq!(signature.sig.len(), 130); // 65 bytes in hex
    }

    // Create a helper function to generate a sample EvmTransactionData for testing
    fn create_sample_evm_tx_data() -> EvmTransactionData {
        EvmTransactionData {
            gas_price: Some(20_000_000_000),
            gas_limit: 21000,
            nonce: Some(5),
            value: U256::from(1000000000000000000u128), // 1 ETH
            data: Some("0x".to_string()),
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            speed: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            raw: None,
        }
    }

    // Tests for EvmTransactionData methods
    #[test]
    fn test_evm_tx_with_price_params() {
        let tx_data = create_sample_evm_tx_data();
        let price_params = PriceParams {
            gas_price: None,
            max_fee_per_gas: Some(30_000_000_000),
            max_priority_fee_per_gas: Some(2_000_000_000),
            is_min_bumped: None,
            extra_fee: None,
        };

        let updated_tx = tx_data.with_price_params(price_params);

        assert_eq!(updated_tx.max_fee_per_gas, Some(30_000_000_000));
        assert_eq!(updated_tx.max_priority_fee_per_gas, Some(2_000_000_000));
    }

    #[test]
    fn test_evm_tx_with_gas_estimate() {
        let tx_data = create_sample_evm_tx_data();
        let new_gas_limit = 30000;

        let updated_tx = tx_data.with_gas_estimate(new_gas_limit);

        assert_eq!(updated_tx.gas_limit, new_gas_limit);
    }

    #[test]
    fn test_evm_tx_with_nonce() {
        let tx_data = create_sample_evm_tx_data();
        let new_nonce = 10;

        let updated_tx = tx_data.with_nonce(new_nonce);

        assert_eq!(updated_tx.nonce, Some(new_nonce));
    }

    #[test]
    fn test_evm_tx_with_signed_transaction_data() {
        let tx_data = create_sample_evm_tx_data();

        let signature = EvmTransactionDataSignature {
            r: "r_value".to_string(),
            s: "s_value".to_string(),
            v: 27,
            sig: "signature_value".to_string(),
        };

        let signed_tx_response = SignTransactionResponseEvm {
            signature,
            hash: "0xabcdef1234567890".to_string(),
            raw: vec![1, 2, 3, 4, 5],
        };

        let updated_tx = tx_data.with_signed_transaction_data(signed_tx_response);

        assert_eq!(updated_tx.signature.as_ref().unwrap().r, "r_value");
        assert_eq!(updated_tx.signature.as_ref().unwrap().s, "s_value");
        assert_eq!(updated_tx.signature.as_ref().unwrap().v, 27);
        assert_eq!(updated_tx.hash, Some("0xabcdef1234567890".to_string()));
        assert_eq!(updated_tx.raw, Some(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn test_evm_tx_to_address() {
        // Test with valid address
        let tx_data = create_sample_evm_tx_data();
        let address_result = tx_data.to_address();
        assert!(address_result.is_ok());
        let address_option = address_result.unwrap();
        assert!(address_option.is_some());
        assert_eq!(
            address_option.unwrap().to_string().to_lowercase(),
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_lowercase()
        );

        // Test with None address (contract creation)
        let mut contract_creation_tx = create_sample_evm_tx_data();
        contract_creation_tx.to = None;
        let address_result = contract_creation_tx.to_address();
        assert!(address_result.is_ok());
        assert!(address_result.unwrap().is_none());

        // Test with empty address string
        let mut empty_address_tx = create_sample_evm_tx_data();
        empty_address_tx.to = Some("".to_string());
        let address_result = empty_address_tx.to_address();
        assert!(address_result.is_ok());
        assert!(address_result.unwrap().is_none());

        // Test with invalid address
        let mut invalid_address_tx = create_sample_evm_tx_data();
        invalid_address_tx.to = Some("0xINVALID".to_string());
        let address_result = invalid_address_tx.to_address();
        assert!(address_result.is_err());
    }

    #[test]
    fn test_evm_tx_data_to_bytes() {
        // Test with valid hex data
        let mut tx_data = create_sample_evm_tx_data();
        tx_data.data = Some("0x1234".to_string());
        let bytes_result = tx_data.data_to_bytes();
        assert!(bytes_result.is_ok());
        assert_eq!(bytes_result.unwrap().as_ref(), &[0x12, 0x34]);

        // Test with empty data
        tx_data.data = Some("".to_string());
        assert!(tx_data.data_to_bytes().is_ok());

        // Test with None data
        tx_data.data = None;
        assert!(tx_data.data_to_bytes().is_ok());

        // Test with invalid hex data
        tx_data.data = Some("0xZZ".to_string());
        assert!(tx_data.data_to_bytes().is_err());
    }

    // Tests for EvmTransactionDataTrait implementation
    #[test]
    fn test_evm_tx_is_legacy() {
        let mut tx_data = create_sample_evm_tx_data();

        // Legacy transaction has gas_price
        assert!(tx_data.is_legacy());

        // Not legacy if gas_price is None
        tx_data.gas_price = None;
        assert!(!tx_data.is_legacy());
    }

    #[test]
    fn test_evm_tx_is_eip1559() {
        let mut tx_data = create_sample_evm_tx_data();

        // Not EIP-1559 initially
        assert!(!tx_data.is_eip1559());

        // Set EIP-1559 fields
        tx_data.max_fee_per_gas = Some(30_000_000_000);
        tx_data.max_priority_fee_per_gas = Some(2_000_000_000);
        assert!(tx_data.is_eip1559());

        // Not EIP-1559 if one field is missing
        tx_data.max_priority_fee_per_gas = None;
        assert!(!tx_data.is_eip1559());
    }

    #[test]
    fn test_evm_tx_is_speed() {
        let mut tx_data = create_sample_evm_tx_data();

        // No speed initially
        assert!(!tx_data.is_speed());

        // Set speed
        tx_data.speed = Some(Speed::Fast);
        assert!(tx_data.is_speed());
    }

    // Tests for NetworkTransactionData methods
    #[test]
    fn test_network_tx_data_get_evm_transaction_data() {
        let evm_tx_data = create_sample_evm_tx_data();
        let network_data = NetworkTransactionData::Evm(evm_tx_data.clone());

        // Should succeed for EVM data
        let result = network_data.get_evm_transaction_data();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().chain_id, evm_tx_data.chain_id);

        // Should fail for non-EVM data
        let solana_data = NetworkTransactionData::Solana(SolanaTransactionData {
            recent_blockhash: None,
            fee_payer: "test".to_string(),
            instructions: vec![],
            hash: None,
        });
        assert!(solana_data.get_evm_transaction_data().is_err());
    }

    #[test]
    fn test_network_tx_data_get_solana_transaction_data() {
        let solana_tx_data = SolanaTransactionData {
            recent_blockhash: Some("hash123".to_string()),
            fee_payer: "payer123".to_string(),
            instructions: vec!["instruction1".to_string()],
            hash: None,
        };
        let network_data = NetworkTransactionData::Solana(solana_tx_data.clone());

        // Should succeed for Solana data
        let result = network_data.get_solana_transaction_data();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().fee_payer, solana_tx_data.fee_payer);

        // Should fail for non-Solana data
        let evm_data = NetworkTransactionData::Evm(create_sample_evm_tx_data());
        assert!(evm_data.get_solana_transaction_data().is_err());
    }

    #[test]
    fn test_network_tx_data_get_stellar_transaction_data() {
        let stellar_tx_data = StellarTransactionData {
            source_account: "account123".to_string(),
            fee: 100,
            sequence_number: 5,
            operations: vec!["op1".to_string()],
            hash: Some("hash123".to_string()),
        };
        let network_data = NetworkTransactionData::Stellar(stellar_tx_data.clone());

        // Should succeed for Stellar data
        let result = network_data.get_stellar_transaction_data();
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().source_account,
            stellar_tx_data.source_account
        );

        // Should fail for non-Stellar data
        let evm_data = NetworkTransactionData::Evm(create_sample_evm_tx_data());
        assert!(evm_data.get_stellar_transaction_data().is_err());
    }

    // Test for TryFrom<NetworkTransactionData> for TxLegacy
    #[test]
    fn test_try_from_network_tx_data_for_tx_legacy() {
        // Create a valid EVM transaction
        let evm_tx_data = create_sample_evm_tx_data();
        let network_data = NetworkTransactionData::Evm(evm_tx_data.clone());

        // Should convert successfully
        let result = TxLegacy::try_from(network_data);
        assert!(result.is_ok());
        let tx_legacy = result.unwrap();

        // Verify fields
        assert_eq!(tx_legacy.chain_id, Some(evm_tx_data.chain_id));
        assert_eq!(tx_legacy.nonce, evm_tx_data.nonce.unwrap());
        assert_eq!(tx_legacy.gas_limit, evm_tx_data.gas_limit);
        assert_eq!(tx_legacy.gas_price, evm_tx_data.gas_price.unwrap());
        assert_eq!(tx_legacy.value, evm_tx_data.value);

        // Should fail for non-EVM data
        let solana_data = NetworkTransactionData::Solana(SolanaTransactionData {
            recent_blockhash: None,
            fee_payer: "test".to_string(),
            instructions: vec![],
            hash: None,
        });
        assert!(TxLegacy::try_from(solana_data).is_err());
    }

    #[test]
    fn test_try_from_evm_tx_data_for_tx_eip1559() {
        // Create a valid EVM transaction with EIP-1559 fields
        let mut evm_tx_data = create_sample_evm_tx_data();
        evm_tx_data.max_fee_per_gas = Some(30_000_000_000);
        evm_tx_data.max_priority_fee_per_gas = Some(2_000_000_000);

        // Should convert successfully
        let result = TxEip1559::try_from(evm_tx_data.clone());
        assert!(result.is_ok());
        let tx_eip1559 = result.unwrap();

        // Verify fields
        assert_eq!(tx_eip1559.chain_id, evm_tx_data.chain_id);
        assert_eq!(tx_eip1559.nonce, evm_tx_data.nonce.unwrap());
        assert_eq!(tx_eip1559.gas_limit, evm_tx_data.gas_limit);
        assert_eq!(
            tx_eip1559.max_fee_per_gas,
            evm_tx_data.max_fee_per_gas.unwrap()
        );
        assert_eq!(
            tx_eip1559.max_priority_fee_per_gas,
            evm_tx_data.max_priority_fee_per_gas.unwrap()
        );
        assert_eq!(tx_eip1559.value, evm_tx_data.value);
        assert!(tx_eip1559.access_list.0.is_empty());
    }

    #[test]
    fn test_try_from_evm_tx_data_for_tx_legacy() {
        // Create a valid EVM transaction with legacy fields
        let evm_tx_data = create_sample_evm_tx_data();

        // Should convert successfully
        let result = TxLegacy::try_from(evm_tx_data.clone());
        assert!(result.is_ok());
        let tx_legacy = result.unwrap();

        // Verify fields
        assert_eq!(tx_legacy.chain_id, Some(evm_tx_data.chain_id));
        assert_eq!(tx_legacy.nonce, evm_tx_data.nonce.unwrap());
        assert_eq!(tx_legacy.gas_limit, evm_tx_data.gas_limit);
        assert_eq!(tx_legacy.gas_price, evm_tx_data.gas_price.unwrap());
        assert_eq!(tx_legacy.value, evm_tx_data.value);
    }
}
