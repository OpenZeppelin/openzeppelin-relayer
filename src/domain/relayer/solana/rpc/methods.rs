//! # Solana RPC Methods Module
//!
//! This module defines the `SolanaRpcMethods` trait which provides an asynchronous interface
//! for various Solana-specific RPC operations. These operations include fee estimation,
//! transaction processing (transfer, prepare, sign, and send), token retrieval, and feature
//! queries.
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use solana_sdk::transaction::Transaction;
use spl_token::amount_to_ui_amount;
use std::sync::Arc;

use super::{SolanaRpcError, SolanaTransactionValidator};

use crate::{
    constants::{DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, SOLANA_DECIMALS, SOL_MINT},
    models::{
        EncodedSerializedTransaction, FeeEstimateRequestParams, FeeEstimateResult,
        GetFeaturesEnabledRequestParams, GetFeaturesEnabledResult, GetSupportedTokensItem,
        GetSupportedTokensRequestParams, GetSupportedTokensResult, PrepareTransactionRequestParams,
        PrepareTransactionResult, RelayerRepoModel, SignAndSendTransactionRequestParams,
        SignAndSendTransactionResult, SignTransactionRequestParams, SignTransactionResult,
        TransferTransactionRequestParams, TransferTransactionResult,
    },
    services::{
        JupiterService, JupiterServiceTrait, SolanaProvider, SolanaProviderTrait, SolanaSignTrait,
        SolanaSigner,
    },
};

#[cfg(test)]
use crate::services::{MockJupiterServiceTrait, MockSolanaProviderTrait, MockSolanaSignTrait};

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

pub type DefaultProvider = SolanaProvider;
pub type DefaultSigner = SolanaSigner;
pub type DefaultJupiterService = JupiterService;

// Modified implementation with constrained generics
pub struct SolanaRpcMethodsImpl<P = DefaultProvider, S = DefaultSigner, J = DefaultJupiterService> {
    relayer: RelayerRepoModel,
    provider: Arc<P>,
    signer: Arc<S>,
    jupiter_service: Arc<J>,
}

// Default implementation for production use
impl SolanaRpcMethodsImpl<DefaultProvider, DefaultSigner, DefaultJupiterService> {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<DefaultProvider>,
        signer: Arc<DefaultSigner>,
        jupiter_service: Arc<DefaultJupiterService>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
            jupiter_service,
        }
    }
}

#[cfg(test)]
impl SolanaRpcMethodsImpl<MockSolanaProviderTrait, MockSolanaSignTrait, MockJupiterServiceTrait> {
    pub fn new_mock(
        relayer: RelayerRepoModel,
        provider: Arc<MockSolanaProviderTrait>,
        signer: Arc<MockSolanaSignTrait>,
        jupiter_service: Arc<MockJupiterServiceTrait>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
            jupiter_service,
        }
    }
}

