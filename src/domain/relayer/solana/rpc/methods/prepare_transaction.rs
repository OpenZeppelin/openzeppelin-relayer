//! prepareTransaction RPC method implementation.
use std::str::FromStr;

use futures::try_join;
use solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature,
    transaction::Transaction,
};

use crate::{
    models::{
        EncodedSerializedTransaction, PrepareTransactionRequestParams, PrepareTransactionResult,
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
    /// Prepares a transaction by adding relayer-specific instructions.
    ///
    /// # Description
    ///
    /// This function takes an existing Base64-encoded serialized transaction and adds
    /// relayer-specific instructions.
    /// The updated transaction will include additional data required by the relayer, and the
    /// function also provides updated fee information and an expiration block height.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded serialized transaction that the end user would like
    ///   relayed.
    /// * `fee_token` - A string representing the token mint address to be used for fee payment.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded transaction with the added relayer-specific instructions.
    /// * `fee_in_spl` - The fee amount in SPL tokens (in the smallest unit).
    /// * `fee_in_lamports` - The fee amount in lamports.
    /// * `fee_token` - The token mint address used for fee payments.
    /// * `valid_until_block_height` - The block height until which the transaction remains valid.
    pub(crate) async fn prepare_transaction_impl(
        &self,
        params: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        // Validate transaction
        validate_prepare_transaction(
            &transaction_request,
            &params.fee_token,
            &self.relayer,
            &*self.provider,
        )
        .await?;

        let recent_blockhash = self
            .provider
            .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
            .await?;

        // Create new transaction message with relayer as fee payer
        let mut message = transaction_request.message.clone();
        message.recent_blockhash = recent_blockhash.0;
        // Update fee payer
        if message.account_keys[0] != relayer_pubkey {
            message.account_keys[0] = relayer_pubkey;
        }

        // Create new transaction with single signature slot
        let transaction = Transaction {
            signatures: vec![Signature::default()],
            message,
        };

        let total_fee = self.estimate_fee_payer_total_fee(&transaction).await?;
        let lamports_outflow = self.estimate_relayer_lampart_outflow(&transaction).await?;
        let total_outflow = total_fee + lamports_outflow;

        // Validate relayer has sufficient balance
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_outflow,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await?;

        // Get fee quote
        let fee_quote = self
            .get_fee_token_quote(&params.fee_token, total_fee)
            .await?;

        // Sign transaction
        let (signed_transaction, _) = self.relayer_sign_transaction(transaction)?;

        // Serialize and encode
        let encoded_tx = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        Ok(PrepareTransactionResult {
            transaction: encoded_tx,
            fee_in_spl: fee_quote.fee_in_spl.to_string(),
            fee_in_lamports: fee_quote.fee_in_lamports.to_string(),
            fee_token: params.fee_token,
            valid_until_blockheight: recent_blockhash.1,
        })
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
        SolanaTransactionValidator::validate_lamports_transfers(tx, policy, &relayer_pubkey),
        SolanaTransactionValidator::validate_token_transfers(tx, policy, provider, &relayer_pubkey,),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use crate::{
        constants::SOL_MINT,
        models::{RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy},
    };

    use super::*;
    use solana_sdk::{
        hash::Hash, message::Message, signature::Keypair, signer::Signer, system_instruction,
    };
    use spl_associated_token_account::get_associated_token_address;

    #[tokio::test]
    async fn test_prepare_transaction_success() {
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx) =
            setup_test_context();

        // Setup policy with SOL
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

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
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        assert_eq!(prepare_result.fee_token, SOL_MINT);
        assert_eq!(prepare_result.fee_in_lamports, "5000");
        assert_eq!(prepare_result.fee_in_spl, "5000");
        assert_eq!(prepare_result.valid_until_blockheight, 100);
    }

    #[tokio::test]
    async fn test_prepare_transaction_insufficient_balance() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Set high minimum balance requirement
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: 100_000_000, // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
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
                })
            })
        });
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(matches!(
            result,
            Err(SolanaRpcError::SolanaTransactionValidation(
                SolanaTransactionValidationError::InsufficientFunds(_)
            ))
        ));
    }

    #[tokio::test]
    async fn test_prepare_transaction_updates_fee_payer() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: 100_000_000, // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&wrong_fee_payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();
        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

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
                })
            })
        });
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
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
        let (mut relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();
        println!("Setting up known keypair for signature verification");
        let relayer_keypair = Keypair::new();
        let expected_signature = Signature::new_unique();

        relayer.address = relayer_keypair.pubkey().to_string();
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: 100_000_000, // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

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
                })
            })
        });

        // Create test transaction
        let ix = system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);
        let message = Message::new(&[ix], Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
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
        let (mut relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Configure policy with allowed tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "AllowedToken111111111111111111111111111111".to_string(),
                symbol: Some("ALLOWED".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
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
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
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
