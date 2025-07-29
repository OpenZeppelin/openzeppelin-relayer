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
use super::*;
use std::str::FromStr;

use log::debug;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    hash::Hash,
    instruction::{AccountMeta, CompiledInstruction, Instruction},
    message::{Message, MessageHeader},
    program_pack::Pack,
    pubkey::Pubkey,
    signature::Signature,
    system_instruction::SystemInstruction,
    transaction::Transaction,
};
use solana_system_interface::program;

use spl_token::{amount_to_ui_amount, state::Account};

use crate::{
    constants::{
        DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, NATIVE_SOL, SOLANA_DECIMALS, WRAPPED_SOL_MINT,
    },
    domain::{SolanaTokenProgram, TokenInstruction},
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

pub struct FeeQuote {
    pub fee_in_spl: u64,
    pub fee_in_spl_ui: String,
    pub fee_in_lamports: u64,
    pub conversion_rate: f64,
}

impl Default for FeeQuote {
    fn default() -> Self {
        Self {
            fee_in_spl: 0,
            fee_in_spl_ui: "0".to_string(),
            fee_in_lamports: 0,
            conversion_rate: 0.0,
        }
    }
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
    pub(crate) async fn relayer_sign_transaction(
        &self,
        mut transaction: Transaction,
    ) -> Result<(Transaction, Signature), SolanaRpcError> {
        // Parse relayer public key
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        // Find the position of the relayer's public key in account_keys
        let signer_index = transaction
            .message
            .account_keys
            .iter()
            .position(|key| *key == relayer_pubkey)
            .ok_or_else(|| {
                SolanaRpcError::Internal(
                    "Relayer public key not found in transaction signers".to_string(),
                )
            })?;

        // Check if this is a signer position (within num_required_signatures)
        if signer_index >= transaction.message.header.num_required_signatures as usize {
            return Err(SolanaRpcError::Internal(
                "Relayer is not marked as a required signer in the transaction".to_string(),
            ));
        }

        // Generate signature
        let signature = self.signer.sign(&transaction.message_data()).await?;

        // Ensure signatures array has enough elements
        while transaction.signatures.len() <= signer_index {
            transaction.signatures.push(Signature::default());
        }

        // Place signature in the correct position
        transaction.signatures[signer_index] = signature;

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
    #[allow(dead_code)]
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
            if program_id == program::id() {
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
        // If token is WSOL/SOL, return direct conversion
        if token == NATIVE_SOL || token == WRAPPED_SOL_MINT {
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
            .swap_config
            .as_ref()
            .and_then(|config| config.slippage_percentage)
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
    pub(crate) async fn create_transaction(
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

        Ok((transaction, recent_blockhash))
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
        let (transaction, recent_blockhash) = self.create_transaction(instructions).await?;

        let (signed_transaction, _) = self.relayer_sign_transaction(transaction).await?;

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
        let mut instructions: Vec<Instruction> = Vec::new();
        let program_id =
            SolanaTokenProgram::get_token_program_for_mint(&*self.provider, token_mint).await?;
        let source_ata =
            SolanaTokenProgram::get_associated_token_address(&program_id, source, token_mint);
        let destination_ata =
            SolanaTokenProgram::get_associated_token_address(&program_id, destination, token_mint);

        // Verify source account and balance
        let unpacked_source_account =
            SolanaTokenProgram::get_and_unpack_token_account(&*self.provider, source, token_mint)
                .await?;

        if unpacked_source_account.amount < amount {
            return Err(SolanaRpcError::InsufficientFunds(format!(
                "Insufficient token balance: required {} but found {}",
                amount, unpacked_source_account.amount
            )));
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

            instructions.push(SolanaTokenProgram::create_associated_token_account(
                &program_id,
                relayer_pubkey,
                destination,
                token_mint,
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

        instructions.push(SolanaTokenProgram::create_transfer_checked_instruction(
            &program_id,
            &source_ata,
            token_mint,
            &destination_ata,
            source,
            amount,
            token_decimals,
        )?);

        Ok(instructions)
    }

    /// This function estimates the fee for a transaction and applies a margin if specified.
    pub(crate) async fn estimate_fee_with_margin(
        &self,
        transaction: &Transaction,
        fee_margin_percentage: Option<f32>,
    ) -> Result<u64, SolanaRpcError> {
        // Estimate the fee
        let total_fee = self
            .estimate_fee_payer_total_fee(transaction)
            .await
            .map_err(|e| {
                error!("Failed to estimate total fee: {}", e);
                SolanaRpcError::Estimation(e.to_string())
            })?;

        debug!("Estimated SOL fee: {} lamports", total_fee);

        // Apply buffer if specified
        let buffered_fee = if let Some(factor) = fee_margin_percentage {
            (total_fee as f64 * (1.0 + factor as f64 / 100.0)) as u64
        } else {
            total_fee
        };

        Ok(buffered_fee)
    }

    /// Estimates fee for a transaction and converts it to the specified token
    pub(crate) async fn estimate_and_convert_fee(
        &self,
        transaction: &Transaction,
        fee_token: &str,
        fee_margin_percentage: Option<f32>,
    ) -> Result<(FeeQuote, u64), SolanaRpcError> {
        let fee_with_margin = self
            .estimate_fee_with_margin(transaction, fee_margin_percentage)
            .await?;

        // Convert to token quote
        let fee_quote = self
            .get_fee_token_quote(fee_token, fee_with_margin)
            .await
            .map_err(|e| {
                error!("Failed to fee quote: {}", e);
                SolanaRpcError::Estimation(e.to_string())
            })?;

        debug!(
            "Fee estimate: {} {} (SOL fee: {} lamports, conversion rate: {})",
            fee_quote.fee_in_spl_ui, fee_token, fee_with_margin, fee_quote.conversion_rate
        );

        Ok((fee_quote, fee_with_margin))
    }

    /// Converts a compiled instruction to a Solana SDK instruction.
    pub(crate) fn convert_compiled_instruction(
        &self,
        compiled_instruction: &CompiledInstruction,
        account_keys: &[Pubkey],
        header: &MessageHeader,
    ) -> Instruction {
        // Retrieve the program id using the program_id_index
        let program_id = account_keys[compiled_instruction.program_id_index as usize];

        let account_metas = compiled_instruction
            .accounts
            .iter()
            .map(|&index| {
                let key = account_keys[index as usize];
                let is_signer = (index as usize) < header.num_required_signatures as usize;

                let is_writable = if is_signer {
                    // Writable signers are first (total_signers - readonly_signers)
                    (index as usize)
                        < header.num_required_signatures as usize
                            - header.num_readonly_signed_accounts as usize
                } else {
                    // Writable non-signers are first (total_non_signers - readonly_unsigned)
                    let non_signer_index =
                        (index as usize) - header.num_required_signatures as usize;
                    non_signer_index
                        < (account_keys.len() - header.num_required_signatures as usize)
                            - header.num_readonly_unsigned_accounts as usize
                };

                AccountMeta {
                    pubkey: key,
                    is_signer,
                    is_writable,
                }
            })
            .collect();

        Instruction {
            program_id,
            accounts: account_metas,
            data: compiled_instruction.data.clone(),
        }
    }

    /// Creates a modified transaction with user fee payment instruction
    pub(crate) async fn create_transaction_with_user_fee_payment(
        &self,
        relayer_pubkey: &Pubkey,
        transaction_request: &Transaction,
        fee_token: &str,
        amount: u64,
    ) -> Result<(Transaction, (Hash, u64)), SolanaRpcError> {
        // Get source address (fee payer) from the transaction
        let source = transaction_request.message.account_keys[0];

        let token_mint = Pubkey::from_str(fee_token)
            .map_err(|_| SolanaRpcError::InvalidParams("Invalid token mint address".to_string()))?;

        if &source == relayer_pubkey {
            return Err(SolanaRpcError::InvalidParams(
                "Relayer cannot pay fee to itself".to_string(),
            ));
        }

        let draft_fee_instructions = self
            .handle_token_transfer(&source, relayer_pubkey, &token_mint, amount)
            .await?;

        let original_instructions = transaction_request
            .message
            .instructions
            .iter()
            .map(|ci| {
                self.convert_compiled_instruction(
                    ci,
                    &transaction_request.message.account_keys,
                    &transaction_request.message.header,
                )
            })
            .collect::<Vec<Instruction>>();

        let mut all_instructions =
            Vec::with_capacity(draft_fee_instructions.len() + original_instructions.len());
        all_instructions.extend(draft_fee_instructions);
        all_instructions.extend(original_instructions);

        let recent_blockhash = self
            .provider
            .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
            .await?;

        let message = solana_sdk::message::Message::new_with_blockhash(
            &all_instructions,
            Some(relayer_pubkey), // Set relayer as fee payer
            &recent_blockhash.0,
        );

        let transaction = Transaction {
            signatures: vec![solana_sdk::signature::Signature::default()], // Placeholder signature
            message,
        };

        // Create new transaction with the modified message
        Ok((transaction, recent_blockhash))
    }

    /// Validates that the transaction includes a proper fee payment instruction
    /// when user is required to pay fees
    pub(crate) async fn confirm_user_fee_payment(
        &self,
        transaction: &Transaction,
        estimated_fee: u64,
    ) -> Result<(), SolanaRpcError> {
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|_| SolanaRpcError::Internal("Invalid relayer address".to_string()))?;

        // Check if transaction contains a SOL transfer to the relayer
        let sol_payment = self.find_sol_payment_to_relayer(transaction, &relayer_pubkey);

        // Find any token transfers to the relayer
        let token_payments = self
            .find_token_payments_to_relayer(transaction, &relayer_pubkey)
            .await?;

        // Check if either SOL payment or token payment is sufficient
        if let Some(sol_amount) = sol_payment {
            if sol_amount >= estimated_fee {
                // SOL payment is sufficient
                return Ok(());
            }
        }

        // Check if any token payment is sufficient
        if !token_payments.is_empty() {
            for (token_mint, amount) in token_payments {
                // Get the conversion rate for this token
                let fee_quote = self
                    .get_fee_token_quote(&token_mint.to_string(), estimated_fee)
                    .await?;

                if amount >= fee_quote.fee_in_spl {
                    return Ok(());
                }
            }
        }

        // If we reach here, no sufficient payment was found
        Err(SolanaRpcError::InvalidParams(
            "Transaction doesn't contain required fee payment instruction or payment amount is insufficient".to_string(),
        ))
    }

    /// Finds SOL payment to the relayer in the transaction
    pub(crate) fn find_sol_payment_to_relayer(
        &self,
        transaction: &Transaction,
        relayer_pubkey: &Pubkey,
    ) -> Option<u64> {
        // Look for system program transfers to relayer
        for ix in transaction.message.instructions.iter() {
            let program_id = transaction.message.account_keys[ix.program_id_index as usize];

            // Check if it's system program
            if program_id == program::id() {
                if let Ok(SystemInstruction::Transfer { lamports }) =
                    bincode::deserialize::<SystemInstruction>(&ix.data)
                {
                    // Check destination account
                    if ix.accounts.len() >= 2 {
                        let dest_idx = ix.accounts[1] as usize;
                        if dest_idx < transaction.message.account_keys.len() {
                            let dest = transaction.message.account_keys[dest_idx];
                            if dest == *relayer_pubkey {
                                return Some(lamports);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Finds token payments to the relayer in the transaction
    ///
    /// This function checks the transaction instructions for token transfers to the relayer's
    /// token accounts. It verifies that the destination token account is owned by the relayer
    /// and that the source token account has sufficient balance.
    pub(crate) async fn find_token_payments_to_relayer(
        &self,
        transaction: &Transaction,
        relayer_pubkey: &Pubkey,
    ) -> Result<Vec<(Pubkey, u64)>, SolanaRpcError> {
        let mut payments = Vec::new();

        // Get relayer's token accounts for allowed tokens
        let policy = self.relayer.policies.get_solana_policy();
        let allowed_tokens = match &policy.allowed_tokens {
            Some(tokens) => tokens,
            None => return Ok(payments),
        };

        for ix in &transaction.message.instructions {
            let program_id = transaction.message.account_keys[ix.program_id_index as usize];

            if !SolanaTokenProgram::is_token_program(&program_id) {
                continue;
            }

            let token_ix = match SolanaTokenProgram::unpack_instruction(&program_id, &ix.data) {
                Ok(ix) => ix,
                Err(_) => continue,
            };

            match token_ix {
                TokenInstruction::Transfer { amount }
                | TokenInstruction::TransferChecked { amount, .. } => {
                    if ix.accounts.len() < 2 {
                        continue;
                    }

                    // Get source and destination token accounts from instruction
                    let source_token_idx = ix.accounts[0] as usize;
                    let dest_token_idx = match token_ix {
                        TokenInstruction::TransferChecked { .. } => ix.accounts[2] as usize,
                        _ => ix.accounts[1] as usize,
                    };

                    if dest_token_idx >= transaction.message.account_keys.len()
                        || source_token_idx >= transaction.message.account_keys.len()
                    {
                        continue;
                    }

                    let source_token_account = transaction.message.account_keys[source_token_idx];
                    let dest_token_account = transaction.message.account_keys[dest_token_idx];

                    // Check destination account first
                    match self
                        .provider
                        .get_account_from_pubkey(&dest_token_account)
                        .await
                    {
                        Ok(dest_account) => {
                            if !SolanaTokenProgram::is_token_program(&dest_account.owner) {
                                continue;
                            }

                            let dest_token_account = match SolanaTokenProgram::unpack_account(
                                &program_id,
                                &dest_account,
                            ) {
                                Ok(account) => account,
                                Err(e) => {
                                    error!("Failed to unpack destination token account: {}", e);
                                    continue;
                                }
                            };

                            // Check if destination token account is owned by relayer
                            if dest_token_account.owner != *relayer_pubkey {
                                debug!(
                                    "Token account owner {} is not relayer {}",
                                    dest_token_account.owner, relayer_pubkey
                                );
                                continue;
                            }

                            // Now check source account balance
                            match self
                                .provider
                                .get_account_from_pubkey(&source_token_account)
                                .await
                            {
                                Ok(source_account) => {
                                    if !SolanaTokenProgram::is_token_program(&source_account.owner)
                                    {
                                        debug!(
                                            "Source token account owner {} is not a token program",
                                            source_account.owner
                                        );
                                        continue;
                                    }

                                    let source_token_account =
                                        match SolanaTokenProgram::unpack_account(
                                            &program_id,
                                            &source_account,
                                        ) {
                                            Ok(account) => account,
                                            Err(e) => {
                                                error!(
                                                    "Failed to unpack source token account: {}",
                                                    e
                                                );
                                                continue;
                                            }
                                        };

                                    // Check if source has enough tokens
                                    if source_token_account.amount < amount {
                                        continue;
                                    }

                                    let token_mint = dest_token_account.mint;

                                    // Check if this token mint is in allowed tokens
                                    if allowed_tokens
                                        .iter()
                                        .any(|t| t.mint == token_mint.to_string())
                                    {
                                        payments.push((token_mint, amount));
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to get source token account: {}", e);
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to get destination token account: {}", e);
                            continue;
                        }
                    }
                }
                TokenInstruction::Other => {
                    continue;
                }
            }
        }
        Ok(payments)
    }
}

#[cfg(test)]
mod tests {

    use crate::{
        constants::WRAPPED_SOL_MINT,
        models::{
            RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
            SolanaAllowedTokensSwapConfig,
        },
        services::{QuoteResponse, RoutePlan, SwapInfo},
    };

    use super::*;
    use solana_sdk::{
        instruction::AccountMeta,
        signature::{Keypair, Signature},
        signer::Signer,
    };
    use solana_system_interface::instruction;
    use spl_associated_token_account::{
        get_associated_token_address, instruction::create_associated_token_account,
    };

    #[tokio::test]
    async fn test_relayer_sign_transaction() {
        let (relayer, mut signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();
        let relayer_pubkey = Pubkey::from_str(&relayer.address).unwrap();
        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(&relayer_pubkey, &recipient, 1000);
        let message = Message::new(&[instruction], Some(&relayer_pubkey));
        let transaction = Transaction::new_unsigned(message);
        signer.expect_sign().returning(move |_| {
            let signature = Signature::new_unique();
            let signature_clone = signature;
            Box::pin(async move { Ok(signature_clone) })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let result = rpc.relayer_sign_transaction(transaction).await;

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
                mint: NATIVE_SOL.to_string(),
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

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let result = rpc.get_fee_token_quote(NATIVE_SOL, 1_000_000).await;
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
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    slippage_percentage: Some(1.0),
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, amount, _| {
                Box::pin(async move {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: amount,
                        out_amount: 2_000_000, // 1 SOL = 2 USDC
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
        let ix = instruction::transfer(&payer.pubkey(), &recipient, 1000);
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
        let ix = instruction::transfer(&relayer_keypair.pubkey(), &recipient, transfer_amount);
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
            instruction::transfer(&relayer_keypair.pubkey(), &recipient1, amount1),
            instruction::transfer(&relayer_keypair.pubkey(), &recipient2, amount2),
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
            instruction::transfer(&relayer_keypair.pubkey(), &recipient, relayer_amount),
            instruction::transfer(&other_keypair.pubkey(), &recipient, other_amount),
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
        signer.expect_sign().returning(move |_| {
            let signature_clone = expected_signature;
            Box::pin(async move { Ok(signature_clone) })
        });

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

        let instructions = vec![instruction::transfer(
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
            .returning(|_| Box::pin(async { Ok(Signature::new_unique()) }));

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
            instruction::transfer(&relayer_keypair.pubkey(), &recipient1, 1000),
            instruction::transfer(&relayer_keypair.pubkey(), &recipient2, 2000),
        ];

        let result = rpc.create_and_sign_transaction(instructions).await;

        assert!(result.is_ok());
        let (signed_tx, _) = result.unwrap();
        assert_eq!(signed_tx.message.instructions.len(), 2);
    }

    #[tokio::test]
    async fn test_estimate_fee_with_margin_no_margin() {
        let (relayer, signer, mut provider, jupiter_service, tx, job_producer) =
            setup_test_context();

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

        let transaction = Transaction::try_from(tx).unwrap();

        let result = rpc.estimate_fee_with_margin(&transaction, None).await;

        assert!(result.is_ok(), "Fee estimation should succeed");
        assert_eq!(
            result.unwrap(),
            5000,
            "Fee without margin should equal base fee"
        );
    }

    #[tokio::test]
    async fn test_estimate_fee_with_margin_zero_margin() {
        let (relayer, signer, mut provider, jupiter_service, tx, job_producer) =
            setup_test_context();

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

        let transaction = Transaction::try_from(tx).unwrap();

        let result = rpc.estimate_fee_with_margin(&transaction, Some(0.0)).await;

        assert!(result.is_ok(), "Fee estimation should succeed");
        assert_eq!(
            result.unwrap(),
            5000,
            "Fee with 0% margin should equal base fee"
        );
    }

    #[tokio::test]
    async fn test_estimate_fee_with_margin_ten_percent() {
        let (relayer, signer, mut provider, jupiter_service, tx, job_producer) =
            setup_test_context();

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

        let transaction = Transaction::try_from(tx).unwrap();

        let result = rpc.estimate_fee_with_margin(&transaction, Some(10.0)).await;

        assert!(result.is_ok(), "Fee estimation should succeed");
        assert_eq!(result.unwrap(), 5500, "Fee with 10% margin should be 5500");
    }

    #[tokio::test]
    async fn test_estimate_fee_with_margin_large_percentage() {
        let (relayer, signer, mut provider, jupiter_service, tx, job_producer) =
            setup_test_context();

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1000u64) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let transaction = Transaction::try_from(tx).unwrap();

        let result = rpc.estimate_fee_with_margin(&transaction, Some(50.0)).await;

        assert!(result.is_ok(), "Fee estimation should succeed");
        assert_eq!(result.unwrap(), 1500, "Fee with 50% margin should be 1500");
    }

    #[tokio::test]
    async fn test_estimate_and_convert_fee_sol_with_no_margin() {
        let (mut relayer, signer, mut provider, jupiter_service, tx, job_producer) =
            setup_test_context();

        // Setup policy with SOL
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: WRAPPED_SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            fee_margin_percentage: None,
            ..Default::default()
        });

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

        let transaction = Transaction::try_from(tx).unwrap();

        let result = rpc
            .estimate_and_convert_fee(&transaction, WRAPPED_SOL_MINT, None)
            .await;

        assert!(result.is_ok(), "Fee estimation should succeed");
        let (fee_quote, lamports) = result.unwrap();

        assert_eq!(lamports, 5000, "Lamports should equal base fee");
        assert_eq!(
            fee_quote.fee_in_spl, 5000,
            "SPL fee should equal lamports for SOL"
        );
        assert_eq!(
            fee_quote.fee_in_lamports, 5000,
            "Lamports in quote should match"
        );
        assert_eq!(
            fee_quote.fee_in_spl_ui, "0.000005",
            "UI amount should be formatted correctly"
        );
        assert_eq!(
            fee_quote.conversion_rate, 1.0,
            "SOL to SOL conversion rate should be 1.0"
        );
    }

    #[tokio::test]
    async fn test_estimate_and_convert_fee_token_with_margin() {
        let (mut relayer, signer, mut provider, mut jupiter_service, tx, job_producer) =
            setup_test_context();
        // USDC token mint
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        // Setup Jupiter mock to return a fixed quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, amount, _| {
                Box::pin(async move {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: amount,
                        out_amount: 2_000_000, // 1 SOL = 2 USDC
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
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let transaction = Transaction::try_from(tx).unwrap();

        // Test with Token and 20% margin
        let result = rpc
            .estimate_and_convert_fee(&transaction, test_token, Some(20.0))
            .await;

        assert!(result.is_ok(), "Fee estimation should succeed");
        let (fee_quote, lamports) = result.unwrap();

        assert_eq!(lamports, 6000, "Lamports should include 20% margin");
        assert_eq!(
            fee_quote.fee_in_spl, 2000000,
            "SPL fee should be converted using rate with margin applied"
        );
        assert_eq!(
            fee_quote.fee_in_lamports, 6000,
            "Lamports in quote should match with margin"
        );
        assert_eq!(
            fee_quote.fee_in_spl_ui, "2",
            "UI amount should be formatted correctly for 6 decimals"
        );
        assert_eq!(
            fee_quote.conversion_rate, 333333.3333333333,
            "Conversion rate should be 2.0"
        );
    }

    #[tokio::test]
    async fn test_convert_compiled_instruction_basic() {
        let (relayer, signer, provider, jupiter_service, _, job_producer) = setup_test_context();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        // Create sample account keys
        let account_keys = vec![
            Pubkey::new_unique(), // fee payer (signer, writable)
            Pubkey::new_unique(), // another signer (signer, writable)
            Pubkey::new_unique(), // read-only signer
            Pubkey::new_unique(), // program id
            Pubkey::new_unique(), // writable non-signer
            Pubkey::new_unique(), // read-only non-signer
        ];

        // Define message header
        let header = MessageHeader {
            num_required_signatures: 3,
            num_readonly_signed_accounts: 1,
            num_readonly_unsigned_accounts: 1,
        };

        let compiled_instruction = CompiledInstruction {
            program_id_index: 3,
            accounts: vec![0, 1, 4, 2, 5],
            data: vec![1, 2, 3, 4],
        };

        let instruction =
            rpc.convert_compiled_instruction(&compiled_instruction, &account_keys, &header);

        assert_eq!(instruction.program_id, account_keys[3]);
        assert_eq!(instruction.data, vec![1, 2, 3, 4]);

        assert_eq!(instruction.accounts.len(), 5);

        // Check each account meta
        // fee payer (index 0): signer, writable
        assert_eq!(instruction.accounts[0].pubkey, account_keys[0]);
        assert!(instruction.accounts[0].is_signer);
        assert!(instruction.accounts[0].is_writable);

        // another signer (index 1): signer, writable
        assert_eq!(instruction.accounts[1].pubkey, account_keys[1]);
        assert!(instruction.accounts[1].is_signer);
        assert!(instruction.accounts[1].is_writable);

        // writable non-signer (index 4)
        assert_eq!(instruction.accounts[2].pubkey, account_keys[4]);
        assert!(!instruction.accounts[2].is_signer);
        assert!(instruction.accounts[2].is_writable);

        // read-only signer (index 2)
        assert_eq!(instruction.accounts[3].pubkey, account_keys[2]);
        assert!(instruction.accounts[3].is_signer);
        assert!(!instruction.accounts[3].is_writable);

        // read-only non-signer (index 5)
        assert_eq!(instruction.accounts[4].pubkey, account_keys[5]);
        assert!(!instruction.accounts[4].is_signer);
        assert!(!instruction.accounts[4].is_writable);
    }

    #[tokio::test]
    async fn test_convert_compiled_instruction_transfer_example() {
        let (relayer, signer, provider, jupiter_service, _, job_producer) = setup_test_context();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let amount = 1_000_000;

        let transfer_ix = instruction::transfer(&payer.pubkey(), &recipient, amount);

        let message = Message::new(&[transfer_ix.clone()], Some(&payer.pubkey()));

        let compiled_instruction = &message.instructions[0];

        let converted_ix = rpc.convert_compiled_instruction(
            compiled_instruction,
            &message.account_keys,
            &message.header,
        );
        assert_eq!(converted_ix.program_id, program::id());

        let decoded_ix = bincode::deserialize::<SystemInstruction>(&converted_ix.data).unwrap();
        match decoded_ix {
            SystemInstruction::Transfer { lamports } => {
                assert_eq!(lamports, amount);
            }
            _ => panic!("Expected Transfer instruction"),
        }

        assert_eq!(converted_ix.accounts.len(), 2);
        assert_eq!(converted_ix.accounts[0].pubkey, payer.pubkey());
        assert!(converted_ix.accounts[0].is_signer);
        assert!(converted_ix.accounts[0].is_writable);

        assert_eq!(converted_ix.accounts[1].pubkey, recipient);
        assert!(!converted_ix.accounts[1].is_signer);
        assert!(converted_ix.accounts[1].is_writable);

        assert_eq!(converted_ix.program_id, transfer_ix.program_id);
        assert_eq!(converted_ix.data, transfer_ix.data);

        let original_source = transfer_ix
            .accounts
            .iter()
            .find(|a| a.pubkey == payer.pubkey())
            .unwrap();
        let original_dest = transfer_ix
            .accounts
            .iter()
            .find(|a| a.pubkey == recipient)
            .unwrap();

        let converted_source = converted_ix
            .accounts
            .iter()
            .find(|a| a.pubkey == payer.pubkey())
            .unwrap();
        let converted_dest = converted_ix
            .accounts
            .iter()
            .find(|a| a.pubkey == recipient)
            .unwrap();

        assert_eq!(original_source.is_signer, converted_source.is_signer);
        assert_eq!(original_source.is_writable, converted_source.is_writable);
        assert_eq!(original_dest.is_signer, converted_dest.is_signer);
        assert_eq!(original_dest.is_writable, converted_dest.is_writable);
    }

    #[tokio::test]
    async fn test_convert_compiled_instruction_spl_token_transfer() {
        let (relayer, signer, provider, jupiter_service, _, job_producer) = setup_test_context();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let payer = Keypair::new();
        let owner = Keypair::new();
        let mint = Pubkey::new_unique();
        let source = get_associated_token_address(&payer.pubkey(), &mint);
        let destination = get_associated_token_address(&Pubkey::new_unique(), &mint);
        let amount = 1_000_000;

        let transfer_ix = spl_token::instruction::transfer_checked(
            &spl_token::id(),
            &source,
            &mint,
            &destination,
            &owner.pubkey(),
            &[],
            amount,
            9, // decimals
        )
        .unwrap();

        let message = Message::new_with_blockhash(
            &[transfer_ix.clone()],
            Some(&payer.pubkey()),
            &Hash::new_unique(),
        );

        let compiled_instruction = &message.instructions[0];
        let converted_ix = rpc.convert_compiled_instruction(
            compiled_instruction,
            &message.account_keys,
            &message.header,
        );

        assert_eq!(converted_ix.program_id, spl_token::id());

        let decoded_ix =
            spl_token::instruction::TokenInstruction::unpack(&converted_ix.data).unwrap();

        match decoded_ix {
            spl_token::instruction::TokenInstruction::TransferChecked {
                amount: decoded_amount,
                decimals,
                ..
            } => {
                assert_eq!(decoded_amount, amount);
                assert_eq!(decimals, 9);
            }
            _ => panic!("Expected TransferChecked instruction"),
        }

        assert!(converted_ix
            .accounts
            .iter()
            .any(|a| a.pubkey == source && a.is_writable));
        assert!(converted_ix
            .accounts
            .iter()
            .any(|a| a.pubkey == destination && a.is_writable));
        assert!(converted_ix
            .accounts
            .iter()
            .any(|a| a.pubkey == mint && !a.is_writable));
        assert!(converted_ix
            .accounts
            .iter()
            .any(|a| a.pubkey == owner.pubkey() && a.is_signer));
    }

    #[tokio::test]
    async fn test_create_transaction_with_user_fee_payment_token() {
        let (mut relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();
        // USDC token mint
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();
        let user_keypair = Keypair::new();
        let user_pubkey = user_keypair.pubkey();
        let relayer_pubkey = relayer_keypair.pubkey();

        let token_mint = Pubkey::from_str(test_token).unwrap();

        let user_token_account = get_associated_token_address(&user_keypair.pubkey(), &token_mint);
        let relayer_token_account =
            get_associated_token_address(&relayer_keypair.pubkey(), &token_mint);

        let expected_blockhash = Hash::new_unique();
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(move |_| Box::pin(async move { Ok((expected_blockhash, 100u64)) }));

        provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                Box::pin(async move {
                    // Create a token account with sufficient balance
                    let mut account_data = vec![0; spl_token::state::Account::LEN];
                    let mut token_account = Account {
                        mint: token_mint,
                        owner: user_pubkey,
                        amount: 10_000_000,
                        state: spl_token::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    if pubkey == user_token_account {
                        token_account.owner = user_pubkey;
                    } else if pubkey == relayer_token_account {
                        token_account.owner = relayer_pubkey;
                    }

                    spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

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
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );
        let recipient = Pubkey::new_unique();
        let ix = instruction::transfer(&user_pubkey, &recipient, 1000);
        let message = Message::new(&[ix], Some(&user_pubkey));
        let transaction = Transaction::new_unsigned(message);

        let fee_amount = 2_000_000; // 2.0 USDC

        let result = rpc
            .create_transaction_with_user_fee_payment(
                &relayer_pubkey,
                &transaction,
                test_token,
                fee_amount,
            )
            .await;

        assert!(
            result.is_ok(),
            "Should successfully create transaction with token fee payment"
        );

        let (modified_tx, _) = result.unwrap();

        assert_eq!(
            modified_tx.message.account_keys[0],
            relayer_keypair.pubkey(),
            "Relayer should be fee payer"
        );

        let has_original_instruction = modified_tx.message.instructions.iter().any(|ix| {
            let program_idx = ix.program_id_index as usize;

            if program_idx < modified_tx.message.account_keys.len()
                && modified_tx.message.account_keys[program_idx] == program::id()
            {
                if let Ok(SystemInstruction::Transfer { lamports }) = bincode::deserialize(&ix.data)
                {
                    if lamports == 1000 {
                        return true;
                    }
                }
            }
            false
        });
        assert!(
            has_original_instruction,
            "Transaction should include original instruction"
        );

        let has_token_transfer = modified_tx.message.instructions.iter().any(|ix| {
            let program_idx = ix.program_id_index as usize;

            if program_idx < modified_tx.message.account_keys.len()
                && modified_tx.message.account_keys[program_idx] == spl_token::id()
            {
                if let Ok(token_ix) = spl_token::instruction::TokenInstruction::unpack(&ix.data) {
                    match token_ix {
                        spl_token::instruction::TokenInstruction::TransferChecked {
                            amount,
                            ..
                        } => {
                            return amount == fee_amount;
                        }
                        _ => return false,
                    }
                }
            }
            false
        });

        assert!(
            has_token_transfer,
            "Transaction should include token transfer instruction"
        );

        let includes_correct_token_accounts = modified_tx
            .message
            .account_keys
            .iter()
            .any(|key| *key == user_token_account)
            && modified_tx
                .message
                .account_keys
                .iter()
                .any(|key| *key == relayer_token_account)
            && modified_tx
                .message
                .account_keys
                .iter()
                .any(|key| *key == token_mint);

        assert!(
            includes_correct_token_accounts,
            "Transaction should reference correct token accounts"
        );
    }

    #[tokio::test]
    async fn test_create_transaction_with_user_fee_payment_relayer_is_source() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer) =
            setup_test_context();

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let recipient = Pubkey::new_unique();
        let ix = instruction::transfer(&relayer_keypair.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let fee_amount = 5000;

        let result = rpc
            .create_transaction_with_user_fee_payment(
                &relayer_keypair.pubkey(),
                &transaction,
                NATIVE_SOL,
                fee_amount,
            )
            .await;

        assert!(result.is_err(), "Should fail when relayer is the source");
        match result {
            Err(SolanaRpcError::InvalidParams(msg)) => {
                assert!(
                    msg.contains("Relayer cannot pay fee to itself"),
                    "Error message should mention relayer can't pay itself"
                );
            }
            _ => panic!("Expected InvalidParams error"),
        }
    }

    #[tokio::test]
    async fn test_create_transaction_with_user_fee_payment_insufficient_balance() {
        let (mut relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();
        // USDC token mint
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();
        let user_keypair = Keypair::new();
        let user_pubkey = user_keypair.pubkey();
        let relayer_pubkey = relayer_keypair.pubkey();

        let token_mint = Pubkey::from_str(test_token).unwrap();

        let user_token_account = get_associated_token_address(&user_keypair.pubkey(), &token_mint);
        let relayer_token_account =
            get_associated_token_address(&relayer_keypair.pubkey(), &token_mint);

        let expected_blockhash = Hash::new_unique();
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(move |_| Box::pin(async move { Ok((expected_blockhash, 100u64)) }));

        provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                Box::pin(async move {
                    // Create a token account with sufficient balance
                    let mut account_data = vec![0; spl_token::state::Account::LEN];
                    let mut token_account = Account {
                        mint: token_mint,
                        owner: user_pubkey,
                        amount: 1_000,
                        state: spl_token::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    if pubkey == user_token_account {
                        token_account.owner = user_pubkey;
                    } else if pubkey == relayer_token_account {
                        token_account.owner = relayer_pubkey;
                    }

                    spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

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
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );
        // Create a simple transaction from user
        let recipient = Pubkey::new_unique();
        let ix = instruction::transfer(&user_pubkey, &recipient, 1000);
        let message = Message::new(&[ix], Some(&user_pubkey));
        let transaction = Transaction::new_unsigned(message);

        let fee_amount = 2_000_000; // 2.0 USDC

        // Call function to create transaction with user fee payment
        let result = rpc
            .create_transaction_with_user_fee_payment(
                &relayer_pubkey,
                &transaction,
                test_token,
                fee_amount,
            )
            .await;

        assert!(
            result.is_err(),
            "Should fail when token balance is insufficient"
        );
        match result {
            Err(SolanaRpcError::InsufficientFunds(_)) => {}
            _ => panic!("Expected InsufficientFunds error"),
        }
    }

    #[tokio::test]
    async fn test_confirm_user_fee_payment_sol_sufficient() {
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

        let payer = Keypair::new();
        let fee_amount = 5000;

        let payment_ix =
            instruction::transfer(&payer.pubkey(), &relayer_keypair.pubkey(), fee_amount);

        let recipient = Pubkey::new_unique();
        let regular_ix = instruction::transfer(&payer.pubkey(), &recipient, 1000);

        let message = Message::new(&[payment_ix, regular_ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.confirm_user_fee_payment(&transaction, fee_amount).await;

        assert!(result.is_ok(), "Should accept sufficient SOL payment");
    }

    #[tokio::test]
    async fn test_confirm_user_fee_payment_sol_insufficient() {
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

        // Create transaction with SOL payment to relayer (but insufficient)
        let payer = Keypair::new();
        let payment_amount = 4000; // Less than required
        let required_amount = 5000;

        // Create SOL transfer to relayer instruction
        let payment_ix =
            instruction::transfer(&payer.pubkey(), &relayer_keypair.pubkey(), payment_amount);

        let message = Message::new(&[payment_ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc
            .confirm_user_fee_payment(&transaction, required_amount)
            .await;

        assert!(result.is_err(), "Should reject insufficient SOL payment");
        match result {
            Err(SolanaRpcError::InvalidParams(msg)) => {
                assert!(
                    msg.contains("insufficient"),
                    "Error should mention payment insufficiency"
                );
            }
            _ => panic!("Expected InvalidParams error"),
        }
    }

    #[tokio::test]
    async fn test_confirm_user_fee_payment_token_sufficient() {
        let (mut relayer, signer, mut provider, mut jupiter_service, _, job_producer) =
            setup_test_context();
        // USDC token mint
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();
        let user_keypair = Keypair::new();
        let user_pubkey = user_keypair.pubkey();
        let relayer_pubkey = relayer_keypair.pubkey();

        let token_mint = Pubkey::from_str(test_token).unwrap();

        let user_token_account = get_associated_token_address(&user_keypair.pubkey(), &token_mint);
        let relayer_token_account =
            get_associated_token_address(&relayer_keypair.pubkey(), &token_mint);

        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, amount, _| {
                Box::pin(async move {
                    Ok(QuoteResponse {
                        input_mint: WRAPPED_SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: amount,
                        out_amount: amount * 2, // 1 SOL = 2 USDC
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

        provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                Box::pin(async move {
                    // Create a token account with sufficient balance
                    let mut account_data = vec![0; spl_token::state::Account::LEN];
                    let mut token_account = Account {
                        mint: token_mint,
                        owner: user_pubkey,
                        amount: 10000000,
                        state: spl_token::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    if pubkey == user_token_account {
                        token_account.owner = user_pubkey;
                    } else {
                        token_account.owner = relayer_pubkey;
                    }

                    spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

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

        let token_payment = 1_000_000; // Amount in token units
        let sol_fee = 5000; // Equivalent amount in SOL

        let transfer_ix = spl_token::instruction::transfer_checked(
            &spl_token::id(),
            &user_token_account,
            &token_mint,
            &relayer_token_account,
            &user_keypair.pubkey(),
            &[],
            token_payment,
            6, // decimals
        )
        .unwrap();

        let message = Message::new(&[transfer_ix], Some(&user_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.confirm_user_fee_payment(&transaction, sol_fee).await;

        assert!(result.is_ok(), "Should accept sufficient token payment");
    }

    #[tokio::test]
    async fn test_confirm_user_fee_payment_no_payment() {
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

        // Create transaction WITHOUT payment to relayer
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();

        let regular_ix = instruction::transfer(&payer.pubkey(), &recipient, 1000);

        let message = Message::new(&[regular_ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc.confirm_user_fee_payment(&transaction, 5000).await;

        assert!(result.is_err(), "Should reject transaction with no payment");
        match result {
            Err(SolanaRpcError::InvalidParams(msg)) => {
                assert!(
                    msg.contains("doesn't contain required fee payment"),
                    "Error should mention missing payment"
                );
            }
            _ => panic!("Expected InvalidParams error"),
        }
    }
    #[tokio::test]
    async fn test_find_token_payments_to_relayer() {
        let (mut relayer, signer, mut provider, jupiter_service, _, job_producer) =
            setup_test_context();
        // USDC mint address
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // noboost

        // Setup policy with token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        });

        let relayer_keypair = Keypair::new();
        relayer.address = relayer_keypair.pubkey().to_string();
        let relayer_pubkey = relayer_keypair.pubkey();
        let user_keypair = Keypair::new();
        let user_pubkey = user_keypair.pubkey();

        println!("relayer pubkey: {}", relayer_pubkey);
        println!("user pubkey: {}", user_keypair.pubkey());

        // Create token mint
        let token_mint = Pubkey::from_str(test_token).unwrap();

        // Get token accounts
        let user_token_account = get_associated_token_address(&user_keypair.pubkey(), &token_mint);
        let relayer_token_account = get_associated_token_address(&relayer_pubkey, &token_mint);

        provider
            .expect_get_account_from_pubkey()
            .returning(move |pubkey| {
                let pubkey = *pubkey;
                Box::pin(async move {
                    // Create a token account with sufficient balance
                    let mut account_data = vec![0; spl_token::state::Account::LEN];
                    let mut token_account = Account {
                        mint: token_mint,
                        owner: user_pubkey,
                        amount: 10000000,
                        state: spl_token::state::AccountState::Initialized,
                        ..Default::default()
                    };
                    if pubkey == user_token_account {
                        token_account.owner = user_pubkey;
                    } else {
                        token_account.owner = relayer_pubkey;
                    }

                    spl_token::state::Account::pack(token_account, &mut account_data).unwrap();

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
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );
        let token_payment = 1_000_000; // 1 USDC

        let transfer_ix = spl_token::instruction::transfer_checked(
            &spl_token::id(),
            &user_token_account,
            &token_mint,
            &relayer_token_account,
            &user_keypair.pubkey(),
            &[],
            token_payment,
            6, // decimals
        )
        .unwrap();

        let message = Message::new(&[transfer_ix], Some(&user_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        let result = rpc
            .find_token_payments_to_relayer(&transaction, &relayer_pubkey)
            .await;

        assert!(result.is_ok(), "Token payment detection should succeed");
        let payments = result.unwrap();

        assert_eq!(payments.len(), 1, "Should find one token payment");
        assert_eq!(payments[0].0, token_mint, "Token mint should match");
        assert_eq!(payments[0].1, token_payment, "Payment amount should match");
    }
}
