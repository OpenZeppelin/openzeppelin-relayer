//! Solana token programs interaction module.
//!
//! This module provides abstractions and utilities for interacting with Solana token programs,
//! specifically SPL Token and Token-2022. It offers unified interfaces for common token operations
//! like transfers, account creation, and account data parsing.
//!
//! This module abstracts away differences between token program versions, allowing
//! for consistent interaction regardless of which token program (SPL Token or Token-2022)
//! is being used.
use ::spl_token::state::Account as SplTokenAccount;
use log::error;
use solana_sdk::{
    account::Account as SolanaAccount, instruction::Instruction, program_pack::Pack, pubkey::Pubkey,
};
use spl_associated_token_account::get_associated_token_address_with_program_id;

use spl_associated_token_account::instruction::create_associated_token_account;
use spl_token_2022::state::Account as Token2022Account;

use crate::services::SolanaProviderTrait;

/// Represents a Solana token account with its key properties.
///
/// This struct contains the essential information about a token account,
/// including the mint address, owner, token amount, and frozen status.
#[derive(Debug)]
pub struct TokenAccount {
    /// The mint address of the token
    pub mint: Pubkey,
    /// The owner of the token account
    pub owner: Pubkey,
    /// The amount of tokens held in this account
    pub amount: u64,
    /// Whether the account is frozen
    pub is_frozen: bool,
}

/// Error types that can occur during token operations.
///
/// This enum provides specific error variants for different token-related failures,
/// making it easier to diagnose and handle token operation issues.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// Error when a token instruction is invalid
    #[error("Invalid token instruction: {0}")]
    InvalidTokenInstruction(String),
    /// Error when a token mint is invalid
    #[error("Invalid token mint: {0}")]
    InvalidTokenMint(String),
    /// Error when a token program is invalid
    #[error("Invalid token program: {0}")]
    InvalidTokenProgram(String),
    /// Error when an instruction fails
    #[error("Instruction error: {0}")]
    Instruction(String),
    /// Error when an account operation fails
    #[error("Account error: {0}")]
    AccountError(String),
}

/// Represents different types of token instructions.
///
/// This enum provides variants for the most common token instructions,
/// with a catch-all variant for other instruction types.
#[derive(Debug)]
pub enum TokenInstruction {
    /// A simple transfer instruction
    Transfer { amount: u64 },
    /// A transfer with decimal checking
    TransferChecked { amount: u64, decimals: u8 },
    /// Catch-all variant for other instruction types
    Other,
}

/// Implementation of the Solana token program functionality.
///
/// This struct provides concrete implementations for the SolanaToken trait,
/// supporting both the SPL Token and Token-2022 programs.
pub struct SolanaTokenProgram;

impl SolanaTokenProgram {
    /// Get the token program for a mint
    pub async fn get_token_program_for_mint<P: SolanaProviderTrait>(
        provider: &P,
        mint: &Pubkey,
    ) -> Result<Pubkey, TokenError> {
        let account = provider
            .get_account_from_pubkey(mint)
            .await
            .map_err(|e| TokenError::InvalidTokenMint(e.to_string()))?;

        if account.owner == spl_token::id() {
            Ok(spl_token::id())
        } else if account.owner == spl_token_2022::id() {
            Ok(spl_token_2022::id())
        } else {
            Err(TokenError::InvalidTokenProgram(format!(
                "Unknown token program: {}",
                account.owner
            )))
        }
    }

    /// Checks if a program ID corresponds to a known token program.
    ///
    /// # Arguments
    ///
    /// * `program_id` - The program ID to check
    ///
    /// # Returns
    ///
    /// `true` if the program ID is SPL Token or Token-2022, `false` otherwise
    pub fn is_token_program(program_id: &Pubkey) -> bool {
        program_id == &spl_token::id() || program_id == &spl_token_2022::id()
    }

    /// Creates a transfer checked instruction.
    ///
    /// # Arguments
    ///
    /// * `program_id` - The program ID of the token program
    /// * `source` - The source token account
    /// * `mint` - The mint address
    /// * `destination` - The destination token account
    /// * `authority` - The authority that can sign for the source account
    /// * `amount` - The amount to transfer
    /// * `decimals` - The number of decimals for the token
    ///
    /// # Returns
    ///
    /// A Result containing either the transfer instruction or a TokenError
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

