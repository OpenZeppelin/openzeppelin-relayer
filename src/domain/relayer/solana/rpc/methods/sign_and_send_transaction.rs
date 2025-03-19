//! Signs a prepared transaction and immediately submits it to the Solana blockchain.
//!
//! # Description
//!
//! This function combines the signing and submission steps into one operation. After validating
//! and signing the provided transaction, it is immediately sent to the blockchain for
//! execution. This is particularly useful when you want to reduce the number of
//! client-server interactions.
//!
//! # Parameters
//!
//! * `transaction` - A Base64-encoded prepared transaction that needs to be signed and submitted.
//!
//! # Returns
//!
//! On success, returns a tuple containing:
//!
//! * `transaction` - A Base64-encoded signed transaction that has been submitted.
//! * `signature` - Signature of the submitted transaction.
use std::str::FromStr;

use futures::try_join;
use log::info;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};

use crate::{
    models::{
        produce_solana_rpc_webhook_payload, EncodedSerializedTransaction,
        SignAndSendTransactionRequestParams, SignAndSendTransactionResult, SolanaWebhookRpcPayload,
    },
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::*;

impl<P, S, J, JP> SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
{
    pub(crate) async fn sign_and_send_transaction_impl(
        &self,
        params: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError> {
        info!("Processing sign and send transaction request");
        let transaction_request = Transaction::try_from(params.transaction)?;

        validate_sign_and_send_transaction(&transaction_request, &self.relayer, &*self.provider)
            .await?;

        let total_fee = self
            .estimate_fee_payer_total_fee(&transaction_request)
            .await
            .map_err(|e| {
                error!("Failed to estimate total fee: {}", e);
                SolanaRpcError::Estimation(e.to_string())
            })?;

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
        .await
        .map_err(|e| {
            error!("Insufficient funds: {}", e);
            SolanaRpcError::InsufficientFunds(e.to_string())
        })?;

        let (signed_transaction, _) = self.relayer_sign_transaction(transaction_request).await?;

        let send_signature = self
            .provider
            .send_transaction(&signed_transaction)
            .await
            .map_err(|e| {
                error!("Failed to send transaction: {}", e);
                SolanaRpcError::Send(e.to_string())
            })?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        let result = SignAndSendTransactionResult {
            transaction: serialized_transaction,
            signature: send_signature.to_string(),
        };

        if let Some(notification_id) = &self.relayer.notification_id {
            let webhook_result = self
                .job_producer
                .produce_send_notification_job(
                    produce_solana_rpc_webhook_payload(
                        notification_id,
                        "sign_and_send_transaction".to_string(),
                        SolanaWebhookRpcPayload::SignAndSendTransaction(result.clone()),
                    ),
                    None,
                )
                .await;

            if let Err(e) = webhook_result {
                error!("Failed to produce notification job: {}", e);
            }
        }
        info!(
            "Transaction signed and sent successfully with signature: {}",
            result.signature
        );
        Ok(result)
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
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        let expected_signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature = expected_signature;
            Box::pin(async move { Ok(signature) })
        });

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
            Arc::new(job_producer),
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
        let (relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
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
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        let expected_signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature = expected_signature;
            Box::pin(async move { Ok(signature) })
        });

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
            Arc::new(job_producer),
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
        let (relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

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
        signer.expect_sign().returning(move |_| {
            let signature = expected_signature;
            Box::pin(async move { Ok(signature) })
        });

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
            Arc::new(job_producer),
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
        let (relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

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
        signer.expect_sign().returning(move |_| {
            let signature = expected_signature;
            Box::pin(async move { Ok(signature) })
        });

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
            Arc::new(job_producer),
        );

        let result = rpc
            .sign_and_send_transaction_impl(SignAndSendTransactionRequestParams {
                transaction: encoded_tx,
            })
            .await;

        assert!(result.is_err());
        match result {
            Err(SolanaRpcError::InsufficientFunds(err)) => {
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
    async fn test_sign_and_send_transaction_with_webhook_success() {
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx, mut job_producer) =
            setup_test_context();

        relayer.notification_id = Some("test-webhook-id".to_string());

        let signature = Signature::new_unique();
        signer.expect_sign().returning(move |_| {
            let signature = signature;
            Box::pin(async move { Ok(signature) })
        });

        provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

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
            .returning(move |_| Box::pin(async move { Ok(signature) }));

        // Expect webhook job to be produced
        job_producer
            .expect_produce_send_notification_job()
            .withf(move |notification, _| {
                matches!(notification.notification_id.as_str(), "test-webhook-id")
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = SignAndSendTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_and_send_transaction_impl(params).await;
        assert!(result.is_ok());
    }
}
