//! signAndSendTransaction RPC method implementation.
use std::str::FromStr;

use futures::try_join;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};

use crate::{
    models::{
        EncodedSerializedTransaction, SignAndSendTransactionRequestParams,
        SignAndSendTransactionResult,
    },
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::*;

impl<P, S, J> SolanaRpcMethodsImpl<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    /// Signs a prepared transaction and immediately submits it to the Solana blockchain.
    ///
    /// # Description
    ///
    /// This function combines the signing and submission steps into one operation. After validating
    /// and signing the provided transaction, it is immediately sent to the blockchain for
    /// execution. This is particularly useful when you want to reduce the number of
    /// client-server interactions.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded prepared transaction that needs to be signed and
    ///   submitted.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded signed transaction that has been submitted.
    /// * `signature` - Signature of the submitted transaction.
    pub(crate) async fn sign_and_send_transaction_impl(
        &self,
        params: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        validate_sign_and_send_transaction(&transaction_request, &self.relayer, &*self.provider)
            .await?;

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

        let (signed_transaction, _) = self.relayer_sign_transaction(transaction_request)?;

        let send_signature = self.provider.send_transaction(&signed_transaction).await?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        Ok(SignAndSendTransactionResult {
            transaction: serialized_transaction,
            signature: send_signature.to_string(),
        })
    }
}

async fn validate_sign_and_send_transaction<P: SolanaProviderTrait + Send + Sync>(
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
    use super::*;
    use mockall::predicate::{self};
    use solana_sdk::{message::Message, signature::Signature, system_instruction};

    #[tokio::test]
    async fn test_sign_and_send_transaction_success() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let expected_signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

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
    async fn test_sign_and_send_transaction_invalid_blockhash() {
        let (relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc
            .sign_and_send_transaction_impl(SignAndSendTransactionRequestParams {
                transaction: encoded_tx,
            })
            .await;

        assert!(matches!(
            result,
            Err(SolanaRpcError::SolanaTransactionValidation(_))
        ));
    }

    #[tokio::test]
    async fn test_sign_and_send_transaction_simulation_failure() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let expected_signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

        provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async { Err(SolanaProviderError::RpcError("Simulate error".to_string())) })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc
            .sign_and_send_transaction_impl(SignAndSendTransactionRequestParams {
                transaction: encoded_tx,
            })
            .await;
        assert!(matches!(
            result,
            Err(SolanaRpcError::SolanaTransactionValidation(_))
        ));
    }

    #[tokio::test]
    async fn test_sign_and_send_transaction_with_lamports_outflow() {
        let (relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();

        // Create transaction with lamports transfer
        let recipient = Pubkey::new_unique();
        let transfer_amount = 1_000_000;
        let ix = system_instruction::transfer(
            &Pubkey::from_str(&relayer.address).unwrap(),
            &recipient,
            transfer_amount,
        );

        let message = Message::new(&[ix], Some(&Pubkey::from_str(&relayer.address).unwrap()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        let expected_signature = Signature::new_unique();
        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

        provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(10_000_000) }));

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

        let result = rpc
            .sign_and_send_transaction_impl(SignAndSendTransactionRequestParams {
                transaction: encoded_tx,
            })
            .await;

        assert!(result.is_ok());
        let send_result = result.unwrap();
        assert_eq!(send_result.signature, expected_signature.to_string());
    }

    #[tokio::test]
    async fn test_sign_and_send_transaction_with_lamports_outflow_fail() {
        let (relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();

        // Create transaction with lamports transfer
        let recipient = Pubkey::new_unique();
        let transfer_amount = 10_000_000;
        let ix = system_instruction::transfer(
            &Pubkey::from_str(&relayer.address).unwrap(),
            &recipient,
            transfer_amount,
        );

        let message = Message::new(&[ix], Some(&Pubkey::from_str(&relayer.address).unwrap()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        let expected_signature = Signature::new_unique();
        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

        provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(10_000_000) }));

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

        let result = rpc
            .sign_and_send_transaction_impl(SignAndSendTransactionRequestParams {
                transaction: encoded_tx,
            })
            .await;

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
}
