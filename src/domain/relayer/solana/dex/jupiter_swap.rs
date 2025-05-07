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
        info!("Executing Jupiter swap: {:?}", params);

        // 1. Get the best route from Jupiter's API
        let quote = self
            .jupiter_service
            .get_quote(QuoteRequest {
                input_mint: params.source_mint.clone(),
                output_mint: params.destination_mint.clone(),
                amount: params.amount,
                slippage: params.slippage_percent as f32,
            })
            .await
            .map_err(|e| RelayerError::DexError(format!("Failed to get Jupiter quote: {}", e)))?;
        info!("Received quote: {:?}", quote);

        // 2. Get the swap transaction from Jupiter's API
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

        info!("Received swap transaction: {:?}", swap_tx);

        // 3. Sign the transaction using the signer service
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

        info!("Transaction confirmed: {}", signature);

        // 6. Return the result with actual amounts
        Ok(SwapResult {
            source_amount: params.amount,
            destination_amount: quote.out_amount.clone(),
            transaction_signature: signature.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use log::info;
    use solana_sdk::transaction::VersionedTransaction;

    use crate::{
        models::EncodedSerializedTransaction,
        services::{
            JupiterService, JupiterServiceTrait, QuoteRequest, SwapRequest, UltraOrderRequest,
        },
    };

    #[tokio::test]
    async fn test_jupiter_swap_success() {
        let jupiter_service = JupiterService::new_from_network("mainnet-beta");

        let quote = jupiter_service
            .get_quote(QuoteRequest {
                input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                output_mint: "So11111111111111111111111111111111111111112".to_string(),
                amount: 1000000,
                slippage: 0.0,
            })
            .await;

        println!("Quote: {:?}", quote);

        let swap_tx = jupiter_service
            .get_swap_transaction(SwapRequest {
                quote_response: quote.unwrap().clone(),
                user_public_key: "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf".to_string(),
                wrap_and_unwrap_sol: Some(true),
                fee_account: None,
                compute_unit_price_micro_lamports: None,
                prioritization_fee_lamports: None,
            })
            .await;

        println!("Swap: {:?}", swap_tx);

        // Mock input data
        // let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        // let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        // let swap_amount = 1000000; // 1 USDC
        // let slippage = 0.5; // 0.5%
        // let relayer_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
    }

    #[tokio::test]
    async fn test_jupiter_swap_success_ultra() {
        let jupiter_service = JupiterService::new_from_network("mainnet-beta");

        let quote = jupiter_service
            .get_ultra_order(UltraOrderRequest {
                input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                output_mint: "So11111111111111111111111111111111111111112".to_string(),
                amount: 1000000,
                taker: "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf".to_string(),
            })
            .await;

        println!("Quote: {:?}", quote);

        let tx = VersionedTransaction::try_from(EncodedSerializedTransaction::new(
            quote.unwrap().transaction.unwrap(),
        ))
        .unwrap();

        println!("Tx: {:?}", tx);

        // Mock input data
        // let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        // let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        // let swap_amount = 1000000; // 1 USDC
        // let slippage = 0.5; // 0.5%
        // let relayer_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
    }
}
