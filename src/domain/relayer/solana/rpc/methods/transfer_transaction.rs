//! Creates a transfer transaction for a specified token, sender, and recipient.
//!
//! # Description
//!
//! This function constructs a partially signed transfer transaction using the provided
//! parameters. In addition to the transfer, it calculates fee amounts both in SPL tokens
//! and in lamports, and sets an expiration block height for the transaction.
//!
//! # Parameters
//!
//! * `amount` - The amount to transfer, specified in the smallest unit of the token.
//! * `token` - A string representing the token mint address for both the transfer and the fee
//!   payment.
//! * `source` - A string representing the sender's public key.
//! * `destination` - A string representing the recipient's public key.
//!
//! # Returns
//!
//! On success, returns a tuple containing:
//!
//! * `transaction` - A Base64-encoded partially signed transaction.
//! * `fee_in_spl` - The fee amount in SPL tokens (smallest unit).
//! * `fee_in_lamports` - The fee amount in lamports (SOL equivalent).
//! * `fee_token` - The token mint address used for fee payments.
//! * `valid_until_blockheight` - The block height until which the transaction remains valid.

use std::str::FromStr;

use log::info;
use solana_sdk::{hash::Hash, pubkey::Pubkey, transaction::Transaction};

use crate::{
    domain::relayer::solana::rpc::methods::utils::FeeQuote,
    models::{
        produce_solana_rpc_webhook_payload, EncodedSerializedTransaction, SolanaFeePaymentStrategy,
        SolanaWebhookRpcPayload, TransferTransactionRequestParams, TransferTransactionResult,
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
    pub(crate) async fn transfer_transaction_impl(
        &self,
        params: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError> {
        info!(
            "Processing transfer transaction for: {} and amount {}",
            params.token, params.amount
        );
        let source = Pubkey::from_str(&params.source)
            .map_err(|_| SolanaRpcError::InvalidParams("Invalid source address".to_string()))?;
        let destination = Pubkey::from_str(&params.destination).map_err(|_| {
            SolanaRpcError::InvalidParams("Invalid destination address".to_string())
        })?;
        let token_mint = Pubkey::from_str(&params.token)
            .map_err(|_| SolanaRpcError::InvalidParams("Invalid token mint address".to_string()))?;
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|_| SolanaRpcError::Internal("Invalid relayer address".to_string()))?;

        validate_token_transfer_transaction(
            &params.source,
            &params.destination,
            &params.token,
            params.amount,
            &self.relayer,
        )?;

        let (transaction, recent_blockhash, total_fee, fee_quote) = self
            .create_transfer_transaction_with_fee_strategy(
                &source,
                &destination,
                &relayer_pubkey,
                &token_mint,
                &params.token,
                params.amount,
            )
            .await?;

        SolanaTransactionValidator::validate_max_fee(
            total_fee,
            &self.relayer.policies.get_solana_policy(),
        )?;
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

        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)?;

        let result = TransferTransactionResult {
            transaction: encoded_tx,
            fee_in_spl: fee_quote.fee_in_spl.to_string(),
            fee_in_lamports: fee_quote.fee_in_lamports.to_string(),
            fee_token: params.token,
            valid_until_blockheight: recent_blockhash.1,
        };

        if let Some(notification_id) = &self.relayer.notification_id {
            let webhook_result = self
                .job_producer
                .produce_send_notification_job(
                    produce_solana_rpc_webhook_payload(
                        notification_id,
                        "transfer_transaction".to_string(),
                        SolanaWebhookRpcPayload::TransferTransaction(result.clone()),
                    ),
                    None,
                )
                .await;

            if let Err(e) = webhook_result {
                error!("Failed to produce notification job: {}", e);
            }
        }

        info!("Transfer transaction processed successfully");

        Ok(result)
    }

    /// Creates a transfer transaction with the specified fee strategy.
    ///
    /// This function handles the creation of a transfer transaction, including the fee payment
    /// strategy. It constructs the transaction based on the relayer's policies and the user's
    /// preferences.
    async fn create_transfer_transaction_with_fee_strategy(
        &self,
        source: &Pubkey,
        destination: &Pubkey,
        relayer_pubkey: &Pubkey,
        token_mint: &Pubkey,
        token_mint_str: &str,
        amount: u64,
    ) -> Result<(Transaction, (Hash, u64), u64, FeeQuote), SolanaRpcError> {
        let policies = self.relayer.policies.get_solana_policy();
        let user_pays_fee =
            policies.fee_payment_strategy.unwrap_or_default() == SolanaFeePaymentStrategy::User;
        let token_transfer_instruction = self
            .handle_token_transfer(source, destination, token_mint, amount)
            .await?;

        if user_pays_fee {
            let minimal_fee_amount = 1; // Smallest possible amount for structure estimation
            let draft_fee_instructions = self
                .handle_token_transfer(source, relayer_pubkey, token_mint, minimal_fee_amount)
                .await?;

            // Create a structurally complete draft transaction
            let (draft_transaction, _) = self
                .create_transaction(
                    [draft_fee_instructions, token_transfer_instruction.to_vec()].concat(),
                )
                .await?;

            let (fee_quote, buffered_base_fee) = self
                .estimate_and_convert_fee(
                    &draft_transaction,
                    token_mint_str,
                    policies.fee_margin_percentage,
                )
                .await?;

            // Create the real fee payment instruction with the correct amount
            let fee_payment_instructions = self
                .handle_token_transfer(source, relayer_pubkey, token_mint, fee_quote.fee_in_spl)
                .await?;

            let (transaction, recent_blockhash) = self
                .create_and_sign_transaction(
                    [
                        fee_payment_instructions,
                        token_transfer_instruction.to_vec(),
                    ]
                    .concat(),
                )
                .await?;

            Ok((transaction, recent_blockhash, buffered_base_fee, fee_quote))
        } else {
            let (transaction, recent_blockhash) = self
                .create_and_sign_transaction(token_transfer_instruction)
                .await?;
            let (estimated_fee_quote, buffered_total_fee) = self
                .estimate_and_convert_fee(
                    &transaction,
                    token_mint_str,
                    policies.fee_margin_percentage,
                )
                .await?;

            Ok((
                transaction,
                recent_blockhash,
                buffered_total_fee,
                estimated_fee_quote,
            ))
        }
    }
}

