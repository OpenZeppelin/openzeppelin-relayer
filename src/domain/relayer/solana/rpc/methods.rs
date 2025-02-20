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

#[cfg(test)]
use crate::services::{MockSolanaProviderTrait, MockSolanaSignTrait};

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

// Modified implementation with constrained generics
pub struct SolanaRpcMethodsImpl<P = DefaultProvider, S = DefaultSigner> {
    relayer: RelayerRepoModel,
    provider: Arc<P>,
    signer: Arc<S>,
}

// Default implementation for production use
impl SolanaRpcMethodsImpl<DefaultProvider, DefaultSigner> {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<DefaultProvider>,
        signer: Arc<DefaultSigner>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
        }
    }
}

#[cfg(test)]
impl SolanaRpcMethodsImpl<MockSolanaProviderTrait, MockSolanaSignTrait> {
    pub fn new_mock(
        relayer: RelayerRepoModel,
        provider: Arc<MockSolanaProviderTrait>,
        signer: Arc<MockSolanaSignTrait>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
        }
    }
}

#[async_trait]
impl<P, S> SolanaRpcMethods for SolanaRpcMethodsImpl<P, S>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
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
        services::{MockSolanaProviderTrait, MockSolanaSignTrait},
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
        Arc<MockSolanaSignTrait>,
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

        (relayer, Arc::new(mock_signer), encoded_tx)
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let (relayer, signer, encoded_tx) = setup_test_context();

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        // mock simulate_transaction
        mock_provider.expect_simulate_transaction().returning(|_| {
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

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (relayer, signer, encoded_tx) = setup_test_context();

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_signatures() {
        let (mut relayer, signer, encoded_tx) = setup_test_context();

        // Update policy with low max signatures
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_signatures: Some(0),
            ..Default::default()
        });

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock_provider.expect_simulate_transaction().returning(|_| {
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

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (mut relayer, signer, encoded_tx) = setup_test_context();

        // Update policy with disallowed programs
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_programs: Some(vec!["different_program".to_string()]),
            ..Default::default()
        });

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock_provider.expect_simulate_transaction().returning(|_| {
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

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (mut relayer, signer, encoded_tx) = setup_test_context();

        // Update policy with small max data size
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_tx_data_size: 10,
            ..Default::default()
        });

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock_provider.expect_simulate_transaction().returning(|_| {
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

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (relayer, signer, _) = setup_test_context();

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&wrong_fee_payer.pubkey())); // Different fee payer
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (mut relayer, signer, encoded_tx) = setup_test_context();

        // Update policy with disallowed accounts
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![Pubkey::new_unique().to_string()]),
            ..Default::default()
        });

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock_provider.expect_simulate_transaction().returning(|_| {
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
        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        // This should pass since our test transaction doesn't use disallowed accounts
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_lamports_transfer() {
        let (mut relayer, signer, _) = setup_test_context();

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

        let mut mock_provider = MockSolanaProviderTrait::new();
        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock_provider.expect_simulate_transaction().returning(|_| {
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

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (relayer, signer, encoded_tx) = setup_test_context();

        let mut mock_provider = MockSolanaProviderTrait::new();
        let expected_signature = Signature::new_unique();

        mock_provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock_provider.expect_simulate_transaction().returning(|_| {
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

        mock_provider
            .expect_send_transaction()
            .returning(move |_| Box::pin(async move { Ok(expected_signature) }));

        let provider = Arc::new(mock_provider);
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
        let (mut relayer, signer, _) = setup_test_context();

        // Update relayer policy with some tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![
                SolanaAllowedTokensPolicy {
                    mint: "mint1".to_string(),
                    symbol: Some("TOKEN1".to_string()),
                    decimals: Some(9),
                    max_allowed_fee: Some(1000),
                },
                SolanaAllowedTokensPolicy {
                    mint: "mint2".to_string(),
                    symbol: Some("TOKEN2".to_string()),
                    decimals: Some(6),
                    max_allowed_fee: None,
                },
            ]),
            ..Default::default()
        });

        let provider = Arc::new(MockSolanaProviderTrait::new());
        let rpc = SolanaRpcMethodsImpl::new_mock(relayer, provider, signer);

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
}
