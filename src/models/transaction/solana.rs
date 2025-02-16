// use std::{str::FromStr, sync::Arc};

// use base64::{engine::general_purpose::STANDARD, Engine};
// use solana_client::rpc_response::RpcSimulateTransactionResult;
// use solana_sdk::{
//     commitment_config::CommitmentConfig,
//     instruction::{AccountMeta, CompiledInstruction, Instruction},
//     pubkey::Pubkey,
//     transaction::Transaction,
// };

// use crate::{
//     models::{RelayerRepoModel, RelayerSolanaPolicy},
//     services::{SolanaProvider, SolanaProviderTrait},
// };

// #[derive(Debug, thiserror::Error)]
// #[allow(dead_code)]
// pub enum TransactionProcessorError {
//     #[error("Failed to decode transaction: {0}")]
//     DecodeError(String),
//     #[error("Failed to deserialize transaction: {0}")]
//     DeserializeError(String),
//     #[error("Validation error: {0}")]
//     ValidationError(String),
//     #[error("Signing error: {0}")]
//     SigningError(String),
//     #[error("Simulation error: {0}")]
//     SimulationError(String),
//     #[error("Policy violation: {0}")]
//     PolicyViolation(String),
//     #[error("Blockhash {0} is expired")]
//     ExpiredBlockhash(String),
// }

// #[allow(dead_code)]
// pub struct TransactionProcessor {
//     provider: Arc<SolanaProvider>,
// }

// #[allow(dead_code)]
// impl TransactionProcessor {
//     pub fn new(provider: Arc<SolanaProvider>) -> Self {
//         Self { provider }
//     }

//     pub fn decode_and_deserialize_tx(
//         &self,
//         tx_str: &str,
//     ) -> Result<Transaction, TransactionProcessorError> {
//         let tx_bytes = STANDARD
//             .decode(tx_str)
//             .map_err(|e| TransactionProcessorError::DecodeError(e.to_string()))?;

//         let transaction: Transaction = bincode::deserialize(&tx_bytes)
//             .map_err(|e| TransactionProcessorError::DeserializeError(e.to_string()))?;

//         Ok(transaction)
//     }

//     pub fn serialize_and_encode_tx(
//         &self,
//         tx: &Transaction,
//     ) -> Result<String, TransactionProcessorError> {
//         let tx_bytes = bincode::serialize(tx)
//             .map_err(|e| TransactionProcessorError::DeserializeError(e.to_string()))?;

//         let tx_str = STANDARD.encode(&tx_bytes);

//         Ok(tx_str)
//     }

//     fn decompile_instruction(
//         &self,
//         tx: &Transaction,
//         ix: &CompiledInstruction,
//         account_keys: &[Pubkey],
//     ) -> Instruction {
//         let program_id = account_keys[ix.program_id_index as usize];
//         let accounts = ix
//             .accounts
//             .iter()
//             .map(|&i| AccountMeta {
//                 pubkey: account_keys[i as usize],
//                 is_signer: tx.message.header.num_required_signatures > i,
//                 is_writable: tx.message.is_maybe_writable(i as usize, None),
//             })
//             .collect();

//         Instruction {
//             program_id,
//             accounts,
//             data: ix.data.clone(),
//         }
//     }

//     pub fn decompile_instructions(&self, tx: &Transaction) -> Vec<Instruction> {
//         tx.message
//             .instructions
//             .iter()
//             .map(|ix| self.decompile_instruction(tx, ix, &tx.message.account_keys))
//             .collect()
//     }

//     /// Validates transaction against policies
//     pub fn validate_transaction(
//         &self,
//         tx: &Transaction,
//         relayer: &RelayerRepoModel,
//     ) -> Result<(), TransactionProcessorError> {
//         let policy = &relayer.policies.get_solana_policy();

//         self.validate_allowed_accounts(tx, policy);
//         self.validate_allowed_programs(tx, policy);
//         self.validate_disallowed_accounts(tx, policy);
//         self.validate_num_signatures(tx, policy);
//         self.validate_fee_payer(tx, &relayer.address);
//         self.validate_blockhash(tx);
//         self.validate_data_size(tx, policy);
//         // self.validate_transaction_fee(tx, policy);
//         //