/// Validates a token transfer transaction transaction
fn validate_token_transfer_transaction(
    source: &str,
    destination: &str,
    token_mint: &str,
    amount: u64,
    relayer: &RelayerRepoModel,
) -> Result<(), SolanaTransactionValidationError> {
    let policy = &relayer.policies.get_solana_policy();
    SolanaTransactionValidator::validate_allowed_account(source, policy)?;
    SolanaTransactionValidator::validate_disallowed_account(source, policy)?;
    SolanaTransactionValidator::validate_allowed_account(destination, policy)?;
    SolanaTransactionValidator::validate_disallowed_account(destination, policy)?;
    SolanaTransactionValidator::validate_allowed_token(token_mint, policy)?;
    if amount == 0 {
        return Err(SolanaTransactionValidationError::ValidationError(
            "Amount must be greater than 0".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        constants::WRAPPED_SOL_MINT,
        models::{
            NetworkType, RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
            SolanaAllowedTokensSwapConfig,
        },
        services::{QuoteResponse, RoutePlan, SwapInfo},
    };

    use super::*;
    use solana_sdk::{
        hash::Hash,
        program_option::COption,
        program_pack::Pack,
        signature::{Keypair, Signature},
        signer::Signer,
    };
    use spl_token::state::Account;

    #[tokio::test]
    async fn test_transfer_wsol_spl_token_success_relayer_fee_strategy() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();
        let test_token = WRAPPED_SOL_MINT;

        // Create valid token account data
        let token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(), // Source account owner
            amount: 10_000_000_000,      // 10 WSOL
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        // Pack the account data
        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        // Set up policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        // Mock provider responses
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
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

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 5000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_in_spl, "5000");
        assert_eq!(transfer_result.fee_in_lamports, "5000");
        assert_eq!(transfer_result.fee_token, test_token);
        assert_ne!(transfer_result.valid_until_blockheight, 0);
    }

    #[tokio::test]
    async fn test_transfer_wsol_spl_token_success_user_fee_strategy() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();
        let test_token = WRAPPED_SOL_MINT;

        // Create valid token account data
        let token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(), // Source account owner
            amount: 10_000_000_000,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::User),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
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

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 5000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_in_spl, "5000");
        assert_eq!(transfer_result.fee_in_lamports, "5000");
        assert_eq!(transfer_result.fee_token, test_token);
        assert_ne!(transfer_result.valid_until_blockheight, 0);
    }

    #[tokio::test]
    async fn test_transfer_spl_token_success() {
        let (mut relayer, mut signer, mut provider, mut jupiter_service, _, job_producer) =
            setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        // Create valid token account data
        let token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(), // Source account owner
            amount: 10_000_000,          // 10 USDC (assuming 6 decimals)
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        // Pack the account data
        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        // Set up policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        // Mock provider responses
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: 5000,
                        out_amount: 100_000,
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

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 1_000_000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_in_spl, "100000");
        assert_eq!(transfer_result.fee_in_lamports, "5000");
        assert_eq!(transfer_result.fee_token, test_token);
        assert_ne!(transfer_result.valid_until_blockheight, 0);
    }

    #[tokio::test]
    async fn test_transfer_spl_token_success_token_account_creation() {
        let mut ctx = setup_test_context_relayer_fee_strategy();
        let source_pubkey = ctx.source_keypair.pubkey();
        let destination_pubkey = ctx.destination;

        let source_token_account = spl_token::state::Account {
            mint: Pubkey::from_str(&ctx.token).unwrap(),
            owner: source_pubkey,
            amount: 10_000_000,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        let mut source_account_data = vec![0; spl_token::state::Account::LEN];
        spl_token::state::Account::pack(source_token_account, &mut source_account_data).unwrap();

        ctx.relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: ctx.token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        ctx.signer.expect_sign().returning(move |_| {
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        ctx.provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        ctx.provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        ctx.provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        ctx.provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                let account_data = source_account_data.clone();
                Box::pin(async move {
                    if pubkey == ctx.token_mint {
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
                    } else if pubkey == ctx.user_token_account {
                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: account_data,
                            owner: spl_token::id(),
                            executable: false,
                            rent_epoch: 0,
                        })
                    } else {
                        Err(crate::services::SolanaProviderError::InvalidAddress(
                            format!("Invalid token address {}", pubkey),
                        ))
                    }
                })
            });

        ctx.provider
            .expect_get_minimum_balance_for_rent_exemption()
            .returning(|_| Box::pin(async { Ok(1111) }));

        let token = ctx.token.to_string();

        ctx.jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, _, _| {
                let token_clone = token.clone();
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: token_clone,
                        in_amount: 5000,
                        out_amount: 100_000,
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
            ctx.relayer,
            Arc::new(ctx.provider),
            Arc::new(ctx.signer),
            Arc::new(ctx.jupiter_service),
            Arc::new(ctx.job_producer),
        );

        let params = TransferTransactionRequestParams {
            token: ctx.token.to_string(),
            source: source_pubkey.to_string(),
            destination: destination_pubkey.to_string(),
            amount: 1_000_000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_in_spl, "100000");
        assert_eq!(transfer_result.fee_in_lamports, "6111");
        assert_eq!(transfer_result.fee_token, ctx.token.to_string());
        assert_ne!(transfer_result.valid_until_blockheight, 0);
    }

    #[tokio::test]
    async fn test_transfer_spl_insufficient_balance() {
        let (_, signer, mut provider, jupiter_service, _, job_producer) = setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        // Create test relayer
        let relayer = RelayerRepoModel {
            id: "id".to_string(),
            name: "Relayer".to_string(),
            network: "TestNet".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                allowed_programs: None,
                max_signatures: Some(10),
                max_tx_data_size: Some(1000),
                min_balance: Some(10000),
                allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                    mint: test_token.to_string(),
                    symbol: Some("USDC".to_string()),
                    decimals: Some(6),
                    max_allowed_fee: Some(1000),
                    swap_config: Some(SolanaAllowedTokensSwapConfig {
                        slippage_percentage: Some(1.0),
                        ..Default::default()
                    }),
                }]),
                fee_payment_strategy: Some(SolanaFeePaymentStrategy::Relayer),
                fee_margin_percentage: Some(0.5),
                allowed_accounts: None,
                disallowed_accounts: None,
                max_allowed_fee_lamports: None,
                swap_config: None,
            }),
            signer_id: "test".to_string(),
            address: Keypair::new().pubkey().to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        };
        // Create token account with low balance
        let token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(),
            amount: 100,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
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

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 1_000_000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(matches!(result, Err(SolanaRpcError::InsufficientFunds(_))));
    }
}
