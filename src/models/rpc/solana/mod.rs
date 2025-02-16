use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use solana_sdk::transaction::Transaction;
use thiserror::Error;

#[derive(Debug, Error, Deserialize, Serialize)]
pub enum SolanaEncodingError {
    #[error("Failed to serialize transaction: {0}")]
    SerializationError(String),
    #[error("Failed to decode base64: {0}")]
    DecodeError(String),
    #[error("Failed to deserialize transaction: {0}")]
    DeserializeError(String),
}
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct EncodedSerializedTransaction(String);

impl EncodedSerializedTransaction {
    pub fn into_inner(self) -> String {
        self.0
    }

    pub fn validate_sign_request(&self) -> Result<(), SolanaEncodingError> {
        Ok(())
    }
}

impl TryFrom<&solana_sdk::transaction::Transaction> for EncodedSerializedTransaction {
    type Error = SolanaEncodingError;

    fn try_from(transaction: &Transaction) -> Result<Self, Self::Error> {
        let serialized = bincode::serialize(transaction)
            .map_err(|e| SolanaEncodingError::SerializationError(e.to_string()))?;

        Ok(Self(STANDARD.encode(serialized)))
    }
}

impl TryFrom<EncodedSerializedTransaction> for solana_sdk::transaction::Transaction {
    type Error = SolanaEncodingError;

    fn try_from(encoded: EncodedSerializedTransaction) -> Result<Self, Self::Error> {
        // Decode base64
        let tx_bytes = STANDARD
            .decode(encoded.0)
            .map_err(|e| SolanaEncodingError::DecodeError(e.to_string()))?;

        // Deserialize into Transaction
        bincode::deserialize(&tx_bytes)
            .map_err(|e| SolanaEncodingError::DeserializeError(e.to_string()))?
    }
}

// feeEstimate
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct FeeEstimateRequestParams {
    pub transaction: String,
    pub fee_token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct FeeEstimateResult {
    pub estimated_fee: String,
    pub conversion_rate: String,
}

// transferTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct TransferTransactionRequestParams {
    pub amount: usize,
    pub token: String,
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct TransferTransactionResult {
    pub transaction: String,
    pub fee_in_spl: String,
    pub fee_in_lamports: String,
    pub fee_token: String,
    pub valid_until_blockheight: u64,
}

// prepareTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct PrepareTransactionRequestParams {
    pub transaction: usize,
    pub fee_token: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct PrepareTransactionResult {
    pub transaction: String,
    pub fee_in_spl: String,
    pub fee_in_lamports: String,
    pub fee_token: String,
    pub valid_until_blockheight: u64,
}

// signTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignTransactionRequestParams {
    pub transaction: EncodedSerializedTransaction,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignTransactionResult {
    pub transaction: EncodedSerializedTransaction,
    pub signature: String,
}

// signAndSendTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignAndSendTransactionRequestParams {
    pub transaction: EncodedSerializedTransaction,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignAndSendTransactionResult {
    pub transaction: EncodedSerializedTransaction,
    pub signature: String,
}

// getSupportedTokens
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct GetSupportedTokensRequestParams {}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct GetSupportedTokensItem {
    pub mint: String,
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct GetSupportedTokensResult {
    pub tokens: Vec<GetSupportedTokensItem>,
}

// getFeaturesEnabled
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct GetFeaturesEnabledRequestParams {}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct GetFeaturesEnabledResult {
    pub features: Vec<String>,
}

pub enum SolanaRpcMethod {
    FeeEstimate,
    TransferTransaction,
    PrepareTransaction,
    SignTransaction,
    SignAndSendTransaction,
    GetSupportedTokens,
    GetFeaturesEnabled,
}

impl SolanaRpcMethod {
    pub fn from_str(method: &str) -> Option<Self> {
        match method {
            "feeEstimate" => Some(SolanaRpcMethod::FeeEstimate),
            "transferTransaction" => Some(SolanaRpcMethod::TransferTransaction),
            "prepareTransaction" => Some(SolanaRpcMethod::PrepareTransaction),
            "signTransaction" => Some(SolanaRpcMethod::SignTransaction),
            "signAndSendTransaction" => Some(SolanaRpcMethod::SignAndSendTransaction),
            "getSupportedTokens" => Some(SolanaRpcMethod::GetSupportedTokens),
            "getFeaturesEnabled" => Some(SolanaRpcMethod::GetFeaturesEnabled),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum SolanaRpcResult {
    FeeEstimate(FeeEstimateResult),
    TransferTransaction(TransferTransactionResult),
    PrepareTransaction(PrepareTransactionResult),
    SignTransaction(SignTransactionResult),
    SignAndSendTransaction(SignAndSendTransactionResult),
    GetSupportedTokens(GetSupportedTokensResult),
    GetFeaturesEnabled(GetFeaturesEnabledResult),
}
