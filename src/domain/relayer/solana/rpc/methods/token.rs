use ::spl_token::state::Account as SplTokenAccount;
use log::error;
use solana_sdk::{
    account::Account as SolanaAccount, instruction::Instruction, program_pack::Pack, pubkey::Pubkey,
};
use spl_associated_token_account::get_associated_token_address_with_program_id;

use spl_associated_token_account::instruction::create_associated_token_account;
use spl_token_2022::state::Account as Token2022Account;

pub struct TokenAccount {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    pub is_frozen: bool,
}

/// Error types for token operations
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("Invalid token account: {0}")]
    InvalidTokenAccount(String),
    #[error("Invalid token instruction: {0}")]
    InvalidTokenInstruction(String),
    #[error("Invalid token mint: {0}")]
    InvalidTokenMint(String),
    #[error("Invalid token owner: {0}")]
    InvalidTokenOwner(String),
    #[error("Invalid token decimals: {0}")]
    InvalidTokenDecimals(String),
    #[error("Invalid token amount: {0}")]
    InvalidTokenAmount(String),
    #[error("Invalid token program: {0}")]
    InvalidTokenProgram(String),
    #[error("Instruction error: {0}")]
    Instruction(String),
    #[error("Account error: {0}")]
    AccountError(String),
}

/// Trait for token operations
pub trait SolanaToken {
    /// Check if a program ID is this token program
    fn is_token_program(program_id: &Pubkey) -> bool;

    /// Unpack a token account
    fn unpack_account(
        program_id: &Pubkey,
        account: &SolanaAccount,
    ) -> Result<TokenAccount, TokenError>;

    /// Get the associated token address for a wallet and mint
    fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey;

    /// Create an associated token account instruction
    fn create_associated_token_account(
        program_id: &Pubkey,
        payer: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Instruction;

    /// Create a transfer checked instruction
    fn create_transfer_checked_instruction(
        program_id: &Pubkey,
        source: &Pubkey,
        mint: &Pubkey,
        destination: &Pubkey,
        authority: &Pubkey,
        amount: u64,
        decimals: u8,
    ) -> Result<Instruction, TokenError>;

    /// Unpack a token instruction
    fn unpack_instruction(program_id: &Pubkey, data: &[u8])
        -> Result<TokenInstruction, TokenError>;
}

/// Token instruction types
#[derive(Debug)]
pub enum TokenInstruction {
    Transfer { amount: u64 },
    TransferChecked { amount: u64, decimals: u8 },
    // "Other" variant to catch all other instruction types
    Other,
}

pub struct SolanaTokenProgram;

impl SolanaTokenProgram {
    pub fn is_token_program(program_id: &Pubkey) -> bool {
        program_id == &spl_token::id() || program_id == &spl_token_2022::id()
    }

    pub fn create_transfer_checked_instruction(
        program_id: &Pubkey,
        source: &Pubkey,
        mint: &Pubkey,
        destination: &Pubkey,
        authority: &Pubkey,
        amount: u64,
        decimals: u8,
    ) -> Result<Instruction, TokenError> {
        if !Self::is_token_program(program_id) {
            return Err(TokenError::InvalidTokenProgram(format!(
                "Unknown token program: {}",
                program_id
            )));
        }
        if program_id == &spl_token::id() {
            return spl_token::instruction::transfer_checked(
                program_id,
                source,
                mint,
                destination,
                authority,
                &[],
                amount,
                decimals,
            )
            .map_err(|e| TokenError::Instruction(e.to_string()));
        } else if program_id == &spl_token_2022::id() {
            return spl_token_2022::instruction::transfer_checked(
                program_id,
                source,
                mint,
                destination,
                authority,
                &[],
                amount,
                decimals,
            )
            .map_err(|e| TokenError::Instruction(e.to_string()));
        }
        Err(TokenError::InvalidTokenProgram(format!(
            "Unknown token program: {}",
            program_id
        )))
    }