//         Ok(())
//     }

//     pub fn validate_fee_payer(
//         &self,
//         tx: &Transaction,
//         relayer_signer_address: &str,
//     ) -> Result<(), TransactionProcessorError> {
//         // Get fee payer (first account in account_keys)
//         let fee_payer = tx.message.account_keys.first().ok_or_else(|| {
//             TransactionProcessorError::ValidationError("No fee payer account found".to_string())
//         })?;

//         // Convert relayer address string to Pubkey
//         let relayer_pubkey = Pubkey::from_str(relayer_signer_address).map_err(|e| {
//             TransactionProcessorError::ValidationError(format!("Invalid relayer address: {}", e))
//         })?;

//         // Verify fee payer matches relayer address
//         if fee_payer != &relayer_pubkey {
//             return Err(TransactionProcessorError::PolicyViolation(format!(
//                 "Fee payer {} does not match relayer address {}",
//                 fee_payer, relayer_pubkey
//             )));
//         }

//         // Verify fee payer is a signer
//         if !tx.message.header.num_required_signatures > 0 {
//             return Err(TransactionProcessorError::ValidationError(
//                 "Fee payer must be a signer".to_string(),
//             ));
//         }

//         Ok(())
//     }

//     pub async fn validate_blockhash(
//         &self,
//         tx: &Transaction,
//     ) -> Result<(), TransactionProcessorError> {
//         let blockhash = tx.message.recent_blockhash;

//         // Check if blockhash is still valid
//         let is_valid = self
//             .provider
//             .is_blockhash_valid(&blockhash, CommitmentConfig::confirmed())
//             .await
//             .map_err(|e| {
//                 TransactionProcessorError::ValidationError(format!(
//                     "Failed to check blockhash validity: {}",
//                     e
//                 ))
//             })?;

//         if !is_valid {
//             return Err(TransactionProcessorError::ExpiredBlockhash(format!(
//                 "Blockhash {} is no longer valid",
//                 blockhash
//             )));
//         }

//         Ok(())
//     }

//     pub fn validate_num_signatures(
//         &self,
//         tx: &Transaction,
//         policy: &RelayerSolanaPolicy,
//     ) -> Result<(), TransactionProcessorError> {
//         let num_signatures = tx.message.header.num_required_signatures;

//         // Check against maximum allowed signatures
//         if let Some(max_signatures) = policy.max_signatures {
//             if num_signatures > max_signatures as u8 {
//                 return Err(TransactionProcessorError::PolicyViolation(format!(
//                     "Transaction requires {} signatures, which exceeds maximum allowed {}",
//                     num_signatures, max_signatures
//                 )));
//             }
//         }

//         Ok(())
//     }

//     pub fn validate_allowed_programs(
//         &self,
//         tx: &Transaction,
//         policy: &RelayerSolanaPolicy,
//     ) -> Result<(), TransactionProcessorError> {
//         if let Some(allowed_programs) = &policy.allowed_programs {
//             for program_id in tx
//                 .message
//                 .instructions
//                 .iter()
//                 .map(|ix| tx.message.account_keys[ix.program_id_index as usize])
//             {
//                 if !allowed_programs.contains(&program_id.to_string()) {
//                     return Err(TransactionProcessorError::PolicyViolation(format!(
//                         "Program {} not allowed",
//                         program_id
//                     )));
//                 }
//             }
//         }

//         Ok(())
//     }

//     pub fn validate_allowed_accounts(
//         &self,
//         tx: &Transaction,
//         policy: &RelayerSolanaPolicy,
//     ) -> Result<(), TransactionProcessorError> {
//         if let Some(allowed_accounts) = &policy.allowed_accounts {
//             for account_key in &tx.message.account_keys {
//                 if !allowed_accounts.contains(&account_key.to_string()) {
//                     return Err(TransactionProcessorError::PolicyViolation(format!(
//                         "Account {} not allowed",
//                         account_key
//                     )));
//                 }
//             }
//         }

//         Ok(())
//     }

