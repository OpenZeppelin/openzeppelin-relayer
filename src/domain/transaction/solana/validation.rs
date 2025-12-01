use std::collections::HashMap;

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
    constants::{DEFAULT_SOLANA_MAX_TX_DATA_SIZE, DEFAULT_SOLANA_MIN_BALANCE},
    domain::{SolanaTokenProgram, TokenInstruction as SolanaTokenInstruction},
    models::RelayerSolanaPolicy,
    services::provider::{SolanaProviderError, SolanaProviderTrait},
};
use serde::Serialize;
use solana_client::rpc_response::RpcSimulateTransactionResult;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{pubkey::Pubkey, transaction::Transaction};
use solana_system_interface::{instruction::SystemInstruction, program};
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error, Serialize)]
#[allow(dead_code)]
pub enum SolanaTransactionValidationError {
    #[error("Failed to decode transaction: {0}")]
    DecodeError(String),
    #[error("Failed to deserialize transaction: {0}")]
    DeserializeError(String),
    #[error("Validation error: {0}")]
    SigningError(String),
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
    #[error("Blockhash {0} is expired")]
    ExpiredBlockhash(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
    #[error("Fee payer error: {0}")]
    FeePayer(String),
    #[error("Insufficient funds: {0}")]
    InsufficientFunds(String),
    #[error("Insufficient balance: {0}")]
    InsufficientBalance(String),
    #[error("Underlying Solana provider error: {0}")]
    UnderlyingSolanaProvider(#[from] SolanaProviderError),
}

impl SolanaTransactionValidationError {
    /// Determines if this validation error is transient (retriable) or permanent.
    ///
    /// Transient errors are typically RPC/network issues that may succeed on retry:
    /// - RPC connection errors
    /// - Network timeouts
    /// - Node behind errors
    ///
    /// Permanent errors are validation failures that won't change on retry:
    /// - Policy violations
    /// - Invalid transaction structure
    /// - Insufficient funds (actual balance issue, not RPC error)
    pub fn is_transient(&self) -> bool {
        match self {
            // Policy violations are always permanent
            Self::PolicyViolation(_) => false,

            // Fee payer mismatch is permanent
            Self::FeePayer(_) => false,

            // Decode/deserialize errors are permanent (invalid transaction)
            Self::DecodeError(_) | Self::DeserializeError(_) => false,

            // Expired blockhash is permanent (cannot be fixed by retry)
            Self::ExpiredBlockhash(_) => false,

            // Signing errors are permanent
            Self::SigningError(_) => false,

            Self::UnderlyingSolanaProvider(err) => err.is_transient(),

            // Generic validation errors - check message for transient patterns
            Self::ValidationError(msg) => {
                // Check for known transient error patterns in the message
                msg.contains("RPC")
                    || msg.contains("timeout")
                    || msg.contains("timed out")
                    || msg.contains("connection")
                    || msg.contains("network")
                    || msg.contains("Failed to check")
                    || msg.contains("Failed to get")
                    || msg.contains("node behind")
                    || msg.contains("rate limit")
            }

            // Balance errors - check if it's an RPC error or actual insufficient balance
            Self::InsufficientBalance(msg) | Self::InsufficientFunds(msg) => {
                // If the message indicates an RPC failure, it's transient
                msg.contains("Failed to get balance")
                    || msg.contains("RPC")
                    || msg.contains("timeout")
                    || msg.contains("network")
            }
        }
    }
}

#[allow(dead_code)]
pub struct SolanaTransactionValidator {}

#[allow(dead_code)]
impl SolanaTransactionValidator {
    pub fn validate_allowed_token(
        token_mint: &str,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        // Check if allowed tokens are configured
        let no_tokens_configured = match &policy.allowed_tokens {
            None => true,                      // No tokens configured
            Some(tokens) => tokens.is_empty(), // Tokens configured but empty
        };

        // If no allowed tokens are configured or empty, allow all tokens
        if no_tokens_configured {
            return Ok(());
        }

        // If allowed tokens are configured, check if the token is in the list
        let allowed_token = policy.get_allowed_token_entry(token_mint);
        if allowed_token.is_none() {
            return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                "Token {token_mint} not allowed for transfers"
            )));
        }

