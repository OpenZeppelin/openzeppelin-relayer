//! Prepares a transaction by adding relayer-specific instructions.
//!
//! # Description
//!
//! This function takes an existing Base64-encoded serialized transaction and adds
//! relayer-specific instructions.
//! The updated transaction will include additional data required by the relayer, and the
//! function also provides updated fee information and an expiration block height.
//!
//! # Parameters
//!
//! * `transaction` - A Base64-encoded serialized transaction that the end user would like relayed.
//! * `fee_token` - A string representing the token mint address to be used for fee payment.
//!
//! # Returns
//!
//! On success, returns a tuple containing:
//!
//! * `transaction` - A Base64-encoded transaction with the added relayer-specific instructions.
//! * `fee_in_spl` - The fee amount in SPL tokens (in the smallest unit).
//! * `fee_in_lamports` - The fee amount in lamports.
//! * `fee_token` - The token mint address used for fee payments.
//! * `valid_until_block_height` - The block height until which the transaction remains valid.use
//!   std::str::FromStr;
use futures::try_join;
use log::info;
use solana_sdk::{
    commitment_config::CommitmentConfig, hash::Hash, pubkey::Pubkey, signature::Signature,
    transaction::Transaction,
};
use std::str::FromStr;

use super::{utils::FeeQuote, *};
use crate::{
    models::{
        EncodedSerializedTransaction, PrepareTransactionRequestParams, PrepareTransactionResult,
        SolanaFeePaymentStrategy,
    },
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

impl<P, S, J, JP> SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
{
    pub(crate) async fn prepare_transaction_impl(
        &self,
        params: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError> {
        info!(
            "Processing prepare transaction request for fee token: {}",
            params.fee_token
        );

        let transaction_request = Transaction::try_from(params.transaction.clone())?;
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        validate_prepare_transaction(
            &transaction_request,
            &params.fee_token,
            &self.relayer,
            &*self.provider,
        )
        .await?;

        let (transaction, recent_blockhash, total_fee, fee_quote) = self
            .prepare_transaction_with_fee_strategy(
                &transaction_request,
                &relayer_pubkey,
                &params.fee_token,
            )
            .await?;

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

        // Sign transaction
        let (signed_transaction, _) = self.relayer_sign_transaction(transaction).await?;

        // Serialize and encode
        let encoded_tx = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        info!(
            "Successfully prepared transaction. Fee: {} SPL tokens, valid until block height: {}",
            fee_quote.fee_in_spl, recent_blockhash.1
        );

        Ok(PrepareTransactionResult {
            transaction: encoded_tx,
            fee_in_spl: fee_quote.fee_in_spl.to_string(),
            fee_in_lamports: fee_quote.fee_in_lamports.to_string(),
            fee_token: params.fee_token,
            valid_until_blockheight: recent_blockhash.1,
        })
    }

    /// Prepares a transaction with the appropriate fee strategy.
    ///
    /// This function creates a transaction based on the fee payment strategy defined in the relayer's
    /// policies. It either uses the relayer as the fee payer or allows the user to pay the fee.
    /// It also estimates the fee and returns the transaction, recent blockhash, total fee, and fee quote.
    async fn prepare_transaction_with_fee_strategy(
        &self,
        transaction_request: &Transaction,
        relayer_pubkey: &Pubkey,
        fee_token: &str,
    ) -> Result<(Transaction, (Hash, u64), u64, FeeQuote), SolanaRpcError> {
        let policies = self.relayer.policies.get_solana_policy();
        let user_pays_fee =
            policies.fee_payment_strategy.unwrap_or_default() == SolanaFeePaymentStrategy::User;

        let result = if user_pays_fee {
            // First create draft transaction with minimal fee to get structure right
            let (draft_transaction, _) = self
                .create_transaction_with_user_fee_payment(
                    relayer_pubkey,
                    transaction_request,
                    fee_token,
                    1, // Minimal amount for estimation
                )
                .await?;

            // Calculate actual fee needed
            let (fee_quote, buffered_total_fee) = self
                .estimate_and_convert_fee(
                    &draft_transaction,
                    fee_token,
                    policies.fee_margin_percentage,
                )
                .await?;

            // Create final transaction with correct fee amount
            let (transaction, recent_blockhash) = self
                .create_transaction_with_user_fee_payment(
                    relayer_pubkey,
                    transaction_request,
                    fee_token,
                    fee_quote.fee_in_spl,
                )
                .await?;

            (transaction, recent_blockhash, buffered_total_fee, fee_quote)
        } else {
            // Get latest blockhash for transaction
            let recent_blockhash = self
                .provider
                .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
                .await?;

            // Create new transaction message with relayer as fee payer
            let mut message = transaction_request.message.clone();
            message.recent_blockhash = recent_blockhash.0;

            // Update fee payer if needed
            if message.account_keys[0] != *relayer_pubkey {
                message.account_keys[0] = *relayer_pubkey;
            }

            // Create transaction with updated message
            let transaction = Transaction {
                signatures: vec![Signature::default()],
                message,
            };

            let (fee_quote, buffered_total_fee) = self
                .estimate_and_convert_fee(&transaction, fee_token, policies.fee_margin_percentage)
                .await?;

            (transaction, recent_blockhash, buffered_total_fee, fee_quote)
        };

        Ok(result)
    }
}

/// Validates a transaction before estimating fee.
async fn validate_prepare_transaction<P: SolanaProviderTrait + Send + Sync>(
    tx: &Transaction,
    token: &str,
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
        SolanaTransactionValidator::validate_data_size(tx, policy)?;
        SolanaTransactionValidator::validate_allowed_token(token, policy)?;
        Ok::<(), SolanaTransactionValidationError>(())
    };

    // Run all validations concurrently.
    try_join!(
        sync_validations,
        SolanaTransactionValidator::simulate_transaction(tx, provider),
        SolanaTransactionValidator::validate_lamports_transfers(tx, &relayer_pubkey),
        SolanaTransactionValidator::validate_token_transfers(tx, policy, provider, &relayer_pubkey,),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use super::*;
    use crate::{
        constants::WRAPPED_SOL_MINT,
        models::{
            RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
            SolanaAllowedTokensSwapConfig,
        },
        services::{QuoteResponse, RoutePlan, SwapInfo},
    };
    use solana_sdk::{
        hash::Hash, message::Message, program_pack::Pack, signature::Keypair, signer::Signer,
    };
    use solana_system_interface::instruction;
    use spl_associated_token_account::get_associated_token_address;
    use spl_token::state::Account;

    #[tokio::test]
    async fn test_prepare_transaction_success_relayer_fee_strategy() {
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Setup policy with WSOL
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: WRAPPED_SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Box::pin(async move { Ok(signature) }));

        // Mock provider responses
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

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
                    inner_instructions: None,
                    replacement_blockhash: None,
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

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: WRAPPED_SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        assert_eq!(prepare_result.fee_token, WRAPPED_SOL_MINT);
        assert_eq!(prepare_result.fee_in_lamports, "5000");
        assert_eq!(prepare_result.fee_in_spl, "5000");
        assert_eq!(prepare_result.valid_until_blockheight, 100);
    }

    #[tokio::test]
    async fn test_prepare_transaction_success_user_fee_strategy() {
        let mut ctx = setup_test_context_single_tx_user_fee_strategy();

        ctx.provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                let relayer_pubkey = ctx.relayer_keypair.pubkey();
                let user_pubkey = ctx.user_keypair.pubkey();
                let payer_pubkey = ctx.payer_keypair.pubkey();
                Box::pin(async move {
                    let mut account_data = vec![0; Account::LEN];

                    if pubkey == ctx.relayer_token_account {
                        // Create relayer's token account
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: relayer_pubkey,
                            amount: 10000000,
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
                            amount: ctx.main_transfer_amount,
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
                    } else if pubkey == ctx.payer_token_account {
                        // Create payers's token account with sufficient balance
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: payer_pubkey,
                            amount: ctx.main_transfer_amount + ctx.fee_amount,
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
                    } else if pubkey == ctx.token_mint {
                        let mut mint_data = vec![0; spl_token::state::Mint::LEN];
                        let mint = spl_token::state::Mint {
                            is_initialized: true,
                            mint_authority: solana_sdk::program_option::COption::Some(
                                Pubkey::new_unique(),
                            ),
                            supply: 1_000_000_000_000,
                            decimals: 6,
                            ..Default::default()
                        };
                        spl_token::state::Mint::pack(mint, &mut mint_data).unwrap();

                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: mint_data,
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

        ctx.provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        ctx.provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(6000u64) }));

        ctx.provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        ctx.jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "SOL".to_string(),
                        output_mint: "USDC".to_string(),
                        in_amount: 500000000,
                        out_amount: 80000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                        slippage_bps: 1,
                        swap_mode: "ExactIn".to_string(),
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
                    inner_instructions: None,
                    replacement_blockhash: None,
                    loaded_accounts_data_size: None,
                })
            })
        });

        let signature = Signature::new_unique();
        ctx.signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            ctx.relayer,
            Arc::new(ctx.provider),
            Arc::new(ctx.signer),
            Arc::new(ctx.jupiter_service),
            Arc::new(ctx.job_producer),
        );

        let token_test = &ctx.token;

        let params = PrepareTransactionRequestParams {
            transaction: ctx.encoded_tx,
            fee_token: token_test.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;

        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        assert_eq!(
            prepare_result.fee_token,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(prepare_result.fee_in_lamports, "6029");
        assert_eq!(prepare_result.fee_in_spl, "80000");
        assert_eq!(prepare_result.valid_until_blockheight, 100);
    }

    #[tokio::test]
    async fn test_prepare_transaction_insufficient_balance() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Set high minimum balance requirement
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            min_balance: Some(100_000_000), // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: WRAPPED_SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(50_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
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

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: WRAPPED_SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(matches!(result, Err(SolanaRpcError::InsufficientFunds(_))));
    }

    #[tokio::test]
    async fn test_prepare_transaction_updates_fee_payer() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            min_balance: Some(100_000_000), // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: WRAPPED_SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&wrong_fee_payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();
        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Box::pin(async move { Ok(signature) }));
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

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
                    inner_instructions: None,
                    replacement_blockhash: None,
                    loaded_accounts_data_size: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: WRAPPED_SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        let final_tx = Transaction::try_from(prepare_result.transaction).unwrap();
        assert_eq!(
            final_tx.message.account_keys[0],
            Pubkey::from_str(&relayer.address).unwrap()
        );
    }

    #[tokio::test]
    async fn test_prepare_transaction_signature_verification() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();
        println!("Setting up known keypair for signature verification");
        let relayer_keypair = Keypair::new();
        let expected_signature = Signature::new_unique();

        relayer.address = relayer_keypair.pubkey().to_string();
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            min_balance: Some(100_000_000), // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: WRAPPED_SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        signer.expect_sign().returning(move |_| {
            let signature = expected_signature;
            Box::pin(async move { Ok(signature) })
        });
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

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
                    inner_instructions: None,
                    replacement_blockhash: None,
                    loaded_accounts_data_size: None,
                })
            })
        });

        // Create test transaction
        let ix = instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);
        let message = Message::new(&[ix], Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: WRAPPED_SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        let final_tx = Transaction::try_from(prepare_result.transaction).unwrap();

        // Verify signature presence and correctness
        assert_eq!(
            final_tx.signatures.len(),
            1,
            "Transaction should have exactly one signature"
        );
        assert_eq!(
            final_tx.signatures[0], expected_signature,
            "Transaction should have the expected signature"
        );
        assert_eq!(
            final_tx.message.account_keys[0].to_string(),
            relayer_keypair.pubkey().to_string(),
            "Fee payer should match relayer address"
        );
    }

    #[tokio::test]
    async fn test_prepare_transaction_not_allowed_token() {
        let (mut relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        // Configure policy with allowed tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "AllowedToken111111111111111111111111111111".to_string(),
                symbol: Some("ALLOWED".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        // Create transaction with not allowed token
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let not_allowed_token = Pubkey::new_unique();

        let ix = spl_token::instruction::transfer(
            &spl_token::id(),
            &get_associated_token_address(&payer.pubkey(), &not_allowed_token),
            &get_associated_token_address(&recipient, &not_allowed_token),
            &payer.pubkey(),
            &[],
            100,
        )
        .unwrap();

        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        // Setup provider mocks
        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
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

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: WRAPPED_SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Token So11111111111111111111111111111111111111112 not \
                         allowed for transfers"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }
}