    pub fn unpack_account(
        program_id: &Pubkey,
        account: &SolanaAccount,
    ) -> Result<TokenAccount, TokenError> {
        if !Self::is_token_program(program_id) {
            return Err(TokenError::InvalidTokenProgram(format!(
                "Unknown token program: {}",
                program_id
            )));
        }
        if program_id == &spl_token::id() {
            let account = SplTokenAccount::unpack(&account.data)
                .map_err(|e| TokenError::AccountError(format!("Invalid token account: {}", e)))?;

            return Ok(TokenAccount {
                mint: account.mint,
                owner: account.owner,
                amount: account.amount,
                is_frozen: account.is_frozen(),
            });
        } else if program_id == &spl_token_2022::id() {
            let account = Token2022Account::unpack(&account.data)
                .map_err(|e| TokenError::AccountError(format!("Invalid token account: {}", e)))?;

            return Ok(TokenAccount {
                mint: account.mint,
                owner: account.owner,
                amount: account.amount,
                is_frozen: account.is_frozen(),
            });
        }
        Err(TokenError::InvalidTokenProgram(format!(
            "Unknown token program: {}",
            program_id
        )))
    }

    pub fn get_associated_token_address(
        program_id: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Pubkey {
        get_associated_token_address_with_program_id(program_id, wallet, mint)
    }

    pub fn create_associated_token_account(
        program_id: &Pubkey,
        payer: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Instruction {
        create_associated_token_account(payer, wallet, mint, program_id)
    }

    pub fn unpack_instruction(
        program_id: &Pubkey,
        data: &[u8],
    ) -> Result<TokenInstruction, TokenError> {
        if !Self::is_token_program(program_id) {
            return Err(TokenError::InvalidTokenProgram(format!(
                "Unknown token program: {}",
                program_id
            )));
        }
        if program_id == &spl_token::id() {
            match spl_token::instruction::TokenInstruction::unpack(data) {
                Ok(instr) => match instr {
                    spl_token::instruction::TokenInstruction::Transfer { amount } => {
                        Ok(TokenInstruction::Transfer { amount })
                    }
                    spl_token::instruction::TokenInstruction::TransferChecked {
                        amount,
                        decimals,
                    } => Ok(TokenInstruction::TransferChecked { amount, decimals }),
                    _ => Ok(TokenInstruction::Other), // Catch all other instruction types
                },
                Err(e) => Err(TokenError::InvalidTokenInstruction(e.to_string())),
            }
        } else if program_id == &spl_token_2022::id() {
            match spl_token_2022::instruction::TokenInstruction::unpack(data) {
                Ok(instr) => match instr {
                    spl_token_2022::instruction::TokenInstruction::Transfer { amount } => {
                        Ok(TokenInstruction::Transfer { amount })
                    }
                    spl_token_2022::instruction::TokenInstruction::TransferChecked {
                        amount,
                        decimals,
                    } => Ok(TokenInstruction::TransferChecked { amount, decimals }),
                    _ => Ok(TokenInstruction::Other), // Catch all other instruction types
                },
                Err(e) => Err(TokenError::InvalidTokenInstruction(e.to_string())),
            }
        } else {
            Err(TokenError::InvalidTokenProgram(format!(
                "Unknown token program: {}",
                program_id
            )))
        }
    }
}

// impl SolanaTokenType {
//     /// Get the token program for a mint
//     pub async fn get_token_program_for_mint<P: SolanaProviderTrait>(
//         provider: &P,
//         mint: &Pubkey,
//     ) -> Result<Self, TokenError> {
//         let account = provider
//             .get_account_from_pubkey(mint)
//             .await
//             .map_err(|e| TokenError::InvalidTokenMint(e.to_string()))?;

//         if account.owner == SplToken::program_id() {
//             Ok(Self::SplToken)
//         } else if account.owner == Token2022::program_id() {
//             Ok(Self::Token2022)
//         } else {
//             Err(TokenError::InvalidTokenProgram(format!(
//                 "Unknown token program: {}",
//                 account.owner
//             )))
//         }
//     }
// }

// /// Helper function to get the token program for a mint
// pub async fn get_token_program_for_mint<P: SolanaProviderTrait>(
//     provider: &P,
//     mint: &Pubkey,
// ) -> Result<SolanaTokenType, TokenError> {
//     SolanaTokenType::get_token_program_for_mint(provider, mint).await
// }