        Ok(())
    }

    /// Validates that the transaction's fee payer matches the relayer's address.
    pub fn validate_fee_payer(
        tx: &Transaction,
        relayer_pubkey: &Pubkey,
    ) -> Result<(), SolanaTransactionValidationError> {
        // Get fee payer (first account in account_keys)
        let fee_payer = tx.message.account_keys.first().ok_or_else(|| {
            SolanaTransactionValidationError::FeePayer("No fee payer account found".to_string())
        })?;

        // Verify fee payer matches relayer address
        if fee_payer != relayer_pubkey {
            return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                "Fee payer {fee_payer} does not match relayer address {relayer_pubkey}"
            )));
        }

        // Verify fee payer is a signer
        if tx.message.header.num_required_signatures < 1 {
            return Err(SolanaTransactionValidationError::FeePayer(
                "Fee payer must be a signer".to_string(),
            ));
        }

        Ok(())
    }

    /// Validates that the transaction's blockhash is still valid.
    ///
    /// Checks if the provided blockhash is still valid on-chain. If the blockhash has expired,
    /// the transaction will fail when submitted.
    ///
    /// **Note**: For single-signer transactions, expired blockhashes can be refreshed during
    /// resubmission. However, validation still occurs to provide early feedback.
    /// For multi-signer transactions, expired blockhashes cannot be refreshed without
    /// invalidating existing signatures, so validation is critical.
    pub async fn validate_blockhash<T: SolanaProviderTrait>(
        tx: &Transaction,
        provider: &T,
    ) -> Result<(), SolanaTransactionValidationError> {
        let blockhash = tx.message.recent_blockhash;

        // Check if blockhash is still valid
        let is_valid = provider
            .is_blockhash_valid(&blockhash, CommitmentConfig::confirmed())
            .await?;

        if !is_valid {
            return Err(SolanaTransactionValidationError::ExpiredBlockhash(format!(
                "Blockhash {blockhash} is no longer valid"
            )));
        }

        Ok(())
    }

    /// Validates the number of required signatures against policy limits.
    pub fn validate_max_signatures(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        let num_signatures = tx.message.header.num_required_signatures;

        let Some(max_signatures) = policy.max_signatures else {
            return Ok(());
        };

        if num_signatures > max_signatures {
            return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                "Transaction requires {num_signatures} signatures, which exceeds maximum allowed {max_signatures}"
            )));
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
                        "Program {program_id} not allowed"
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn validate_allowed_account(
        account: &str,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(allowed_accounts) = &policy.allowed_accounts {
            if !allowed_accounts.contains(&account.to_string()) {
                return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                    "Account {account} not allowed"
                )));
            }
        }

        Ok(())
    }

    /// Validates that the transaction's accounts are allowed by the relayer's policy.
    pub fn validate_tx_allowed_accounts(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(allowed_accounts) = &policy.allowed_accounts {
            for account_key in &tx.message.account_keys {
                info!(account_key = %account_key, "checking account");
                if !allowed_accounts.contains(&account_key.to_string()) {
                    return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                        "Account {account_key} not allowed"
                    )));
                }
            }
        }

        Ok(())
    }

    pub fn validate_disallowed_account(
        account: &str,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(disallowed_accounts) = &policy.disallowed_accounts {
            if disallowed_accounts.contains(&account.to_string()) {
                return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                    "Account {account} not allowed"
                )));
            }
        }

        Ok(())
    }

    /// Validates that the transaction's accounts are not disallowed by the relayer's policy.
    pub fn validate_tx_disallowed_accounts(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        let Some(disallowed_accounts) = &policy.disallowed_accounts else {
            return Ok(());
        };

        for account_key in &tx.message.account_keys {
            if disallowed_accounts.contains(&account_key.to_string()) {
                return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                    "Account {account_key} is explicitly disallowed"
                )));
            }
        }

        Ok(())
    }

    /// Validates that the transaction's data size is within policy limits.
    pub fn validate_data_size(
        tx: &Transaction,
        config: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        let max_size: usize = config
            .max_tx_data_size
            .unwrap_or(DEFAULT_SOLANA_MAX_TX_DATA_SIZE)
            .into();
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

    /// Validates that the relayer is not used as source in lamports transfers.
    pub async fn validate_lamports_transfers(
        tx: &Transaction,
        relayer_account: &Pubkey,
    ) -> Result<(), SolanaTransactionValidationError> {
        // Iterate over each instruction in the transaction
        for (ix_index, ix) in tx.message.instructions.iter().enumerate() {
            let program_id = tx.message.account_keys[ix.program_id_index as usize];

            // Check if the instruction comes from the System Program (native SOL transfers)
            #[allow(clippy::collapsible_match)]
            if program_id == program::id() {
                if let Ok(system_ix) = bincode::deserialize::<SystemInstruction>(&ix.data) {
                    if let SystemInstruction::Transfer { .. } = system_ix {
                        // In a system transfer instruction, the first account is the source and the
                        // second is the destination.
                        let source_index = ix.accounts.first().ok_or_else(|| {
                            SolanaTransactionValidationError::ValidationError(format!(
                                "Missing source account in instruction {ix_index}"
                            ))
                        })?;
                        let source_pubkey = &tx.message.account_keys[*source_index as usize];

                        // Only validate transfers where the source is the relayer fee account.
                        if source_pubkey == relayer_account {
                            return Err(SolanaTransactionValidationError::PolicyViolation(
                                "Lamports transfers are not allowed from the relayer account"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Validates transfer amount against policy limits.
    pub fn validate_max_fee(
        amount: u64,
        policy: &RelayerSolanaPolicy,
    ) -> Result<(), SolanaTransactionValidationError> {
        if let Some(max_amount) = policy.max_allowed_fee_lamports {
            if amount > max_amount {
                return Err(SolanaTransactionValidationError::PolicyViolation(format!(
                    "Fee amount {amount} exceeds max allowed fee amount {max_amount}"
                )));
            }
        }

        Ok(())
    }

    /// Validates transfer amount against policy limits.
    pub async fn validate_sufficient_relayer_balance(
        fee: u64,
        relayer_address: &str,
        policy: &RelayerSolanaPolicy,
        provider: &impl SolanaProviderTrait,
    ) -> Result<(), SolanaTransactionValidationError> {
        let balance = provider.get_balance(relayer_address).await?;
        // Ensure minimum balance policy is maintained
        let min_balance = policy.min_balance.unwrap_or(DEFAULT_SOLANA_MIN_BALANCE);
        let required_balance = fee + min_balance;

        if balance < required_balance {
            return Err(SolanaTransactionValidationError::InsufficientBalance(format!(
                "Insufficient relayer balance. Required: {required_balance}, Available: {balance}, Fee: {fee}, Min balance: {min_balance}"
            )));
        }

        Ok(())
    }

    /// Validates token transfers against policy restrictions.
    pub async fn validate_token_transfers(
        tx: &Transaction,
        policy: &RelayerSolanaPolicy,
        provider: &impl SolanaProviderTrait,
        relayer_account: &Pubkey,
    ) -> Result<(), SolanaTransactionValidationError> {
        let allowed_tokens = match &policy.allowed_tokens {
            Some(tokens) if !tokens.is_empty() => tokens,
            _ => return Ok(()), // No token restrictions
        };

        // Track cumulative transfers from each source account
        let mut account_transfers: HashMap<Pubkey, u64> = HashMap::new();
        let mut account_balances: HashMap<Pubkey, u64> = HashMap::new();

        for ix in &tx.message.instructions {
            let program_id = tx.message.account_keys[ix.program_id_index as usize];

            if !SolanaTokenProgram::is_token_program(&program_id) {
                continue;
            }

            let token_ix = match SolanaTokenProgram::unpack_instruction(&program_id, &ix.data) {
                Ok(ix) => ix,
                Err(_) => continue, // Skip instructions we can't decode
            };

            // Decode token instruction
            match token_ix {
                SolanaTokenInstruction::Transfer { amount }
                | SolanaTokenInstruction::TransferChecked { amount, .. } => {
                    // Get source account info
                    let source_index = ix.accounts[0] as usize;
                    let source_pubkey = &tx.message.account_keys[source_index];

                    // Validate source account is writable but not signer
                    if !tx.message.is_maybe_writable(source_index, None) {
                        return Err(SolanaTransactionValidationError::ValidationError(
                            "Source account must be writable".to_string(),
                        ));
                    }
                    if tx.message.is_signer(source_index) {
                        return Err(SolanaTransactionValidationError::ValidationError(
                            "Source account must not be signer".to_string(),
                        ));
                    }

                    if source_pubkey == relayer_account {
                        return Err(SolanaTransactionValidationError::PolicyViolation(
                            "Relayer account cannot be source".to_string(),
                        ));
                    }

                    let dest_index = match token_ix {
                        SolanaTokenInstruction::TransferChecked { .. } => ix.accounts[2] as usize,
                        _ => ix.accounts[1] as usize,
                    };
                    let destination_pubkey = &tx.message.account_keys[dest_index];

                    // Validate destination account is writable but not signer
                    if !tx.message.is_maybe_writable(dest_index, None) {
                        return Err(SolanaTransactionValidationError::ValidationError(
                            "Destination account must be writable".to_string(),
                        ));
                    }
                    if tx.message.is_signer(dest_index) {
                        return Err(SolanaTransactionValidationError::ValidationError(
                            "Destination account must not be signer".to_string(),
                        ));
                    }

                    let owner_index = match token_ix {
                        SolanaTokenInstruction::TransferChecked { .. } => ix.accounts[3] as usize,
                        _ => ix.accounts[2] as usize,
                    };
                    // Validate owner is signer but not writable
                    if !tx.message.is_signer(owner_index) {
                        return Err(SolanaTransactionValidationError::ValidationError(format!(
                            "Owner must be signer {}",
                            &tx.message.account_keys[owner_index]
                        )));
                    }

                    // Get mint address from token account - only once per source account
                    if !account_balances.contains_key(source_pubkey) {
                        let source_account = provider
                            .get_account_from_pubkey(source_pubkey)
                            .await
                            .map_err(|e| {
                                SolanaTransactionValidationError::ValidationError(e.to_string())
                            })?;

                        let token_account =
                            SolanaTokenProgram::unpack_account(&program_id, &source_account)
                                .map_err(|e| {
                                    SolanaTransactionValidationError::ValidationError(format!(
                                        "Invalid token account: {e}"
                                    ))
                                })?;

                        if token_account.is_frozen {
                            return Err(SolanaTransactionValidationError::PolicyViolation(
                                "Token account is frozen".to_string(),
                            ));
                        }

                        let token_config = allowed_tokens
                            .iter()
                            .find(|t| t.mint == token_account.mint.to_string());

                        // check if token is allowed by policy
                        if token_config.is_none() {
                            return Err(SolanaTransactionValidationError::PolicyViolation(
                                format!("Token {} not allowed for transfers", token_account.mint),
                            ));
                        }
                        // Store the balance for later use
                        account_balances.insert(*source_pubkey, token_account.amount);

                        // Validate decimals for TransferChecked
                        if let (
                            Some(config),
                            SolanaTokenInstruction::TransferChecked { decimals, .. },
                        ) = (token_config, &token_ix)
                        {
                            if Some(*decimals) != config.decimals {
                                return Err(SolanaTransactionValidationError::ValidationError(
                                    format!(
                                        "Invalid decimals: expected {:?}, got {}",
                                        config.decimals, decimals
                                    ),
                                ));
                            }
                        }

                        // if relayer is destination, check max fee
                        if destination_pubkey == relayer_account {
                            // Check max fee if configured
                            if let Some(config) = token_config {
                                if let Some(max_fee) = config.max_allowed_fee {
                                    if amount > max_fee {
                                        return Err(
                                            SolanaTransactionValidationError::PolicyViolation(
                                                format!(
                                                    "Transfer amount {} exceeds max fee \
                                                    allowed {} for token {}",
                                                    amount, max_fee, token_account.mint
                                                ),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                    }

                    *account_transfers.entry(*source_pubkey).or_insert(0) += amount;
                }
                _ => {
                    // For any other token instruction, verify relayer account is not used
                    // as a source by checking if it's marked as writable
                    for account in ix.accounts.iter() {
                        let account_index = *account as usize;
                        if account_index < tx.message.account_keys.len() {
                            let pubkey = &tx.message.account_keys[account_index];
                            if pubkey == relayer_account
                                && tx.message.is_maybe_writable(account_index, None)
                                && !tx.message.is_signer(account_index)
                            {
                                // It's ok if relayer is just signing
                                return Err(SolanaTransactionValidationError::PolicyViolation(
                                            "Relayer account cannot be used as writable account in token instructions".to_string(),
                                        ));
                            }
                        }
                    }
                }
            }
        }

        // validate that cumulative transfers don't exceed balances
        for (account, total_transfer) in account_transfers {
            let balance = *account_balances.get(&account).unwrap();

            if balance < total_transfer {
                return Err(SolanaTransactionValidationError::ValidationError(
                    format!(
                        "Insufficient balance for cumulative transfers: account {account} has balance {balance} but requires {total_transfer} across all instructions"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Simulates transaction
    pub async fn simulate_transaction<T: SolanaProviderTrait>(
        tx: &Transaction,
        provider: &T,
    ) -> Result<RpcSimulateTransactionResult, SolanaTransactionValidationError> {
        let new_tx = Transaction::new_unsigned(tx.message.clone());

        let result = provider.simulate_transaction(&new_tx).await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        models::{relayer::SolanaAllowedTokensSwapConfig, SolanaAllowedTokensPolicy},
        services::provider::{MockSolanaProviderTrait, SolanaProviderError},
    };

    use super::*;
    use mockall::predicate::*;
    use solana_sdk::{
        instruction::{AccountMeta, Instruction},
        message::Message,
        program_pack::Pack,
        signature::{Keypair, Signer},
    };
    use solana_system_interface::{instruction, program};
    use spl_token_interface::{instruction as token_instruction, state::Account};

    fn setup_token_transfer_test(
        transfer_amount: Option<u64>,
    ) -> (
        Transaction,
        RelayerSolanaPolicy,
        MockSolanaProviderTrait,
        Keypair, // source owner
        Pubkey,  // token mint
        Pubkey,  // source token account
        Pubkey,  // destination token account
    ) {
        let owner = Keypair::new();
        let mint = Pubkey::new_unique();
        let source = Pubkey::new_unique();
        let destination = Pubkey::new_unique();

        // Create token transfer instruction
        let transfer_ix = token_instruction::transfer(
            &spl_token_interface::id(),
            &source,
            &destination,
            &owner.pubkey(),
            &[],
            transfer_amount.unwrap_or(100),
        )
        .unwrap();

        let message = Message::new(&[transfer_ix], Some(&owner.pubkey()));
        let mut transaction = Transaction::new_unsigned(message);

        // Ensure owner is marked as signer but not writable
        if let Some(owner_index) = transaction
            .message
            .account_keys
            .iter()
            .position(|&pubkey| pubkey == owner.pubkey())
        {
            transaction.message.header.num_required_signatures = (owner_index + 1) as u8;
            transaction.message.header.num_readonly_signed_accounts = 1;
        }

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: mint.to_string(),
                decimals: Some(9),
                symbol: Some("USDC".to_string()),
                max_allowed_fee: Some(100),
                swap_config: Some(SolanaAllowedTokensSwapConfig {
                    ..Default::default()
                }),
            }]),
            ..Default::default()
        };

        let mut mock_provider = MockSolanaProviderTrait::new();

        // Setup default mock responses
        let token_account = Account {
            mint,
            owner: owner.pubkey(),
            amount: 999,
            state: spl_token_interface::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        mock_provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let local_account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1000000,
                        data: local_account_data,
                        owner: spl_token_interface::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        (
            transaction,
            policy,
            mock_provider,
            owner,
            mint,
            source,
            destination,
        )
    }

    fn create_test_transaction(fee_payer: &Pubkey) -> Transaction {
        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(fee_payer, &recipient, 1000);
        let message = Message::new(&[instruction], Some(fee_payer));
        Transaction::new_unsigned(message)
    }

    fn create_multi_signer_test_transaction(
        fee_payer: &Pubkey,
        additional_signer: &Pubkey,
    ) -> Transaction {
        let recipient = Pubkey::new_unique();
        let instruction = instruction::transfer(fee_payer, &recipient, 1000);
        // Create message with 2 required signatures
        let mut message = Message::new(&[instruction], Some(fee_payer));
        // Add second signer to account keys
        if !message.account_keys.contains(additional_signer) {
            message.account_keys.push(*additional_signer);
        }
        // Set num_required_signatures to 2
        message.header.num_required_signatures = 2;
        Transaction::new_unsigned(message)
    }

    #[test]
    fn test_validate_fee_payer_success() {
        let relayer_keypair = Keypair::new();
        let relayer_address = relayer_keypair.pubkey();
        let tx = create_test_transaction(&relayer_address);

        let result = SolanaTransactionValidator::validate_fee_payer(&tx, &relayer_address);

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_fee_payer_mismatch() {
        let wrong_keypair = Keypair::new();
        let relayer_address = Keypair::new().pubkey();

        let tx = create_test_transaction(&wrong_keypair.pubkey());

        let result = SolanaTransactionValidator::validate_fee_payer(&tx, &relayer_address);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[tokio::test]
    async fn test_validate_blockhash_valid() {
        // Use multi-signer transaction so blockhash validation actually runs
        let fee_payer = Keypair::new().pubkey();
        let additional_signer = Keypair::new().pubkey();
        let transaction = create_multi_signer_test_transaction(&fee_payer, &additional_signer);
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider
            .expect_is_blockhash_valid()
            .with(
                eq(transaction.message.recent_blockhash),
                eq(CommitmentConfig::confirmed()),
            )
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let result =
            SolanaTransactionValidator::validate_blockhash(&transaction, &mock_provider).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_blockhash_expired() {
        // Use multi-signer transaction so blockhash validation actually runs
        let fee_payer = Keypair::new().pubkey();
        let additional_signer = Keypair::new().pubkey();
        let transaction = create_multi_signer_test_transaction(&fee_payer, &additional_signer);
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let result =
            SolanaTransactionValidator::validate_blockhash(&transaction, &mock_provider).await;

        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::ExpiredBlockhash(_)
        ));
    }

    #[tokio::test]
    async fn test_validate_blockhash_provider_error() {
        // Use multi-signer transaction so blockhash validation actually runs
        let fee_payer = Keypair::new().pubkey();
        let additional_signer = Keypair::new().pubkey();
        let transaction = create_multi_signer_test_transaction(&fee_payer, &additional_signer);
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider.expect_is_blockhash_valid().returning(|_, _| {
            Box::pin(async { Err(SolanaProviderError::RpcError("RPC error".to_string())) })
        });

        let result =
            SolanaTransactionValidator::validate_blockhash(&transaction, &mock_provider).await;

        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::UnderlyingSolanaProvider(_)
        ));
    }

    #[tokio::test]
    async fn test_validate_blockhash_validates_single_signer() {
        // Single-signer transactions are now validated (no longer skipped)
        // This provides early feedback even though blockhash can be refreshed during resubmit
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let mut mock_provider = MockSolanaProviderTrait::new();

        // Expect provider call for blockhash validation
        mock_provider
            .expect_is_blockhash_valid()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let result =
            SolanaTransactionValidator::validate_blockhash(&transaction, &mock_provider).await;

        // Should succeed after validation
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_max_signatures_within_limit() {
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let policy = RelayerSolanaPolicy {
            max_signatures: Some(2),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_max_signatures(&transaction, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_max_signatures_exceeds_limit() {
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let policy = RelayerSolanaPolicy {
            max_signatures: Some(0),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_max_signatures(&transaction, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_max_signatures_no_limit() {
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let policy = RelayerSolanaPolicy {
            max_signatures: None,
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_max_signatures(&transaction, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_max_signatures_exact_limit() {
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let policy = RelayerSolanaPolicy {
            max_signatures: Some(1),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_max_signatures(&transaction, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_allowed_programs_success() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());
        let policy = RelayerSolanaPolicy {
            allowed_programs: Some(vec![program::id().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_programs(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_allowed_programs_disallowed() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            allowed_programs: Some(vec![Pubkey::new_unique().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_programs(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_allowed_programs_no_restrictions() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            allowed_programs: None,
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_programs(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_allowed_programs_multiple_instructions() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();

        let ix1 = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let ix2 = instruction::transfer(&payer.pubkey(), &recipient, 2000);
        let message = Message::new(&[ix1, ix2], Some(&payer.pubkey()));
        let tx = Transaction::new_unsigned(message);

        let policy = RelayerSolanaPolicy {
            allowed_programs: Some(vec![program::id().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_programs(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tx_allowed_accounts_success() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();

        let ix = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let tx = Transaction::new_unsigned(message);

        let policy = RelayerSolanaPolicy {
            allowed_accounts: Some(vec![
                payer.pubkey().to_string(),
                recipient.to_string(),
                program::id().to_string(),
            ]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_allowed_accounts(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tx_allowed_accounts_disallowed() {
        let payer = Keypair::new();

        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            allowed_accounts: Some(vec![payer.pubkey().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_allowed_accounts(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_tx_allowed_accounts_no_restrictions() {
        let tx = create_test_transaction(&Keypair::new().pubkey());

        let policy = RelayerSolanaPolicy {
            allowed_accounts: None,
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_allowed_accounts(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tx_allowed_accounts_system_program() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            allowed_accounts: Some(vec![payer.pubkey().to_string(), program::id().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_allowed_accounts(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_tx_disallowed_accounts_success() {
        let payer = Keypair::new();

        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![Pubkey::new_unique().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_disallowed_accounts(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tx_disallowed_accounts_blocked() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();

        let ix = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let tx = Transaction::new_unsigned(message);

        let policy = RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![recipient.to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_disallowed_accounts(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_tx_disallowed_accounts_no_restrictions() {
        let tx = create_test_transaction(&Keypair::new().pubkey());

        let policy = RelayerSolanaPolicy {
            disallowed_accounts: None,
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_disallowed_accounts(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tx_disallowed_accounts_system_program() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![program::id().to_string()]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_tx_disallowed_accounts(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_data_size_within_limit() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            max_tx_data_size: Some(1500),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_data_size(&tx, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_data_size_exceeds_limit() {
        let payer = Keypair::new();
        let tx = create_test_transaction(&payer.pubkey());

        let policy = RelayerSolanaPolicy {
            max_tx_data_size: Some(10),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_data_size(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_data_size_large_instruction() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();

        let large_data = vec![0u8; 1000];
        let ix = Instruction::new_with_bytes(
            program::id(),
            &large_data,
            vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new(recipient, false),
            ],
        );

        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let tx = Transaction::new_unsigned(message);

        let policy = RelayerSolanaPolicy {
            max_tx_data_size: Some(500),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_data_size(&tx, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_data_size_multiple_instructions() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();

        let ix1 = instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let ix2 = instruction::transfer(&payer.pubkey(), &recipient, 2000);
        let message = Message::new(&[ix1, ix2], Some(&payer.pubkey()));
        let tx = Transaction::new_unsigned(message);

        let policy = RelayerSolanaPolicy {
            max_tx_data_size: Some(1500),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_data_size(&tx, &policy);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_simulate_transaction_success() {
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider
            .expect_simulate_transaction()
            .with(eq(transaction.clone()))
            .returning(move |_| {
                let simulation_result = RpcSimulateTransactionResult {
                    err: None,
                    logs: Some(vec!["Program log: success".to_string()]),
                    accounts: None,
                    units_consumed: Some(100000),
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
                    loaded_accounts_data_size: None,
                    fee: None,
                    pre_balances: None,
                    post_balances: None,
                    pre_token_balances: None,
                    post_token_balances: None,
                    loaded_addresses: None,
                };
                Box::pin(async { Ok(simulation_result) })
            });

        let result =
            SolanaTransactionValidator::simulate_transaction(&transaction, &mock_provider).await;

        assert!(result.is_ok());
        let simulation = result.unwrap();
        assert!(simulation.err.is_none());
        assert_eq!(simulation.units_consumed, Some(100000));
    }

    #[tokio::test]
    async fn test_simulate_transaction_failure() {
        let transaction = create_test_transaction(&Keypair::new().pubkey());
        let mut mock_provider = MockSolanaProviderTrait::new();

        mock_provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Err(SolanaProviderError::RpcError(
                    "Simulation failed".to_string(),
                ))
            })
        });

        let result =
            SolanaTransactionValidator::simulate_transaction(&transaction, &mock_provider).await;

        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::UnderlyingSolanaProvider(_)
        ));
    }

    #[tokio::test]
    async fn test_validate_token_transfers_success() {
        let (tx, policy, provider, ..) = setup_token_transfer_test(Some(100));

        let result = SolanaTransactionValidator::validate_token_transfers(
            &tx,
            &policy,
            &provider,
            &Pubkey::new_unique(),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_token_transfers_insufficient_balance() {
        let (tx, policy, provider, ..) = setup_token_transfer_test(Some(2000));

        let result = SolanaTransactionValidator::validate_token_transfers(
            &tx,
            &policy,
            &provider,
            &Pubkey::new_unique(),
        )
        .await;

        match result {
            Err(SolanaTransactionValidationError::ValidationError(msg)) => {
                assert!(
                    msg.contains("Insufficient balance for cumulative transfers: account "),
                    "Unexpected error message: {}",
                    msg
                );
                assert!(
                    msg.contains("has balance 999 but requires 2000 across all instructions"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            other => panic!(
                "Expected ValidationError for insufficient balance, got {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_validate_token_transfers_relayer_max_fee() {
        let (tx, policy, provider, _owner, _mint, _source, destination) =
            setup_token_transfer_test(Some(500));

        let result = SolanaTransactionValidator::validate_token_transfers(
            &tx,
            &policy,
            &provider,
            &destination,
        )
        .await;

        match result {
            Err(SolanaTransactionValidationError::PolicyViolation(msg)) => {
                assert!(
                    msg.contains("Transfer amount 500 exceeds max fee allowed 100"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            other => panic!(
                "Expected ValidationError for insufficient balance, got {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_validate_token_transfers_relayer_max_fee_not_applied_for_secondary_accounts() {
        let (tx, policy, provider, ..) = setup_token_transfer_test(Some(500));

        let result = SolanaTransactionValidator::validate_token_transfers(
            &tx,
            &policy,
            &provider,
            &Pubkey::new_unique(),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_token_transfers_disallowed_token() {
        let (tx, mut policy, provider, ..) = setup_token_transfer_test(Some(100));

        policy.allowed_tokens = Some(vec![SolanaAllowedTokensPolicy {
            mint: Pubkey::new_unique().to_string(), // Different mint
            decimals: Some(9),
            symbol: Some("USDT".to_string()),
            max_allowed_fee: None,
            swap_config: Some(SolanaAllowedTokensSwapConfig {
                ..Default::default()
            }),
        }]);

        let result = SolanaTransactionValidator::validate_token_transfers(
            &tx,
            &policy,
            &provider,
            &Pubkey::new_unique(),
        )
        .await;

        match result {
            Err(SolanaTransactionValidationError::PolicyViolation(msg)) => {
                assert!(
                    msg.contains("not allowed for transfers"),
                    "Error message '{}' should contain 'not allowed for transfers'",
                    msg
                );
            }
            other => panic!("Expected PolicyViolation error, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_allowed_token_no_tokens_configured() {
        let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC mint

        let policy = RelayerSolanaPolicy {
            allowed_tokens: None, // No tokens configured
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_token(token_mint, &policy);

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_allowed_token_empty_tokens_list() {
        let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC mint

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![]), // Empty tokens list
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_token(token_mint, &policy);

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_allowed_token_success() {
        let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC mint

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![
                SolanaAllowedTokensPolicy {
                    mint: token_mint.to_string(),
                    decimals: Some(6),
                    symbol: Some("USDC".to_string()),
                    max_allowed_fee: Some(1000),
                    swap_config: None,
                },
                SolanaAllowedTokensPolicy {
                    mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), // USDT mint
                    decimals: Some(6),
                    symbol: Some("USDT".to_string()),
                    max_allowed_fee: Some(2000),
                    swap_config: None,
                },
            ]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_token(token_mint, &policy);

        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_allowed_token_not_allowed() {
        let token_mint = "11111111111111111111111111111112"; // System Program (not a valid token mint)

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![
                SolanaAllowedTokensPolicy {
                    mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // USDC mint
                    decimals: Some(6),
                    symbol: Some("USDC".to_string()),
                    max_allowed_fee: Some(1000),
                    swap_config: None,
                },
                SolanaAllowedTokensPolicy {
                    mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), // USDT mint
                    decimals: Some(6),
                    symbol: Some("USDT".to_string()),
                    max_allowed_fee: Some(2000),
                    swap_config: None,
                },
            ]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_token(token_mint, &policy);

        match result {
            Err(SolanaTransactionValidationError::PolicyViolation(msg)) => {
                assert_eq!(
                    msg,
                    format!("Token {} not allowed for transfers", token_mint),
                    "Error message should match expected format"
                );
            }
            other => panic!("Expected PolicyViolation error, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_allowed_token_case_sensitive() {
        let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC mint
        let uppercase_mint = token_mint.to_uppercase();

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: token_mint.to_string(), // lowercase version
                decimals: Some(6),
                symbol: Some("USDC".to_string()),
                max_allowed_fee: Some(1000),
                swap_config: None,
            }]),
            ..Default::default()
        };

        // Test with exact case - should succeed
        let result = SolanaTransactionValidator::validate_allowed_token(token_mint, &policy);
        assert!(result.is_ok());

        // Test with different case - should fail (case sensitive)
        let result = SolanaTransactionValidator::validate_allowed_token(&uppercase_mint, &policy);
        assert!(matches!(
            result.unwrap_err(),
            SolanaTransactionValidationError::PolicyViolation(_)
        ));
    }

    #[test]
    fn test_validate_allowed_token_with_minimal_config() {
        let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC mint

        let policy = RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: token_mint.to_string(),
                decimals: None,
                symbol: None,
                max_allowed_fee: None,
                swap_config: None,
            }]),
            ..Default::default()
        };

        let result = SolanaTransactionValidator::validate_allowed_token(token_mint, &policy);

        assert!(result.is_ok());
    }
}
