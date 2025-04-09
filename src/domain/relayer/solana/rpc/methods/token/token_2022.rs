use solana_sdk::{
    account::Account as SolanaAccount, instruction::Instruction, pubkey::Pubkey,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token_2022::{
    instruction as token_2022_instruction, state::Account as Token2022Account,
};

use super::{SolanaToken, TokenAccount, TokenError, TokenInstruction};

pub struct Token2022;

impl SolanaToken for Token2022 {
    fn program_id(&self) -> Pubkey {
        spl_token_2022::id()
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
        token_2022_instruction::transfer_checked(
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
        Token2022Account::unpack(data)
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
        token_2022_instruction::TokenInstruction::unpack(data)
            .map_err(|e| TokenError::UnpackError(e.to_string()))
            .map(|instruction| match instruction {
                token_2022_instruction::TokenInstruction::Transfer { amount } => {
                    TokenInstruction::Transfer { amount }
                }
                token_2022_instruction::TokenInstruction::TransferChecked { amount, decimals, .. } => {
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
