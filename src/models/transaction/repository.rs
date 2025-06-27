use super::evm::Speed;
use crate::{
    constants::DEFAULT_TRANSACTION_SPEED,
    domain::{evm::PriceParams, SignTransactionResponseEvm},
    models::{
        transaction::request::evm::EvmTransactionRequest, AddressError, EvmNetwork, MemoSpec,
        NetworkRepoModel, NetworkTransactionRequest, NetworkType, OperationSpec, RelayerError,
        RelayerRepoModel, SignerError, StellarNetwork, TransactionError, U256,
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

use crate::constants::STELLAR_DEFAULT_TRANSACTION_FEE;
use crate::models::transaction::stellar::DecoratedSignature;
use soroban_rs::xdr::{
    Transaction as SorobanTransaction, TransactionEnvelope, TransactionV1Envelope, VecM,
};

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
    pub status_reason: Option<String>,
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
    pub status_reason: Option<String>,
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
    /// Validates the transaction repository model
    ///
    /// # Returns
    /// * `Ok(())` if the transaction is valid
    /// * `Err(TransactionError)` if validation fails
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    /// Creates transaction data for replacement by combining existing transaction data with new request data.
    ///
    /// Preserves critical fields like chain_id, from address, and nonce while applying new transaction parameters.
    /// Pricing fields are cleared and must be calculated separately.
    ///
    /// # Arguments
    /// * `old_data` - The existing transaction data to preserve core fields from
    /// * `request` - The new transaction request containing updated parameters
    ///
    /// # Returns
    /// New `EvmTransactionData` configured for replacement transaction
    pub fn for_replacement(old_data: &EvmTransactionData, request: &EvmTransactionRequest) -> Self {
        Self {
            // Preserve existing fields from old transaction
            chain_id: old_data.chain_id,
            from: old_data.from.clone(),
            nonce: old_data.nonce, // Preserve original nonce for replacement

            // Apply new fields from request
            to: request.to.clone(),
            value: request.value,
            data: request.data.clone(),
            gas_limit: request.gas_limit,
            speed: request
                .speed
                .clone()
                .or_else(|| old_data.speed.clone())
                .or(Some(DEFAULT_TRANSACTION_SPEED)),

            // Clear pricing fields - these will be calculated later
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,

            // Reset signing fields
            signature: None,
            hash: None,
            raw: None,
        }
    }

    /// Updates the transaction data with calculated price parameters.
    ///
    /// # Arguments
    /// * `price_params` - Calculated pricing parameters containing gas price and EIP-1559 fees
    ///
    /// # Returns
    /// The updated `EvmTransactionData` with pricing information applied
    pub fn with_price_params(mut self, price_params: PriceParams) -> Self {
        self.gas_price = price_params.gas_price;
        self.max_fee_per_gas = price_params.max_fee_per_gas;
        self.max_priority_fee_per_gas = price_params.max_priority_fee_per_gas;

        self
    }

    /// Updates the transaction data with an estimated gas limit.
    ///
    /// # Arguments
    /// * `gas_limit` - The estimated gas limit for the transaction
    ///
    /// # Returns
    /// The updated `EvmTransactionData` with the new gas limit
    pub fn with_gas_estimate(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Updates the transaction data with a specific nonce value.
    ///
    /// # Arguments
    /// * `nonce` - The nonce value to set for the transaction
    ///
    /// # Returns
    /// The updated `EvmTransactionData` with the specified nonce
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = Some(nonce);
        self
    }

    /// Updates the transaction data with signature information from a signed transaction response.
    ///
    /// # Arguments
    /// * `sig` - The signed transaction response containing signature, hash, and raw transaction data
    ///
    /// # Returns
    /// The updated `EvmTransactionData` with signature information applied
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
            status_reason: None,
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
    pub fee: Option<u32>,
    pub sequence_number: Option<i64>,
    pub operations: Vec<OperationSpec>,
    pub memo: Option<MemoSpec>,
    pub valid_until: Option<String>,
    pub network_passphrase: String,
    #[serde(skip_serializing, skip_deserializing)]
    pub signatures: Vec<DecoratedSignature>,
    pub hash: Option<String>,
    #[serde(skip_serializing, skip_deserializing)]
    pub simulation_transaction_data: Option<String>,
}

impl StellarTransactionData {
    /// Updates the Stellar transaction data with a specific sequence number.
    ///
    /// # Arguments
    /// * `sequence_number` - The sequence number for the Stellar account
    ///
    /// # Returns
    /// The updated `StellarTransactionData` with the specified sequence number
    pub fn with_sequence_number(mut self, sequence_number: i64) -> Self {
        self.sequence_number = Some(sequence_number);
        self
    }

    /// Builds an unsigned Stellar transaction envelope from the current data.
    ///
    /// Useful for transaction simulation before signing.
    ///
    /// # Returns
    /// * `Ok(TransactionEnvelope)` containing the unsigned transaction
    /// * `Err(SignerError)` if the transaction data cannot be converted
    pub fn unsigned_envelope(&self) -> Result<TransactionEnvelope, SignerError> {
        let tx = SorobanTransaction::try_from(self.clone())?;
        Ok(TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        }))
    }

    /// Builds a signed Stellar transaction envelope using the stored signatures.
    ///
    /// # Returns
    /// * `Ok(TransactionEnvelope)` containing the signed transaction with all signatures
    /// * `Err(SignerError)` if the transaction data cannot be converted or has too many signatures
    pub fn signed_envelope(&self) -> Result<TransactionEnvelope, SignerError> {
        let tx = SorobanTransaction::try_from(self.clone())?;
        let sigs = VecM::try_from(self.signatures.clone())
            .map_err(|_| SignerError::ConversionError("too many signatures".into()))?;
        Ok(TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: sigs,
        }))
    }

    /// Updates instance with the given signature appended to the signatures list.
    ///
    /// # Arguments
    /// * `sig` - The decorated signature to append
    ///
    /// # Returns
    /// The updated `StellarTransactionData` with the new signature added
    pub fn attach_signature(mut self, sig: DecoratedSignature) -> Self {
        self.signatures.push(sig);
        self
    }

    /// Updates instance with the transaction hash populated.
    ///
    /// # Arguments
    /// * `hash` - The transaction hash to set
    ///
    /// # Returns
    /// The updated `StellarTransactionData` with the hash field set
    pub fn with_hash(mut self, hash: String) -> Self {
        self.hash = Some(hash);
        self
    }

    /// Return a new instance with simulation data applied (fees and transaction extension).
    pub fn with_simulation_data(
        mut self,
        sim_response: soroban_rs::stellar_rpc_client::SimulateTransactionResponse,
    ) -> Result<Self, crate::models::SignerError> {
        use log::info;

        // Update fee based on simulation (using soroban-helpers formula)
        let operations_count = self.operations.len() as u64;
        let inclusion_fee = operations_count * STELLAR_DEFAULT_TRANSACTION_FEE as u64;
        let resource_fee = sim_response.min_resource_fee;

        let updated_fee = u32::try_from(inclusion_fee + resource_fee)
            .map_err(|_| crate::models::SignerError::ConversionError("Fee too high".to_string()))?
            .max(STELLAR_DEFAULT_TRANSACTION_FEE);
        self.fee = Some(updated_fee);

        // Store simulation transaction data for TransactionExt::V1
        self.simulation_transaction_data = Some(sim_response.transaction_data);

        info!(
            "Applied simulation fee: {} stroops and stored transaction extension data",
            updated_fee
        );
        Ok(self)
    }
}