//     pub fn validate_disallowed_accounts(
//         &self,
//         tx: &Transaction,
//         policy: &RelayerSolanaPolicy,
//     ) -> Result<(), TransactionProcessorError> {
//         if let Some(disallowed_accounts) = &policy.disallowed_accounts {
//             for account_key in &tx.message.account_keys {
//                 if disallowed_accounts.contains(&account_key.to_string()) {
//                     return Err(TransactionProcessorError::PolicyViolation(format!(
//                         "Account {} is explicitly disallowed",
//                         account_key
//                     )));
//                 }
//             }
//         }

//         Ok(())
//     }

//     pub fn validate_data_size(
//         &self,
//         tx: &Transaction,
//         config: &RelayerSolanaPolicy,
//     ) -> Result<(), TransactionProcessorError> {
//         let max_size: usize = config.max_tx_data_size.into();
//         let tx_bytes = bincode::serialize(tx)
//             .map_err(|e| TransactionProcessorError::DeserializeError(e.to_string()))?;

//         if tx_bytes.len() > max_size {
//             return Err(TransactionProcessorError::ValidationError(format!(
//                 "Transaction size {} exceeds maximum allowed {}",
//                 tx_bytes.len(),
//                 max_size
//             )));
//         }
//         Ok(())
//     }

//     // pub async fn validate_transaction_fee(
//     //     &self,
//     //     tx: &Transaction,
//     //     policy: &RelayerSolanaPolicy,
//     // ) -> Result<(), TransactionProcessorError> {
//     //     // Simulate transaction to get fee
//     //     let simulation_result = self.provider
//     //         .simulate_transaction(tx)
//     //         .await
//     //         .map_err(|e| TransactionProcessorError::ValidationError(
//     //             format!("Failed to simulate transaction: {}", e)
//     //         ))?;

//     //     let fee = simulation_result.value.units_consumed
//     //         .ok_or_else(|| TransactionProcessorError::ValidationError(
//     //             "Failed to get fee from simulation".to_string()
//     //         ))?;

//     //     // Check against maximum allowed fee
//     //     if let Some(max_fee) = policy.max_fee_in_lamports {
//     //         if fee > max_fee {
//     //             return Err(TransactionProcessorError::PolicyViolation(
//     //                 format!(
//     //                     "Transaction fee {} lamports exceeds maximum allowed {} lamports",
//     //                     fee, max_fee
//     //                 )
//     //             ));
//     //         }
//     //     }

//     //     // Check against minimum required fee
//     //     if let Some(min_fee) = policy.min_fee_in_lamports {
//     //         if fee < min_fee {
//     //             return Err(TransactionProcessorError::PolicyViolation(
//     //                 format!(
//     //                     "Transaction fee {} lamports is below minimum required {} lamports",
//     //                     fee, min_fee
//     //                 )
//     //             ));
//     //         }
//     //     }

//     //     Ok(())
//     // }

//     //   pub fn validate_token_transfers(&self, tx: &Transaction, policy: &RelayerSolanaPolicy) ->
//     // Result<(), TransactionProcessorError> {     // Check if we have token transfer
//     // restrictions     let allowed_tokens = match &policy.allowed_tokens {
//     //         Some(tokens) if !tokens.is_empty() => tokens,
//     //         _ => return Ok(()) // No token restrictions
//     //     };

//     //     for ix in &tx.message.instructions {
//     //         let program_id = tx.message.account_keys[ix.program_id_index as usize];

//     //         // Check if instruction is a token transfer (SPL Token Program)
//     //         if program_id == spl_token::id() {
//     //             // Decode token instruction
//     //             if let Ok(token_ix) = spl_token::instruction::TokenInstruction::unpack(&ix.data)
//     // {                 match token_ix {
//     //                     spl_token::instruction::TokenInstruction::Transfer { .. } |
//     //                     spl_token::instruction::TokenInstruction::TransferChecked { .. } => {
//     //                         // Get mint address from token account
//     //                         let token_account = &tx.message.account_keys[ix.accounts[0] as
//     // usize];                         let mint = self.get_token_mint(token_account)?;

//     //                         if !allowed_tokens.iter().any(|t| t.mint == mint.to_string()) {
//     //                             return Err(TransactionProcessorError::PolicyViolation(
//     //                                 format!("Token {} not allowed for transfers", mint)
//     //                             ));
//     //                         }
//     //                     },
//     //                     _ => continue // Not a transfer instruction
//     //                 }
//     //             }
//     //         }
//     //     }

