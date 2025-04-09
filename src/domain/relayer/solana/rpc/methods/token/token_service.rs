use log::{debug, error};
use solana_sdk::{
    account::Account as SolanaAccount, instruction::Instruction, pubkey::Pubkey,
    transaction::Transaction,
};
use std::str::FromStr;

use crate::{
    constants::{DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, NATIVE_SOL, SOLANA_DECIMALS, WRAPPED_SOL_MINT},
    models::Relayer,
    services::{JupiterServiceTrait, SolanaProviderTrait},
};

use super::{get_token_program_for_mint, SolanaRpcError, SolanaToken, TokenError};

/// Service for handling token-related operations
pub struct TokenService<P, J>
where
    P: SolanaProviderTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    provider: P,
    jupiter_service: J,
    relayer: Relayer,
}

impl<P, J> TokenService<P, J>
where
    P: SolanaProviderTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    /// Create a new token service
    pub fn new(provider: P, jupiter_service: J, relayer: Relayer) -> Self {
        Self {
            provider,
            jupiter_service,
            relayer,
        }
    }

    /// Get a fee quote for a token
    pub async fn get_fee_token_quote(
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

    /// Handle a token transfer between two accounts
    pub async fn handle_token_transfer(
        &self,
        source: &Pubkey,
        destination: &Pubkey,
        token_mint: &Pubkey,
        amount: u64,
    ) -> Result<Vec<Instruction>, SolanaRpcError> {
        // Get the token program for this mint
        let token_program = get_token_program_for_mint(&self.provider, token_mint)
            .await
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        let mut instructions = Vec::new();
        let source_ata = token_program.get_associated_token_address(source, token_mint);
        let destination_ata = token_program.get_associated_token_address(destination, token_mint);

        // Verify source account and balance
        let source_account = self
            .provider
            .get_account_from_pubkey(&source_ata)
            .await
            .map_err(|e| {
                SolanaRpcError::TokenAccount(format!("Invalid source token account: {}", e))
            })?;
        
        let source_token_account = token_program
            .unpack_account(&source_account.data)
            .map_err(|e| SolanaRpcError::InvalidParams(format!("Invalid token account: {}", e)))?;

        if source_token_account.amount < amount {
            return Err(SolanaRpcError::InsufficientFunds(format!(
                "Insufficient token balance: required {} but found {}",
                amount, source_token_account.amount
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

            instructions.push(token_program.create_associated_token_account(
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

        instructions.push(
            token_program
                .create_transfer_checked_instruction(
                    &source_ata,
                    token_mint,
                    &destination_ata,
                    source,
                    amount,
                    token_decimals,
                )
                .map_err(|e| SolanaRpcError::TransactionPreparation(e.to_string()))?,
        );

        Ok(instructions)
    }

    /// Find token payments to the relayer in a transaction
    pub async fn find_token_payments_to_relayer(
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

            // Try to get the token program for this instruction
            let token_program = match get_token_program_for_mint(&self.provider, &program_id).await {
                Ok(program) => program,
                Err(_) => continue, // Not a token program, skip
            };

            if !token_program.is_token_program(&program_id) {
                continue;
            }

            if let Ok(token_ix) = token_program.unpack_instruction(&ix.data) {
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

                        let source_token_account =
                            transaction.message.account_keys[source_token_idx];
                        let dest_token_account = transaction.message.account_keys[dest_token_idx];

                        // Check destination account first
                        match self
                            .provider
                            .get_account_from_pubkey(&dest_token_account)
                            .await
                        {
                            Ok(dest_account) => {
                                if !token_program.verify_account_owner(&dest_account) {
                                    continue;
                                }

                                let dest_token_account = match token_program.unpack_account(&dest_account.data) {
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
                                        if !token_program.verify_account_owner(&source_account) {
                                            continue;
                                        }

                                        let source_token_account =
                                            match token_program.unpack_account(&source_account.data) {
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
                    _ => continue,
                }
            }
        }

        Ok(payments)
    }
}

/// Fee quote for a token
#[derive(Debug, Clone)]
pub struct FeeQuote {
    pub fee_in_spl: u64,
    pub fee_in_spl_ui: String,
    pub fee_in_lamports: u64,
    pub conversion_rate: f64,
}

/// Convert a token amount to UI amount
fn amount_to_ui_amount(amount: u64, decimals: u8) -> f64 {
    amount as f64 / 10f64.powi(decimals as i32)
} 