impl
    TryFrom<(
        &NetworkTransactionRequest,
        &RelayerRepoModel,
        &NetworkRepoModel,
    )> for TransactionRepoModel
{
    type Error = RelayerError;

    fn try_from(
        (request, relayer_model, network_model): (
            &NetworkTransactionRequest,
            &RelayerRepoModel,
            &NetworkRepoModel,
        ),
    ) -> Result<Self, Self::Error> {
        let now = Utc::now().to_rfc3339();

        match request {
            NetworkTransactionRequest::Evm(evm_request) => {
                let network = EvmNetwork::try_from(network_model.clone())?;
                Ok(Self {
                    id: Uuid::new_v4().to_string(),
                    relayer_id: relayer_model.id.clone(),
                    status: TransactionStatus::Pending,
                    status_reason: None,
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
                        chain_id: network.id(),
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
                status_reason: None,
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
            NetworkTransactionRequest::Stellar(stellar_request) => {
                let source_account = stellar_request.source_account.clone();
                let operations = stellar_request.operations.clone();

                // Validate Soroban operation exclusivity (InvokeContract, CreateContract, UploadWasm)
                let has_soroban_operation = operations.iter().any(|op| {
                    matches!(
                        op,
                        OperationSpec::InvokeContract { .. }
                            | OperationSpec::CreateContract { .. }
                            | OperationSpec::UploadWasm { .. }
                    )
                });

                if has_soroban_operation {
                    // Check if there's exactly one operation
                    if operations.len() != 1 {
                        return Err(RelayerError::PolicyConfigurationError(
                            "Soroban operations (InvokeContract, CreateContract, UploadWasm) must be exclusive - only one such operation is allowed per transaction and it cannot be mixed with other operations".to_string()
                        ));
                    }

                    // Check if memo is None when using InvokeHostFunction
                    if let Some(ref memo) = stellar_request.memo {
                        if !matches!(memo, MemoSpec::None) {
                            return Err(RelayerError::PolicyConfigurationError(
                                "Memo must be null when using InvokeHostFunction operations"
                                    .to_string(),
                            ));
                        }
                    }
                }

                Ok(Self {
                    id: Uuid::new_v4().to_string(),
                    relayer_id: relayer_model.id.clone(),
                    status: TransactionStatus::Pending,
                    status_reason: None,
                    created_at: now,
                    sent_at: None,
                    confirmed_at: None,
                    valid_until: None,
                    network_type: NetworkType::Stellar,
                    network_data: NetworkTransactionData::Stellar(StellarTransactionData {
                        source_account,
                        operations,
                        memo: stellar_request.memo.clone(),
                        valid_until: stellar_request.valid_until.clone(),
                        network_passphrase: StellarNetwork::try_from(network_model.clone())?
                            .passphrase,
                        signatures: Vec::new(),
                        hash: None,
                        fee: None,
                        sequence_number: None,
                        simulation_transaction_data: None,
                    }),
                    priced_at: None,
                    hashes: Vec::new(),
                    noop_count: None,
                    is_canceled: Some(false),
                })
            }
        }
    }
}

impl EvmTransactionData {
    /// Converts the transaction's 'to' field to an Alloy Address.
    ///
    /// # Returns
    /// * `Ok(Some(AlloyAddress))` if the 'to' field contains a valid address
    /// * `Ok(None)` if the 'to' field is None or empty (contract creation)
    /// * `Err(SignerError)` if the address format is invalid
    pub fn to_address(&self) -> Result<Option<AlloyAddress>, SignerError> {
        Ok(match self.to.as_deref().filter(|s| !s.is_empty()) {
            Some(addr_str) => Some(AlloyAddress::from_str(addr_str).map_err(|e| {
                AddressError::ConversionError(format!("Invalid 'to' address: {}", e))
            })?),
            None => None,
        })
    }

    /// Converts the transaction's data field from hex string to bytes.
    ///
    /// # Returns
    /// * `Ok(Bytes)` containing the decoded transaction data
    /// * `Err(SignerError)` if the hex string is invalid
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
    use soroban_rs::xdr::{BytesM, Signature, SignatureHint};

    use crate::{
        config::{
            EvmNetworkConfig, NetworkConfigCommon, SolanaNetworkConfig, StellarNetworkConfig,
        },
        models::{
            AssetSpec, NetworkConfigData, RelayerEvmPolicy, RelayerNetworkPolicy,
            RelayerSolanaPolicy, RelayerStellarPolicy,
        },
    };

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
            total_cost: U256::ZERO,
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
        use crate::models::transaction::stellar::{AssetSpec, MemoSpec, OperationSpec};

        let stellar_tx_data = StellarTransactionData {
            source_account: "account123".to_string(),
            fee: Some(100),
            sequence_number: Some(5),
            operations: vec![OperationSpec::Payment {
                destination: "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ".to_string(),
                amount: 100000000, // 10 XLM in stroops
                asset: AssetSpec::Native,
            }],
            memo: Some(MemoSpec::Text {
                value: "Test memo".to_string(),
            }),
            valid_until: Some("2025-01-01T00:00:00Z".to_string()),
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: Some("hash123".to_string()),
            simulation_transaction_data: None,
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

    fn dummy_signature() -> DecoratedSignature {
        let hint = SignatureHint([0; 4]);
        let bytes: Vec<u8> = vec![0u8; 64];
        let bytes_m: BytesM<64> = bytes.try_into().expect("BytesM conversion");
        DecoratedSignature {
            hint,
            signature: Signature(bytes_m),
        }
    }

    fn test_stellar_tx_data() -> StellarTransactionData {
        StellarTransactionData {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            fee: Some(100),
            sequence_number: Some(1),
            operations: vec![OperationSpec::Payment {
                destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                amount: 1000,
                asset: AssetSpec::Native,
            }],
            memo: Some(MemoSpec::None),
            valid_until: None,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            signatures: Vec::new(),
            hash: None,
            simulation_transaction_data: None,
        }
    }

    #[test]
    fn test_with_sequence_number() {
        let tx = test_stellar_tx_data();
        let updated = tx.with_sequence_number(42);
        assert_eq!(updated.sequence_number, Some(42));
    }

    #[test]
    fn test_unsigned_envelope() {
        let tx = test_stellar_tx_data();
        let env = tx.unsigned_envelope();
        assert!(env.is_ok());
        let env = env.unwrap();
        // Should be a TransactionV1Envelope with no signatures
        match env {
            soroban_rs::xdr::TransactionEnvelope::Tx(tx_env) => {
                assert_eq!(tx_env.signatures.len(), 0);
            }
            _ => {
                panic!("Expected TransactionEnvelope::Tx variant");
            }
        }
    }

    #[test]
    fn test_signed_envelope() {
        let mut tx = test_stellar_tx_data();
        tx.signatures.push(dummy_signature());
        let env = tx.signed_envelope();
        assert!(env.is_ok());
        let env = env.unwrap();
        match env {
            soroban_rs::xdr::TransactionEnvelope::Tx(tx_env) => {
                assert_eq!(tx_env.signatures.len(), 1);
            }
            _ => {
                panic!("Expected TransactionEnvelope::Tx variant");
            }
        }
    }

    #[test]
    fn test_attach_signature() {
        let tx = test_stellar_tx_data();
        let sig = dummy_signature();
        let updated = tx.attach_signature(sig.clone());
        assert_eq!(updated.signatures.len(), 1);
        assert_eq!(updated.signatures[0], sig);
    }

    #[test]
    fn test_with_hash() {
        let tx = test_stellar_tx_data();
        let updated = tx.with_hash("hash123".to_string());
        assert_eq!(updated.hash, Some("hash123".to_string()));
    }

    #[test]
    fn test_evm_tx_for_replacement() {
        let old_data = create_sample_evm_tx_data();
        let new_request = EvmTransactionRequest {
            to: Some("0xNewRecipient".to_string()),
            value: U256::from(2000000000000000000u64), // 2 ETH
            data: Some("0xNewData".to_string()),
            gas_limit: 25000,
            gas_price: Some(30000000000), // 30 Gwei (should be ignored)
            max_fee_per_gas: Some(40000000000), // Should be ignored
            max_priority_fee_per_gas: Some(2000000000), // Should be ignored
            speed: Some(Speed::Fast),
            valid_until: None,
        };

        let result = EvmTransactionData::for_replacement(&old_data, &new_request);

        // Should preserve old data fields
        assert_eq!(result.chain_id, old_data.chain_id);
        assert_eq!(result.from, old_data.from);
        assert_eq!(result.nonce, old_data.nonce);

        // Should use new request fields
        assert_eq!(result.to, new_request.to);
        assert_eq!(result.value, new_request.value);
        assert_eq!(result.data, new_request.data);
        assert_eq!(result.gas_limit, new_request.gas_limit);
        assert_eq!(result.speed, new_request.speed);

        // Should clear all pricing fields (regardless of what's in the request)
        assert_eq!(result.gas_price, None);
        assert_eq!(result.max_fee_per_gas, None);
        assert_eq!(result.max_priority_fee_per_gas, None);

        // Should reset signing fields
        assert_eq!(result.signature, None);
        assert_eq!(result.hash, None);
        assert_eq!(result.raw, None);
    }

    #[test]
    fn test_transaction_repo_model_validate() {
        let transaction = TransactionRepoModel::default();
        let result = transaction.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_from_network_transaction_request_evm() {
        use crate::models::{NetworkRepoModel, NetworkType, RelayerRepoModel};

        let evm_request = NetworkTransactionRequest::Evm(EvmTransactionRequest {
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string()),
            value: U256::from(1000000000000000000u128),
            data: Some("0x1234".to_string()),
            gas_limit: 21000,
            gas_price: Some(20000000000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: Some(Speed::Fast),
            valid_until: Some("2024-12-31T23:59:59Z".to_string()),
        });

        let relayer_model = RelayerRepoModel {
            id: "relayer-id".to_string(),
            name: "Test Relayer".to_string(),
            network: "network-id".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
            address: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };

        let network_model = NetworkRepoModel {
            id: "evm:ethereum".to_string(),
            name: "ethereum".to_string(),
            network_type: NetworkType::Evm,
            config: NetworkConfigData::Evm(EvmNetworkConfig {
                common: NetworkConfigCommon {
                    network: "ethereum".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://mainnet.infura.io".to_string()]),
                    explorer_urls: Some(vec!["https://etherscan.io".to_string()]),
                    average_blocktime_ms: Some(12000),
                    is_testnet: Some(false),
                    tags: Some(vec!["mainnet".to_string()]),
                },
                chain_id: Some(1),
                required_confirmations: Some(12),
                features: None,
                symbol: Some("ETH".to_string()),
            }),
        };

        let result = TransactionRepoModel::try_from((&evm_request, &relayer_model, &network_model));
        assert!(result.is_ok());
        let transaction = result.unwrap();

        assert_eq!(transaction.relayer_id, relayer_model.id);
        assert_eq!(transaction.status, TransactionStatus::Pending);
        assert_eq!(transaction.network_type, NetworkType::Evm);
        assert_eq!(
            transaction.valid_until,
            Some("2024-12-31T23:59:59Z".to_string())
        );
        assert!(transaction.is_canceled == Some(false));

        if let NetworkTransactionData::Evm(evm_data) = transaction.network_data {
            assert_eq!(evm_data.from, relayer_model.address);
            assert_eq!(
                evm_data.to,
                Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string())
            );
            assert_eq!(evm_data.value, U256::from(1000000000000000000u128));
            assert_eq!(evm_data.chain_id, 1);
            assert_eq!(evm_data.gas_limit, 21000);
            assert_eq!(evm_data.gas_price, Some(20000000000));
            assert_eq!(evm_data.speed, Some(Speed::Fast));
        } else {
            panic!("Expected EVM transaction data");
        }
    }

    #[test]
    fn test_try_from_network_transaction_request_solana() {
        use crate::models::{
            NetworkRepoModel, NetworkTransactionRequest, NetworkType, RelayerRepoModel,
        };

        let solana_request = NetworkTransactionRequest::Solana(
            crate::models::transaction::request::solana::SolanaTransactionRequest {
                fee_payer: "fee_payer_address".to_string(),
                instructions: vec!["instruction1".to_string(), "instruction2".to_string()],
            },
        );

        let relayer_model = RelayerRepoModel {
            id: "relayer-id".to_string(),
            name: "Test Solana Relayer".to_string(),
            network: "network-id".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy::default()),
            address: "solana_address".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };

        let network_model = NetworkRepoModel {
            id: "solana:mainnet".to_string(),
            name: "mainnet".to_string(),
            network_type: NetworkType::Solana,
            config: NetworkConfigData::Solana(SolanaNetworkConfig {
                common: NetworkConfigCommon {
                    network: "mainnet".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://api.mainnet-beta.solana.com".to_string()]),
                    explorer_urls: Some(vec!["https://explorer.solana.com".to_string()]),
                    average_blocktime_ms: Some(400),
                    is_testnet: Some(false),
                    tags: Some(vec!["mainnet".to_string()]),
                },
            }),
        };

        let result =
            TransactionRepoModel::try_from((&solana_request, &relayer_model, &network_model));
        assert!(result.is_ok());
        let transaction = result.unwrap();

        assert_eq!(transaction.relayer_id, relayer_model.id);
        assert_eq!(transaction.status, TransactionStatus::Pending);
        assert_eq!(transaction.network_type, NetworkType::Solana);
        assert_eq!(transaction.valid_until, None);

        if let NetworkTransactionData::Solana(solana_data) = transaction.network_data {
            assert_eq!(solana_data.fee_payer, "fee_payer_address");
            assert_eq!(solana_data.instructions.len(), 2);
            assert_eq!(solana_data.instructions[0], "instruction1");
            assert_eq!(solana_data.instructions[1], "instruction2");
            assert_eq!(solana_data.recent_blockhash, None);
            assert_eq!(solana_data.hash, None);
        } else {
            panic!("Expected Solana transaction data");
        }
    }

    #[test]
    fn test_try_from_network_transaction_request_stellar() {
        use crate::models::transaction::request::stellar::StellarTransactionRequest;
        use crate::models::{
            NetworkRepoModel, NetworkTransactionRequest, NetworkType, RelayerRepoModel,
        };

        let stellar_request = NetworkTransactionRequest::Stellar(StellarTransactionRequest {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            network: "mainnet".to_string(),
            operations: vec![OperationSpec::Payment {
                destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
                amount: 1000000,
                asset: AssetSpec::Native,
            }],
            memo: Some(MemoSpec::Text {
                value: "Test memo".to_string(),
            }),
            valid_until: Some("2024-12-31T23:59:59Z".to_string()),
        });

        let relayer_model = RelayerRepoModel {
            id: "relayer-id".to_string(),
            name: "Test Stellar Relayer".to_string(),
            network: "network-id".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "signer-id".to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
            address: "stellar_address".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };

        let network_model = NetworkRepoModel {
            id: "stellar:mainnet".to_string(),
            name: "mainnet".to_string(),
            network_type: NetworkType::Stellar,
            config: NetworkConfigData::Stellar(StellarNetworkConfig {
                common: NetworkConfigCommon {
                    network: "mainnet".to_string(),
                    from: None,
                    rpc_urls: Some(vec!["https://horizon.stellar.org".to_string()]),
                    explorer_urls: Some(vec!["https://stellarchain.io".to_string()]),
                    average_blocktime_ms: Some(5000),
                    is_testnet: Some(false),
                    tags: Some(vec!["mainnet".to_string()]),
                },
                passphrase: Some("Public Global Stellar Network ; September 2015".to_string()),
            }),
        };

        let result =
            TransactionRepoModel::try_from((&stellar_request, &relayer_model, &network_model));
        assert!(result.is_ok());
        let transaction = result.unwrap();

        assert_eq!(transaction.relayer_id, relayer_model.id);
        assert_eq!(transaction.status, TransactionStatus::Pending);
        assert_eq!(transaction.network_type, NetworkType::Stellar);
        assert_eq!(transaction.valid_until, None);

        if let NetworkTransactionData::Stellar(stellar_data) = transaction.network_data {
            assert_eq!(
                stellar_data.source_account,
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
            );
            assert_eq!(stellar_data.operations.len(), 1);
            if let OperationSpec::Payment {
                destination,
                amount,
                asset,
            } = &stellar_data.operations[0]
            {
                assert_eq!(
                    destination,
                    "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                );
                assert_eq!(*amount, 1000000);
                assert_eq!(*asset, AssetSpec::Native);
            } else {
                panic!("Expected Payment operation");
            }
            assert_eq!(
                stellar_data.memo,
                Some(MemoSpec::Text {
                    value: "Test memo".to_string()
                })
            );
            assert_eq!(
                stellar_data.valid_until,
                Some("2024-12-31T23:59:59Z".to_string())
            );
            assert_eq!(stellar_data.signatures.len(), 0);
            assert_eq!(stellar_data.hash, None);
            assert_eq!(stellar_data.fee, None);
            assert_eq!(stellar_data.sequence_number, None);
        } else {
            panic!("Expected Stellar transaction data");
        }
    }

    #[test]
    fn test_try_from_network_transaction_data_for_tx_eip1559() {
        // Create a valid EVM transaction with EIP-1559 fields
        let mut evm_tx_data = create_sample_evm_tx_data();
        evm_tx_data.max_fee_per_gas = Some(30_000_000_000);
        evm_tx_data.max_priority_fee_per_gas = Some(2_000_000_000);
        let network_data = NetworkTransactionData::Evm(evm_tx_data.clone());

        // Should convert successfully
        let result = TxEip1559::try_from(network_data);
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

        // Should fail for non-EVM data
        let solana_data = NetworkTransactionData::Solana(SolanaTransactionData {
            recent_blockhash: None,
            fee_payer: "test".to_string(),
            instructions: vec![],
            hash: None,
        });
        assert!(TxEip1559::try_from(solana_data).is_err());
    }

    #[test]
    fn test_evm_transaction_data_defaults() {
        let default_data = EvmTransactionData::default();

        assert_eq!(
            default_data.from,
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        );
        assert_eq!(
            default_data.to,
            Some("0x70997970C51812dc3A010C7d01b50e0d17dc79C8".to_string())
        );
        assert_eq!(default_data.gas_price, Some(20000000000));
        assert_eq!(default_data.value, U256::from(1000000000000000000u128));
        assert_eq!(default_data.data, Some("0x".to_string()));
        assert_eq!(default_data.nonce, Some(1));
        assert_eq!(default_data.chain_id, 1);
        assert_eq!(default_data.gas_limit, 21000);
        assert_eq!(default_data.hash, None);
        assert_eq!(default_data.signature, None);
        assert_eq!(default_data.speed, None);
        assert_eq!(default_data.max_fee_per_gas, None);
        assert_eq!(default_data.max_priority_fee_per_gas, None);
        assert_eq!(default_data.raw, None);
    }

    #[test]
    fn test_transaction_repo_model_defaults() {
        let default_model = TransactionRepoModel::default();

        assert_eq!(default_model.id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(
            default_model.relayer_id,
            "00000000-0000-0000-0000-000000000002"
        );
        assert_eq!(default_model.status, TransactionStatus::Pending);
        assert_eq!(default_model.created_at, "2023-01-01T00:00:00Z");
        assert_eq!(default_model.status_reason, None);
        assert_eq!(default_model.sent_at, None);
        assert_eq!(default_model.confirmed_at, None);
        assert_eq!(default_model.valid_until, None);
        assert_eq!(default_model.network_type, NetworkType::Evm);
        assert_eq!(default_model.priced_at, None);
        assert_eq!(default_model.hashes.len(), 0);
        assert_eq!(default_model.noop_count, None);
        assert_eq!(default_model.is_canceled, Some(false));
    }

    #[test]
    fn test_evm_tx_for_replacement_with_speed_fallback() {
        let mut old_data = create_sample_evm_tx_data();
        old_data.speed = Some(Speed::SafeLow);

        // Request with no speed - should use old data's speed
        let new_request = EvmTransactionRequest {
            to: Some("0xNewRecipient".to_string()),
            value: U256::from(2000000000000000000u64),
            data: Some("0xNewData".to_string()),
            gas_limit: 25000,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
            valid_until: None,
        };

        let result = EvmTransactionData::for_replacement(&old_data, &new_request);
        assert_eq!(result.speed, Some(Speed::SafeLow));

        // Old data with no speed - should use default
        let mut old_data_no_speed = create_sample_evm_tx_data();
        old_data_no_speed.speed = None;

        let result2 = EvmTransactionData::for_replacement(&old_data_no_speed, &new_request);
        assert_eq!(result2.speed, Some(DEFAULT_TRANSACTION_SPEED));
    }

    #[test]
    fn test_transaction_status_serialization() {
        use serde_json;

        // Test serialization of different status values
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Sent).unwrap(),
            "\"sent\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Mined).unwrap(),
            "\"mined\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Confirmed).unwrap(),
            "\"confirmed\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Canceled).unwrap(),
            "\"canceled\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Submitted).unwrap(),
            "\"submitted\""
        );
        assert_eq!(
            serde_json::to_string(&TransactionStatus::Expired).unwrap(),
            "\"expired\""
        );
    }

    #[test]
    fn test_evm_tx_contract_creation() {
        // Test transaction data for contract creation (no 'to' address)
        let mut tx_data = create_sample_evm_tx_data();
        tx_data.to = None;

        let tx_legacy = TxLegacy::try_from(&tx_data).unwrap();
        assert_eq!(tx_legacy.to, TxKind::Create);

        let tx_eip1559 = TxEip1559::try_from(&tx_data).unwrap();
        assert_eq!(tx_eip1559.to, TxKind::Create);
    }

    #[test]
    fn test_evm_tx_default_values_in_conversion() {
        // Test conversion with missing nonce and gas price
        let mut tx_data = create_sample_evm_tx_data();
        tx_data.nonce = None;
        tx_data.gas_price = None;
        tx_data.max_fee_per_gas = None;
        tx_data.max_priority_fee_per_gas = None;

        let tx_legacy = TxLegacy::try_from(&tx_data).unwrap();
        assert_eq!(tx_legacy.nonce, 0); // Default nonce
        assert_eq!(tx_legacy.gas_price, 0); // Default gas price

        let tx_eip1559 = TxEip1559::try_from(&tx_data).unwrap();
        assert_eq!(tx_eip1559.nonce, 0); // Default nonce
        assert_eq!(tx_eip1559.max_fee_per_gas, 0); // Default max fee
        assert_eq!(tx_eip1559.max_priority_fee_per_gas, 0); // Default max priority fee
    }

    // Helper function to create test network and relayer models
    fn test_models() -> (NetworkRepoModel, RelayerRepoModel) {
        use crate::config::{NetworkConfigCommon, StellarNetworkConfig};
        use crate::constants::DEFAULT_STELLAR_MIN_BALANCE;
        use crate::models::network::NetworkConfigData;
        use crate::models::relayer::{RelayerNetworkPolicy, RelayerStellarPolicy};

        let network_config = NetworkConfigData::Stellar(StellarNetworkConfig {
            common: NetworkConfigCommon {
                network: "testnet".to_string(),
                from: None,
                rpc_urls: Some(vec!["https://test.stellar.org".to_string()]),
                explorer_urls: None,
                average_blocktime_ms: Some(5000), // 5 seconds for Stellar
                is_testnet: Some(true),
                tags: None,
            },
            passphrase: Some("Test SDF Network ; September 2015".to_string()),
        });

        let network_model = NetworkRepoModel {
            id: "stellar:testnet".to_string(),
            name: "testnet".to_string(),
            network_type: NetworkType::Stellar,
            config: network_config,
        };

        let relayer_model = RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "stellar:testnet".to_string(),
            paused: false,
            network_type: NetworkType::Stellar,
            signer_id: "test-signer".to_string(),
            policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy {
                max_fee: None,
                timeout_seconds: None,
                min_balance: DEFAULT_STELLAR_MIN_BALANCE,
            }),
            address: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };

        (network_model, relayer_model)
    }

    #[test]
    fn test_invoke_host_function_must_be_exclusive() {
        let (network_model, relayer_model) = test_models();

        // Test case 1: Single InvokeHostFunction - should succeed
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![OperationSpec::InvokeContract {
                    contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                        .to_string(),
                    function_name: "transfer".to_string(),
                    args: vec![],
                    auth: None,
                }],
                memo: None,
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));
        assert!(result.is_ok(), "Single InvokeHostFunction should succeed");

        // Test case 2: InvokeHostFunction mixed with Payment - should fail
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![
                    OperationSpec::Payment {
                        destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                            .to_string(),
                        amount: 1000,
                        asset: AssetSpec::Native,
                    },
                    OperationSpec::InvokeContract {
                        contract_address:
                            "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA".to_string(),
                        function_name: "transfer".to_string(),
                        args: vec![],
                        auth: None,
                    },
                ],
                memo: None,
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));

        match result {
            Ok(_) => panic!("Expected Soroban operation mixed with Payment to fail"),
            Err(err) => {
                let err_str = err.to_string();
                assert!(
                    err_str.contains("Soroban operations") && err_str.contains("must be exclusive"),
                    "Expected error about Soroban operation exclusivity, got: {}",
                    err_str
                );
            }
        }

        // Test case 3: Multiple InvokeHostFunction operations - should fail
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![
                    OperationSpec::InvokeContract {
                        contract_address:
                            "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA".to_string(),
                        function_name: "transfer".to_string(),
                        args: vec![],
                        auth: None,
                    },
                    OperationSpec::InvokeContract {
                        contract_address:
                            "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA".to_string(),
                        function_name: "approve".to_string(),
                        args: vec![],
                        auth: None,
                    },
                ],
                memo: None,
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));

        match result {
            Ok(_) => panic!("Expected multiple Soroban operations to fail"),
            Err(err) => {
                let err_str = err.to_string();
                assert!(
                    err_str.contains("Soroban operations") && err_str.contains("must be exclusive"),
                    "Expected error about Soroban operation exclusivity, got: {}",
                    err_str
                );
            }
        }

        // Test case 4: Multiple Payment operations - should succeed
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![
                    OperationSpec::Payment {
                        destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                            .to_string(),
                        amount: 1000,
                        asset: AssetSpec::Native,
                    },
                    OperationSpec::Payment {
                        destination: "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
                            .to_string(),
                        amount: 2000,
                        asset: AssetSpec::Native,
                    },
                ],
                memo: None,
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));
        assert!(result.is_ok(), "Multiple Payment operations should succeed");

        // Test case 5: InvokeHostFunction with non-None memo - should fail
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![OperationSpec::InvokeContract {
                    contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                        .to_string(),
                    function_name: "transfer".to_string(),
                    args: vec![],
                    auth: None,
                }],
                memo: Some(MemoSpec::Text {
                    value: "This should fail".to_string(),
                }),
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));

        match result {
            Ok(_) => panic!("Expected InvokeHostFunction with non-None memo to fail"),
            Err(err) => {
                let err_str = err.to_string();
                assert!(
                    err_str.contains("Memo must be null when using InvokeHostFunction operations"),
                    "Expected error about memo restriction, got: {}",
                    err_str
                );
            }
        }

        // Test case 6: InvokeHostFunction with memo None - should succeed
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![OperationSpec::InvokeContract {
                    contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                        .to_string(),
                    function_name: "transfer".to_string(),
                    args: vec![],
                    auth: None,
                }],
                memo: Some(MemoSpec::None),
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));
        assert!(
            result.is_ok(),
            "InvokeHostFunction with MemoSpec::None should succeed"
        );

        // Test case 7: InvokeHostFunction with no memo field - should succeed
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![OperationSpec::InvokeContract {
                    contract_address: "CA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUWDA"
                        .to_string(),
                    function_name: "transfer".to_string(),
                    args: vec![],
                    auth: None,
                }],
                memo: None,
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));
        assert!(
            result.is_ok(),
            "InvokeHostFunction with no memo should succeed"
        );

        // Test case 8: Payment operation with memo - should succeed
        let stellar_request =
            crate::models::transaction::request::stellar::StellarTransactionRequest {
                source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                    .to_string(),
                network: "testnet".to_string(),
                operations: vec![OperationSpec::Payment {
                    destination: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
                        .to_string(),
                    amount: 1000,
                    asset: AssetSpec::Native,
                }],
                memo: Some(MemoSpec::Text {
                    value: "Payment memo is allowed".to_string(),
                }),
                valid_until: None,
            };

        let request = NetworkTransactionRequest::Stellar(stellar_request);
        let result = TransactionRepoModel::try_from((&request, &relayer_model, &network_model));
        assert!(result.is_ok(), "Payment operation with memo should succeed");
    }
}
