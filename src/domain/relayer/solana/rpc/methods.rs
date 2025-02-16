//! # Solana RPC Methods Module
//!
//! This module defines the `SolanaRpcMethods` trait which provides an asynchronous interface
//! for various Solana-specific RPC operations. These operations include fee estimation,
//! transaction processing (transfer, prepare, sign, and send), token retrieval, and feature
//! queries.
use std::sync::Arc;

use async_trait::async_trait;
use solana_sdk::transaction::Transaction;

use super::{SolanaRpcError, SolanaTransactionValidator};
#[cfg(test)]
use mockall::automock;

use crate::{
    models::{
        EncodedSerializedTransaction, FeeEstimateRequestParams, FeeEstimateResult,
        GetFeaturesEnabledRequestParams, GetFeaturesEnabledResult, GetSupportedTokensItem,
        GetSupportedTokensRequestParams, GetSupportedTokensResult, PrepareTransactionRequestParams,
        PrepareTransactionResult, RelayerRepoModel, SignAndSendTransactionRequestParams,
        SignAndSendTransactionResult, SignTransactionRequestParams, SignTransactionResult,
        TransferTransactionRequestParams, TransferTransactionResult,
    },
    services::{SolanaProvider, SolanaProviderTrait, SolanaSignTrait, SolanaSigner},
};

#[cfg_attr(test, automock)]
#[async_trait]
pub trait SolanaRpcMethods: Send + Sync {
    async fn fee_estimate(
        &self,
        request: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError>;
    async fn transfer_transaction(
        &self,
        request: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError>;
    async fn prepare_transaction(
        &self,
        request: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError>;
    async fn sign_transaction(
        &self,
        request: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError>;
    async fn sign_and_send_transaction(
        &self,
        request: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError>;
    async fn get_supported_tokens(
        &self,
        request: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError>;
    async fn get_features_enabled(
        &self,
        request: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError>;
}

#[allow(dead_code)]
pub struct SolanaRpcMethodsImpl {
    relayer: RelayerRepoModel,
    provider: Arc<SolanaProvider>,
    signer: Arc<SolanaSigner>,
}

impl SolanaRpcMethodsImpl {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<SolanaProvider>,
        signer: Arc<SolanaSigner>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
        }
    }
}

#[async_trait]
impl SolanaRpcMethods for SolanaRpcMethodsImpl {
    /// Retrieves the supported tokens.
    async fn get_supported_tokens(
        &self,
        _params: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError> {
        let tokens = self
            .relayer
            .policies
            .get_solana_policy()
            .allowed_tokens
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|token| GetSupportedTokensItem {
                        mint: token.mint.clone(),
                        symbol: token.symbol.as_deref().unwrap_or("").to_string(),
                        decimals: token.decimals.unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(GetSupportedTokensResult { tokens })
    }

    async fn fee_estimate(
        &self,
        _params: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError> {
        // Implementation
        Ok(FeeEstimateResult {
            estimated_fee: "0".to_string(),
            conversion_rate: "0".to_string(),
        })
    }

    async fn transfer_transaction(
        &self,
        _params: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError> {
        // Implementation
        Ok(TransferTransactionResult {
            transaction: "".to_string(),
            fee_in_spl: "0".to_string(),
            fee_in_lamports: "0".to_string(),
            fee_token: "".to_string(),
            valid_until_blockheight: 0,
        })
    }

    async fn prepare_transaction(
        &self,
        _params: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError> {
        // Implementation
        Ok(PrepareTransactionResult {
            transaction: "".to_string(),
            fee_in_spl: "0".to_string(),
            fee_in_lamports: "0".to_string(),
            fee_token: "".to_string(),
            valid_until_blockheight: 0,
        })
    }

    /// Signs a Solana transaction using the relayer's signer.
    async fn sign_transaction(
        &self,
        params: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        SolanaTransactionValidator::validate_sign_transaction(
            &transaction_request,
            &self.relayer,
            &self.provider,
        )
        .await?;

        let mut transaction = transaction_request.clone();

        let signature = self.signer.sign(&transaction.message_data())?;

        transaction.signatures[0] = signature;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&transaction)?;

        Ok(SignTransactionResult {
            transaction: serialized_transaction,
            signature: signature.to_string(),
        })
    }

    /// Signs a Solana transaction using the relayer's signer and sends it to network.
    async fn sign_and_send_transaction(
        &self,
        params: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        SolanaTransactionValidator::validate_sign_transaction(
            &transaction_request,
            &self.relayer,
            &self.provider,
        )
        .await?;

        let mut transaction = transaction_request.clone();

        let signature = self.signer.sign(&transaction.message_data())?;

        transaction.signatures[0] = signature;

        let send_signature = self.provider.send_transaction(&transaction).await?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&transaction)?;

        Ok(SignAndSendTransactionResult {
            transaction: serialized_transaction,
            signature: send_signature.to_string(),
        })
    }

    async fn get_features_enabled(
        &self,
        _params: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError> {
        // gasless is enabled out of the box to be compliant with the spec
        Ok(GetFeaturesEnabledResult {
            features: vec!["gasless".to_string()],
        })
    }
}
