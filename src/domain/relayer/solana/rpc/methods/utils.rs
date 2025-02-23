use std::str::FromStr;

use super::*;

use solana_sdk::{
    commitment_config::CommitmentConfig, hash::Hash, instruction::Instruction, message::Message,
    program_pack::Pack, pubkey::Pubkey, signature::Signature,
    system_instruction::SystemInstruction, system_program, transaction::Transaction,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account,
};
use spl_token::{amount_to_ui_amount, state::Account};

use crate::{
    constants::{DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE, SOLANA_DECIMALS, SOL_MINT},
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

pub struct FeeQuote {
    pub fee_in_spl: u64,
    pub fee_in_spl_ui: String,
    pub fee_in_lamports: u64,
    pub conversion_rate: f64,
}

impl<P, S, J> SolanaRpcMethodsImpl<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
{
    pub(crate) fn relayer_sign_transaction(
        &self,
        mut transaction: Transaction,
    ) -> Result<(Transaction, Signature), SolanaRpcError> {
        let signature = self.signer.sign(&transaction.message_data())?;

        transaction.signatures[0] = signature;

        Ok((transaction, signature))
    }

    pub(crate) async fn estimate_fee_payer_total_fee(
        &self,
        transaction: &Transaction,
    ) -> Result<u64, SolanaRpcError> {
        let tx_fee = self
            .provider
            .calculate_total_fee(transaction.message())
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

    pub(crate) async fn estimate_relayer_lampart_outflow(
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
                        let source_index = ix.accounts.first().ok_or_else(|| {
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

    pub(crate) async fn get_fee_token_quote(
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

    pub(crate) async fn create_and_sign_transaction(
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

    pub(crate) async fn handle_token_transfer(
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
                relayer_pubkey,
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

#[cfg(test)]
mod tests {

    use crate::{
        models::{RelayerNetworkPolicy, RelayerSolanaPolicy, SolanaAllowedTokensPolicy},
        services::QuoteResponse,
    };

    use super::*;
    use solana_sdk::{
        signature::{Keypair, Signature},
        signer::Signer,
        system_instruction,
    };

    #[test]
    fn test_relayer_sign_transaction() {
        let (relayer, mut signer, provider, jupiter_service, _) = setup_test_context();

        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let instruction = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);
        let signature = Signature::new_unique();

        signer.expect_sign().returning(move |_| Ok(signature));

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

        let result = rpc.get_fee_token_quote(test_token, 1_000_000_000).await;
        assert!(result.is_ok());
        let quote = result.unwrap();
        assert_eq!(quote.fee_in_spl, 2_000_000);
        assert_eq!(quote.fee_in_spl_ui, "2");
        assert_eq!(quote.fee_in_lamports, 1_000_000_000);
        assert_eq!(quote.conversion_rate, 2.0);
    }
}
