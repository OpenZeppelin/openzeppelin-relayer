mod spl_token;
mod token_2022;
mod token_service;

use log::{debug, error};
use solana_sdk::{
    account::Account as SolanaAccount, instruction::Instruction, program_pack::Pack, pubkey::Pubkey,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::{Account as TokenAccount, Mint};
use spl_token_2022::state::{Account as Token2022Account, Mint as Token2022Mint};

use crate::{
    constants::{NATIVE_SOL, SOLANA_DECIMALS, WRAPPED_SOL_MINT},
    models::Relayer,
    services::{JupiterServiceTrait, SolanaProviderTrait},
};

pub use spl_token::SplToken;
pub use token_2022::Token2022;
pub use token_service::{FeeQuote, TokenService};

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
}

/// Trait for token operations
pub trait SolanaToken {
    /// Get the program ID for this token type
    fn program_id() -> Pubkey;

    /// Check if a program ID is this token program
    fn is_token_program(program_id: &Pubkey) -> bool {
        program_id == &Self::program_id()
    }

    /// Get the associated token address for a wallet and mint
    fn get_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
        get_associated_token_address(wallet, mint)
    }

    /// Create an associated token account instruction
    fn create_associated_token_account(
        payer: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Instruction;

    /// Create a transfer checked instruction
    fn create_transfer_checked_instruction(
        source: &Pubkey,
        mint: &Pubkey,
        destination: &Pubkey,
        authority: &Pubkey,
        amount: u64,
        decimals: u8,
    ) -> Result<Instruction, TokenError>;

    /// Unpack a token account
    fn unpack_account(data: &[u8]) -> Result<TokenAccount, TokenError>;

    /// Unpack a token instruction
    fn unpack_instruction(data: &[u8]) -> Result<TokenInstruction, TokenError>;

    /// Verify if an account is owned by this token program
    fn verify_account_owner(account: &SolanaAccount) -> bool {
        account.owner == Self::program_id()
    }

    /// Get the mint from an account
    fn get_mint_from_account(account: &TokenAccount) -> Pubkey {
        account.mint
    }
}

/// Token instruction types
#[derive(Debug)]
pub enum TokenInstruction {
    Transfer { amount: u64 },
    TransferChecked { amount: u64, decimals: u8 },
    MintTo { amount: u64 },
    Burn { amount: u64 },
    InitializeAccount,
    InitializeMint { decimals: u8 },
    InitializeMultisig { m: u8 },
    Revoke,
    SetAuthority,
    CloseAccount,
    FreezeAccount,
    ThawAccount,
}

/// Token program type
#[derive(Debug, Clone, Copy)]
pub enum SolanaTokenType {
    SplToken,
    Token2022,
}

impl SolanaTokenType {
    /// Get the token program for a mint
    pub async fn get_token_program_for_mint<P: SolanaProviderTrait>(
        provider: &P,
        mint: &Pubkey,
    ) -> Result<Self, TokenError> {
        let account = provider
            .get_account_from_pubkey(mint)
            .await
            .map_err(|e| TokenError::InvalidTokenMint(e.to_string()))?;

        if account.owner == SplToken::program_id() {
            Ok(Self::SplToken)
        } else if account.owner == Token2022::program_id() {
            Ok(Self::Token2022)
        } else {
            Err(TokenError::InvalidTokenProgram(format!(
                "Unknown token program: {}",
                account.owner
            )))
        }
    }
}

/// Helper function to get the token program for a mint
pub async fn get_token_program_for_mint<P: SolanaProviderTrait>(
    provider: &P,
    mint: &Pubkey,
) -> Result<Box<dyn SolanaToken>, TokenError> {
    match SolanaTokenType::get_token_program_for_mint(provider, mint).await? {
        SolanaTokenType::SplToken => Ok(Box::new(SplToken)),
        SolanaTokenType::Token2022 => Ok(Box::new(Token2022)),
    }
}
