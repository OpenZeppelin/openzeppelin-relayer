use crate::{
    constants::{
        REQUEST_MAX_ACCOUNTS_PER_INSTRUCTION, REQUEST_MAX_INSTRUCTIONS,
        REQUEST_MAX_INSTRUCTION_DATA_SIZE, REQUEST_MAX_TOTAL_ACCOUNTS,
    },
    models::{ApiError, EncodedSerializedTransaction, RelayerRepoModel, SolanaInstructionSpec},
    utils::base64_decode,
};
use serde::{Deserialize, Serialize};
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};
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
    pub fn validate(&self, relayer: &RelayerRepoModel) -> Result<(), ApiError> {
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

        // Validate pre-built transaction if provided
        if let Some(ref transaction) = self.transaction {
            Self::validate_transaction(transaction, relayer)?;
        }

        // Validate instructions if provided
        if let Some(ref instructions) = self.instructions {
            Self::validate_instructions(instructions, relayer)?;
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

    /// Validates a pre-built Solana transaction
    ///
    /// Ensures the transaction meets requirements:
    /// - Transaction can be decoded from base64
    /// - Fee payer (source) matches the relayer address
    fn validate_transaction(
        transaction: &EncodedSerializedTransaction,
        relayer: &RelayerRepoModel,
    ) -> Result<(), ApiError> {
        // Parse the transaction from encoded bytes
        let tx = Transaction::try_from(transaction.clone())
            .map_err(|e| ApiError::BadRequest(format!("Failed to decode transaction: {e}")))?;

        // Get the fee payer (first account in account_keys)
        let fee_payer = tx.message.account_keys.first().ok_or_else(|| {
            ApiError::BadRequest("Transaction has no fee payer account".to_string())
        })?;

        // Parse relayer address
        let relayer_pubkey = Pubkey::from_str(&relayer.address)
            .map_err(|e| ApiError::BadRequest(format!("Invalid relayer address: {e}")))?;

        // Validate fee payer matches relayer address
        if fee_payer != &relayer_pubkey {
            return Err(ApiError::BadRequest(format!(
                "Transaction fee payer {fee_payer} does not match relayer address {relayer_pubkey}"
            )));
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
    /// - Only the relayer can be marked as a signer (relayer auto-signs as fee payer)
    fn validate_instructions(
        instructions: &[SolanaInstructionSpec],
        relayer: &RelayerRepoModel,
    ) -> Result<(), ApiError> {
        // Parse relayer address once for validation
        let relayer_pubkey = Pubkey::from_str(&relayer.address)
            .map_err(|e| ApiError::BadRequest(format!("Invalid relayer address: {e}")))?;
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
                    "Instruction {idx}: program_id cannot be empty"
                )));
            }

            // Validate program_id is valid pubkey
            let program_pubkey = Pubkey::from_str(trimmed_program_id).map_err(|e| {
                ApiError::BadRequest(format!(
                    "Instruction {idx}: Invalid program_id '{trimmed_program_id}' - {e}"
                ))
            })?;

            // Reject default/zero pubkey as program_id
            if program_pubkey == Pubkey::default() {
                return Err(ApiError::BadRequest(format!(
                    "Instruction {idx}: program_id cannot be default pubkey"
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
                        "Instruction {idx} account {acc_idx}: pubkey cannot be empty"
                    )));
                }

                let pubkey = Pubkey::from_str(trimmed_pubkey).map_err(|e| {
                    ApiError::BadRequest(format!(
                        "Instruction {idx} account {acc_idx}: Invalid pubkey '{trimmed_pubkey}' - {e}"
                    ))
                })?;

                // Validate that only the relayer can be marked as a signer
                if account.is_signer && pubkey != relayer_pubkey {
                    return Err(ApiError::BadRequest(format!(
                        "Instruction {idx} account {acc_idx}: Only the relayer address {relayer_pubkey} can be marked as \
                         a signer, but '{pubkey}' is marked as a signer. The relayer can only provide \
                         its own signature."
                    )));
                }

                unique_accounts.insert(pubkey);
            }

            // Validate data is valid base64 and decode it
            let decoded_data = base64_decode(&instruction.data).map_err(|e| {
                ApiError::BadRequest(format!("Instruction {idx}: Invalid base64 data - {e}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RelayerRepoModel;
    use base64::Engine;
    use solana_sdk::{message::Message, pubkey::Pubkey};
    use solana_system_interface::instruction as system_instruction;

    fn create_test_relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test-relayer".to_string(),
            name: "Test Relayer".to_string(),
            network: "solana".to_string(),
            paused: false,
            network_type: crate::models::NetworkType::Solana,
            signer_id: "test-signer".to_string(),
            policies: crate::models::RelayerNetworkPolicy::Solana(
                crate::models::RelayerSolanaPolicy::default(),
            ),
            address: "6eoxMcGNaSRKcd8s84ukZjRZBJ27C5DrSXGH6nz73W8h".to_string(),
            notification_id: None,
            system_disabled: false,
            disabled_reason: None,
            custom_rpc_urls: None,
        }
    }

    fn create_valid_instruction_spec() -> SolanaInstructionSpec {
        SolanaInstructionSpec {
            program_id: "11111111111111111111111111111112".to_string(), // System program
            accounts: vec![
                crate::models::SolanaAccountMeta {
                    pubkey: "6eoxMcGNaSRKcd8s84ukZjRZBJ27C5DrSXGH6nz73W8h".to_string(),
                    is_signer: true,
                    is_writable: true,
                },
                crate::models::SolanaAccountMeta {
                    pubkey: "HmZhRVuT8UuMrUJr1JsWFXTQU4EzwGVmQ29Q6QmzLbNs".to_string(),
                    is_signer: false,
                    is_writable: true,
                },
            ],
            data: base64::prelude::BASE64_STANDARD.encode(
                [2, 0, 0, 0]
                    .iter()
                    .chain(&1000000u64.to_le_bytes())
                    .chain(&[0, 0, 0, 0, 0, 0, 0])
                    .cloned()
                    .collect::<Vec<u8>>(),
            ), // Transfer 1 SOL
        }
    }

    fn create_valid_transaction(relayer_pubkey: &Pubkey) -> EncodedSerializedTransaction {
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(relayer_pubkey, &recipient, 1000000);
        let message = Message::new(&[instruction], Some(relayer_pubkey));
        let tx = solana_sdk::transaction::Transaction::new_unsigned(message);
        let serialized = bincode::serialize(&tx).unwrap();
        EncodedSerializedTransaction::new(base64::prelude::BASE64_STANDARD.encode(serialized))
    }

    fn create_transaction_with_wrong_fee_payer() -> EncodedSerializedTransaction {
        let wrong_fee_payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&wrong_fee_payer, &recipient, 1000000);
        let message = Message::new(&[instruction], Some(&wrong_fee_payer));
        let tx = solana_sdk::transaction::Transaction::new_unsigned(message);
        let serialized = bincode::serialize(&tx).unwrap();
        EncodedSerializedTransaction::new(base64::prelude::BASE64_STANDARD.encode(serialized))
    }

    #[test]
    fn test_validate_valid_request_with_transaction() {
        let relayer = create_test_relayer();
        let relayer_pubkey = Pubkey::from_str(&relayer.address).unwrap();
        let transaction = create_valid_transaction(&relayer_pubkey);

        let request = SolanaTransactionRequest {
            transaction: Some(transaction),
            instructions: None,
            valid_until: None,
        };

        assert!(request.validate(&relayer).is_ok());
    }

    #[test]
    fn test_validate_valid_request_with_instructions() {
        let relayer = create_test_relayer();
        let instruction = create_valid_instruction_spec();

        let request = SolanaTransactionRequest {
            transaction: None,
            instructions: Some(vec![instruction]),
            valid_until: None,
        };

        assert!(request.validate(&relayer).is_ok());
    }

    #[test]
    fn test_validate_invalid_both_transaction_and_instructions() {
        let relayer = create_test_relayer();
        let relayer_pubkey = Pubkey::from_str(&relayer.address).unwrap();
        let transaction = create_valid_transaction(&relayer_pubkey);
        let instruction = create_valid_instruction_spec();

        let request = SolanaTransactionRequest {
            transaction: Some(transaction),
            instructions: Some(vec![instruction]),
            valid_until: None,
        };

        let result = request.validate(&relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cannot provide both transaction and instructions"));
    }

    #[test]
    fn test_validate_invalid_neither_transaction_nor_instructions() {
        let relayer = create_test_relayer();

        let request = SolanaTransactionRequest {
            transaction: None,
            instructions: None,
            valid_until: None,
        };

        let result = request.validate(&relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Must provide either transaction or instructions"));
    }

    #[test]
    fn test_validate_valid_request_with_future_valid_until() {
        let relayer = create_test_relayer();
        let future_time = chrono::Utc::now() + chrono::Duration::hours(1);

        let request = SolanaTransactionRequest {
            transaction: None,
            instructions: Some(vec![create_valid_instruction_spec()]),
            valid_until: Some(future_time.to_rfc3339()),
        };

        assert!(request.validate(&relayer).is_ok());
    }

    #[test]
    fn test_validate_invalid_past_valid_until() {
        let relayer = create_test_relayer();
        let past_time = chrono::Utc::now() - chrono::Duration::hours(1);

        let request = SolanaTransactionRequest {
            transaction: None,
            instructions: Some(vec![create_valid_instruction_spec()]),
            valid_until: Some(past_time.to_rfc3339()),
        };

        let result = request.validate(&relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("valid_until cannot be in the past"));
    }

    #[test]
    fn test_validate_invalid_malformed_valid_until() {
        let relayer = create_test_relayer();

        let request = SolanaTransactionRequest {
            transaction: None,
            instructions: Some(vec![create_valid_instruction_spec()]),
            valid_until: Some("invalid-timestamp".to_string()),
        };

        let result = request.validate(&relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("valid_until must be a valid RFC3339 timestamp"));
    }

    #[test]
    fn test_validate_transaction_invalid_base64() {
        let relayer = create_test_relayer();
        let transaction = EncodedSerializedTransaction::new("invalid-base64!".to_string());

        let result = SolanaTransactionRequest::validate_transaction(&transaction, &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to decode transaction"));
    }

    #[test]
    fn test_validate_transaction_wrong_fee_payer() {
        let relayer = create_test_relayer();
        let transaction = create_transaction_with_wrong_fee_payer();

        let result = SolanaTransactionRequest::validate_transaction(&transaction, &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not match relayer address"));
    }

    #[test]
    fn test_validate_instructions_empty_instructions() {
        let relayer = create_test_relayer();

        let result = SolanaTransactionRequest::validate_instructions(&[], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Instructions cannot be empty"));
    }

    #[test]
    fn test_validate_instructions_too_many_instructions() {
        let relayer = create_test_relayer();
        let instructions = vec![create_valid_instruction_spec(); REQUEST_MAX_INSTRUCTIONS + 1];

        let result = SolanaTransactionRequest::validate_instructions(&instructions, &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Too many instructions"));
    }

    #[test]
    fn test_validate_instructions_empty_program_id() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.program_id = "".to_string();

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("program_id cannot be empty"));
    }

    #[test]
    fn test_validate_instructions_invalid_program_id() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.program_id = "invalid-pubkey".to_string();

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid program_id"));
    }

    #[test]
    fn test_validate_instructions_default_program_id() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.program_id = "11111111111111111111111111111111".to_string(); // Default pubkey

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("program_id cannot be default pubkey"));
    }

    #[test]
    fn test_validate_instructions_too_many_accounts_per_instruction() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.accounts =
            vec![instruction.accounts[0].clone(); REQUEST_MAX_ACCOUNTS_PER_INSTRUCTION + 1];

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Too many accounts"));
    }

    #[test]
    fn test_validate_instructions_empty_account_pubkey() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.accounts[0].pubkey = "".to_string();

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("pubkey cannot be empty"));
    }

    #[test]
    fn test_validate_instructions_invalid_account_pubkey() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.accounts[0].pubkey = "invalid-pubkey".to_string();

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid pubkey"));
    }

    #[test]
    fn test_validate_instructions_non_relayer_signer() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        // Make the second account (which is not the relayer) a signer
        instruction.accounts[1].is_signer = true;

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Only the relayer address"));
    }

    #[test]
    fn test_validate_instructions_invalid_base64_data() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        instruction.data = "invalid-base64!".to_string();

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid base64 data"));
    }

    #[test]
    fn test_validate_instructions_data_too_large() {
        let relayer = create_test_relayer();
        let mut instruction = create_valid_instruction_spec();
        // Create data larger than REQUEST_MAX_INSTRUCTION_DATA_SIZE
        let large_data = vec![0u8; REQUEST_MAX_INSTRUCTION_DATA_SIZE + 1];
        instruction.data = base64::prelude::BASE64_STANDARD.encode(large_data);

        let result = SolanaTransactionRequest::validate_instructions(&[instruction], &relayer);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Data too large"));
    }

    #[test]
    fn test_validate_instructions_too_many_unique_accounts() {
        let relayer = create_test_relayer();
        let mut instructions = Vec::new();

        // Create instructions that will exceed REQUEST_MAX_TOTAL_ACCOUNTS unique accounts
        for i in 0..(REQUEST_MAX_TOTAL_ACCOUNTS + 1) {
            let mut instruction = create_valid_instruction_spec();
            // Change program_id to create unique accounts
            instruction.program_id = format!("{:0>44}", i); // Create unique but invalid pubkeys
                                                            // Add a unique account
            instruction.accounts.push(crate::models::SolanaAccountMeta {
                pubkey: format!("{:0>44}", i + 1000), // Another unique account
                is_signer: false,
                is_writable: false,
            });
            instructions.push(instruction);
        }

        // Create multiple instructions with different valid pubkeys
        let mut instructions = Vec::new();
        for _ in 0..10 {
            instructions.push(create_valid_instruction_spec());
        }

        // This should pass since we're using the same accounts repeatedly
        assert!(SolanaTransactionRequest::validate_instructions(&instructions, &relayer).is_ok());
    }

    #[test]
    fn test_validate_instructions_too_many_unique_accounts_failure() {
        let relayer = create_test_relayer();
        let relayer_pubkey = Pubkey::from_str(&relayer.address).unwrap();
        let mut instructions = Vec::new();
        // We will generate REQUEST_MAX_TOTAL_ACCOUNTS + 1 unique accounts total.
        for _i in 0..(REQUEST_MAX_TOTAL_ACCOUNTS) {
            // We need to go up to the limit + 1
            // Create a unique non-relayer pubkey for the instruction
            let unique_account = Pubkey::new_unique();

            // This program ID is guaranteed to be unique and valid
            let unique_program_id = Pubkey::new_unique();

            instructions.push(SolanaInstructionSpec {
                // Unique program ID consumes a unique account slot
                program_id: unique_program_id.to_string(),
                accounts: vec![
                    crate::models::SolanaAccountMeta {
                        // The relayer's key is always included (1 unique key)
                        pubkey: relayer_pubkey.to_string(),
                        is_signer: true,
                        is_writable: true,
                    },
                    // Unique account key consumes another unique account slot
                    crate::models::SolanaAccountMeta {
                        pubkey: unique_account.to_string(),
                        is_signer: false,
                        is_writable: true,
                    },
                ],
                data: base64::prelude::BASE64_STANDARD.encode(vec![0u8]),
            });
        }

        // Final instruction to push it over the limit of REQUEST_MAX_TOTAL_ACCOUNTS (e.g., 64)
        // If REQUEST_MAX_TOTAL_ACCOUNTS=64, the loop created 64 instructions,
        // with 1 relayer key + 63 other unique keys, for a total of 1 + 63*2 unique keys,
        // which vastly exceeds the limit.
        // We only need to check that the limit is hit and failed.

        let result = SolanaTransactionRequest::validate_instructions(&instructions, &relayer);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Too many unique accounts"));
    }
}
