use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use solana_sdk::transaction::{Transaction, VersionedTransaction};
use thiserror::Error;
use utoipa::ToSchema;

#[derive(Debug, Error, Deserialize, Serialize)]
#[allow(clippy::enum_variant_names)]
pub enum SolanaEncodingError {
    #[error("Failed to serialize transaction: {0}")]
    Serialization(String),
    #[error("Failed to decode base64: {0}")]
    Decode(String),
    #[error("Failed to deserialize transaction: {0}")]
    Deserialize(String),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct EncodedSerializedTransaction(String);

impl EncodedSerializedTransaction {
    pub fn new(encoded: String) -> Self {
        Self(encoded)
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl TryFrom<&solana_sdk::transaction::Transaction> for EncodedSerializedTransaction {
    type Error = SolanaEncodingError;

    fn try_from(transaction: &Transaction) -> Result<Self, Self::Error> {
        let serialized = bincode::serialize(transaction)
            .map_err(|e| SolanaEncodingError::Serialization(e.to_string()))?;

        Ok(Self(STANDARD.encode(serialized)))
    }
}

impl TryFrom<EncodedSerializedTransaction> for solana_sdk::transaction::Transaction {
    type Error = SolanaEncodingError;

    fn try_from(encoded: EncodedSerializedTransaction) -> Result<Self, Self::Error> {
        let tx_bytes = STANDARD
            .decode(encoded.0)
            .map_err(|e| SolanaEncodingError::Decode(e.to_string()))?;

        let decoded_tx: Transaction = bincode::deserialize(&tx_bytes)
            .map_err(|e| SolanaEncodingError::Deserialize(e.to_string()))?;

        Ok(decoded_tx)
    }
}

// Implement conversion from versioned transaction
impl TryFrom<&VersionedTransaction> for EncodedSerializedTransaction {
    type Error = SolanaEncodingError;

    fn try_from(transaction: &VersionedTransaction) -> Result<Self, Self::Error> {
        let serialized = bincode::serialize(transaction)
            .map_err(|e| SolanaEncodingError::Serialization(e.to_string()))?;

        Ok(Self(STANDARD.encode(serialized)))
    }
}

// Implement conversion to versioned transaction
impl TryFrom<EncodedSerializedTransaction> for VersionedTransaction {
    type Error = SolanaEncodingError;

    fn try_from(encoded: EncodedSerializedTransaction) -> Result<Self, Self::Error> {
        let tx_bytes = STANDARD
            .decode(&encoded.0)
            .map_err(|e| SolanaEncodingError::Decode(e.to_string()))?;

        bincode::deserialize(&tx_bytes).map_err(|e| SolanaEncodingError::Deserialize(e.to_string()))
    }
}

// feeEstimate
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FeeEstimateRequestParams {
    pub transaction: EncodedSerializedTransaction,
    pub fee_token: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
pub struct FeeEstimateResult {
    pub estimated_fee: String,
    pub conversion_rate: String,
}

// transferTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferTransactionRequestParams {
    pub amount: u64,
    pub token: String,
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct TransferTransactionResult {
    pub transaction: EncodedSerializedTransaction,
    pub fee_in_spl: String,
    pub fee_in_lamports: String,
    pub fee_token: String,
    pub valid_until_blockheight: u64,
}

// prepareTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareTransactionRequestParams {
    pub transaction: EncodedSerializedTransaction,
    pub fee_token: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
pub struct PrepareTransactionResult {
    pub transaction: EncodedSerializedTransaction,
    pub fee_in_spl: String,
    pub fee_in_lamports: String,
    pub fee_token: String,
    pub valid_until_blockheight: u64,
}

// signTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SignTransactionRequestParams {
    pub transaction: EncodedSerializedTransaction,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, ToSchema)]
pub struct SignTransactionResult {
    pub transaction: EncodedSerializedTransaction,
    pub signature: String,
}

// signAndSendTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SignAndSendTransactionRequestParams {
    pub transaction: EncodedSerializedTransaction,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
pub struct SignAndSendTransactionResult {
    pub transaction: EncodedSerializedTransaction,
    pub signature: String,
}

// getSupportedTokens
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSupportedTokensRequestParams {}

#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
pub struct GetSupportedTokensItem {
    pub mint: String,
    pub symbol: String,
    pub decimals: u8,
    #[schema(nullable = false)]
    pub max_allowed_fee: Option<u64>,
    #[schema(nullable = false)]
    pub conversion_slippage_percentage: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
pub struct GetSupportedTokensResult {
    pub tokens: Vec<GetSupportedTokensItem>,
}

// getFeaturesEnabled
#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GetFeaturesEnabledRequestParams {}

#[derive(Debug, Deserialize, Serialize, PartialEq, ToSchema)]
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
    pub fn from_string(method: &str) -> Option<Self> {
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

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "method", content = "params")]
#[schema(as = SolanaRpcRequest)]
pub enum SolanaRpcRequest {
    #[serde(rename = "feeEstimate")]
    #[schema(example = "feeEstimate")]
    FeeEstimate(FeeEstimateRequestParams),
    #[serde(rename = "transferTransaction")]
    #[schema(example = "transferTransaction")]
    TransferTransaction(TransferTransactionRequestParams),
    #[serde(rename = "prepareTransaction")]
    #[schema(example = "prepareTransaction")]
    PrepareTransaction(PrepareTransactionRequestParams),
    #[serde(rename = "signTransaction")]
    #[schema(example = "signTransaction")]
    SignTransaction(SignTransactionRequestParams),
    #[serde(rename = "signAndSendTransaction")]
    #[schema(example = "signAndSendTransaction")]
    SignAndSendTransaction(SignAndSendTransactionRequestParams),
    #[serde(rename = "getSupportedTokens")]
    #[schema(example = "getSupportedTokens")]
    GetSupportedTokens(GetSupportedTokensRequestParams),
    #[serde(rename = "getFeaturesEnabled")]
    #[schema(example = "getFeaturesEnabled")]
    GetFeaturesEnabled(GetFeaturesEnabledRequestParams),
}

#[derive(Debug, Serialize, Deserialize, ToSchema, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        hash::Hash,
        message::Message,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
    };
    use solana_system_interface::instruction;