    /// Unpacks a Solana account into a TokenAccount structure.
    ///
    /// # Arguments
    ///
    /// * `program_id` - The program ID of the token program
    /// * `account` - The Solana account to unpack
    ///
    /// # Returns
    ///
    /// A Result containing either the unpacked TokenAccount or a TokenError
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

    /// Gets the associated token address for a wallet and mint.
    ///
    /// # Arguments
    ///
    /// * `program_id` - The program ID of the token program
    /// * `wallet` - The wallet address
    /// * `mint` - The mint address
    ///
    /// # Returns
    ///
    /// The associated token address
    pub fn get_associated_token_address(
        program_id: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Pubkey {
        get_associated_token_address_with_program_id(program_id, wallet, mint)
    }

    /// Creates an instruction to create an associated token account.
    ///
    /// # Arguments
    ///
    /// * `program_id` - The program ID of the token program
    /// * `payer` - The account that will pay for the account creation
    /// * `wallet` - The wallet address
    /// * `mint` - The mint address
    ///
    /// # Returns
    ///
    /// An instruction to create the associated token account
    pub fn create_associated_token_account(
        program_id: &Pubkey,
        payer: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Instruction {
        create_associated_token_account(payer, wallet, mint, program_id)
    }

    /// Unpacks a token instruction from its binary data.
    ///
    /// # Arguments
    ///
    /// * `program_id` - The program ID of the token program
    /// * `data` - The binary instruction data
    ///
    /// # Returns
    ///
    /// A Result containing either the unpacked TokenInstruction or a TokenError
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
                    #[allow(deprecated)]
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

#[cfg(test)]
mod tests {
    use mockall::predicate::eq;
    use solana_sdk::{program_pack::Pack, pubkey::Pubkey};
    use spl_associated_token_account::get_associated_token_address_with_program_id;
    use spl_associated_token_account::instruction::create_associated_token_account;
    use spl_token::state::Account;

    use crate::{
        domain::{SolanaTokenProgram, TokenError, TokenInstruction},
        services::MockSolanaProviderTrait,
    };

    #[tokio::test]
    async fn test_get_token_program_for_mint_spl_token() {
        let mint = Pubkey::new_unique();
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider
            .expect_get_account_from_pubkey()
            .with(eq(mint))
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(solana_sdk::account::Account {
                        lamports: 1000000,
                        data: vec![],
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        let result = SolanaTokenProgram::get_token_program_for_mint(&mock_provider, &mint).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), spl_token::id());
    }

    #[tokio::test]
    async fn test_get_token_program_for_mint_token_2022() {
        let mint = Pubkey::new_unique();
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider
            .expect_get_account_from_pubkey()
            .with(eq(mint))
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(solana_sdk::account::Account {
                        lamports: 1000000,
                        data: vec![],
                        owner: spl_token_2022::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        let result = SolanaTokenProgram::get_token_program_for_mint(&mock_provider, &mint).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), spl_token_2022::id());
    }

    #[tokio::test]
    async fn test_get_token_program_for_mint_invalid() {
        let mint = Pubkey::new_unique();
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider
            .expect_get_account_from_pubkey()
            .with(eq(mint))
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(solana_sdk::account::Account {
                        lamports: 1000000,
                        data: vec![],
                        owner: Pubkey::new_unique(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        let result = SolanaTokenProgram::get_token_program_for_mint(&mock_provider, &mint).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TokenError::InvalidTokenProgram(_)
        ));
    }

    #[test]
    fn test_is_token_program() {
        assert!(SolanaTokenProgram::is_token_program(&spl_token::id()));
        assert!(SolanaTokenProgram::is_token_program(&spl_token_2022::id()));
        assert!(!SolanaTokenProgram::is_token_program(&Pubkey::new_unique()));
    }

    #[test]
    fn test_create_transfer_checked_instruction_spl_token() {
        let program_id = spl_token::id();
        let source = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let destination = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let amount = 1000;
        let decimals = 9;

        let result = SolanaTokenProgram::create_transfer_checked_instruction(
            &program_id,
            &source,
            &mint,
            &destination,
            &authority,
            amount,
            decimals,
        );

        assert!(result.is_ok());
        let instruction = result.unwrap();
        assert_eq!(instruction.program_id, program_id);
        assert_eq!(instruction.accounts.len(), 4);
        assert_eq!(instruction.accounts[0].pubkey, source);
        assert_eq!(instruction.accounts[1].pubkey, mint);
        assert_eq!(instruction.accounts[2].pubkey, destination);
        assert_eq!(instruction.accounts[3].pubkey, authority);
    }

    #[test]
    fn test_create_transfer_checked_instruction_token_2022() {
        let program_id = spl_token_2022::id();
        let source = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let destination = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let amount = 1000;
        let decimals = 9;

        let result = SolanaTokenProgram::create_transfer_checked_instruction(
            &program_id,
            &source,
            &mint,
            &destination,
            &authority,
            amount,
            decimals,
        );

        assert!(result.is_ok());
        let instruction = result.unwrap();
        assert_eq!(instruction.program_id, program_id);
        assert_eq!(instruction.accounts.len(), 4);
        assert_eq!(instruction.accounts[0].pubkey, source);
        assert_eq!(instruction.accounts[1].pubkey, mint);
        assert_eq!(instruction.accounts[2].pubkey, destination);
        assert_eq!(instruction.accounts[3].pubkey, authority);
    }

    #[test]
    fn test_create_transfer_checked_instruction_invalid_program() {
        let program_id = Pubkey::new_unique(); // Invalid program ID
        let source = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let destination = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let amount = 1000;
        let decimals = 9;

        let result = SolanaTokenProgram::create_transfer_checked_instruction(
            &program_id,
            &source,
            &mint,
            &destination,
            &authority,
            amount,
            decimals,
        );

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TokenError::InvalidTokenProgram(_)
        ));
    }

    #[test]
    fn test_unpack_account_spl_token() {
        let program_id = spl_token::id();
        let mint = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let amount = 1000;

        let mut spl_account = Account::default();
        spl_account.mint = mint;
        spl_account.owner = owner;
        spl_account.amount = amount;
        spl_account.state = spl_token::state::AccountState::Initialized;

        let mut account_data = vec![0; Account::LEN];
        Account::pack(spl_account, &mut account_data).unwrap();

        let solana_account = solana_sdk::account::Account {
            lamports: 0,
            data: account_data,
            owner: program_id,
            executable: false,
            rent_epoch: 0,
        };

        let result = SolanaTokenProgram::unpack_account(&program_id, &solana_account);
        assert!(result.is_ok());

        let token_account = result.unwrap();
        assert_eq!(token_account.mint, mint);
        assert_eq!(token_account.owner, owner);
        assert_eq!(token_account.amount, amount);
        assert_eq!(token_account.is_frozen, false);
    }

    #[test]
    fn test_unpack_account_token_2022() {
        let program_id = spl_token_2022::id();
        let mint = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let amount = 1000;

        let mut spl_account = Account::default();
        spl_account.mint = mint;
        spl_account.owner = owner;
        spl_account.amount = amount;
        spl_account.state = spl_token::state::AccountState::Initialized;

        let mut account_data = vec![0; Account::LEN];
        Account::pack(spl_account, &mut account_data).unwrap();

        let solana_account = solana_sdk::account::Account {
            lamports: 0,
            data: account_data,
            owner: program_id,
            executable: false,
            rent_epoch: 0,
        };

        let result = SolanaTokenProgram::unpack_account(&program_id, &solana_account);
        assert!(result.is_ok());

        let token_account = result.unwrap();
        assert_eq!(token_account.mint, mint);
        assert_eq!(token_account.owner, owner);
        assert_eq!(token_account.amount, amount);
        assert_eq!(token_account.is_frozen, false);
    }

    #[test]
    fn test_unpack_account_invalid_program() {
        let program_id = Pubkey::new_unique(); // Invalid program ID
        let mint = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let amount = 1000;

        let mut spl_account = Account::default();
        spl_account.mint = mint;
        spl_account.owner = owner;
        spl_account.amount = amount;
        spl_account.state = spl_token::state::AccountState::Initialized;

        let mut account_data = vec![0; Account::LEN];
        Account::pack(spl_account, &mut account_data).unwrap();

        let account = solana_sdk::account::Account {
            lamports: 0,
            data: account_data,
            owner: program_id,
            executable: false,
            rent_epoch: 0,
        };

        let result = SolanaTokenProgram::unpack_account(&program_id, &account);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TokenError::InvalidTokenProgram(_)
        ));
    }

