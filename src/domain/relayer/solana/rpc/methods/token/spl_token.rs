use solana_sdk::{
    account::Account as SolanaAccount, instruction::Instruction, pubkey::Pubkey,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::{
    instruction as spl_token_instruction, state::Account as SplTokenAccount,
};

use super::{SolanaToken, TokenAccount, TokenError, TokenInstruction};

pub struct SplToken;

impl SolanaToken for SplToken {
    fn program_id(&self) -> Pubkey {
        spl_token::id()
    }

    fn is_token_program(&self, program_id: &Pubkey) -> bool {
        program_id == &self.program_id()
    }

    fn get_associated_token_address(&self, wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
        get_associated_token_address(wallet, mint)
    }

    fn create_associated_token_account(
        &self,
        payer: &Pubkey,
        wallet: &Pubkey,
        mint: &Pubkey,
    ) -> Instruction {
        create_associated_token_account(payer, wallet, mint, &self.program_id())
    }

    fn create_transfer_checked_instruction(
        &self,
        source: &Pubkey,
        mint: &Pubkey,
        destination: &Pubkey,
        authority: &Pubkey,
        amount: u64,
        decimals: u8,
    ) -> Result<Instruction, TokenError> {
        spl_token_instruction::transfer_checked(
            &self.program_id(),
            source,
            mint,
            destination,
            authority,
            &[],
            amount,
            decimals,
        )
        .map_err(|e| TokenError::InstructionError(e.to_string()))
    }

    fn unpack_account(&self, data: &[u8]) -> Result<TokenAccount, TokenError> {
        SplTokenAccount::unpack(data)
            .map_err(|e| TokenError::UnpackError(e.to_string()))
            .map(|account| TokenAccount {
                mint: account.mint,
                owner: account.owner,
                amount: account.amount,
                delegate: account.delegate,
                state: account.state as u8,
                is_native: account.is_native,
                delegated_amount: account.delegated_amount,
                close_authority: account.close_authority,
            })
    }

    fn unpack_instruction(&self, data: &[u8]) -> Result<TokenInstruction, TokenError> {
        spl_token_instruction::TokenInstruction::unpack(data)
            .map_err(|e| TokenError::UnpackError(e.to_string()))
            .map(|instruction| match instruction {
                spl_token_instruction::TokenInstruction::Transfer { amount } => {
                    TokenInstruction::Transfer { amount }
                }
                spl_token_instruction::TokenInstruction::TransferChecked { amount, decimals, .. } => {
                    TokenInstruction::TransferChecked { amount, decimals }
                }
                _ => TokenInstruction::Other,
            })
    }

    fn verify_account_owner(&self, account: &SolanaAccount) -> bool {
        account.owner == self.program_id()
    }

    fn get_mint_from_account(&self, account_data: &[u8]) -> Result<Pubkey, TokenError> {
        let account = self.unpack_account(account_data)?;
        Ok(account.mint)
    }
}
