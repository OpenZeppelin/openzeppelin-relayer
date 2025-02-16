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
            transaction: EncodedSerializedTransaction::new("".to_string()),
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
            transaction: EncodedSerializedTransaction::new("".to_string()),
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

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use mockall::predicate::*;
//     use solana_sdk::{
//         message::Message,
//         pubkey::Pubkey,
//         signature::{Keypair, Signature, Signer},
//         system_instruction,
//     };

//     fn setup_test_context() -> (
//         RelayerRepoModel,
//         Arc<MockSolanaProvider>,
//         Arc<MockSolanaSigner>,
//         EncodedSerializedTransaction,
//     ) {
//         // Create test transaction
//         let payer = Keypair::new();
//         let recipient = Pubkey::new_unique();
//         let ix = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
//         let message = Message::new(&[ix], Some(&payer.pubkey()));
//         let transaction = Transaction::new_unsigned(message);

//         // Create test relayer
//         let relayer = RelayerRepoModel {
//             id: "test".to_string(),
//             address: payer.pubkey().to_string(),
//             policies: Default::default(),
//             ..Default::default()
//         };

//         // Setup mock provider
//         let mut mock_provider = MockSolanaProvider::new();
//         mock_provider
//             .expect_simulate_transaction()
//             .returning(|_|
// Ok(solana_client::rpc_response::RpcSimulateTransactionResult::default()));

//         // Setup mock signer
//         let mut mock_signer = MockSolanaSigner::new();
//         let test_signature = Signature::new(&[1u8; 64]);
//         mock_signer
//             .expect_sign()
//             .returning(move |_| Ok(test_signature));

//         let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
//             .expect("Failed to encode transaction");

//         (
//             relayer,
//             Arc::new(mock_provider),
//             Arc::new(mock_signer),
//             encoded_tx,
//         )
//     }

//     #[tokio::test]
//     async fn test_sign_transaction_success() {
//         let (relayer, provider, signer, encoded_tx) = setup_test_context();

//         let rpc = SolanaRpcMethodsImpl::new(relayer, provider, signer);

//         let params = SignTransactionRequestParams {
//             transaction: encoded_tx,
//         };

//         let result = rpc.sign_transaction(params).await;

//         assert!(result.is_ok());
//         let sign_result = result.unwrap();

//         // Verify signature format (base58 encoded, 64 bytes)
//         let decoded_sig = bs58::decode(&sign_result.signature)
//             .into_vec()
//             .expect("Failed to decode signature");
//         assert_eq!(decoded_sig.len(), 64);
//     }

//     #[tokio::test]
//     async fn test_sign_transaction_validation_failure() {
//         let (relayer, mut provider, signer, encoded_tx) = setup_test_context();

//         // Mock validation failure
//         provider
//             .expect_simulate_transaction()
//             .returning(|_| Err("Validation failed".to_string()));

//         let rpc = SolanaRpcMethodsImpl::new(relayer, Arc::new(provider), signer);

//         let params = SignTransactionRequestParams {
//             transaction: encoded_tx,
//         };

//         let result = rpc.sign_transaction(params).await;
//         assert!(matches!(result, Err(SolanaRpcError::ValidationError(_))));
//     }

//     #[tokio::test]
//     async fn test_sign_transaction_signing_failure() {
//         let (relayer, provider, mut signer, encoded_tx) = setup_test_context();

//         // Mock signing failure
//         signer
//             .expect_sign()
//             .returning(|_| Err("Signing failed".to_string()));

//         let rpc = SolanaRpcMethodsImpl::new(relayer, provider, Arc::new(signer));

//         let params = SignTransactionRequestParams {
//             transaction: encoded_tx,
//         };

//         let result = rpc.sign_transaction(params).await;
//         assert!(matches!(result, Err(SolanaRpcError::SigningError(_))));
//     }
// }