    fn create_test_transaction() -> Transaction {
        let payer = Keypair::new();

        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(
            &payer.pubkey(),
            &recipient,
            1000, // lamports
        );
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        Transaction::new(&[&payer], message, Hash::default())
    }

    #[test]
    fn test_transaction_to_encoded() {
        let transaction = create_test_transaction();

        let result = EncodedSerializedTransaction::try_from(&transaction);
        assert!(result.is_ok(), "Failed to encode transaction");

        let encoded = result.unwrap();
        assert!(
            !encoded.into_inner().is_empty(),
            "Encoded string should not be empty"
        );
    }

    #[test]
    fn test_encoded_to_transaction() {
        let original_tx = create_test_transaction();
        let encoded = EncodedSerializedTransaction::try_from(&original_tx).unwrap();

        let result = solana_sdk::transaction::Transaction::try_from(encoded);

        assert!(result.is_ok(), "Failed to decode transaction");
        let decoded_tx = result.unwrap();
        assert_eq!(
            original_tx.message.account_keys, decoded_tx.message.account_keys,
            "Account keys should match"
        );
        assert_eq!(
            original_tx.message.instructions, decoded_tx.message.instructions,
            "Instructions should match"
        );
    }

    #[test]
    fn test_invalid_base64_decode() {
        let invalid_encoded = EncodedSerializedTransaction("invalid base64".to_string());
        let result = Transaction::try_from(invalid_encoded);
        assert!(matches!(
            result.unwrap_err(),
            SolanaEncodingError::Decode(_)
        ));
    }

    #[test]
    fn test_invalid_transaction_deserialize() {
        // Create valid base64 but invalid transaction data
        let invalid_data = STANDARD.encode("not a transaction");
        let invalid_encoded = EncodedSerializedTransaction(invalid_data);

        let result = Transaction::try_from(invalid_encoded);
        assert!(matches!(
            result.unwrap_err(),
            SolanaEncodingError::Deserialize(_)
        ));
    }
}