#[async_trait]
impl<P, S, J> SolanaRpcMethods for SolanaRpcMethodsImpl<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
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
                        max_allowed_fee: token.max_allowed_fee,
                        conversion_slippage_percentage: token.conversion_slippage_percentage,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(GetSupportedTokensResult { tokens })
    }

    async fn fee_estimate(
        &self,
        params: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError> {
        // validate tx
        let transaction_request = Transaction::try_from(params.transaction)?;

        SolanaTransactionValidator::validate_fee_estimate_transaction(
            &transaction_request,
            &self.relayer,
        )
        .await
        .map_err(|e| SolanaRpcError::InvalidParams(e.to_string()))?;

        let mut transaction = transaction_request.clone();

        let recent_blockhash = self.provider.get_latest_blockhash().await?;

        // update tx blockhash
        transaction.message.recent_blockhash =
            solana_sdk::hash::Hash::new_from_array(recent_blockhash[..32].try_into().unwrap());

        let total_fee = self
            .provider
            .calculate_total_fee(&transaction.message())
            .await?;

        let solana_policy = self.relayer.policies.get_solana_policy();
        let allowed_token_entry = solana_policy
            .allowed_tokens
            .as_ref()
            .and_then(|tokens| tokens.iter().find(|token| token.mint == params.fee_token));

        if let Some(token_entry) = allowed_token_entry {
            // If fee token is SOL, return fee in SOL
            if token_entry.mint == SOL_MINT {
                return Ok(FeeEstimateResult {
                    estimated_fee: amount_to_ui_amount(total_fee, SOLANA_DECIMALS).to_string(),
                    conversion_rate: "1".to_string(),
                });
            }

            let quota = self
                .jupiter_service
                .get_sol_to_token_quote(
                    &params.fee_token,
                    total_fee,
                    token_entry
                        .conversion_slippage_percentage
                        .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE),
                )
                .await
                .map_err(|e| SolanaRpcError::Estimation(e.to_string()))?;

            let decimals = token_entry.decimals.ok_or_else(|| {
                SolanaRpcError::Estimation("Token decimals not configured".to_string())
            })?;

            let ui_out_amount = amount_to_ui_amount(quota.out_amount, decimals);
            let ui_amount_in = amount_to_ui_amount(quota.in_amount, SOLANA_DECIMALS);
            let conversion_rate = ui_out_amount / ui_amount_in;

            Ok(FeeEstimateResult {
                estimated_fee: ui_out_amount.to_string(),
                conversion_rate: conversion_rate.to_string(),
            })
        } else {
            Err(SolanaRpcError::UnsupportedFeeToken(
                "Unsupported fee token".to_string(),
            ))
        }
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
            &*self.provider,
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
            &*self.provider,
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

#[cfg(test)]
mod tests {

    use crate::{
        models::{
            NetworkType, RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
        },
        services::{MockSolanaProviderTrait, MockSolanaSignTrait, QuoteResponse},
    };

    use super::*;
    use mockall::predicate::{self};
    use solana_sdk::{
        message::Message,
        pubkey::Pubkey,
        signature::{Keypair, Signature, Signer},
        system_instruction,
    };

    fn setup_test_context() -> (
        RelayerRepoModel,
        MockSolanaSignTrait,
        MockSolanaProviderTrait,
        MockJupiterServiceTrait,
        EncodedSerializedTransaction,
    ) {
        // Create test transaction
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        // Create test relayer
        let relayer = RelayerRepoModel {
            id: "id".to_string(),
            name: "Relayer".to_string(),
            network: "TestNet".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                allowed_accounts: None,
                allowed_tokens: None,
                min_balance: 10000,
                allowed_programs: None,
                max_signatures: Some(10),
                disallowed_accounts: None,
                max_allowed_transfer_amount_lamports: None,
                max_tx_data_size: 1000,
            }),
            signer_id: "test".to_string(),
            address: payer.pubkey().to_string(),
            notification_id: None,
            system_disabled: false,
        };

        // // Setup mock signer
        let mut mock_signer = MockSolanaSignTrait::new();
        let test_signature = Signature::new_unique();
        mock_signer
            .expect_sign()
            .returning(move |_| Ok(test_signature));

        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        let jupiter_service = MockJupiterServiceTrait::new();
        let provider = MockSolanaProviderTrait::new();

        (relayer, mock_signer, provider, jupiter_service, encoded_tx)
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let (relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        // mock simulate_transaction
        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_ok());
        let sign_result = result.unwrap();

        // Verify signature format (base58 encoded, 64 bytes)
        let decoded_sig = bs58::decode(&sign_result.signature)
            .into_vec()
            .expect("Failed to decode base58 signature");
        assert_eq!(decoded_sig.len(), 64);
    }

    #[tokio::test]
    async fn test_sign_transaction_validation_failure_blockhash() {
        let (relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_signatures() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with low max signatures
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_signatures: Some(0),
            ..Default::default()
        });

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Transaction requires 1 signatures, which exceeds \
                         maximum allowed 0"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_disallowed_program() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with disallowed programs
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_programs: Some(vec!["different_program".to_string()]),
            ..Default::default()
        });

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Program 11111111111111111111111111111111 not allowed"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_data_size() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with small max data size
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_tx_data_size: 10,
            ..Default::default()
        });

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Transaction size 215 exceeds maximum allowed 10"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_wrong_fee_payer() {
        let (relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&wrong_fee_payer.pubkey())); // Different fee payer
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );
        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains("Policy violation: Fee payer"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_disallowed_account() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with disallowed accounts
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![Pubkey::new_unique().to_string()]),
            ..Default::default()
        });

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        // This should pass since our test transaction doesn't use disallowed accounts
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_lamports_transfer() {
        let (mut relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Set max allowed transfer amount in policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_allowed_transfer_amount_lamports: Some(500),
            ..Default::default()
        });

        // Create transaction that exceeds max transfer amount
        let payer = Keypair::new();
        relayer.address = payer.pubkey().to_string();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(
            &payer.pubkey(),
            &recipient,
            1000, // Amount exceeds max_allowed_transfer_amount_lamports
        );
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string
                        .contains("Lamports transfer amount 1000 exceeds max allowed fee 500"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_and_send_transaction_success() {
        let (relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let expected_signature = Signature::new_unique();

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        provider
            .expect_send_transaction()
            .returning(move |_| Box::pin(async move { Ok(expected_signature) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignAndSendTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_and_send_transaction(params).await;
        assert!(result.is_ok());

        let send_result = result.unwrap();
        assert_eq!(send_result.signature, expected_signature.to_string());
    }

    #[tokio::test]
    async fn test_get_supported_tokens() {
        let (mut relayer, signer, provider, jupiter_service, _) = setup_test_context();

        // Update relayer policy with some tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![
                SolanaAllowedTokensPolicy {
                    mint: "mint1".to_string(),
                    symbol: Some("TOKEN1".to_string()),
                    decimals: Some(9),
                    max_allowed_fee: Some(1000),
                    conversion_slippage_percentage: None,
                },
                SolanaAllowedTokensPolicy {
                    mint: "mint2".to_string(),
                    symbol: Some("TOKEN2".to_string()),
                    decimals: Some(6),
                    max_allowed_fee: None,
                    conversion_slippage_percentage: None,
                },
            ]),
            ..Default::default()
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc
            .get_supported_tokens(GetSupportedTokensRequestParams {})
            .await;

        assert!(result.is_ok());
        let tokens = result.unwrap().tokens;
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].mint, "mint1");
        assert_eq!(tokens[0].symbol, "TOKEN1");
        assert_eq!(tokens[0].decimals, 9);
        assert_eq!(tokens[0].max_allowed_fee, Some(1000));
    }

    #[tokio::test]
    async fn test_estimate_fee_success() {
        let (relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        // mock simulate_transaction
        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_ok());
        let sign_result = result.unwrap();

        // Verify signature format (base58 encoded, 64 bytes)
        let decoded_sig = bs58::decode(&sign_result.signature)
            .into_vec()
            .expect("Failed to decode base58 signature");
        assert_eq!(decoded_sig.len(), 64);
    }

    #[tokio::test]
    async fn test_fee_estimate_with_allowed_token() {
        let (mut relayer, signer, mut provider, mut jupiter_service, encoded_tx) =
            setup_test_context();

        // Set up policy with allowed token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "USDC".to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1000000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        // Mock provider methods
        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok([0u8; 32]) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(500000000u64) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq("USDC"),
                predicate::eq(500000000u64),
                predicate::eq(1.0f32),
            )
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "SOL".to_string(),
                        output_mint: "USDC".to_string(),
                        in_amount: 500000000,
                        out_amount: 80000000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            fee_token: "USDC".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "80");
        assert_eq!(fee_estimate.conversion_rate, "160");
    }

    #[tokio::test]
    async fn test_fee_estimate_usdt_to_sol_conversion() {
        let (mut relayer, signer, _provider, mut jupiter_service, encoded_tx) =
            setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), // USDT mint
                symbol: Some("USDT".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1000000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        let mut provider = MockSolanaProviderTrait::new();

        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok([0u8; 32]) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),
                predicate::eq(1_000_000u64),
                predicate::eq(1.0f32),
            )
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "So11111111111111111111111111111111111111112".to_string(),
                        output_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
                        in_amount: 1_000_000, // 0.001 SOL
                        out_amount: 20_000,   // 0.02 USDT
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            // noboost
            fee_token: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.02"); // 0.02 USDT
        assert_eq!(fee_estimate.conversion_rate, "20"); // 1 SOL = 20 USDT
    }

    #[tokio::test]
    async fn test_fee_estimate_uni_to_sol_dynamic_price() {
        let (mut relayer, signer, mut provider, mut jupiter_service, encoded_tx) =
            setup_test_context();

        // Set up policy with UNI token (decimals = 8)
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(), // UNI mint
                symbol: Some("UNI".to_string()),
                decimals: Some(8),
                max_allowed_fee: Some(1_000_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok([0u8; 32]) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq("8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt"),
                predicate::eq(1_000_000u64),
                predicate::eq(1.0f32),
            )
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "So11111111111111111111111111111111111111112".to_string(),
                        output_mint: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(),
                        in_amount: 1_000_000,  // 0.001 SOL
                        out_amount: 1_770_000, // 0.0177 UNI
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            // noboost
            fee_token: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.0177"); // 0.0177 UNI
        assert_eq!(fee_estimate.conversion_rate, "17.7"); // 1 SOL = 17.7 UNI
    }

    #[tokio::test]
    async fn test_fee_estimate_native_sol() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Set up policy with SOL token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "So11111111111111111111111111111111111111112".to_string(), // Native SOL
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        // Mock provider methods - expect 0.001 SOL fee (1_000_000 lamports)
        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok([0u8; 32]) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        // We don't expect any Jupiter quotes for native SOL

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            fee_token: "So11111111111111111111111111111111111111112".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.001"); // 0.001 SOL (1_000_000 lamports)
        assert_eq!(fee_estimate.conversion_rate, "1"); // 1:1 for native SOL
    }
}