    #[test]
    fn test_get_associated_token_address() {
        let program_id = spl_token::id();
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();

        let result = SolanaTokenProgram::get_associated_token_address(&program_id, &wallet, &mint);
        let expected = get_associated_token_address_with_program_id(&program_id, &wallet, &mint);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_create_associated_token_account() {
        let program_id = spl_token::id();
        let payer = Pubkey::new_unique();
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();

        let instruction = SolanaTokenProgram::create_associated_token_account(
            &program_id,
            &payer,
            &wallet,
            &mint,
        );

        let expected = create_associated_token_account(&payer, &wallet, &mint, &program_id);

        assert_eq!(instruction.program_id, expected.program_id);
        assert_eq!(instruction.accounts.len(), expected.accounts.len());

        for (i, account) in instruction.accounts.iter().enumerate() {
            assert_eq!(account.pubkey, expected.accounts[i].pubkey);
            assert_eq!(account.is_signer, expected.accounts[i].is_signer);
            assert_eq!(account.is_writable, expected.accounts[i].is_writable);
        }
    }

    #[test]
    fn test_unpack_instruction_spl_token_transfer() {
        let program_id = spl_token::id();
        let amount = 1000u64;

        let instruction = spl_token::instruction::transfer(
            &program_id,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &[],
            amount,
        )
        .unwrap();

        let result = SolanaTokenProgram::unpack_instruction(&program_id, &instruction.data);
        assert!(result.is_ok());

        if let TokenInstruction::Transfer {
            amount: parsed_amount,
        } = result.unwrap()
        {
            assert_eq!(parsed_amount, amount);
        } else {
            panic!("Expected Transfer instruction");
        }
    }

    #[test]
    fn test_unpack_instruction_spl_token_transfer_checked() {
        let program_id = spl_token::id();
        let amount = 1000u64;
        let decimals = 9u8;

        let instruction = spl_token::instruction::transfer_checked(
            &program_id,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &[],
            amount,
            decimals,
        )
        .unwrap();

        let result = SolanaTokenProgram::unpack_instruction(&program_id, &instruction.data);
        assert!(result.is_ok());

        if let TokenInstruction::TransferChecked {
            amount: parsed_amount,
            decimals: parsed_decimals,
        } = result.unwrap()
        {
            assert_eq!(parsed_amount, amount);
            assert_eq!(parsed_decimals, decimals);
        } else {
            panic!("Expected TransferChecked instruction");
        }
    }

    #[test]
    fn test_unpack_instruction_token_2022_transfer() {
        let program_id = spl_token_2022::id();
        let amount = 1000u64;

        #[allow(deprecated)]
        let instruction = spl_token_2022::instruction::transfer(
            &program_id,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &[],
            amount,
        )
        .unwrap();

        let result = SolanaTokenProgram::unpack_instruction(&program_id, &instruction.data);
        assert!(result.is_ok());

        if let TokenInstruction::Transfer {
            amount: parsed_amount,
        } = result.unwrap()
        {
            assert_eq!(parsed_amount, amount);
        } else {
            panic!("Expected Transfer instruction");
        }
    }

    #[test]
    fn test_unpack_instruction_token_2022_transfer_checked() {
        let program_id = spl_token_2022::id();
        let amount = 1000u64;
        let decimals = 9u8;

        let instruction = spl_token_2022::instruction::transfer_checked(
            &program_id,
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &Pubkey::new_unique(),
            &[],
            amount,
            decimals,
        )
        .unwrap();

        let result = SolanaTokenProgram::unpack_instruction(&program_id, &instruction.data);
        assert!(result.is_ok());

        if let TokenInstruction::TransferChecked {
            amount: parsed_amount,
            decimals: parsed_decimals,
        } = result.unwrap()
        {
            assert_eq!(parsed_amount, amount);
            assert_eq!(parsed_decimals, decimals);
        } else {
            panic!("Expected TransferChecked instruction");
        }
    }

    #[test]
    fn test_unpack_instruction_invalid_program() {
        let program_id = Pubkey::new_unique(); // Invalid program ID
        let data = vec![0, 1, 2, 3];

        let result = SolanaTokenProgram::unpack_instruction(&program_id, &data);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TokenError::InvalidTokenProgram(_)
        ));
    }
}
