//! Estimates the fee for an arbitrary transaction using a specified fee token.
//!
//! # Description
//!
//! This function simulates fee estimation for a transaction by executing it against the current
//! blockchain state. It calculates the fee in the UI unit of the selected token (accounting
//! for token decimals) and returns a conversion rate from SOL to the specified token.
//!
//! # Parameters
//!
//! * `transaction` - A Base64-encoded serialized transaction. This transaction can be signed or
//!   unsigned.
//! * `fee_token` - A string representing the token mint address to be used for fee payment.
//!
//! # Returns
//!
//! On success, returns a tuple containing:
//!
//! * `estimated_fee` - A string with the fee amount in the token's UI units.
//! * `conversion_rate` - A string with the conversion rate from SOL to the specified token.use
use std::str::FromStr;

use futures::try_join;
use log::info;
use solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature,
    transaction::Transaction,
};

use crate::{
    domain::SolanaRpcError,
    jobs::JobProducerTrait,
    models::{
        FeeEstimateRequestParams, FeeEstimateResult, RelayerRepoModel, SolanaFeePaymentStrategy,
    },
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::{
    utils::FeeQuote, SolanaRpcMethodsImpl, SolanaTransactionValidationError,
    SolanaTransactionValidator,
};

impl<P, S, J, JP> SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
{
    /// Estimates the fee for an arbitrary transaction using a specified fee token.
    ///
    /// # Description
    ///
    /// This function simulates fee estimation for a transaction by executing it against the current
    /// blockchain state. It calculates the fee in the UI unit of the selected token (accounting
    /// for token decimals) and returns a conversion rate from SOL to the specified token.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded serialized transaction. This transaction can be signed or
    ///   unsigned.
    /// * `fee_token` - A string representing the token mint address to be used for fee payment.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `estimated_fee` - A string with the fee amount in the token's UI units.
    /// * `conversion_rate` - A string with the conversion rate from SOL to the specified token.
    pub(crate) async fn fee_estimate_impl(
        &self,
        params: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError> {
        info!(
            "Processing fee estimate request for token: {}",
            params.fee_token
        );

        let transaction_request = Transaction::try_from(params.transaction.clone())?;

        validate_fee_estimate_transaction(
            &transaction_request,
            &params.fee_token,
            &self.relayer,
            &*self.provider,
        )
        .await?;

        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|_| SolanaRpcError::Internal("Invalid relayer address".to_string()))?;

        // Create transaction based on fee payment policy
        let (_, fee_quote) = self
            .create_fee_estimation_transaction(
                &transaction_request,
                &relayer_pubkey,
                &params.fee_token,
            )
            .await?;

        SolanaTransactionValidator::validate_max_fee(
            fee_quote.fee_in_lamports,
            &self.relayer.policies.get_solana_policy(),
        )?;

        Ok(FeeEstimateResult {
            estimated_fee: fee_quote.fee_in_spl_ui,
            conversion_rate: fee_quote.conversion_rate.to_string(),
        })
    }

    /// Creates a transaction for fee estimation based on the fee payment policy
    async fn create_fee_estimation_transaction(
        &self,
        transaction_request: &Transaction,
        relayer_pubkey: &Pubkey,
        fee_token: &str,
    ) -> Result<(Transaction, FeeQuote), SolanaRpcError> {
        let policies = self.relayer.policies.get_solana_policy();
        let user_pays_fee =
            policies.fee_payment_strategy.unwrap_or_default() == SolanaFeePaymentStrategy::User;

        // Get latest blockhash
        let recent_blockhash = self
            .provider
            .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
            .await?;

        // Create the appropriate transaction based on fee payment policy
        let transaction = if user_pays_fee {
            // If user pays fee, add a token transfer instruction for fee payment
            self.create_transaction_with_user_fee_payment(
                relayer_pubkey,
                transaction_request,
                fee_token,
                1, // Minimal amount for estimation
            )
            .await?
            .0 // Take just the transaction, not the blockhash
        } else {
            // Otherwise use the original transaction with relayer as fee payer
            let mut message = transaction_request.message.clone();
            message.recent_blockhash = recent_blockhash.0;

            // Update fee payer if needed
            if message.account_keys[0] != *relayer_pubkey {
                message.account_keys[0] = *relayer_pubkey;
            }
            Transaction {
                signatures: vec![Signature::default()],
                message,
            }
        };

        // Update transaction blockhash
        let mut final_transaction = transaction;
        final_transaction.message.recent_blockhash = recent_blockhash.0;

        // Estimate fee for the transaction
        let (fee_quote, _) = self
            .estimate_and_convert_fee(
                &final_transaction,
                fee_token,
                policies.fee_margin_percentage,
            )
            .await?;

        Ok((final_transaction, fee_quote))
    }
}

/// Validates a transaction before estimating fee.
async fn validate_fee_estimate_transaction<P: SolanaProviderTrait + Send + Sync>(
    tx: &Transaction,
    token_mint: &str,
    relayer: &RelayerRepoModel,
    provider: &P,
) -> Result<(), SolanaTransactionValidationError> {
    let policy = &relayer.policies.get_solana_policy();

    let sync_validations = async {
        SolanaTransactionValidator::validate_tx_allowed_accounts(tx, policy)?;
        SolanaTransactionValidator::validate_tx_disallowed_accounts(tx, policy)?;
        SolanaTransactionValidator::validate_allowed_programs(tx, policy)?;
        SolanaTransactionValidator::validate_max_signatures(tx, policy)?;
        SolanaTransactionValidator::validate_data_size(tx, policy)?;
        SolanaTransactionValidator::validate_allowed_token(token_mint, policy)?;
        Ok::<(), SolanaTransactionValidationError>(())
    };

    let relayer_pubkey = Pubkey::from_str(&relayer.address).map_err(|_| {
        SolanaTransactionValidationError::ValidationError("Invalid relayer address".to_string())
    })?;

    // Run all validations concurrently.
    try_join!(
        sync_validations,
        SolanaTransactionValidator::validate_token_transfers(tx, policy, provider, &relayer_pubkey,),
        SolanaTransactionValidator::validate_lamports_transfers(tx, &relayer_pubkey),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use crate::{
        constants::WRAPPED_SOL_MINT,
        domain::{
            setup_test_context, setup_test_context_single_tx_user_fee_strategy, SolanaRpcMethods,
        },
        models::{
            RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
            SolanaAllowedTokensSwapConfig,
        },
        services::{
            MockSolanaProviderTrait, QuoteResponse, RoutePlan, SolanaProviderError, SwapInfo,
        },
    };

    use super::*;
    use mockall::predicate::{self};
    use solana_sdk::{hash::Hash, program_pack::Pack, signer::Signer};
    use spl_token::state::Account;

    #[tokio::test]
    async fn test_fee_estimate_with_allowed_token_relayer_fee_strategy() {
        let (mut relayer, signer, mut provider, mut jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Set up policy with allowed token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "USDC".to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1000000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        // Mock provider methods
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

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

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
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
    async fn test_fee_estimate_with_allowed_token_user_fee_strategy() {
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
                    } else if pubkey == ctx.payer_token_account {
                        // Create payers's token account with sufficient balance
                        let token_account = spl_token::state::Account {
                            mint: ctx.token_mint,
                            owner: payer_pubkey,
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
                        Err(SolanaProviderError::RpcError(format!(
                            "Account not found {}",
                            pubkey
                        )))
                    }
                })
            });

        ctx.provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        ctx.provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(500000000u64) }));

        ctx.jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq(ctx.token.clone()),
                predicate::eq(502499999u64),
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

        let rpc = SolanaRpcMethodsImpl::new_mock(
            ctx.relayer,
            Arc::new(ctx.provider),
            Arc::new(ctx.signer),
            Arc::new(ctx.jupiter_service),
            Arc::new(ctx.job_producer),
        );

        let token_test = &ctx.token;

        let params = FeeEstimateRequestParams {
            transaction: ctx.encoded_tx,
            fee_token: token_test.clone(),
        };

        let result = rpc.fee_estimate(params).await;

        result.unwrap();

        // assert!(result.is_ok());

        // let fee_estimate = result.unwrap();
        // assert_eq!(fee_estimate.estimated_fee, "80");
        // assert_eq!(fee_estimate.conversion_rate, "160");
    }

    #[tokio::test]
    async fn test_fee_estimate_usdt_to_sol_conversion() {
        let (mut relayer, signer, _provider, mut jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), // USDT mint
                symbol: Some("USDT".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1000000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let mut provider = MockSolanaProviderTrait::new();

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

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

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            // noboost
            fee_token: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), // noboost
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.02"); // 0.02 USDT
        assert_eq!(fee_estimate.conversion_rate, "20"); // 1 SOL = 20 USDT
    }

    #[tokio::test]
    async fn test_fee_estimate_uni_to_sol_dynamic_price() {
        let (mut relayer, signer, mut provider, mut jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Set up policy with UNI token (decimals = 8)
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(), // UNI mint
                symbol: Some("UNI".to_string()),
                decimals: Some(8),
                max_allowed_fee: Some(1_000_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
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

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            // noboost
            fee_token: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(), // noboost
        };

        let result = rpc.fee_estimate(params).await;

        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.0177"); // 0.0177 UNI
        assert_eq!(fee_estimate.conversion_rate, "17.7"); // 1 SOL = 17.7 UNI
    }

    #[tokio::test]
    async fn test_fee_estimate_wrapped_sol() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx, job_producer) =
            setup_test_context();

        // Set up policy with WSOL token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: WRAPPED_SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        // Mock provider methods - expect 0.001 SOL fee (1_000_000 lamports)
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            fee_token: WRAPPED_SOL_MINT.to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.001"); // 0.001 SOL (1_000_000 lamports)
        assert_eq!(fee_estimate.conversion_rate, "1"); // 1:1 for native SOL
    }
}
