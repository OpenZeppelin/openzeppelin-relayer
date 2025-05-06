//! Jupiter DEX integration

use std::sync::Arc;

use super::{DexStrategy, SwapParams, SwapResult};
use crate::domain::relayer::RelayerError;
use crate::models::EncodedSerializedTransaction;
use crate::services::{
    JupiterService, JupiterServiceTrait, QuoteRequest, SolanaProvider, SolanaProviderTrait,
    SolanaSignTrait, SolanaSigner, SwapRequest,
};
use async_trait::async_trait;
use log::info;
use solana_sdk::transaction::VersionedTransaction;

pub struct JupiterSwapDex {
    provider: Arc<SolanaProvider>,
    signer: Arc<SolanaSigner>,
    jupiter_service: Arc<JupiterService>,
}

impl JupiterSwapDex {
    pub fn new(
        provider: Arc<SolanaProvider>,
        signer: Arc<SolanaSigner>,
        jupiter_service: Arc<JupiterService>,
    ) -> Self {
        Self {
            provider,
            signer,
            jupiter_service,
        }
    }
}

#[async_trait]
impl DexStrategy for JupiterSwapDex {
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError> {
        info!(
            "Executing Jupiter swap: {} -> {}, amount: {}, slippage: {}%",
            params.source_mint, params.destination_mint, params.amount, params.slippage_percent
        );

        // Convert slippage from percentage to basis points (1% = 100 bps)
        let slippage_bps = (params.slippage_percent * 100.0) as f32;

        // 1. Get the best route from Jupiter's API
        info!("Getting quote from Jupiter for swap");
        let quote = self
            .jupiter_service
            .get_quote(QuoteRequest {
                input_mint: params.source_mint.clone(),
                output_mint: params.destination_mint.clone(),
                amount: params.amount,
                slippage: slippage_bps,
            })
            .await
            .map_err(|e| RelayerError::DexError(format!("Failed to get Jupiter quote: {}", e)))?;

        info!(
            "Received quote: in_amount={}, out_amount={}, price_impact={}%",
            quote.in_amount,
            quote.out_amount.clone(),
            quote.price_impact_pct
        );

        // 2. Get the swap transaction from Jupiter's API
        info!("Getting swap transaction from Jupiter");
        let swap_tx = self
            .jupiter_service
            .get_swap_transaction(SwapRequest {
                quote_response: quote.clone(),
                user_public_key: params.owner_address,
                wrap_and_unwrap_sol: Some(true),
                fee_account: None,
                compute_unit_price_micro_lamports: None,
                prioritization_fee_lamports: None,
            })
            .await
            .map_err(|e| {
                RelayerError::DexError(format!("Failed to get swap transaction: {}", e))
            })?;

        // 3. Sign the transaction using the signer service
        info!("Signing swap transaction");
        let mut swap_tx = VersionedTransaction::try_from(EncodedSerializedTransaction::new(
            swap_tx.swap_transaction,
        ))
        .map_err(|e| RelayerError::DexError(format!("Failed to decode swap transaction: {}", e)))?;
        let signature = self
            .signer
            .sign(&swap_tx.message.serialize())
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to sign transaction: {}", e))
                // TODO improve error handling
            })?;

        swap_tx.signatures[0] = signature;

        // 4. Send the transaction and get the signature
        info!("Sending swap transaction to the network");
        let signature = self
            .provider
            .send_versioned_transaction(&swap_tx)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to send transaction: {}", e))
            })?;

        // 5. Wait for transaction confirmation
        info!("Waiting for transaction confirmation: {}", signature);
        self.provider
            .confirm_transaction(&signature)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Transaction failed to confirm: {}", e))
            })?;

        // 6. Return the result with actual amounts
        Ok(SwapResult {
            source_amount: params.amount,
            destination_amount: quote.out_amount.clone(),
            transaction_signature: signature.to_string(),
        })
    }
}
