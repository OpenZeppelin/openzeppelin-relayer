use crate::models::{
    ApiError, EncodedSerializedTransaction, RelayerRepoModel, SolanaInstructionSpec,
};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
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
    /// - Program IDs are valid Solana public keys
    /// - Account public keys are valid
    /// - Instruction data is valid base64
    fn validate_instructions(instructions: &[SolanaInstructionSpec]) -> Result<(), ApiError> {
        if instructions.is_empty() {
            return Err(ApiError::BadRequest(
                "Instructions cannot be empty".to_string(),
            ));
        }

        for (idx, instruction) in instructions.iter().enumerate() {
            // Validate program_id
            Pubkey::from_str(&instruction.program_id).map_err(|_| {
                ApiError::BadRequest(format!("Instruction {}: Invalid program_id", idx))
            })?;

            // Validate account pubkeys
            for (acc_idx, account) in instruction.accounts.iter().enumerate() {
                Pubkey::from_str(&account.pubkey).map_err(|_| {
                    ApiError::BadRequest(format!(
                        "Instruction {} account {}: Invalid pubkey",
                        idx, acc_idx
                    ))
                })?;
            }

            // Validate data is valid base64
            general_purpose::STANDARD
                .decode(&instruction.data)
                .map_err(|_| {
                    ApiError::BadRequest(format!("Instruction {}: Invalid base64 data", idx))
                })?;
        }

        Ok(())
    }
}
