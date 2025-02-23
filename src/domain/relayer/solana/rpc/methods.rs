//! # Solana RPC Methods Module
//!
//! This module defines the `SolanaRpcMethods` trait which provides an asynchronous interface
//! for various Solana-specific RPC operations. These operations include fee estimation,
//! transaction processing (transfer, prepare, sign, and send), token retrieval, and feature
//! queries.
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    hash::Hash,
    instruction::Instruction,
    message::Message,
    program_pack::Pack,
    pubkey::Pubkey,
    signature::Signature,
    system_instruction::{self, SystemInstruction},
    system_program,
    transaction::Transaction,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::{amount_to_ui_amount, state::Account};
use std::{str::FromStr, sync::Arc};

use super::{SolanaRpcError, SolanaTransactionValidator};

use crate::{
    constants::{DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, SOLANA_DECIMALS, SOL_MINT},
    models::{
        EncodedSerializedTransaction, FeeEstimateRequestParams, FeeEstimateResult,
        GetFeaturesEnabledRequestParams, GetFeaturesEnabledResult, GetSupportedTokensItem,
        GetSupportedTokensRequestParams, GetSupportedTokensResult, PrepareTransactionRequestParams,
        PrepareTransactionResult, RelayerRepoModel, SignAndSendTransactionRequestParams,
        SignAndSendTransactionResult, SignTransactionRequestParams, SignTransactionResult,
        TransferTransactionRequestParams, TransferTransactionResult,
    },
    services::{
        JupiterService, JupiterServiceTrait, SolanaProvider, SolanaProviderTrait, SolanaSignTrait,
        SolanaSigner,
    },
};

#[cfg(test)]
use crate::services::{MockJupiterServiceTrait, MockSolanaProviderTrait, MockSolanaSignTrait};

