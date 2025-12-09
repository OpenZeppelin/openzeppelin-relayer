//! Utility functions for Solana transaction domain logic.

use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    transaction::Transaction as SolanaTransaction,
};
use std::str::FromStr;

use crate::{
    constants::MAXIMUM_SOLANA_TX_ATTEMPTS,
    models::{
        EncodedSerializedTransaction, SolanaInstructionSpec, SolanaTransactionStatus,
        TransactionError, TransactionRepoModel, TransactionStatus,
    },
    utils::base64_decode,
};

/// Checks if a Solana transaction has exceeded the maximum number of resubmission attempts.
///
/// Each time a transaction is resubmitted with a fresh blockhash, a new signature is generated
/// and appended to tx.hashes. This function checks if that limit has been exceeded.
///
/// Similar to EVM's `too_many_attempts` but tailored for Solana's resubmission behavior.
pub fn too_many_solana_attempts(tx: &TransactionRepoModel) -> bool {
    tx.hashes.len() >= MAXIMUM_SOLANA_TX_ATTEMPTS
}

/// Determines if a transaction's blockhash can be safely updated.
///
/// A blockhash can only be updated if the transaction requires a single signature (the relayer).
/// Multi-signer transactions cannot have their blockhash updated because it would invalidate
/// the existing signatures from other parties.
///
/// # Returns
/// - `true` if the transaction has only one required signer (relayer can update blockhash)
/// - `false` if the transaction has multiple required signers (blockhash is locked)
///
/// # Use Cases
/// - **Prepare phase**: Decide whether to fetch a fresh blockhash
/// - **Submit phase**: Decide whether BlockhashNotFound error is retriable
pub fn is_resubmitable(tx: &SolanaTransaction) -> bool {
    tx.message.header.num_required_signatures <= 1
}

/// Maps Solana on-chain transaction status to repository transaction status.
///
/// This mapping is used consistently across status checks to ensure uniform
/// status transitions:
/// - `Processed` → `Mined`: Transaction included in a block
/// - `Confirmed` → `Mined`: Transaction confirmed by supermajority
/// - `Finalized` → `Confirmed`: Transaction finalized (irreversible)
/// - `Failed` → `Failed`: Transaction failed on-chain
pub fn map_solana_status_to_transaction_status(
    solana_status: SolanaTransactionStatus,
) -> TransactionStatus {
    match solana_status {
        SolanaTransactionStatus::Processed => TransactionStatus::Mined,
        SolanaTransactionStatus::Confirmed => TransactionStatus::Mined,
        SolanaTransactionStatus::Finalized => TransactionStatus::Confirmed,
        SolanaTransactionStatus::Failed => TransactionStatus::Failed,
    }
}

/// Decodes a Solana transaction from the transaction repository model.
///
/// Extracts the Solana transaction data and deserializes it into a SolanaTransaction.
/// This is a pure helper function that can be used anywhere in the Solana transaction domain.
///
/// Note: This only works for transactions that have already been built (transaction field is Some).
/// For instructions-based transactions that haven't been prepared yet, this will return an error.
pub fn decode_solana_transaction(
    tx: &TransactionRepoModel,
) -> Result<SolanaTransaction, TransactionError> {
    let solana_data = tx.network_data.get_solana_transaction_data()?;

    if let Some(transaction_str) = &solana_data.transaction {
        decode_solana_transaction_from_string(transaction_str)
    } else {
        Err(TransactionError::ValidationError(
            "Transaction not yet built - only available after preparation".to_string(),
        ))
    }
}

/// Decodes a Solana transaction from a base64-encoded string.
pub fn decode_solana_transaction_from_string(
    encoded: &str,
) -> Result<SolanaTransaction, TransactionError> {
    let encoded_tx = EncodedSerializedTransaction::new(encoded.to_string());
    SolanaTransaction::try_from(encoded_tx)
        .map_err(|e| TransactionError::ValidationError(format!("Invalid transaction: {e}")))
}

/// Converts instruction specifications to Solana SDK instructions.
///
/// Validates and converts each instruction specification by:
/// - Parsing program IDs and account pubkeys from base58 strings
/// - Decoding base64 instruction data
///
/// # Arguments
/// * `instructions` - Array of instruction specifications from the request
///
/// # Returns
/// Vector of Solana SDK `Instruction` objects ready to be included in a transaction
pub fn convert_instruction_specs_to_instructions(
    instructions: &[SolanaInstructionSpec],
) -> Result<Vec<Instruction>, TransactionError> {
    let mut solana_instructions = Vec::new();

    for (idx, spec) in instructions.iter().enumerate() {
        let program_id = Pubkey::from_str(&spec.program_id).map_err(|e| {
            TransactionError::ValidationError(format!("Instruction {idx}: Invalid program_id: {e}"))
        })?;

        let accounts = spec
            .accounts
            .iter()
            .enumerate()
            .map(|(acc_idx, a)| {
                let pubkey = Pubkey::from_str(&a.pubkey).map_err(|e| {
                    TransactionError::ValidationError(format!(
                        "Instruction {idx} account {acc_idx}: Invalid pubkey: {e}"
                    ))
                })?;
                Ok(AccountMeta {
                    pubkey,
                    is_signer: a.is_signer,
                    is_writable: a.is_writable,
                })
            })
            .collect::<Result<Vec<_>, TransactionError>>()?;

        let data = base64_decode(&spec.data).map_err(|e| {
            TransactionError::ValidationError(format!(
                "Instruction {idx}: Invalid base64 data: {e}"
            ))
        })?;

        solana_instructions.push(Instruction {
            program_id,
            accounts,
            data,
        });
    }

    Ok(solana_instructions)
}

