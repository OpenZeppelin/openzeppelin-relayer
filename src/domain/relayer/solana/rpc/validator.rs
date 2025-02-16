/// Validator for Solana transactions that enforces relayer policies and transaction
/// constraints.
///
/// This validator ensures that transactions meet the following criteria:
/// * Use allowed programs and accounts
/// * Have valid blockhash
/// * Meet size and signature requirements
/// * Have correct fee payer configuration
/// * Comply with relayer policies
use crate::{
    models::{RelayerRepoModel, RelayerSolanaPolicy},
    services::{SolanaProvider, SolanaProviderTrait},
};
use solana_client::rpc_response::RpcSimulateTransactionResult;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, CompiledInstruction, Instruction},
    pubkey::Pubkey,
    transaction::Transaction,
};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum SolanaTransactionValidationError {
    #[error("Failed to decode transaction: {0}")]
    DecodeError(String),
    #[error("Failed to deserialize transaction: {0}")]
    DeserializeError(String),
    #[error("Validation error: {0}")]
    SigningError(String),
    #[error("Simulation error: {0}")]
    SimulationError(String),
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
    #[error("Blockhash {0} is expired")]
    ExpiredBlockhash(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Fee payer error: {0}")]
    FeePayer(String),
}

#[allow(dead_code)]
pub struct SolanaTransactionValidator {}

#[allow(dead_code)]
impl SolanaTransactionValidator {
    fn decompile_instruction(
        &self,
        tx: &Transaction,
        ix: &CompiledInstruction,
        account_keys: &[Pubkey],
    ) -> Instruction {
        let program_id = account_keys[ix.program_id_index as usize];
        let accounts = ix
            .accounts
            .iter()
            .map(|&i| AccountMeta {
                pubkey: account_keys[i as usize],
                is_signer: tx.message.header.num_required_signatures > i,
                is_writable: tx.message.is_maybe_writable(i as usize, None),
            })
            .collect();

        Instruction {
            program_id,
            accounts,
            data: ix.data.clone(),
        }
    }

    pub fn decompile_instructions(&self, tx: &Transaction) -> Vec<Instruction> {
        tx.message
            .instructions
            .iter()
            .map(|ix| self.decompile_instruction(tx, ix, &tx.message.account_keys))
            .collect()
    }

    /// Validates a transaction against all relayer policies and constraints before signing.
    ///
    /// # Arguments
    ///
    /// * `tx` - The Solana transaction to validate
    /// * `relayer` - The relayer model containing policies
    /// * `provider` - The Solana provider for blockchain interaction
    pub async fn validate_sign_transaction(
        tx: &Transaction,
        relayer: &RelayerRepoModel,
        provider: &SolanaProvider,
    ) -> Result<(), SolanaTransactionValidationError> {
        let policy = &relayer.policies.get_solana_policy();

        SolanaTransactionValidator::validate_allowed_accounts(tx, policy)?;
        SolanaTransactionValidator::validate_allowed_programs(tx, policy)?;
        SolanaTransactionValidator::validate_disallowed_accounts(tx, policy)?;
        SolanaTransactionValidator::validate_num_signatures(tx, policy)?;
        SolanaTransactionValidator::validate_fee_payer(tx, &relayer.address)?;
        SolanaTransactionValidator::validate_blockhash(tx, provider).await?;
        SolanaTransactionValidator::validate_data_size(tx, policy)?;
        // SolanaTransactionValidationError::validate_transaction_fee(tx, policy);

        Ok(())
    }

    /// Validates that the transaction's fee payer matches the relayer's address.
    pub fn validate_fee_payer(
        tx: &Transaction,
        relayer_signer_address: &str,
    ) -> Result<(), SolanaTransactionValidationError> {
        // Get fee payer (first account in account_keys)
        let fee_payer = tx.message.account_keys.first().ok_or_else(|| {
            SolanaTransactionValidationError::FeePayer("No fee payer account found".to_string())
        })?;

        // Convert relayer address string to Pubkey
        let relayer_pubkey = Pubkey::from_str(relayer_signer_address).map_err(|e| {
            SolanaTransactionValidationError::ValidationError(format!(
                "Invalid relayer address: {}",
                e
            ))
        })?;

        // Verify fee payer matches relayer address
        if fee_payer != &relayer_pubkey {
            return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                "Fee payer {} does not match relayer address {}",
                fee_payer, relayer_pubkey
            )));
        }

        // Verify fee payer is a signer
        if !tx.message.header.num_required_signatures > 0 {
            return Err(SolanaTransactionValidationError::FeePayer(
                "Fee payer must be a signer".to_string(),
            ));
        }

        Ok(())
    }

    /// Validates that the transaction's blockhash is still valid.
    pub async fn validate_blockhash(
        tx: &Transaction,
        provider: &SolanaProvider,
    ) -> Result<(), SolanaTransactionValidationError> {
        let blockhash = tx.message.recent_blockhash;

        // Check if blockhash is still valid
        let is_valid = provider
            .is_blockhash_valid(&blockhash, CommitmentConfig::confirmed())
            .await
            .map_err(|e| {
                SolanaTransactionValidationError::ValidationError(format!(
                    "Failed to check blockhash validity: {}",
                    e
                ))
            })?;

        if !is_valid {
            return Err(SolanaTransactionValidationError::ExpiredBlockhash(format!(
                "Blockhash {} is no longer valid",
                blockhash
            )));
        }

        Ok(())
    }

    /// Validates the number of required signatures against policy limits.
    pub fn validate_num_signatures(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        let num_signatures = tx.message.header.num_required_signatures;

        // Check against maximum allowed signatures
        if let Some(max_signatures) = policy.max_signatures {
            if num_signatures > max_signatures as u8 {
                return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                    "Transaction requires {} signatures, which exceeds maximum allowed {}",
                    num_signatures, max_signatures
                )));
            }
        }

        Ok(())
    }

    /// Validates that the transaction's programs are allowed by the relayer's policy.
    pub fn validate_allowed_programs(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(allowed_programs) = &policy.allowed_programs {
            for program_id in tx
                .message
                .instructions
                .iter()
                .map(|ix| tx.message.account_keys[ix.program_id_index as usize])
            {
                if !allowed_programs.contains(&program_id.to_string()) {
                    return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                        "Program {} not allowed",
                        program_id
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validates that the transaction's accounts are allowed by the relayer's policy.
    pub fn validate_allowed_accounts(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(allowed_accounts) = &policy.allowed_accounts {
            for account_key in &tx.message.account_keys {
                if !allowed_accounts.contains(&account_key.to_string()) {
                    return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                        "Account {} not allowed",
                        account_key
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validates that the transaction's accounts are not disallowed by the relayer's policy.
    pub fn validate_disallowed_accounts(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(disallowed_accounts) = &policy.disallowed_accounts {
            for account_key in &tx.message.account_keys {
                if disallowed_accounts.contains(&account_key.to_string()) {
                    return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                        "Account {} is explicitly disallowed",
                        account_key
                    )));
                }
            }
        }

        Ok(())
    }

    /// Validates that the transaction's data size is within policy limits.
    pub fn validate_data_size(
        tx: &Transaction,
        config: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        let max_size: usize = config.max_tx_data_size.into();
        let tx_bytes = bincode::serialize(tx)
            .map_err(|e| SolanaTransactionValidationError::DeserializeError(e.to_string()))?;

        if tx_bytes.len() > max_size {
            return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                "Transaction size {} exceeds maximum allowed {}",
                tx_bytes.len(),
                max_size
            )));
        }
        Ok(())
    }

    // pub async fn validate_transaction_fee(
    //     &self,
    //     tx: &Transaction,
    //     policy: &RelayerSolanaPolicy,
    // ) -> Result<(), TransactionValidationError> {
    //     // Simulate transaction to get fee
    //     let simulation_result = self.provider
    //         .simulate_transaction(tx)
    //         .await
    //         .map_err(|e| TransactionValidationError::ValidationError(
    //             format!("Failed to simulate transaction: {}", e)
    //         ))?;

    //     let fee = simulation_result.value.units_consumed
    //         .ok_or_else(|| TransactionValidationError::ValidationError(
    //             "Failed to get fee from simulation".to_string()
    //         ))?;

    //     // Check against maximum allowed fee
    //     if let Some(max_fee) = policy.max_fee_in_lamports {
    //         if fee > max_fee {
    //             return Err(TransactionValidationError::PolicyViolation(
    //                 format!(
    //                     "Transaction fee {} lamports exceeds maximum allowed {} lamports",
    //                     fee, max_fee
    //                 )
    //             ));
    //         }
    //     }

    //     // Check against minimum required fee
    //     if let Some(min_fee) = policy.min_fee_in_lamports {
    //         if fee < min_fee {
    //             return Err(TransactionValidationError::PolicyViolation(
    //                 format!(
    //                     "Transaction fee {} lamports is below minimum required {} lamports",
    //                     fee, min_fee
    //                 )
    //             ));
    //         }
    //     }

    //     Ok(())
    // }

    //   pub fn validate_token_transfers(&self, tx: &Transaction, policy: &RelayerSolanaPolicy) ->
    // Result<(), TransactionValidationError> {     // Check if we have token transfer
    // restrictions     let allowed_tokens = match &policy.allowed_tokens {
    //         Some(tokens) if !tokens.is_empty() => tokens,
    //         _ => return Ok(()) // No token restrictions
    //     };

    //     for ix in &tx.message.instructions {
    //         let program_id = tx.message.account_keys[ix.program_id_index as usize];

    //         // Check if instruction is a token transfer (SPL Token Program)
    //         if program_id == spl_token::id() {
    //             // Decode token instruction
    //             if let Ok(token_ix) = spl_token::instruction::TokenInstruction::unpack(&ix.data)
    // {                 match token_ix {
    //                     spl_token::instruction::TokenInstruction::Transfer { .. } |
    //                     spl_token::instruction::TokenInstruction::TransferChecked { .. } => {
    //                         // Get mint address from token account
    //                         let token_account = &tx.message.account_keys[ix.accounts[0] as
    // usize];                         let mint = self.get_token_mint(token_account)?;

    //                         if !allowed_tokens.iter().any(|t| t.mint == mint.to_string()) {
    //                             return Err(TransactionValidationError::PolicyViolation(
    //                                 format!("Token {} not allowed for transfers", mint)
    //                             ));
    //                         }
    //                     },
    //                     _ => continue // Not a transfer instruction
    //                 }
    //             }
    //         }
    //     }

    //     Ok(())
    // }

    /// Simulates transaction
    pub async fn simulate_transaction(
        tx: &Transaction,
        provider: &SolanaProvider,
    ) -> Result<RpcSimulateTransactionResult, SolanaTransactionValidationError> {
        provider
            .simulate_transaction(tx)
            .await
            .map_err(|e| SolanaTransactionValidationError::SimulationError(e.to_string()))
    }
}