//     //     Ok(())
//     // }

//     /// Simulates transaction
//     pub async fn simulate_transaction(
//         &self,
//         tx: &Transaction,
//     ) -> Result<RpcSimulateTransactionResult, TransactionProcessorError> {
//         self.provider
//             .simulate_transaction(tx)
//             .await
//             .map_err(|e| TransactionProcessorError::SimulationError(e.to_string()))
//     }
// }

// #[cfg(test)]
// mod tests {
//     use crate::services::get_solana_network_provider_from_str;

//     use super::*;
//     use solana_sdk::{
//         hash::Hash,
//         message::Message,
//         pubkey::Pubkey,
//         signature::{Keypair, Signer},
//         system_instruction, system_program,
//     };

//     #[test]
//     fn test_serialize_and_encode_tx() {
//         // Create a test transaction
//         let payer = Keypair::new();
//         let recipient = Pubkey::new_unique();
//         let instruction = system_instruction::transfer(
//             &payer.pubkey(),
//             &recipient,
//             1000, // lamports
//         );
//         let message = Message::new(&[instruction], Some(&payer.pubkey()));
//         let tx = Transaction::new(&[&payer], message, Hash::default());

//         // Call serialize_and_encode_tx
//         let tx_processor = TransactionProcessor::new(Arc::new(
//             get_solana_network_provider_from_str("mainnet-beta").unwrap(),
//         ));
//         let result = tx_processor.serialize_and_encode_tx(&tx);

//         // Verify the result
//         assert!(result.is_ok());
//         let encoded = result.unwrap();
//         assert!(!encoded.is_empty());

//         // Verify the encoded string is valid base64
//         assert!(STANDARD.decode(&encoded).is_ok());
//     }

//     #[test]
//     fn test_decode_and_deserialize() {
//         // Create test transaction
//         let payer = Keypair::new();
//         let recipient = Pubkey::new_unique();
//         let original_tx = Transaction::new(
//             &[&payer],
//             Message::new(
//                 &[system_instruction::transfer(
//                     &payer.pubkey(),
//                     &recipient,
//                     1000,
//                 )],
//                 Some(&payer.pubkey()),
//             ),
//             Hash::default(),
//         );

//         // Encode
//         let tx_processor = TransactionProcessor::new(Arc::new(
//             get_solana_network_provider_from_str("mainnet-beta").unwrap(),
//         ));
//         let encoded = tx_processor.serialize_and_encode_tx(&original_tx).unwrap();

//         // Decode and deserialize
//         let decoded_tx: Transaction = tx_processor.decode_and_deserialize_tx(&encoded).unwrap();

//         // Verify transaction fields match
//         assert_eq!(original_tx.signatures, decoded_tx.signatures);
//         assert_eq!(
//             original_tx.message.account_keys,
//             decoded_tx.message.account_keys
//         );
//         assert_eq!(
//             original_tx.message.instructions,
//             decoded_tx.message.instructions
//         );
//     }

//     #[test]
//     fn test_decompile_instructions() {
//         let tx_processor = TransactionProcessor::new(Arc::new(
//             get_solana_network_provider_from_str("mainnet-beta").unwrap(),
//         ));
//         // Create test transaction
//         let payer = Keypair::new();
//         let recipient = Pubkey::new_unique();
//         let instruction = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
//         let message = Message::new(&[instruction], Some(&payer.pubkey()));
//         let tx = Transaction::new_unsigned(message);

//         // Decompile instructions
//         let decompiled = tx_processor.decompile_instructions(&tx);

//         assert_eq!(decompiled.len(), 1);
//         let instruction_1 = &decompiled[0];

//         // Verify instruction contains expected components
//         assert_eq!(instruction_1.program_id, system_program::id());
//         assert_eq!(instruction_1.accounts.len(), 2);
//         assert_eq!(instruction_1.accounts[0].pubkey, payer.pubkey());
//         assert_eq!(instruction_1.accounts[1].pubkey, recipient);
//         assert_eq!(instruction_1.data.len(), 12);
//     }
// }