/// Builds a Solana transaction from instruction specifications.
///
/// # Arguments
/// * `instructions` - Array of instruction specifications
/// * `payer` - Public key of the fee payer (must be the first signer)
/// * `recent_blockhash` - Recent blockhash from the network
///
/// # Returns
/// A fully formed transaction ready to be signed
pub fn build_transaction_from_instructions(
    instructions: &[SolanaInstructionSpec],
    payer: &Pubkey,
    recent_blockhash: Hash,
) -> Result<SolanaTransaction, TransactionError> {
    let solana_instructions = convert_instruction_specs_to_instructions(instructions)?;

    let mut tx = SolanaTransaction::new_with_payer(&solana_instructions, Some(payer));
    tx.message.recent_blockhash = recent_blockhash;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{
            NetworkTransactionData, NetworkType, SolanaAccountMeta, SolanaTransactionData,
            TransactionStatus,
        },
        utils::base64_encode,
    };
    use chrono::Utc;
    use solana_sdk::message::Message;
    use solana_system_interface::instruction as system_instruction;

    #[test]
    fn test_decode_solana_transaction_invalid_data() {
        // Create a transaction with invalid base64 data
        let tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Solana,
            network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                transaction: Some("invalid-base64!!!".to_string()),
                ..Default::default()
            }),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
        };

        let result = decode_solana_transaction(&tx);
        assert!(result.is_err());

        if let Err(TransactionError::ValidationError(msg)) = result {
            assert!(msg.contains("Invalid transaction"));
        } else {
            panic!("Expected ValidationError");
        }
    }

    #[test]
    fn test_decode_solana_transaction_not_built() {
        // Create a transaction that hasn't been built yet (transaction field is None)
        let tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Solana,
            network_data: NetworkTransactionData::Solana(SolanaTransactionData {
                transaction: None, // Not built yet
                ..Default::default()
            }),
            priced_at: None,
            hashes: Vec::new(),
            noop_count: None,
            is_canceled: Some(false),
        };

        let result = decode_solana_transaction(&tx);
        assert!(result.is_err());

        if let Err(TransactionError::ValidationError(msg)) = result {
            assert!(msg.contains("not yet built"));
        } else {
            panic!("Expected ValidationError");
        }
    }

    #[test]
    fn test_convert_instruction_specs_to_instructions_success() {
        let program_id = Pubkey::new_unique();
        let account = Pubkey::new_unique();

        let specs = vec![SolanaInstructionSpec {
            program_id: program_id.to_string(),
            accounts: vec![SolanaAccountMeta {
                pubkey: account.to_string(),
                is_signer: false,
                is_writable: true,
            }],
            data: base64_encode(b"test data"),
        }];

        let result = convert_instruction_specs_to_instructions(&specs);
        assert!(result.is_ok());

        let instructions = result.unwrap();
        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0].program_id, program_id);
        assert_eq!(instructions[0].accounts.len(), 1);
        assert_eq!(instructions[0].accounts[0].pubkey, account);
        assert!(!instructions[0].accounts[0].is_signer);
        assert!(instructions[0].accounts[0].is_writable);
    }

    #[test]
    fn test_build_transaction_from_instructions_success() {
        let payer = Pubkey::new_unique();
        let program_id = Pubkey::new_unique();
        let account = Pubkey::new_unique();
        let blockhash = Hash::new_unique();

        let instructions = vec![SolanaInstructionSpec {
            program_id: program_id.to_string(),
            accounts: vec![SolanaAccountMeta {
                pubkey: account.to_string(),
                is_signer: false,
                is_writable: true,
            }],
            data: base64_encode(b"test data"),
        }];

        let result = build_transaction_from_instructions(&instructions, &payer, blockhash);
        assert!(result.is_ok());

        let tx = result.unwrap();
        assert_eq!(tx.message.account_keys[0], payer);
        assert_eq!(tx.message.recent_blockhash, blockhash);
    }

    #[test]
    fn test_build_transaction_invalid_program_id() {
        let payer = Pubkey::new_unique();
        let blockhash = Hash::new_unique();

        let instructions = vec![SolanaInstructionSpec {
            program_id: "invalid".to_string(),
            accounts: vec![],
            data: base64_encode(b"test"),
        }];

        let result = build_transaction_from_instructions(&instructions, &payer, blockhash);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_transaction_invalid_base64_data() {
        let payer = Pubkey::new_unique();
        let program_id = Pubkey::new_unique();
        let blockhash = Hash::new_unique();

        let instructions = vec![SolanaInstructionSpec {
            program_id: program_id.to_string(),
            accounts: vec![],
            data: "not-valid-base64!!!".to_string(),
        }];

        let result = build_transaction_from_instructions(&instructions, &payer, blockhash);
        assert!(result.is_err());
    }

    #[test]
    fn test_is_resubmitable_single_signer() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer, &recipient, 1000);

        // Create transaction with single signer
        let tx = SolanaTransaction::new_with_payer(&[instruction], Some(&payer));

        // Single signer - should be able to update blockhash
        assert!(is_resubmitable(&tx));
        assert_eq!(tx.message.header.num_required_signatures, 1);
    }

    #[test]
    fn test_is_resubmitable_multi_signer() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let additional_signer = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer, &recipient, 1000);

        // Create transaction with multiple signers
        let mut message = Message::new(&[instruction], Some(&payer));
        // Add additional signer
        message.account_keys.push(additional_signer);
        message.header.num_required_signatures = 2;

        let tx = SolanaTransaction::new_unsigned(message);

        // Multi-signer - cannot update blockhash
        assert!(!is_resubmitable(&tx));
        assert_eq!(tx.message.header.num_required_signatures, 2);
    }

    #[test]
    fn test_is_resubmitable_no_signers() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer, &recipient, 1000);

        // Create transaction with no required signatures (edge case)
        let mut message = Message::new(&[instruction], Some(&payer));
        message.header.num_required_signatures = 0;

        let tx = SolanaTransaction::new_unsigned(message);

        // No signers (edge case) - should be able to update
        assert!(is_resubmitable(&tx));
        assert_eq!(tx.message.header.num_required_signatures, 0);
    }

    #[test]
    fn test_too_many_solana_attempts_under_limit() {
        let tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Solana,
            network_data: NetworkTransactionData::Solana(SolanaTransactionData::default()),
            priced_at: None,
            hashes: vec!["hash1".to_string(), "hash2".to_string()], // Less than limit
            noop_count: None,
            is_canceled: Some(false),
        };

        // Should not be too many attempts when under limit
        assert!(!too_many_solana_attempts(&tx));
    }

    #[test]
    fn test_too_many_solana_attempts_at_limit() {
        let tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Solana,
            network_data: NetworkTransactionData::Solana(SolanaTransactionData::default()),
            priced_at: None,
            hashes: vec!["hash".to_string(); MAXIMUM_SOLANA_TX_ATTEMPTS], // Exactly at limit
            noop_count: None,
            is_canceled: Some(false),
        };

        // Should be too many attempts when at limit
        assert!(too_many_solana_attempts(&tx));
    }

    #[test]
    fn test_too_many_solana_attempts_over_limit() {
        let tx = TransactionRepoModel {
            id: "test-tx".to_string(),
            relayer_id: "test-relayer".to_string(),
            status: TransactionStatus::Pending,
            status_reason: None,
            created_at: Utc::now().to_rfc3339(),
            sent_at: None,
            confirmed_at: None,
            valid_until: None,
            delete_at: None,
            network_type: NetworkType::Solana,
            network_data: NetworkTransactionData::Solana(SolanaTransactionData::default()),
            priced_at: None,
            hashes: vec!["hash".to_string(); MAXIMUM_SOLANA_TX_ATTEMPTS + 1], // Over limit
            noop_count: None,
            is_canceled: Some(false),
        };

        // Should be too many attempts when over limit
        assert!(too_many_solana_attempts(&tx));
    }

    #[test]
    fn test_map_solana_status_to_transaction_status_processed() {
        let result = map_solana_status_to_transaction_status(SolanaTransactionStatus::Processed);
        assert_eq!(result, TransactionStatus::Mined);
    }

    #[test]
    fn test_map_solana_status_to_transaction_status_confirmed() {
        let result = map_solana_status_to_transaction_status(SolanaTransactionStatus::Confirmed);
        assert_eq!(result, TransactionStatus::Mined);
    }

    #[test]
    fn test_map_solana_status_to_transaction_status_finalized() {
        let result = map_solana_status_to_transaction_status(SolanaTransactionStatus::Finalized);
        assert_eq!(result, TransactionStatus::Confirmed);
    }

    #[test]
    fn test_map_solana_status_to_transaction_status_failed() {
        let result = map_solana_status_to_transaction_status(SolanaTransactionStatus::Failed);
        assert_eq!(result, TransactionStatus::Failed);
    }
}
