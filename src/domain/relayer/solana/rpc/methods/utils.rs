//! # Solana RPC Method Utilities
//!
//! This module contains utility functions and structures used by the Solana RPC methods
//! implementation. It provides common functionality for fee estimation, transaction
//! preparation, signing, and token operations.
//!
//! ## Key Components
//!
//! * `FeeQuote` - Data structure containing fee estimates in both SPL tokens and lamports
//! * Transaction signing utilities for relayer operations
//! * Fee estimation functions that handle both transaction fees and account creation costs
//! * Transaction creation and signing helpers for common operations
//! * Token transfer utilities that handle associated token account (ATA) creation when needed
//!
//! ## Usage
//!
//! These utilities are primarily used internally by the RPC method implementations to:
//!
//! 1. Calculate accurate fee estimates for transactions
//! 2. Handle the conversion between SOL and other tokens for fee payments
//! 3. Create, sign, and prepare transactions for submission
//! 4. Support token transfers with automatic ATA creation
//!
//! The implementation leverages the Jupiter API for token price quotes and the Solana
//! SDK for transaction manipulation.
use std::str::FromStr;

use super::*;

use solana_sdk::{
    commitment_config::CommitmentConfig, hash::Hash, instruction::Instruction, message::Message,
    program_pack::Pack, pubkey::Pubkey, signature::Signature,
    system_instruction::SystemInstruction, system_program, transaction::Transaction,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::{amount_to_ui_amount, state::Account};

use crate::{
    constants::{DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, SOLANA_DECIMALS, SOL_MINT},
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

pub struct FeeQuote {
    pub fee_in_spl: u64,
    pub fee_in_spl_ui: String,
    pub fee_in_lamports: u64,
    pub conversion_rate: f64,
}

impl<P, S, J, JP> SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
{
    /// Signs a transaction with the relayer's keypair and returns both the signed transaction and
    /// signature.
    ///
    /// This method takes an unsigned (or partially signed) transaction and applies the relayer's
    /// signature to the first signature slot. It's designed to be used by all RPC methods that
    /// require a relayer-signed transaction.
    ///
    /// # Arguments
    ///
    /// * `transaction` - A Solana transaction that needs the relayer's signature. This transaction
    ///   must have at least one signature slot available (typically the first one).
    ///
    /// # Returns
    ///
    /// Returns a result containing:
    /// * A tuple with the signed transaction and the signature on success
    /// * A `SolanaRpcError` on failure (e.g., if signing fails)
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The signer service fails to sign the transaction data
    /// * The transaction format is invalid
    pub(crate) fn relayer_sign_transaction(
        &self,
        mut transaction: Transaction,
    ) -> Result<(Transaction, Signature), SolanaRpcError> {
        let signature = self.signer.sign(&transaction.message_data())?;

        transaction.signatures[0] = signature;

        Ok((transaction, signature))
    }

    /// Estimates the total fee that the fee payer will incur for a given transaction.
    ///
    /// This function calculates the base transaction fee and adds the cost of creating
    /// associated token accounts (ATAs) if any are included in the transaction.
    ///
    /// # Arguments
    ///
    /// * `transaction` - A reference to the transaction for which the fee is being estimated.
    ///
    /// # Returns
    ///
    /// Returns a result containing:
    /// * The total fee in lamports on success
    /// * A `SolanaRpcError` on failure
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The provider fails to calculate the total fee
    /// * The provider fails to get the minimum balance for rent exemption
    pub(crate) async fn estimate_fee_payer_total_fee(
        &self,
        transaction: &Transaction,
    ) -> Result<u64, SolanaRpcError> {
        let tx_fee = self
            .provider
            .calculate_total_fee(transaction.message())
            .await?;

        // Count ATA creation instructions
        let ata_creations = transaction
            .message
            .instructions
            .iter()
            .filter(|ix| {
                transaction.message.account_keys[ix.program_id_index as usize]
                    == spl_associated_token_account::id()
            })
            .count();

        if ata_creations == 0 {
            return Ok(tx_fee);
        }

        let account_creation_fee = self
            .provider
            .get_minimum_balance_for_rent_exemption(Account::LEN)
            .await?;

        Ok(tx_fee + (ata_creations as u64 * account_creation_fee))
    }

    /// Estimates the total lamport outflow from the relayer's account for a given transaction.
    ///
    /// This function iterates over the transaction's instructions and sums up the lamports
    /// transferred from the relayer's account.
    ///
    /// # Arguments
    ///
    /// * `tx` - A reference to the transaction for which the lamport outflow is being estimated.
    ///
    /// # Returns
    ///
    /// Returns a result containing:
    /// * The total lamport outflow on success
    /// * A `SolanaRpcError` on failure
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The relayer's public key cannot be parsed
    /// * The transaction contains invalid instructions
    pub(crate) async fn estimate_relayer_lampart_outflow(
        &self,
        tx: &Transaction,
    ) -> Result<u64, SolanaRpcError> {
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        let mut total_lamports_outflow: u64 = 0;
        for (ix_index, ix) in tx.message.instructions.iter().enumerate() {
            let program_id = tx.message.account_keys[ix.program_id_index as usize];

            // Check if the instruction comes from the System Program (native SOL transfers)
            #[allow(clippy::collapsible_match)]
            if program_id == system_program::id() {
                if let Ok(system_ix) = bincode::deserialize::<SystemInstruction>(&ix.data) {
                    if let SystemInstruction::Transfer { lamports } = system_ix {
                        // In a system transfer instruction, the first account is the source and the
                        // second is the destination.
                        let source_index = ix.accounts.first().ok_or_else(|| {
                            SolanaRpcError::Internal(format!(
                                "Missing source account in instruction {}",
                                ix_index
                            ))
                        })?;
                        let source_pubkey = &tx.message.account_keys[*source_index as usize];

                        // Only validate transfers where the source is the relayer fee account.
                        if source_pubkey == &relayer_pubkey {
                            total_lamports_outflow += lamports;
                        }
                    }
                }
            }
        }

        Ok(total_lamports_outflow)
    }

    /// Retrieves a fee quote for a specified token and total fee amount.
    ///
    /// This function calculates the fee in both SPL tokens and lamports, using the Jupiter
    /// service to get conversion rates if the token is not SOL.
    ///
    /// # Arguments
    ///
    /// * `token` - The mint address of the token for which the fee quote is requested.
    /// * `total_fee` - The total fee amount in lamports.
    ///
    /// # Returns
    ///
    /// Returns a result containing:
    /// * A `FeeQuote` structure on success
    /// * A `SolanaRpcError` on failure
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The token is not allowed by the relayer's policy
    /// * The Jupiter service fails to provide a quote
    pub(crate) async fn get_fee_token_quote(
        &self,
        token: &str,
        total_fee: u64,
    ) -> Result<FeeQuote, SolanaRpcError> {
        // If token is SOL, return direct conversion
        if token == SOL_MINT {
            return Ok(FeeQuote {
                fee_in_spl: total_fee,
                fee_in_spl_ui: amount_to_ui_amount(total_fee, SOLANA_DECIMALS).to_string(),
                fee_in_lamports: total_fee,
                conversion_rate: 1f64,
            });
        }

        // Get token policy
        let token_entry = self
            .relayer
            .policies
            .get_solana_policy()
            .get_allowed_token_entry(token)
            .ok_or_else(|| {
                SolanaRpcError::UnsupportedFeeToken(format!("Token {} not allowed", token))
            })?;

        // Get token decimals
        let decimals = token_entry.decimals.ok_or_else(|| {
            SolanaRpcError::Estimation("Token decimals not configured".to_string())
        })?;

        // Get slippage from policy
        let slippage = token_entry
            .conversion_slippage_percentage
            .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE);

        // Get Jupiter quote
        let quote = self
            .jupiter_service
            .get_sol_to_token_quote(token, total_fee, slippage)
            .await
            .map_err(|e| SolanaRpcError::Estimation(e.to_string()))?;

        let fee_in_spl_ui = amount_to_ui_amount(quote.out_amount, decimals);
        let fee_in_sol_ui = amount_to_ui_amount(quote.in_amount, SOLANA_DECIMALS);
        let conversion_rate = fee_in_spl_ui / fee_in_sol_ui;

        Ok(FeeQuote {
            fee_in_spl: quote.out_amount,
            fee_in_spl_ui: fee_in_spl_ui.to_string(),
            fee_in_lamports: total_fee,
            conversion_rate,
        })
    }

    /// Creates and signs a transaction with the provided instructions.
    ///
    /// This function constructs a transaction using the given instructions, signs it with
    /// the relayer's keypair, and returns the signed transaction along with the recent blockhash.
    ///
    /// # Arguments
    ///
    /// * `instructions` - A vector of Solana instructions to include in the transaction.
    ///
    /// # Returns
    ///
    /// Returns a result containing:
    /// * A tuple with the signed transaction and the recent blockhash on success
    /// * A `SolanaRpcError` on failure
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The provider fails to get the latest blockhash
    /// * The transaction signing fails
    pub(crate) async fn create_and_sign_transaction(
        &self,
        instructions: Vec<Instruction>,
    ) -> Result<(Transaction, (Hash, u64)), SolanaRpcError> {
        let recent_blockhash = self
            .provider
            .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
            .await?;

        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        let message =
            Message::new_with_blockhash(&instructions, Some(&relayer_pubkey), &recent_blockhash.0);

        let transaction = Transaction::new_unsigned(message);

        let (signed_transaction, _) = self.relayer_sign_transaction(transaction)?;

        Ok((signed_transaction, recent_blockhash))
    }

    /// Handles a token transfer between two accounts, creating an associated token account (ATA)
    /// for the destination if necessary.
    ///
    /// This function verifies the source account's balance, creates a destination ATA if it
    /// doesn't exist, and prepares the necessary instructions for the token transfer.
    ///
    /// # Arguments
    ///
    /// * `source` - The public key of the source account.
    /// * `destination` - The public key of the destination account.
    /// * `token_mint` - The mint address of the token being transferred.
    /// * `amount` - The amount of tokens to transfer.
    ///
    /// # Returns
    ///
    /// Returns a result containing:
    /// * A vector of instructions for the token transfer on success
    /// * A `SolanaRpcError` on failure
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The source account has insufficient funds
    /// * The token account is invalid
    /// * The token is not allowed by the relayer's policy
    pub(crate) async fn handle_token_transfer(
        &self,
        source: &Pubkey,
        destination: &Pubkey,
        token_mint: &Pubkey,
        amount: u64,
    ) -> Result<Vec<Instruction>, SolanaRpcError> {
        let mut instructions = Vec::new();
        let source_ata = get_associated_token_address(source, token_mint);
        let destination_ata = get_associated_token_address(destination, token_mint);

        // Verify source account and balance
        let source_account = self.provider.get_account_from_pubkey(&source_ata).await?;
        let unpacked_source_account = Account::unpack(&source_account.data)
            .map_err(|e| SolanaRpcError::InvalidParams(format!("Invalid token account: {}", e)))?;

        if unpacked_source_account.amount < amount {
            return Err(SolanaRpcError::InsufficientFunds(
                "Insufficient token balance".to_string(),
            ));
        }

        // Create destination ATA if needed
        if self
            .provider
            .get_account_from_pubkey(&destination_ata)
            .await
            .is_err()
        {
            let relayer_pubkey = &Pubkey::from_str(&self.relayer.address)
                .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

            instructions.push(create_associated_token_account(
                relayer_pubkey,
                destination,
                token_mint,
                destination,
            ));
        }
        let token_decimals = self
            .relayer
            .policies
            .get_solana_policy()
            .get_allowed_token_decimals(&token_mint.to_string())
            .ok_or_else(|| {
                SolanaRpcError::UnsupportedFeeToken("Token not found in allowed tokens".to_string())
            })?;

        instructions.push(
            spl_token::instruction::transfer_checked(
                &spl_token::id(),
                &source_ata,
                token_mint,
                &destination_ata,
                source,
                &[],
                amount,
                token_decimals,
            )
            .map_err(|e| SolanaRpcError::TransactionPreparation(e.to_string()))?,
        );

        Ok(instructions)
    }
}

#[cfg(test)]
mod tests {

    use crate::{
        models::{RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy},
        services::QuoteResponse,
    };

    use super::*;
    use solana_sdk::{
        instruction::AccountMeta,
        signature::{Keypair, Signature},
        signer::Signer,
        system_instruction,
    };

    #[test]
    fn test_relayer_sign_transaction() {
        let (relayer, mut signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let result = rpc.relayer_sign_transaction(transaction);

        assert!(result.is_ok(), "Transaction signing should succeed");
        let (signed_tx, signature) = result.unwrap();
        assert_eq!(
            signed_tx.signatures[0], signature,
            "Returned signature should match transaction signature"
        );
        assert_eq!(signature.as_ref().len(), 64, "Signature should be 64 bytes");
    }

    #[tokio::test]
    async fn test_get_fee_token_quote_sol() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer) =
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

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let result = rpc.get_fee_token_quote(SOL_MINT, 1_000_000).await;
        assert!(result.is_ok());

        let quote = result.unwrap();
        assert_eq!(quote.fee_in_spl, 1_000_000);
        assert_eq!(quote.fee_in_spl_ui, "0.001");
        assert_eq!(quote.fee_in_lamports, 1_000_000);
        assert_eq!(quote.conversion_rate, 1.0);
    }

    #[tokio::test]
    async fn test_get_fee_token_quote_spl_token() {
        let (mut relayer, signer, provider, mut jupiter_service, _, job_producer) =
            setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        // let test_token = test_token.to_string();
        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, amount, _| {
                Box::pin(async move {
                    Ok(QuoteResponse {
                        input_mint: SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: amount,
                        out_amount: 2_000_000, // 1 SOL = 2 USDC
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
            Arc::new(job_producer),
        );

        let result = rpc.get_fee_token_quote(test_token, 1_000_000_000).await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.fee_in_spl, 2_000_000);
        assert_eq!(quote.fee_in_spl_ui, "2");
        assert_eq!(quote.fee_in_lamports, 1_000_000_000);
        assert_eq!(quote.conversion_rate, 2.0);
    }

    #[tokio::test]
    async fn test_estimate_fee_no_ata_creation() {
        let (relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        // Setup provider mock
        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        // Create simple transfer transaction
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_fee_payer_total_fee(&transaction).await;

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            5000,
            "Should return base transaction fee only"
        );
    }

    #[tokio::test]
    async fn test_estimate_fee_with_single_ata_creation() {
        let (relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        // Setup provider expectations
        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_minimum_balance_for_rent_exemption()
            .returning(|_| Box::pin(async { Ok(2039280) })); // Typical rent exemption

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let payer = Keypair::new();
        let owner = Pubkey::new_unique();
        let mint = Pubkey::new_unique();

        let ata_ix =
            create_associated_token_account(&payer.pubkey(), &owner, &mint, &spl_token::id());

        let message = Message::new(&[ata_ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_fee_payer_total_fee(&transaction).await;

        assert!(result.is_ok());
        let total_fee = result.unwrap();
        assert_eq!(
            total_fee,
            5000 + 2039280,
            "Should include base fee plus rent exemption"
        );
    }

    #[tokio::test]
    async fn test_estimate_fee_with_multiple_ata_creations() {
        let (relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_minimum_balance_for_rent_exemption()
            .returning(|_| Box::pin(async { Ok(2039280) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let payer = Keypair::new();
        let owner1 = Pubkey::new_unique();
        let owner2 = Pubkey::new_unique();
        let mint = Pubkey::new_unique();

        let ata_ix1 =
            create_associated_token_account(&payer.pubkey(), &owner1, &mint, &spl_token::id());
        let ata_ix2 =
            create_associated_token_account(&payer.pubkey(), &owner2, &mint, &spl_token::id());

        let message = Message::new(&[ata_ix1, ata_ix2], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_fee_payer_total_fee(&transaction).await;
        assert!(result.is_ok());
        let total_fee = result.unwrap();
        assert_eq!(
            total_fee,
            5000 + (2 * 2039280),
            "Should include base fee plus rent exemption for two ATAs"
        );
    }

    #[tokio::test]
    async fn test_estimate_no_lamport_outflow() {
        let (relayer, signer, provider, jupiter_service, _, job_producer) = setup_test_context();
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let payer = Keypair::new();
        let message = Message::new(&[], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_relayer_lampart_outflow(&transaction).await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            0,
            "Should return zero when no SOL transfers"
        );
    }

    #[tokio::test]
    async fn test_estimate_single_lamport_transfer() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();

        // Set relayer address
        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let recipient = Pubkey::new_unique();
        let transfer_amount = 1_000_000;
        let ix =
            system_instruction::transfer(&relayer_keypair.pubkey(), &recipient, transfer_amount);
        let message = Message::new(&[ix], Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_relayer_lampart_outflow(&transaction).await;

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            transfer_amount,
            "Should count transfer amount from relayer"
        );
    }

    #[tokio::test]
    async fn test_estimate_multiple_lamport_transfers() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let recipient1 = Pubkey::new_unique();
        let recipient2 = Pubkey::new_unique();
        let amount1 = 1_000_000;
        let amount2 = 2_000_000;

        let instructions = vec![
            system_instruction::transfer(&relayer_keypair.pubkey(), &recipient1, amount1),
            system_instruction::transfer(&relayer_keypair.pubkey(), &recipient2, amount2),
        ];

        let message = Message::new(&instructions, Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_relayer_lampart_outflow(&transaction).await;

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            amount1 + amount2,
            "Should sum all transfers from relayer"
        );
    }

    #[tokio::test]
    async fn test_estimate_ignore_non_relayer_transfers() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();
        let other_keypair = Keypair::new();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let recipient = Pubkey::new_unique();
        let relayer_amount = 1_000_000;
        let other_amount = 2_000_000;

        let instructions = vec![
            system_instruction::transfer(&relayer_keypair.pubkey(), &recipient, relayer_amount),
            system_instruction::transfer(&other_keypair.pubkey(), &recipient, other_amount),
        ];

        let message = Message::new(&instructions, Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_relayer_lampart_outflow(&transaction).await;

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            relayer_amount,
            "Should only count transfers from relayer"
        );
    }

    #[tokio::test]
    async fn test_estimate_non_system_program_instructions() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        // Create non-system program instruction
        let other_program = Pubkey::new_unique();
        let other_ix = Instruction {
            program_id: other_program,
            accounts: vec![
                AccountMeta::new(relayer_keypair.pubkey(), true),
                AccountMeta::new(Pubkey::new_unique(), false),
            ],
            data: vec![0],
        };

        let message = Message::new(&[other_ix], Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.estimate_relayer_lampart_outflow(&transaction).await;

        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            0,
            "Should ignore non-system program instructions"
        );
    }

    #[tokio::test]
    async fn test_create_and_sign_transaction_success() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();
        let recipient = Pubkey::new_unique();
        let amount = 1_000_000;

        let expected_signature = Signature::new_unique();
        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature));

        let expected_blockhash = Hash::new_unique();
        let expected_slot = 100u64;
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(move |_| Box::pin(async move { Ok((expected_blockhash, expected_slot)) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let instructions = vec![system_instruction::transfer(
            &relayer_keypair.pubkey(),
            &recipient,
            amount,
        )];
        let result = rpc.create_and_sign_transaction(instructions).await;

        assert!(result.is_ok(), "Transaction creation should succeed");

        let (signed_tx, (_, slot)) = result.unwrap();

        assert_eq!(signed_tx.message.recent_blockhash, expected_blockhash);
        assert_eq!(slot, expected_slot);
        assert_eq!(signed_tx.signatures[0], expected_signature);
        assert_eq!(
            signed_tx.message.account_keys[0],
            Pubkey::from_str(&relayer_keypair.pubkey().to_string()).unwrap()
        );
    }

    #[tokio::test]
    async fn test_create_and_sign_transaction_multiple_instructions() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();

        signer
            .expect_sign()
            .returning(|_| Ok(Signature::new_unique()));

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let recipient1 = Pubkey::new_unique();
        let recipient2 = Pubkey::new_unique();

        let instructions = vec![
            system_instruction::transfer(&relayer_keypair.pubkey(), &recipient1, 1000),
            system_instruction::transfer(&relayer_keypair.pubkey(), &recipient2, 2000),
        ];

        let result = rpc.create_and_sign_transaction(instructions).await;

        assert!(result.is_ok());
        let (signed_tx, _) = result.unwrap();
        assert_eq!(signed_tx.message.instructions.len(), 2);
    }
}
