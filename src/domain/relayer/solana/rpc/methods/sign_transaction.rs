//! signTransaction RPC method implementation.
use std::str::FromStr;

use futures::try_join;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};

use crate::{
    models::{EncodedSerializedTransaction, SignTransactionRequestParams, SignTransactionResult},
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::*;

impl<P, S, J> SolanaRpcMethodsImpl<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    /// Signs a prepared transaction without submitting it to the blockchain.
    ///
    /// # Description
    ///
    /// This function is used to sign a prepared transaction (one that may have been modified by the
    /// relayer) to ensure its validity and authorization before submission. It returns the
    /// signed transaction along with the corresponding signature.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded prepared transaction that requires signing.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded signed transaction.
    /// * `signature` - Signature of the submitted transaction.
    pub(crate) async fn sign_transaction_impl(
        &self,
        params: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        validate_sign_transaction(&transaction_request, &self.relayer, &*self.provider).await?;

        let total_fee = self
            .estimate_fee_payer_total_fee(&transaction_request)
            .await?;
        let lamports_outflow = self
            .estimate_relayer_lampart_outflow(&transaction_request)
            .await?;
        let total_outflow = total_fee + lamports_outflow;

        // Validate relayer has sufficient balance
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_outflow,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await?;

        let (signed_transaction, signature) = self.relayer_sign_transaction(transaction_request)?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        Ok(SignTransactionResult {
            transaction: serialized_transaction,
            signature: signature.to_string(),
        })
    }
}

async fn validate_sign_transaction<P: SolanaProviderTrait + Send + Sync>(
    tx: &Transaction,
    relayer: &RelayerRepoModel,
    provider: &P,
) -> Result<(), SolanaTransactionValidationError> {
    let policy = &relayer.policies.get_solana_policy();
    let relayer_pubkey = Pubkey::from_str(&relayer.address).map_err(|e| {
        SolanaTransactionValidationError::ValidationError(format!("Invalid relayer address: {}", e))
    })?;

    let sync_validations = async {
        SolanaTransactionValidator::validate_tx_allowed_accounts(tx, policy)?;
        SolanaTransactionValidator::validate_tx_disallowed_accounts(tx, policy)?;
        SolanaTransactionValidator::validate_allowed_programs(tx, policy)?;
        SolanaTransactionValidator::validate_max_signatures(tx, policy)?;
        SolanaTransactionValidator::validate_fee_payer(tx, &relayer_pubkey)?;
        SolanaTransactionValidator::validate_data_size(tx, policy)?;
        Ok::<(), SolanaTransactionValidationError>(())
    };

    // Run all validations concurrently.
    try_join!(
        sync_validations,
        SolanaTransactionValidator::validate_blockhash(tx, provider),
        SolanaTransactionValidator::simulate_transaction(tx, provider),
        SolanaTransactionValidator::validate_lamports_transfers(tx, policy, &relayer_pubkey),
        SolanaTransactionValidator::validate_token_transfers(tx, policy, provider, &relayer_pubkey,),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::models::{RelayerNetworkPolicy, RelayerSolanaPolicy};

    use super::*;
    use mockall::predicate::{self};
    use solana_sdk::{
        hash::Hash,
        message::Message,
        signature::{Keypair, Signature},
        signer::Signer,
        system_instruction,
    };

    #[tokio::test]
    async fn test_create_and_sign_transaction_success() {
        let (relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();
        let expected_signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        // Create test instructions
        let test_instruction =
            system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);

        let result = rpc
            .create_and_sign_transaction(vec![test_instruction])
            .await;

        assert!(result.is_ok());
        let (transaction, (_, slot)) = result.unwrap();

        assert_eq!(transaction.message.instructions.len(), 1);
        assert_eq!(slot, 100);
        assert!(!transaction.signatures.is_empty());
    }

    #[tokio::test]
    async fn test_create_and_sign_transaction_provider_error() {
        let (relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Mock provider error
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| {
                Box::pin(async {
                    Err(crate::services::SolanaProviderError::RpcError(
                        "Test error".to_string(),
                    ))
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let test_instruction =
            system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);

        let result = rpc
            .create_and_sign_transaction(vec![test_instruction])
            .await;

        assert!(matches!(result, Err(SolanaRpcError::Provider(_))));
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

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
    async fn test_sign_transaction_balance_failure() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000) }));

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

        assert!(result.is_err());

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains("Insufficient funds:"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
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
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx) =
            setup_test_context();

        // Update policy with disallowed accounts
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![Pubkey::new_unique().to_string()]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

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
}
