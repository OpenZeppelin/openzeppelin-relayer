//! Jupiter Ultra API DEX integration

use std::sync::Arc;

use super::{DexStrategy, SwapParams, SwapResult};
use crate::domain::relayer::RelayerError;
use crate::models::EncodedSerializedTransaction;
use crate::services::{
    JupiterService, JupiterServiceTrait, SolanaProvider, SolanaSignTrait, SolanaSigner,
    UltraExecuteRequest, UltraOrderRequest,
};
use async_trait::async_trait;
use log::info;
use solana_sdk::transaction::VersionedTransaction;

pub struct JupiterUltraDex {
    provider: Arc<SolanaProvider>,
    signer: Arc<SolanaSigner>,
    jupiter_service: Arc<JupiterService>,
}

impl JupiterUltraDex {
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
impl DexStrategy for JupiterUltraDex {
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError> {
        info!("Executing Jupiter swap using ultra api: {:?}", params);

        let order = self
            .jupiter_service
            .get_ultra_order(UltraOrderRequest {
                input_mint: params.source_mint,
                output_mint: params.destination_mint,
                amount: params.amount,
                taker: params.owner_address,
            })
            .await
            .map_err(|e| {
                RelayerError::DexError(format!("Failed to get Jupiter Ultra order: {}", e))
            })?;

        info!("Received order: {:?}", order);

        let encoded_transaction = order.transaction.ok_or_else(|| {
            RelayerError::DexError("Failed to get transaction from Jupiter order".to_string())
        })?;

        let mut swap_tx =
            VersionedTransaction::try_from(EncodedSerializedTransaction::new(encoded_transaction))
                .map_err(|e| {
                    RelayerError::DexError(format!("Failed to decode swap transaction: {}", e))
                })?;

        let signature = self
            .signer
            .sign(&swap_tx.message.serialize())
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Failed to sign transaction: {}", e))
                // TODO improve error handling
            })?;

        swap_tx.signatures[0] = signature;

        info!("Execute order transaction");
        let serialized_transaction =
            EncodedSerializedTransaction::try_from(&swap_tx).map_err(|e| {
                RelayerError::DexError(format!("Failed to serialize transaction: {}", e))
            })?;
        let response = self
            .jupiter_service
            .execute_ultra_order(UltraExecuteRequest {
                signed_transaction: serialized_transaction.into_inner(),
                request_id: order.request_id,
            })
            .await
            .map_err(|e| RelayerError::DexError(format!("Failed to execute order: {}", e)))?;
        info!("Order executed successfully, response: {:?}", response);

        Ok(SwapResult {
            source_amount: params.amount,
            destination_amount: order.out_amount.clone(),
            transaction_signature: signature.to_string(),
        })
    }
}