#[cfg_attr(test, automock)]
#[async_trait]
pub trait SolanaRpcMethods: Send + Sync {
    async fn fee_estimate(
        &self,
        request: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError>;
    async fn transfer_transaction(
        &self,
        request: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError>;
    async fn prepare_transaction(
        &self,
        request: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError>;
    async fn sign_transaction(
        &self,
        request: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError>;
    async fn sign_and_send_transaction(
        &self,
        request: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError>;
    async fn get_supported_tokens(
        &self,
        request: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError>;
    async fn get_features_enabled(
        &self,
        request: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError>;
}

pub type DefaultProvider = SolanaProvider;
pub type DefaultSigner = SolanaSigner;
pub type DefaultJupiterService = JupiterService;

pub struct FeeQuote {
    fee_in_spl: u64,
    fee_in_spl_ui: String,
    fee_in_lamports: u64,
    conversion_rate: f64,
}

// Modified implementation with constrained generics
pub struct SolanaRpcMethodsImpl<P = DefaultProvider, S = DefaultSigner, J = DefaultJupiterService> {
    relayer: RelayerRepoModel,
    provider: Arc<P>,
    signer: Arc<S>,
    jupiter_service: Arc<J>,
}

// Default implementation for production use
impl SolanaRpcMethodsImpl<DefaultProvider, DefaultSigner, DefaultJupiterService> {
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<DefaultProvider>,
        signer: Arc<DefaultSigner>,
        jupiter_service: Arc<DefaultJupiterService>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
            jupiter_service,
        }
    }
}

#[cfg(test)]
impl SolanaRpcMethodsImpl<MockSolanaProviderTrait, MockSolanaSignTrait, MockJupiterServiceTrait> {
    pub fn new_mock(
        relayer: RelayerRepoModel,
        provider: Arc<MockSolanaProviderTrait>,
        signer: Arc<MockSolanaSignTrait>,
        jupiter_service: Arc<MockJupiterServiceTrait>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
            jupiter_service,
        }
    }
}

impl<P, S, J> SolanaRpcMethodsImpl<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    pub fn relayer_sign_transaction(
        &self,
        mut transaction: Transaction,
    ) -> Result<(Transaction, Signature), SolanaRpcError> {
        let signature = self.signer.sign(&transaction.message_data())?;

        transaction.signatures[0] = signature;

        Ok((transaction, signature))
    }

    pub async fn estimate_fee_payer_total_fee(
        &self,
        transaction: &Transaction,
    ) -> Result<u64, SolanaRpcError> {
        let tx_fee = self
            .provider
            .calculate_total_fee(&transaction.message())
            .await?;

        // Count ATA creation instructions
        let ata_creations = transaction
            .message
            .instructions
            .iter()
            .filter(|ix| {
                transaction.message.account_keys[ix.program_id_index as usize]
                    == spl_associated_token_account::id()
            })
            .count();

        if ata_creations == 0 {
            return Ok(tx_fee);
        }

        let account_creation_fee = self
            .provider
            .get_minimum_balance_for_rent_exemption(Account::LEN)
            .await?;

        Ok(tx_fee + (ata_creations as u64 * account_creation_fee))
    }

    pub async fn estimate_relayer_lampart_outflow(
        &self,
        tx: &Transaction,
    ) -> Result<u64, SolanaRpcError> {
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        let mut total_lamports_outflow: u64 = 0;
        for (ix_index, ix) in tx.message.instructions.iter().enumerate() {
            let program_id = tx.message.account_keys[ix.program_id_index as usize];

            // Check if the instruction comes from the System Program (native SOL transfers)
            if program_id == system_program::id() {
                if let Ok(system_ix) = bincode::deserialize::<SystemInstruction>(&ix.data) {
                    if let SystemInstruction::Transfer { lamports } = system_ix {
                        // In a system transfer instruction, the first account is the source and the
                        // second is the destination.
                        let source_index = ix.accounts.get(0).ok_or_else(|| {
                            SolanaRpcError::Internal(format!(
                                "Missing source account in instruction {}",
                                ix_index
                            ))
                        })?;
                        let source_pubkey = &tx.message.account_keys[*source_index as usize];

                        // Only validate transfers where the source is the relayer fee account.
                        if source_pubkey == &relayer_pubkey {
                            total_lamports_outflow += lamports;
                        }
                    }
                }
            }
        }

        Ok(total_lamports_outflow)
    }

    pub async fn get_fee_token_quote(
        &self,
        token: &str,
        total_fee: u64,
    ) -> Result<FeeQuote, SolanaRpcError> {
        // If token is SOL, return direct conversion
        if token == SOL_MINT {
            return Ok(FeeQuote {
                fee_in_spl: total_fee,
                fee_in_spl_ui: amount_to_ui_amount(total_fee, SOLANA_DECIMALS).to_string(),
                fee_in_lamports: total_fee,
                conversion_rate: 1f64,
            });
        }

        // Get token policy
        let token_entry = self
            .relayer
            .policies
            .get_solana_policy()
            .get_allowed_token_entry(token)
            .ok_or_else(|| {
                SolanaRpcError::UnsupportedFeeToken(format!("Token {} not allowed", token))
            })?;

        // Get token decimals
        let decimals = token_entry.decimals.ok_or_else(|| {
            SolanaRpcError::Estimation("Token decimals not configured".to_string())
        })?;

        // Get slippage from policy
        let slippage = token_entry
            .conversion_slippage_percentage
            .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE);

        // Get Jupiter quote
        let quote = self
            .jupiter_service
            .get_sol_to_token_quote(token, total_fee, slippage)
            .await
            .map_err(|e| SolanaRpcError::Estimation(e.to_string()))?;

        let fee_in_spl_ui = amount_to_ui_amount(quote.out_amount, decimals);
        let fee_in_sol_ui = amount_to_ui_amount(quote.in_amount, SOLANA_DECIMALS);
        let conversion_rate = fee_in_spl_ui / fee_in_sol_ui;

        Ok(FeeQuote {
            fee_in_spl: quote.out_amount,
            fee_in_spl_ui: fee_in_spl_ui.to_string(),
            fee_in_lamports: total_fee,
            conversion_rate,
        })
    }

    pub async fn create_and_sign_transaction(
        &self,
        instructions: Vec<Instruction>,
    ) -> Result<(Transaction, (Hash, u64)), SolanaRpcError> {
        let recent_blockhash = self
            .provider
            .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
            .await?;

        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        let message =
            Message::new_with_blockhash(&instructions, Some(&relayer_pubkey), &recent_blockhash.0);

        let transaction = Transaction::new_unsigned(message);

        let (signed_transaction, _) = self.relayer_sign_transaction(transaction)?;

        Ok((signed_transaction, recent_blockhash))
    }

    pub async fn handle_token_transfer(
        &self,
        source: &Pubkey,
        destination: &Pubkey,
        token_mint: &Pubkey,
        amount: u64,
    ) -> Result<Vec<Instruction>, SolanaRpcError> {
        let mut instructions = Vec::new();
        let source_ata = get_associated_token_address(source, token_mint);
        let destination_ata = get_associated_token_address(destination, token_mint);

        // Verify source account and balance
        let source_account = self.provider.get_account_from_pubkey(&source_ata).await?;
        let unpacked_source_account = Account::unpack(&source_account.data)
            .map_err(|e| SolanaRpcError::InvalidParams(format!("Invalid token account: {}", e)))?;

        if unpacked_source_account.amount < amount {
            return Err(SolanaRpcError::InsufficientFunds(
                "Insufficient token balance".to_string(),
            ));
        }

        // Create destination ATA if needed
        if self
            .provider
            .get_account_from_pubkey(&destination_ata)
            .await
            .is_err()
        {
            let relayer_pubkey = &Pubkey::from_str(&self.relayer.address)
                .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

            instructions.push(create_associated_token_account(
                &relayer_pubkey,
                destination,
                token_mint,
                destination,
            ));
        }
        let token_decimals = self
            .relayer
            .policies
            .get_solana_policy()
            .get_allowed_token_decimals(&token_mint.to_string())
            .ok_or_else(|| {
                SolanaRpcError::UnsupportedFeeToken("Token not found in allowed tokens".to_string())
            })?;

        instructions.push(
            spl_token::instruction::transfer_checked(
                &spl_token::id(),
                &source_ata,
                token_mint,
                &destination_ata,
                source,
                &[],
                amount,
                token_decimals,
            )
            .map_err(|e| SolanaRpcError::TransactionPreparation(e.to_string()))?,
        );

        Ok(instructions)
    }
}

#[async_trait]
impl<P, S, J> SolanaRpcMethods for SolanaRpcMethodsImpl<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    /// Retrieves a list of tokens supported by the relayer for fee payments.
    ///
    /// # Description
    ///
    /// This function queries the relayer for the tokens that are supported for fee payments. For
    /// each token, it returns metadata including the token symbol, mint address, and the number
    /// of decimal places supported.
    ///
    /// # Returns
    ///
    /// On success, returns a vector of [`TokenMetadata`] structures.
    async fn get_supported_tokens(
        &self,
        _params: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError> {
        let tokens = self
            .relayer
            .policies
            .get_solana_policy()
            .allowed_tokens
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|token| GetSupportedTokensItem {
                        mint: token.mint.clone(),
                        symbol: token.symbol.as_deref().unwrap_or("").to_string(),
                        decimals: token.decimals.unwrap_or(0),
                        max_allowed_fee: token.max_allowed_fee,
                        conversion_slippage_percentage: token.conversion_slippage_percentage,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(GetSupportedTokensResult { tokens })
    }

    /// Estimates the fee for an arbitrary transaction using a specified fee token.
    ///
    /// # Description
    ///
    /// This function simulates fee estimation for a transaction by executing it against the current
    /// blockchain state. It calculates the fee in the UI unit of the selected token (accounting
    /// for token decimals) and returns a conversion rate from SOL to the specified token.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded serialized transaction. This transaction can be signed or
    ///   unsigned.
    /// * `fee_token` - A string representing the token mint address to be used for fee payment.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `estimated_fee` - A string with the fee amount in the token's UI units.
    /// * `conversion_rate` - A string with the conversion rate from SOL to the specified token.
    async fn fee_estimate(
        &self,
        params: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        SolanaTransactionValidator::validate_fee_estimate_transaction(
            &transaction_request,
            &params.fee_token,
            &self.relayer,
        )
        .await
        .map_err(|e| SolanaRpcError::InvalidParams(e.to_string()))?;

        let mut transaction = transaction_request.clone();

        let recent_blockhash = self.provider.get_latest_blockhash().await?;

        // update tx blockhash
        transaction.message.recent_blockhash = recent_blockhash;

        let total_fee = self.estimate_fee_payer_total_fee(&transaction).await?;

        let fee_quota = self
            .get_fee_token_quote(&params.fee_token, total_fee)
            .await?;

        Ok(FeeEstimateResult {
            estimated_fee: fee_quota.fee_in_spl_ui,
            conversion_rate: fee_quota.conversion_rate.to_string(),
        })
    }

    /// Creates a transfer transaction for a specified token, sender, and recipient.
    ///
    /// # Description
    ///
    /// This function constructs a partially signed transfer transaction using the provided
    /// parameters. In addition to the transfer, it calculates fee amounts both in SPL tokens
    /// and in lamports, and sets an expiration block height for the transaction.
    ///
    /// # Parameters
    ///
    /// * `amount` - The amount to transfer, specified in the smallest unit of the token.
    /// * `token` - A string representing the token mint address for both the transfer and the fee
    ///   payment.
    /// * `source` - A string representing the sender's public key.
    /// * `destination` - A string representing the recipient's public key.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded partially signed transaction.
    /// * `fee_in_spl` - The fee amount in SPL tokens (smallest unit).
    /// * `fee_in_lamports` - The fee amount in lamports (SOL equivalent).
    /// * `fee_token` - The token mint address used for fee payments.
    /// * `valid_until_blockheight` - The block height until which the transaction remains valid.
    async fn transfer_transaction(
        &self,
        params: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError> {
        let token_mint = Pubkey::from_str(&params.token)
            .map_err(|_| SolanaRpcError::InvalidParams("Invalid token mint address".to_string()))?;
        let source = Pubkey::from_str(&params.source)
            .map_err(|_| SolanaRpcError::InvalidParams("Invalid source address".to_string()))?;
        let destination = Pubkey::from_str(&params.destination).map_err(|_| {
            SolanaRpcError::InvalidParams("Invalid destination address".to_string())
        })?;

        SolanaTransactionValidator::validate_token_transfer_transaction(
            &params.source,
            &params.destination,
            &params.token,
            params.amount,
            &self.relayer,
        )?;

        let instructions = if token_mint.to_string() == SOL_MINT {
            vec![system_instruction::transfer(
                &source,
                &destination,
                params.amount,
            )]
        } else {
            self.handle_token_transfer(&source, &destination, &token_mint, params.amount)
                .await?
        };

        let (transaction, recent_blockhash) =
            self.create_and_sign_transaction(instructions).await?;

        let total_fee = self.estimate_fee_payer_total_fee(&transaction).await?;

        let fee_quote = self.get_fee_token_quote(&params.token, total_fee).await?;

        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_fee,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await?;

        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)?;

        Ok(TransferTransactionResult {
            transaction: encoded_tx,
            fee_in_spl: fee_quote.fee_in_spl.to_string(),
            fee_in_lamports: fee_quote.fee_in_lamports.to_string(),
            fee_token: params.token,
            valid_until_blockheight: recent_blockhash.1,
        })
    }

    /// Prepares a transaction by adding relayer-specific instructions.
    ///
    /// # Description
    ///
    /// This function takes an existing Base64-encoded serialized transaction and adds
    /// relayer-specific instructions.
    /// The updated transaction will include additional data required by the relayer, and the
    /// function also provides updated fee information and an expiration block height.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded serialized transaction that the end user would like
    ///   relayed.
    /// * `fee_token` - A string representing the token mint address to be used for fee payment.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded transaction with the added relayer-specific instructions.
    /// * `fee_in_spl` - The fee amount in SPL tokens (in the smallest unit).
    /// * `fee_in_lamports` - The fee amount in lamports.
    /// * `fee_token` - The token mint address used for fee payments.
    /// * `valid_until_block_height` - The block height until which the transaction remains valid.
    async fn prepare_transaction(
        &self,
        params: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;
        let relayer_pubkey = Pubkey::from_str(&self.relayer.address)
            .map_err(|e| SolanaRpcError::Internal(e.to_string()))?;

        // Validate transaction
        SolanaTransactionValidator::validate_prepare_transaction(
            &transaction_request,
            &params.fee_token,
            &self.relayer,
            &*self.provider,
        )
        .await?;

        let recent_blockhash = self
            .provider
            .get_latest_blockhash_with_commitment(CommitmentConfig::finalized())
            .await?;

        // Create new transaction message with relayer as fee payer
        let mut message = transaction_request.message.clone();
        message.recent_blockhash = recent_blockhash.0;
        // Update fee payer
        if message.account_keys[0] != relayer_pubkey {
            message.account_keys[0] = relayer_pubkey;
        }

        // Create new transaction with single signature slot
        let transaction = Transaction {
            signatures: vec![Signature::default()],
            message,
        };

        let total_fee = self.estimate_fee_payer_total_fee(&transaction).await?;
        let lamports_outflow = self.estimate_relayer_lampart_outflow(&transaction).await?;
        let total_outflow = total_fee + lamports_outflow;

        // Validate relayer has sufficient balance
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_outflow,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await?;

        // Get fee quote
        let fee_quote = self
            .get_fee_token_quote(&params.fee_token, total_fee)
            .await?;

        // Sign transaction
        let (signed_transaction, _) = self.relayer_sign_transaction(transaction)?;

        // Serialize and encode
        let encoded_tx = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        Ok(PrepareTransactionResult {
            transaction: encoded_tx,
            fee_in_spl: fee_quote.fee_in_spl.to_string(),
            fee_in_lamports: fee_quote.fee_in_lamports.to_string(),
            fee_token: params.fee_token,
            valid_until_blockheight: recent_blockhash.1,
        })
    }

    /// Signs a prepared transaction without submitting it to the blockchain.
    ///
    /// # Description
    ///
    /// This function is used to sign a prepared transaction (one that may have been modified by the
    /// relayer) to ensure its validity and authorization before submission. It returns the
    /// signed transaction along with the corresponding signature.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded prepared transaction that requires signing.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded signed transaction.
    /// * `signature` - Signature of the submitted transaction.
    async fn sign_transaction(
        &self,
        params: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        SolanaTransactionValidator::validate_sign_transaction(
            &transaction_request,
            &self.relayer,
            &*self.provider,
        )
        .await?;

        let total_fee = self
            .estimate_fee_payer_total_fee(&transaction_request)
            .await?;
        let lamports_outflow = self
            .estimate_relayer_lampart_outflow(&transaction_request)
            .await?;
        let total_outflow = total_fee + lamports_outflow;

        // Validate relayer has sufficient balance
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_outflow,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await?;

        let (signed_transaction, signature) = self.relayer_sign_transaction(transaction_request)?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        Ok(SignTransactionResult {
            transaction: serialized_transaction,
            signature: signature.to_string(),
        })
    }

    /// Signs a prepared transaction and immediately submits it to the Solana blockchain.
    ///
    /// # Description
    ///
    /// This function combines the signing and submission steps into one operation. After validating
    /// and signing the provided transaction, it is immediately sent to the blockchain for
    /// execution. This is particularly useful when you want to reduce the number of
    /// client-server interactions.
    ///
    /// # Parameters
    ///
    /// * `transaction` - A Base64-encoded prepared transaction that needs to be signed and
    ///   submitted.
    ///
    /// # Returns
    ///
    /// On success, returns a tuple containing:
    ///
    /// * `transaction` - A Base64-encoded signed transaction that has been submitted.
    /// * `signature` - Signature of the submitted transaction.
    async fn sign_and_send_transaction(
        &self,
        params: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError> {
        let transaction_request = Transaction::try_from(params.transaction)?;

        SolanaTransactionValidator::validate_sign_transaction(
            &transaction_request,
            &self.relayer,
            &*self.provider,
        )
        .await?;

        let total_fee = self
            .estimate_fee_payer_total_fee(&transaction_request)
            .await?;
        let lamports_outflow = self
            .estimate_relayer_lampart_outflow(&transaction_request)
            .await?;
        let total_outflow = total_fee + lamports_outflow;

        // Validate relayer has sufficient balance
        SolanaTransactionValidator::validate_sufficient_relayer_balance(
            total_outflow,
            &self.relayer.address,
            &self.relayer.policies.get_solana_policy(),
            &*self.provider,
        )
        .await?;

        let (signed_transaction, _) = self.relayer_sign_transaction(transaction_request)?;

        let send_signature = self.provider.send_transaction(&signed_transaction).await?;

        let serialized_transaction = EncodedSerializedTransaction::try_from(&signed_transaction)?;

        Ok(SignAndSendTransactionResult {
            transaction: serialized_transaction,
            signature: send_signature.to_string(),
        })
    }

    /// Retrieves a list of features enabled by the relayer.
    ///
    /// # Deprecated
    ///
    /// This method is deprecated. It is recommended to use more fine-grained methods for feature
    /// detection.
    ///
    /// # Description
    ///
    /// This function returns a list of enabled features on the relayer.
    ///
    /// # Returns
    ///
    /// On success, returns a vector of strings where each string represents an enabled feature
    /// (e.g., "gasless").
    async fn get_features_enabled(
        &self,
        _params: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError> {
        // gasless is enabled out of the box to be compliant with the spec
        Ok(GetFeaturesEnabledResult {
            features: vec!["gasless".to_string()],
        })
    }
}

#[cfg(test)]
mod tests {

    use crate::{
        domain::SolanaTransactionValidationError,
        models::{
            NetworkType, RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy,
        },
        services::{MockSolanaProviderTrait, MockSolanaSignTrait, QuoteResponse},
    };

    use super::*;
    use mockall::predicate::{self};
    use solana_sdk::{
        message::Message,
        program_option::COption,
        pubkey::Pubkey,
        signature::{Keypair, Signature, Signer},
        system_instruction,
    };

    fn setup_test_context() -> (
        RelayerRepoModel,
        MockSolanaSignTrait,
        MockSolanaProviderTrait,
        MockJupiterServiceTrait,
        EncodedSerializedTransaction,
    ) {
        // Create test transaction
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        // Create test relayer
        let relayer = RelayerRepoModel {
            id: "id".to_string(),
            name: format!("Relayer",),
            network: "testnet".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                allowed_accounts: None,
                allowed_tokens: None,
                min_balance: 10000,
                allowed_programs: None,
                max_signatures: Some(10),
                disallowed_accounts: None,
                max_allowed_transfer_amount_lamports: None,
                max_tx_data_size: 1000,
            }),
            signer_id: "test".to_string(),
            address: payer.pubkey().to_string(),
            notification_id: None,
            system_disabled: false,
        };

        // Setup mock signer
        let mock_signer = MockSolanaSignTrait::new();

        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        let jupiter_service = MockJupiterServiceTrait::new();
        let provider = MockSolanaProviderTrait::new();

        (relayer, mock_signer, provider, jupiter_service, encoded_tx)
    }

    #[test]
    fn test_relayer_sign_transaction() {
        let (relayer, mut signer, provider, jupiter_service, _) = setup_test_context();

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc.relayer_sign_transaction(transaction);

        assert!(result.is_ok(), "Transaction signing should succeed");
        let (signed_tx, signature) = result.unwrap();
        assert_eq!(
            signed_tx.signatures[0], signature,
            "Returned signature should match transaction signature"
        );
        assert_eq!(signature.as_ref().len(), 64, "Signature should be 64 bytes");
    }

    #[tokio::test]
    async fn test_get_fee_token_quote_sol() {
        let (mut relayer, signer, provider, jupiter_service, _) = setup_test_context();

        // Setup policy with SOL
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc.get_fee_token_quote(SOL_MINT, 1_000_000).await;
        assert!(result.is_ok());

        let quote = result.unwrap();
        assert_eq!(quote.fee_in_spl, 1_000_000);
        assert_eq!(quote.fee_in_spl_ui, "0.001");
        assert_eq!(quote.fee_in_lamports, 1_000_000);
        assert_eq!(quote.conversion_rate, 1.0);
    }

    #[tokio::test]
    async fn test_get_fee_token_quote_spl_token() {
        let (mut relayer, signer, provider, mut jupiter_service, _) = setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        // let test_token = test_token.to_string();
        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(move |_, amount, _| {
                Box::pin(async move {
                    Ok(QuoteResponse {
                        input_mint: SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: amount,
                        out_amount: 2_000_000, // 1 SOL = 2 USDC
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc.get_fee_token_quote(&test_token, 1_000_000_000).await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.fee_in_spl, 2_000_000);
        assert_eq!(quote.fee_in_spl_ui, "2");
        assert_eq!(quote.fee_in_lamports, 1_000_000_000);
        assert_eq!(quote.conversion_rate, 2.0);
    }

    #[tokio::test]
    async fn test_create_and_sign_transaction_success() {
        let (relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();
        let expected_signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature.clone()));

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        // Create test instructions
        let test_instruction =
            system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);

        let result = rpc
            .create_and_sign_transaction(vec![test_instruction])
            .await;

        assert!(result.is_ok());
        let (transaction, (_, slot)) = result.unwrap();

        assert_eq!(transaction.message.instructions.len(), 1);
        assert_eq!(slot, 100);
        assert!(!transaction.signatures.is_empty());
    }

    #[tokio::test]
    async fn test_create_and_sign_transaction_provider_error() {
        let (relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Mock provider error
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| {
                Box::pin(async {
                    Err(crate::services::SolanaProviderError::RpcError(
                        "Test error".to_string(),
                    ))
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let test_instruction =
            system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);

        let result = rpc
            .create_and_sign_transaction(vec![test_instruction])
            .await;

        assert!(matches!(result, Err(SolanaRpcError::Provider(_))));
    }

    #[tokio::test]
    async fn test_sign_transaction_success() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        // mock simulate_transaction
        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_ok());
        let sign_result = result.unwrap();

        // Verify signature format (base58 encoded, 64 bytes)
        let decoded_sig = bs58::decode(&sign_result.signature)
            .into_vec()
            .expect("Failed to decode base58 signature");
        assert_eq!(decoded_sig.len(), 64);
    }

    #[tokio::test]
    async fn test_sign_transaction_balance_failure() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000) }));

        // mock simulate_transaction
        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_err());

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains("Insufficient funds:"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_validation_failure_blockhash() {
        let (relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(false) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_signatures() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with low max signatures
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_signatures: Some(0),
            ..Default::default()
        });

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Transaction requires 1 signatures, which exceeds \
                         maximum allowed 0"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_disallowed_program() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with disallowed programs
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_programs: Some(vec!["different_program".to_string()]),
            ..Default::default()
        });

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Program 11111111111111111111111111111111 not allowed"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_data_size() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Update policy with small max data size
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_tx_data_size: 10,
            ..Default::default()
        });

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;
        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Transaction size 215 exceeds maximum allowed 10"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_wrong_fee_payer() {
        let (relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&wrong_fee_payer.pubkey())); // Different fee payer
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );
        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains("Policy violation: Fee payer"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_disallowed_account() {
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx) =
            setup_test_context();

        // Update policy with disallowed accounts
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            disallowed_accounts: Some(vec![Pubkey::new_unique().to_string()]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        // This should pass since our test transaction doesn't use disallowed accounts
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_exceeds_max_lamports_transfer() {
        let (mut relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Set max allowed transfer amount in policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            max_allowed_transfer_amount_lamports: Some(500),
            ..Default::default()
        });

        // Create transaction that exceeds max transfer amount
        let payer = Keypair::new();
        relayer.address = payer.pubkey().to_string();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(
            &payer.pubkey(),
            &recipient,
            1000, // Amount exceeds max_allowed_transfer_amount_lamports
        );
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction)
            .expect("Failed to encode transaction");

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string
                        .contains("Lamports transfer amount 1000 exceeds max allowed fee 500"),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_sign_and_send_transaction_success() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        let expected_signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature.clone()));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        provider
            .expect_send_transaction()
            .returning(move |_| Box::pin(async move { Ok(expected_signature.clone()) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignAndSendTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_and_send_transaction(params).await;
        assert!(result.is_ok());

        let send_result = result.unwrap();
        assert_eq!(send_result.signature, expected_signature.to_string());
    }

    #[tokio::test]
    async fn test_get_supported_tokens() {
        let (mut relayer, signer, provider, jupiter_service, _) = setup_test_context();

        // Update relayer policy with some tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![
                SolanaAllowedTokensPolicy {
                    mint: "mint1".to_string(),
                    symbol: Some("TOKEN1".to_string()),
                    decimals: Some(9),
                    max_allowed_fee: Some(1000),
                    conversion_slippage_percentage: None,
                },
                SolanaAllowedTokensPolicy {
                    mint: "mint2".to_string(),
                    symbol: Some("TOKEN2".to_string()),
                    decimals: Some(6),
                    max_allowed_fee: None,
                    conversion_slippage_percentage: None,
                },
            ]),
            ..Default::default()
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let result = rpc
            .get_supported_tokens(GetSupportedTokensRequestParams {})
            .await;

        assert!(result.is_ok());
        let tokens = result.unwrap().tokens;
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].mint, "mint1");
        assert_eq!(tokens[0].symbol, "TOKEN1");
        assert_eq!(tokens[0].decimals, 9);
        assert_eq!(tokens[0].max_allowed_fee, Some(1000));
    }

    #[tokio::test] // TODO
    async fn test_estimate_fee_success() {
        let (relayer, mut signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();
        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        provider
            .expect_is_blockhash_valid()
            .with(predicate::always(), predicate::always())
            .returning(|_, _| Box::pin(async { Ok(true) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    replacement_blockhash: None,
                    inner_instructions: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = SignTransactionRequestParams {
            transaction: encoded_tx,
        };

        let result = rpc.sign_transaction(params).await;

        assert!(result.is_ok());
        let sign_result = result.unwrap();

        // Verify signature format (base58 encoded, 64 bytes)
        let decoded_sig = bs58::decode(&sign_result.signature)
            .into_vec()
            .expect("Failed to decode base58 signature");
        assert_eq!(decoded_sig.len(), 64);
    }

    #[tokio::test]
    async fn test_fee_estimate_with_allowed_token() {
        let (mut relayer, signer, mut provider, mut jupiter_service, encoded_tx) =
            setup_test_context();

        // Set up policy with allowed token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "USDC".to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1000000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        // Mock provider methods
        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(500000000u64) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq("USDC"),
                predicate::eq(500000000u64),
                predicate::eq(1.0f32),
            )
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "SOL".to_string(),
                        output_mint: "USDC".to_string(),
                        in_amount: 500000000,
                        out_amount: 80000000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            fee_token: "USDC".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "80");
        assert_eq!(fee_estimate.conversion_rate, "160");
    }

    #[tokio::test]
    async fn test_fee_estimate_usdt_to_sol_conversion() {
        let (mut relayer, signer, _provider, mut jupiter_service, encoded_tx) =
            setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(), // USDT mint
                symbol: Some("USDT".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1000000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        let mut provider = MockSolanaProviderTrait::new();

        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),
                predicate::eq(1_000_000u64),
                predicate::eq(1.0f32),
            )
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "So11111111111111111111111111111111111111112".to_string(),
                        output_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
                        in_amount: 1_000_000, // 0.001 SOL
                        out_amount: 20_000,   // 0.02 USDT
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            // noboost
            fee_token: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.02"); // 0.02 USDT
        assert_eq!(fee_estimate.conversion_rate, "20"); // 1 SOL = 20 USDT
    }

    #[tokio::test]
    async fn test_fee_estimate_uni_to_sol_dynamic_price() {
        let (mut relayer, signer, mut provider, mut jupiter_service, encoded_tx) =
            setup_test_context();

        // Set up policy with UNI token (decimals = 8)
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(), // UNI mint
                symbol: Some("UNI".to_string()),
                decimals: Some(8),
                max_allowed_fee: Some(1_000_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .with(
                predicate::eq("8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt"),
                predicate::eq(1_000_000u64),
                predicate::eq(1.0f32),
            )
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: "So11111111111111111111111111111111111111112".to_string(),
                        output_mint: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(),
                        in_amount: 1_000_000,  // 0.001 SOL
                        out_amount: 1_770_000, // 0.0177 UNI
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            // noboost
            fee_token: "8qJSyQprMC57TWKaYEmetUR3UUiTP2M3hXW6D2evU9Tt".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.0177"); // 0.0177 UNI
        assert_eq!(fee_estimate.conversion_rate, "17.7"); // 1 SOL = 17.7 UNI
    }

    #[tokio::test]
    async fn test_fee_estimate_native_sol() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Set up policy with SOL token
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "So11111111111111111111111111111111111111112".to_string(), // Native SOL
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        // Mock provider methods - expect 0.001 SOL fee (1_000_000 lamports)
        provider
            .expect_get_latest_blockhash()
            .returning(|| Box::pin(async { Ok(Hash::new_unique()) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(1_000_000u64) }));

        // We don't expect any Jupiter quotes for native SOL

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = FeeEstimateRequestParams {
            transaction: encoded_tx,
            fee_token: "So11111111111111111111111111111111111111112".to_string(),
        };

        let result = rpc.fee_estimate(params).await;
        assert!(result.is_ok());

        let fee_estimate = result.unwrap();
        assert_eq!(fee_estimate.estimated_fee, "0.001"); // 0.001 SOL (1_000_000 lamports)
        assert_eq!(fee_estimate.conversion_rate, "1"); // 1:1 for native SOL
    }

    #[tokio::test]
    async fn test_transfer_sol_success() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();

        // Set up policy with SOL
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = TransferTransactionRequestParams {
            token: SOL_MINT.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 1_000_000,
        };
        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_token, SOL_MINT);
        assert_eq!(transfer_result.fee_in_lamports, "5000");
    }

    #[tokio::test]
    async fn test_transfer_spl_token_success() {
        let (mut relayer, mut signer, mut provider, mut jupiter_service, _) = setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC

        // Create valid token account data
        let token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(), // Source account owner
            amount: 10_000_000,          // 10 USDC (assuming 6 decimals)
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        // Pack the account data
        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        // Set up policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        // Mock provider responses
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: 5000,
                        out_amount: 100_000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 1_000_000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_in_spl, "100000");
        assert_eq!(transfer_result.fee_in_lamports, "5000");
        assert_eq!(transfer_result.fee_token, test_token);
        assert_ne!(transfer_result.valid_until_blockheight, 0);
    }

    #[tokio::test]
    async fn test_transfer_spl_token_success_token_account_creation() {
        let (mut relayer, mut signer, mut provider, mut jupiter_service, _) = setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC

        let source_token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(),
            amount: 10_000_000,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        let mut source_account_data = vec![0; spl_token::state::Account::LEN];
        spl_token::state::Account::pack(source_token_account, &mut source_account_data).unwrap();

        // Set up policy
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(1_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        // Mock provider responses
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_commitment| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        let call_count = std::sync::atomic::AtomicUsize::new(0);

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let count = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                let account_data = source_account_data.clone();

                let is_source = count == 0;
                Box::pin(async move {
                    if is_source {
                        Ok(solana_sdk::account::Account {
                            lamports: 1_000_000,
                            data: account_data,
                            owner: spl_token::id(),
                            executable: false,
                            rent_epoch: 0,
                        })
                    } else {
                        Err(crate::services::SolanaProviderError::InvalidAddress(
                            "test".to_string(),
                        ))
                    }
                })
            });

        provider
            .expect_get_minimum_balance_for_rent_exemption()
            .returning(|_| Box::pin(async { Ok(1111) }));

        // Mock Jupiter quote
        jupiter_service
            .expect_get_sol_to_token_quote()
            .returning(|_, _, _| {
                Box::pin(async {
                    Ok(QuoteResponse {
                        input_mint: SOL_MINT.to_string(),
                        output_mint: test_token.to_string(),
                        in_amount: 5000,
                        out_amount: 100_000,
                        price_impact_pct: 0.1,
                        other_amount_threshold: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 1_000_000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(result.is_ok());

        let transfer_result = result.unwrap();
        assert_eq!(transfer_result.fee_in_spl, "100000");
        assert_eq!(transfer_result.fee_in_lamports, "6111");
        assert_eq!(transfer_result.fee_token, test_token);
        assert_ne!(transfer_result.valid_until_blockheight, 0);
    }

    #[tokio::test]
    async fn test_transfer_spl_insufficient_balance() {
        let (_, signer, mut provider, jupiter_service, _) = setup_test_context();
        let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

        // Create test relayer
        let relayer = RelayerRepoModel {
            id: "id".to_string(),
            name: format!("Relayer",),
            network: "TestNet".to_string(),
            paused: false,
            network_type: NetworkType::Solana,
            policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
                allowed_accounts: None,
                allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                    mint: test_token.to_string(),
                    symbol: Some("USDC".to_string()),
                    decimals: Some(6),
                    max_allowed_fee: Some(1000),
                    conversion_slippage_percentage: Some(1.0),
                }]),
                min_balance: 10000,
                allowed_programs: None,
                max_signatures: Some(10),
                disallowed_accounts: None,
                max_allowed_transfer_amount_lamports: None,
                max_tx_data_size: 1000,
            }),
            signer_id: "test".to_string(),
            address: Keypair::new().pubkey().to_string(),
            notification_id: None,
            system_disabled: false,
        };
        // Create token account with low balance
        let token_account = spl_token::state::Account {
            mint: Pubkey::from_str(test_token).unwrap(),
            owner: Pubkey::new_unique(),
            amount: 100,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };

        let mut account_data = vec![0; Account::LEN];
        Account::pack(token_account, &mut account_data).unwrap();

        provider
            .expect_get_account_from_pubkey()
            .returning(move |_| {
                let account_data = account_data.clone();
                Box::pin(async move {
                    Ok(solana_sdk::account::Account {
                        lamports: 1_000_000,
                        data: account_data,
                        owner: spl_token::id(),
                        executable: false,
                        rent_epoch: 0,
                    })
                })
            });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = TransferTransactionRequestParams {
            token: test_token.to_string(),
            source: Pubkey::new_unique().to_string(),
            destination: Pubkey::new_unique().to_string(),
            amount: 1_000_000,
        };

        let result = rpc.transfer_transaction(params).await;
        assert!(matches!(result, Err(SolanaRpcError::InsufficientFunds(_))));
    }

    #[tokio::test]
    async fn test_prepare_transaction_success() {
        let (mut relayer, mut signer, mut provider, jupiter_service, encoded_tx) =
            setup_test_context();

        // Setup policy with SOL
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        // Mock provider responses
        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        assert_eq!(prepare_result.fee_token, SOL_MINT);
        assert_eq!(prepare_result.fee_in_lamports, "5000");
        assert_eq!(prepare_result.fee_in_spl, "5000");
        assert_eq!(prepare_result.valid_until_blockheight, 100);
    }

    #[tokio::test]
    async fn test_prepare_transaction_insufficient_balance() {
        let (mut relayer, signer, mut provider, jupiter_service, encoded_tx) = setup_test_context();

        // Set high minimum balance requirement
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: 100_000_000, // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(50_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
                })
            })
        });
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(matches!(
            result,
            Err(SolanaRpcError::SolanaTransactionValidation(
                SolanaTransactionValidationError::InsufficientFunds(_)
            ))
        ));
    }

    #[tokio::test]
    async fn test_prepare_transaction_updates_fee_payer() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();

        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: 100_000_000, // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        // Create transaction with different fee payer
        let wrong_fee_payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let ix = system_instruction::transfer(&wrong_fee_payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[ix], Some(&wrong_fee_payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();
        let signature = Signature::new_unique();

        signer
            .expect_sign()
            .returning(move |_| Ok(signature.clone()));

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
                })
            })
        });
        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer.clone(),
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        let final_tx = Transaction::try_from(prepare_result.transaction).unwrap();
        assert_eq!(
            final_tx.message.account_keys[0],
            Pubkey::from_str(&relayer.address).unwrap()
        );
    }

    #[tokio::test]
    async fn test_prepare_transaction_signature_verification() {
        let (mut relayer, mut signer, mut provider, jupiter_service, _) = setup_test_context();
        println!("Setting up known keypair for signature verification");
        let relayer_keypair = Keypair::new();
        let expected_signature = Signature::new_unique();

        relayer.address = relayer_keypair.pubkey().to_string();
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            min_balance: 100_000_000, // 0.1 SOL minimum balance
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: SOL_MINT.to_string(),
                symbol: Some("SOL".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        signer
            .expect_sign()
            .returning(move |_| Ok(expected_signature.clone()));

        provider
            .expect_get_latest_blockhash_with_commitment()
            .returning(|_| Box::pin(async { Ok((Hash::new_unique(), 100)) }));

        provider
            .expect_calculate_total_fee()
            .returning(|_| Box::pin(async { Ok(5000u64) }));

        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(1_000_000_000) }));

        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
                })
            })
        });

        // Create test transaction
        let ix = system_instruction::transfer(&Pubkey::new_unique(), &Pubkey::new_unique(), 1000);
        let message = Message::new(&[ix], Some(&relayer_keypair.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;
        assert!(result.is_ok());

        let prepare_result = result.unwrap();
        let final_tx = Transaction::try_from(prepare_result.transaction).unwrap();

        // Verify signature presence and correctness
        assert_eq!(
            final_tx.signatures.len(),
            1,
            "Transaction should have exactly one signature"
        );
        assert_eq!(
            final_tx.signatures[0], expected_signature,
            "Transaction should have the expected signature"
        );
        assert_eq!(
            final_tx.message.account_keys[0].to_string(),
            relayer_keypair.pubkey().to_string(),
            "Fee payer should match relayer address"
        );
    }

    #[tokio::test]
    async fn test_prepare_transaction_not_allowed_token() {
        let (mut relayer, signer, mut provider, jupiter_service, _) = setup_test_context();

        // Configure policy with allowed tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: "AllowedToken111111111111111111111111111111".to_string(),
                symbol: Some("ALLOWED".to_string()),
                decimals: Some(9),
                max_allowed_fee: None,
                conversion_slippage_percentage: None,
            }]),
            ..Default::default()
        });

        // Create transaction with not allowed token
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let not_allowed_token = Pubkey::new_unique();

        let ix = spl_token::instruction::transfer(
            &spl_token::id(),
            &get_associated_token_address(&payer.pubkey(), &not_allowed_token),
            &get_associated_token_address(&recipient, &not_allowed_token),
            &payer.pubkey(),
            &[],
            100,
        )
        .unwrap();

        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

        // Setup provider mocks
        provider.expect_simulate_transaction().returning(|_| {
            Box::pin(async {
                Ok(solana_client::rpc_response::RpcSimulateTransactionResult {
                    err: None,
                    logs: None,
                    accounts: None,
                    units_consumed: None,
                    return_data: None,
                    inner_instructions: None,
                    replacement_blockhash: None,
                })
            })
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
        );

        let params = PrepareTransactionRequestParams {
            transaction: encoded_tx,
            fee_token: SOL_MINT.to_string(),
        };

        let result = rpc.prepare_transaction(params).await;

        match result {
            Err(SolanaRpcError::SolanaTransactionValidation(err)) => {
                let error_string = err.to_string();
                assert!(
                    error_string.contains(
                        "Policy violation: Token So11111111111111111111111111111111111111112 not \
                         allowed for transfers"
                    ),
                    "Unexpected error message: {}",
                    err
                );
            }
            other => panic!("Expected ValidationError, got: {:?}", other),
        }
    }
}
