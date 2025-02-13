use serde::{Deserialize, Serialize};

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
    pub transaction: usize,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignTransactionResult {
    pub transaction: String,
    pub signature: String,
}

// signAndSendTransaction
#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignAndSendTransactionRequestParams {
    pub transaction: usize,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SignAndSendTransactionResult {
    pub transaction: String,
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
