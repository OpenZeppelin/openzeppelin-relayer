use crate::{
    constants::{
        REQUEST_MAX_ACCOUNTS_PER_INSTRUCTION, REQUEST_MAX_INSTRUCTIONS,
        REQUEST_MAX_INSTRUCTION_DATA_SIZE, REQUEST_MAX_TOTAL_ACCOUNTS,
    },
    models::{ApiError, EncodedSerializedTransaction, RelayerRepoModel, SolanaInstructionSpec},
    utils::base64_decode,
};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::{collections::HashSet, str::FromStr};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
pub struct SolanaTransactionRequest {
    /// Pre-built base64-encoded transaction (mutually exclusive with instructions)
    #[schema(nullable = true)]
    pub transaction: Option<EncodedSerializedTransaction>,

    /// Instructions to build transaction from (mutually exclusive with transaction)
    #[schema(nullable = true)]
    pub instructions: Option<Vec<SolanaInstructionSpec>>,

    /// Optional RFC3339 timestamp when transaction should expire
    #[schema(nullable = true)]
    pub valid_until: Option<String>,
}

impl SolanaTransactionRequest {
    pub fn validate(&self, _relayer: &RelayerRepoModel) -> Result<(), ApiError> {
        let has_transaction = self.transaction.is_some();
        let has_instructions = self
            .instructions
            .as_ref()
            .map(|i| !i.is_empty())
            .unwrap_or(false);

        match (has_transaction, has_instructions) {
            (true, true) => {
                return Err(ApiError::BadRequest(
                    "Cannot provide both transaction and instructions".to_string(),
                ));
            }
            (false, false) => {
                return Err(ApiError::BadRequest(
                    "Must provide either transaction or instructions".to_string(),
                ));
            }
            _ => {}
        }

        // Validate instructions if provided
        if let Some(ref instructions) = self.instructions {
            Self::validate_instructions(instructions)?;
        }

        // Validate valid_until if provided
        if let Some(valid_until) = &self.valid_until {
            match chrono::DateTime::parse_from_rfc3339(valid_until) {
                Ok(valid_until_dt) => {
                    if valid_until_dt <= chrono::Utc::now() {
                        return Err(ApiError::BadRequest(
                            "valid_until cannot be in the past".to_string(),
                        ));
                    }
                }
                Err(_) => {
                    return Err(ApiError::BadRequest(
                        "valid_until must be a valid RFC3339 timestamp".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Validates Solana instruction specifications
    ///
    /// Ensures all instruction fields are valid:
    /// - Number of instructions within reasonable limits (max 64)
    /// - Program IDs are valid Solana public keys (not empty, not default pubkey)
    /// - Account public keys are valid (not empty)
    /// - Accounts per instruction within Solana's limit (max 64)
    /// - Total unique accounts don't exceed Solana's limit (max 64)
    /// - Instruction data is valid base64 and within size limits (max 1000 bytes)
    fn validate_instructions(instructions: &[SolanaInstructionSpec]) -> Result<(), ApiError> {
        if instructions.is_empty() {
            return Err(ApiError::BadRequest(
                "Instructions cannot be empty".to_string(),
            ));
        }

        if instructions.len() > REQUEST_MAX_INSTRUCTIONS {
            return Err(ApiError::BadRequest(format!(
                "Too many instructions: {} exceeds maximum of {}",
                instructions.len(),
                REQUEST_MAX_INSTRUCTIONS
            )));
        }

        let mut unique_accounts = HashSet::new();

        for (idx, instruction) in instructions.iter().enumerate() {
            // Validate program_id is not empty/whitespace
            let trimmed_program_id = instruction.program_id.trim();
            if trimmed_program_id.is_empty() {
                return Err(ApiError::BadRequest(format!(
                    "Instruction {}: program_id cannot be empty",
                    idx
                )));
            }

            // Validate program_id is valid pubkey
            let program_pubkey = Pubkey::from_str(trimmed_program_id).map_err(|e| {
                ApiError::BadRequest(format!(
                    "Instruction {}: Invalid program_id '{}' - {}",
                    idx, trimmed_program_id, e
                ))
            })?;

            // Reject default/zero pubkey as program_id
            if program_pubkey == Pubkey::default() {
                return Err(ApiError::BadRequest(format!(
                    "Instruction {}: program_id cannot be default pubkey",
                    idx
                )));
            }

            unique_accounts.insert(program_pubkey);

            // Validate number of accounts per instruction
            if instruction.accounts.len() > REQUEST_MAX_ACCOUNTS_PER_INSTRUCTION {
                return Err(ApiError::BadRequest(format!(
                    "Instruction {}: Too many accounts {} exceeds maximum of {}",
                    idx,
                    instruction.accounts.len(),
                    REQUEST_MAX_ACCOUNTS_PER_INSTRUCTION
                )));
            }

            // Validate account pubkeys
            for (acc_idx, account) in instruction.accounts.iter().enumerate() {
                let trimmed_pubkey = account.pubkey.trim();
                if trimmed_pubkey.is_empty() {
                    return Err(ApiError::BadRequest(format!(
                        "Instruction {} account {}: pubkey cannot be empty",
                        idx, acc_idx
                    )));
                }

                let pubkey = Pubkey::from_str(trimmed_pubkey).map_err(|e| {
                    ApiError::BadRequest(format!(
                        "Instruction {} account {}: Invalid pubkey '{}' - {}",
                        idx, acc_idx, trimmed_pubkey, e
                    ))
                })?;

                unique_accounts.insert(pubkey);
            }

            // Validate data is valid base64 and decode it
            let decoded_data = base64_decode(&instruction.data).map_err(|e| {
                ApiError::BadRequest(format!("Instruction {}: Invalid base64 data - {}", idx, e))
            })?;

            // Validate decoded data size
            if decoded_data.len() > REQUEST_MAX_INSTRUCTION_DATA_SIZE {
                return Err(ApiError::BadRequest(format!(
                    "Instruction {}: Data too large ({} bytes, max: {} bytes)",
                    idx,
                    decoded_data.len(),
                    REQUEST_MAX_INSTRUCTION_DATA_SIZE
                )));
            }
        }

        // Validate total unique accounts across all instructions
        if unique_accounts.len() > REQUEST_MAX_TOTAL_ACCOUNTS {
            return Err(ApiError::BadRequest(format!(
                "Too many unique accounts: {} exceeds Solana's limit of {}",
                unique_accounts.len(),
                REQUEST_MAX_TOTAL_ACCOUNTS
            )));
        }

        Ok(())
    }
}
