//! Signs a prepared transaction without submitting it to the blockchain.
//!
//! # Description
//!
//! This function is used to sign a prepared transaction (one that may have been modified by the
//! relayer) to ensure its validity and authorization before submission. It returns the
//! signed transaction along with the corresponding signature.
//!
//! # Parameters
//!
//! * `transaction` - A Base64-encoded prepared transaction that requires signing.
//!
//! # Returns
//!
//! On success, returns a tuple containing:
//!
//! * `transaction` - A Base64-encoded signed transaction.
//! * `signature` - Signature of the submitted transaction.
use std::str::FromStr;

use futures::try_join;
use log::info;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};

use crate::{
    models::{
        produce_solana_rpc_webhook_payload, EncodedSerializedTransaction,
        SignTransactionRequestParams, SignTransactionResult, SolanaFeePaymentStrategy,
        SolanaWebhookRpcPayload,
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
    pub(crate) async fn sign_transaction_impl(
        &self,
        params: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError> {
        info!("Processing sign transaction request");
        let transaction_request = Transaction::try_from(params.transaction)?;

        validate_sign_transaction(&transaction_request, &self.relayer, &*self.provider).await?;

        let policy = self.relayer.policies.get_solana_policy();
        let total_fee = self
            .estimate_fee_with_margin(&transaction_request, policy.fee_margin_percentage)
            .await
            .map_err(|e| {
                error!("Failed to estimate total fee: {}", e);
                SolanaRpcError::Estimation(e.to_string())
            })?;

        let user_pays_fee =
            policy.fee_payment_strategy.unwrap_or_default() == SolanaFeePaymentStrategy::User;

        if user_pays_fee {
            self.confirm_user_fee_payment(&transaction_request, total_fee)
                .await?;
        }

        SolanaTransactionValidator::validate_max_fee(
            total_fee,
            &self.relayer.policies.get_solana_policy(),
        )?;
        // Validate relayer has sufficient balance
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_fee,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await
        .map_err(|e| {
            error!("Insufficient funds: {}", e);
            SolanaRpcError::InsufficientFunds(e.to_string())
        })?;

        let (signed_transaction, signature) =
            self.relayer_sign_transaction(transaction_request).await?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        let result = SignTransactionResult {
            transaction: serialized_transaction,
            signature: signature.to_string(),
        };

        if let Some(notification_id) = &self.relayer.notification_id {
            let webhook_result = self
                .job_producer
                .produce_send_notification_job(
                    produce_solana_rpc_webhook_payload(
                        notification_id,
                        "sign_transaction".to_string(),
                        SolanaWebhookRpcPayload::SignTransaction(result.clone()),
                    ),
                    None,
                )
                .await;

            if let Err(e) = webhook_result {
                error!("Failed to produce notification job: {}", e);
            }
        }
        info!(
            "Transaction signed successfully with signature: {}",
            result.signature
        );

        Ok(result)
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
        SolanaTransactionValidator::validate_lamports_transfers(tx, &relayer_pubkey),
        SolanaTransactionValidator::validate_token_transfers(tx, policy, provider, &relayer_pubkey),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        constants::WRAPPED_SOL_MINT,
        models::{RelayerNetworkPolicy, RelayerSolanaPolicy},
        services::{QuoteResponse, RoutePlan, SwapInfo},
    };

    use super::*;
    use mockall::predicate::{self};
    use solana_sdk::{
        message::Message,
        program_pack::Pack,
        signature::{Keypair, Signature},
        signer::Signer,
    };
    use solana_system_interface::instruction;
    use spl_token::state::Account;

    #[tokio::test]
    async fn test_sign_transaction_success_relayer_fee_strategy() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
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
                    loaded_accounts_data_size: None,
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
    async fn test_sign_transaction_success_user_fee_strategy() {
        let mut ctx = setup_test_context_user_fee_strategy();

        ctx.provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                let relayer_pubkey = ctx.relayer_keypair.pubkey();
                let user_pubkey = ctx.user_keypair.pubkey();
                Box::pin(async move {
                    let mut account_data = vec![0; Account::LEN];

                    if pubkey == ctx.relayer_token_account {
                        // Create relayer's token account
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: relayer_pubkey,
                            amount: 0, // Current balance doesn't matter
                            state: spl_token::state::AccountState::Initialized,
                            ..Default::default()
                        };
                        spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: account_data,
                            owner: spl_token::id(),
                            executable: false,
                            rent_epoch: 0,
                        })
                    } else if pubkey == ctx.user_token_account {
                        // Create user's token account with sufficient balance
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: user_pubkey,
                            amount: ctx.main_transfer_amount + ctx.fee_amount, // Enough for both transfers
                            state: spl_token::state::AccountState::Initialized,
                            ..Default::default()
                        };
                        spl_token::state::Account::pack(token_account, &mut account_data).unwrap();
                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: account_data,
                            owner: spl_token::id(),
                            executable: false,
                            rent_epoch: 0,
                        })
                    } else {
                        Err(SolanaProviderError::RpcError(
                            "Account not found".to_string(),
                        ))
                    }
                })
            });

        let signature = Signature::new_unique();
        ctx.signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        ctx.provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        ctx.provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        ctx.provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        let token_clone = ctx.token.clone();
        ctx.jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, _, _| {
                let token_clone = token_clone.clone();
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: token_clone,
                        in_amount: 5000,
                        out_amount: 100_000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                        swap_mode: "ExactIn".to_string(),
                        slippage_bps: 0,
                        route_plan: vec![RoutePlan {
                            swap_info: SwapInfo {
                                amm_key: "63mqrcydH89L7RhuMC3jLBojrRc2u3QWmjP4UrXsnotS".to_string(),
                                label: "Stabble Stable Swap".to_string(),
                                input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                                    .to_string(),
                                output_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
                                    .to_string(),
                                in_amount: "1000000".to_string(),
                                out_amount: "999984".to_string(),
                                fee_amount: "10".to_string(),
                                fee_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
                                    .to_string(),
                            },
                            percent: 1,
                        }],
                    })
                })
            });

        ctx.provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                    loaded_accounts_data_size: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            ctx.relayer,
            Arc::new(ctx.provider),
            Arc::new(ctx.signer),
            Arc::new(ctx.jupiter_service),
            Arc::new(ctx.job_producer),
        );

        let params = SignTransactionRequestParams {
            transaction: ctx.encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(
            result.is_ok(),
            "Should successfully sign transaction with token fee payment"
        );
        let sign_result = result.unwrap();

        // Verify signature format
        let decoded_sig = bs58::decode(&sign_result.signature)
            .into_vec()
            .expect("Failed to decode base58 signature");
        assert_eq!(decoded_sig.len(), 64);
    }

    #[tokio::test]
    async fn test_sign_transaction_balance_failure_relayer_fee_strategy() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
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
                    loaded_accounts_data_size: None,
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

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_err());

        match result {
            Err(SolanaRpcError::InsufficientFunds(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains("Insufficient balance:"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_balance_failure_user_fee_strategy() {
        let mut ctx = setup_test_context_user_fee_strategy();

        ctx.provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                let relayer_pubkey = ctx.relayer_keypair.pubkey();
                let user_pubkey = ctx.user_keypair.pubkey();
                Box::pin(async move {
                    let mut account_data = vec![0; Account::LEN];

                    if pubkey == ctx.relayer_token_account {
                        // Create relayer's token account
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: relayer_pubkey,
                            amount: 0, // Current balance doesn't matter
                            state: spl_token::state::AccountState::Initialized,
                            ..Default::default()
                        };
                        spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: account_data,
                            owner: spl_token::id(),
                            executable: false,
                            rent_epoch: 0,
                        })
                    } else if pubkey == ctx.user_token_account {
                        // Create user's token account with sufficient balance
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: user_pubkey,
                            amount: ctx.main_transfer_amount + ctx.fee_amount, // Enough for both transfers
                            state: spl_token::state::AccountState::Initialized,
                            ..Default::default()
                        };
                        spl_token::state::Account::pack(token_account, &mut account_data).unwrap();
                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: account_data,
                            owner: spl_token::id(),
                            executable: false,
                            rent_epoch: 0,
                        })
                    } else {
                        Err(SolanaProviderError::RpcError(
                            "Account not found".to_string(),
                        ))
                    }
                })
            });

        let signature = Signature::new_unique();
        ctx.signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        ctx.provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        ctx.provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        ctx.provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000) }));

        let token_clone = ctx.token.clone();
        ctx.jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, _, _| {
                let token_clone = token_clone.clone();
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: token_clone,
                        in_amount: 5000,
                        out_amount: 100_000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                        swap_mode: "ExactIn".to_string(),
                        slippage_bps: 0,
                        route_plan: vec![RoutePlan {
                            swap_info: SwapInfo {
                                amm_key: "63mqrcydH89L7RhuMC3jLBojrRc2u3QWmjP4UrXsnotS".to_string(),
                                label: "Stabble Stable Swap".to_string(),
                                input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
                                    .to_string(),
                                output_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
                                    .to_string(),
                                in_amount: "1000000".to_string(),
                                out_amount: "999984".to_string(),
                                fee_amount: "10".to_string(),
                                fee_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
                                    .to_string(),
                            },
                            percent: 1,
                        }],
                    })
                })
            });

        ctx.provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                    loaded_accounts_data_size: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            ctx.relayer,
            Arc::new(ctx.provider),
            Arc::new(ctx.signer),
            Arc::new(ctx.jupiter_service),
            Arc::new(ctx.job_producer),
        );

        let params = SignTransactionRequestParams {
            transaction: ctx.encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_err());

        match result {
            Err(SolanaRpcError::InsufficientFunds(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains("Insufficient balance:"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_validation_failure_blockhash_relayer_fee_strategy() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            ..Default::default()
        });

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_signatures_relayer_fee_strategy() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();
        // Update policy with low max signatures
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
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
                    loaded_accounts_data_size: None,
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

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Transaction requires 2 signatures, which exceeds \
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
    async fn test_sign_transaction_disallowed_program_relayer_fee_strategy() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

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
                    loaded_accounts_data_size: None,
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
    async fn test_sign_transaction_exceeds_data_size_relayer_fee_strategy() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Update policy with small max data size
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_tx_data_size: Some(10),
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
                    loaded_accounts_data_size: None,
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

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Transaction size 311 exceeds maximum allowed 10"
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
        let (relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
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
            Arc::new(job_producer),
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
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Update policy with disallowed accounts
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![Pubkey::new_unique().to_string()]),
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature = signature;
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
                    loaded_accounts_data_size: None,
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

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        // This should pass since our test transaction doesn't use disallowed accounts
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_lamports_fee() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Set max allowed transfer amount in policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_allowed_fee_lamports: Some(500),
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            ..Default::default()
        });

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

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
                    loaded_accounts_data_size: None,
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

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Fee amount 5000 exceeds max allowed fee amount 500"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_with_webhook_success() {
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
                    loaded_accounts_data_size: None,
                })
            })
        });

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

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        assert!(result.is_ok());
    }
}
