//! JupiterSwapDex
//!
//! Implements the `DexStrategy` trait to perform Solana token swaps via the
//! Jupiter Swap REST API. This module handles:
//!  1. Fetching a swap quote from Jupiter.
//!  2. Building the swap transaction.
//!  3. Decoding and signing the transaction.
//!  4. Sending the signed transaction on-chain.
//!  5. Confirming transaction execution.
use std::sync::Arc;

use super::{DexStrategy, SwapParams, SwapResult};
use crate::domain::relayer::RelayerError;
use crate::models::{EncodedSerializedTransaction, JupiterSwapOptions};
use crate::services::{
    JupiterService, JupiterServiceTrait, PrioritizationFeeLamports, PriorityLevelWitMaxLamports,
    QuoteRequest, SolanaProvider, SolanaProviderError, SolanaProviderTrait, SolanaSignTrait,
    SolanaSigner, SwapRequest,
};
use async_trait::async_trait;
use log::info;
use solana_sdk::transaction::VersionedTransaction;

pub struct JupiterSwapDex<P, S, J>
where
    P: SolanaProviderTrait + 'static,
    S: SolanaSignTrait + 'static,
    J: JupiterServiceTrait + 'static,
{
    provider: Arc<P>,
    signer: Arc<S>,
    jupiter_service: Arc<J>,
    jupiter_swap_options: Option<JupiterSwapOptions>,
}

pub type DefaultJupiterSwapDex = JupiterSwapDex<SolanaProvider, SolanaSigner, JupiterService>;

impl<P, S, J> JupiterSwapDex<P, S, J>
where
    P: SolanaProviderTrait + 'static,
    S: SolanaSignTrait + 'static,
    J: JupiterServiceTrait + 'static,
{
    pub fn new(
        provider: Arc<P>,
        signer: Arc<S>,
        jupiter_service: Arc<J>,
        jupiter_swap_options: Option<JupiterSwapOptions>,
    ) -> Self {
        Self {
            provider,
            signer,
            jupiter_service,
            jupiter_swap_options,
        }
    }
}

#[async_trait]
impl<P, S, J> DexStrategy for JupiterSwapDex<P, S, J>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
{
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError> {
        info!("Executing Jupiter swap: {:?}", params);

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

        let swap_tx = self
            .jupiter_service
            .get_swap_transaction(SwapRequest {
                quote_response: quote.clone(),
                user_public_key: params.owner_address,
                wrap_and_unwrap_sol: Some(true),
                fee_account: None,
                compute_unit_price_micro_lamports: None,
                prioritization_fee_lamports: Some(PrioritizationFeeLamports {
                    priority_level_with_max_lamports: PriorityLevelWitMaxLamports {
                        max_lamports: self
                            .jupiter_swap_options
                            .as_ref()
                            .and_then(|o| o.priority_fee_max_lamports),
                        priority_level: self
                            .jupiter_swap_options
                            .as_ref()
                            .and_then(|o| o.priority_level.clone()),
                    },
                }),
                dynamic_compute_unit_limit: self
                    .jupiter_swap_options
                    .as_ref()
                    .map(|o| o.dynamic_compute_unit_limit.unwrap_or_default()),
            })
            .await
            .map_err(|e| {
                RelayerError::DexError(format!("Failed to get swap transaction: {}", e))
            })?;

        info!("Received swap transaction: {:?}", swap_tx);

        let mut swap_tx = VersionedTransaction::try_from(EncodedSerializedTransaction::new(
            swap_tx.swap_transaction,
        ))
        .map_err(|e| RelayerError::DexError(format!("Failed to decode swap transaction: {}", e)))?;
        let signature = self
            .signer
            .sign(&swap_tx.message.serialize())
            .await
            .map_err(|e| {
                RelayerError::DexError(format!("Failed to sign Dex transaction: {}", e))
            })?;

        swap_tx.signatures[0] = signature;

        let signature = self
            .provider
            .send_versioned_transaction(&swap_tx)
            .await
            .map_err(|e| match e {
                SolanaProviderError::RpcError(err) => {
                    RelayerError::ProviderError(format!("Failed to send transaction: {}", err))
                }
                _ => RelayerError::ProviderError(format!("Unexpected error: {}", e)),
            })?;

        // Wait for transaction confirmation
        info!("Waiting for transaction confirmation: {}", signature);
        self.provider
            .confirm_transaction(&signature)
            .await
            .map_err(|e| {
                RelayerError::ProviderError(format!("Transaction failed to confirm: {}", e))
            })?;

        info!("Transaction confirmed: {}", signature);

        Ok(SwapResult {
            mint: params.source_mint,
            source_amount: params.amount,
            destination_amount: quote.out_amount,
            transaction_signature: signature.to_string(),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::SignerError,
        services::{
            JupiterServiceError, MockJupiterServiceTrait, MockSolanaProviderTrait,
            MockSolanaSignTrait, QuoteResponse, RoutePlan, SwapInfo, SwapResponse,
        },
    };
    use solana_sdk::signature::Signature;
    use std::str::FromStr;

    fn create_mock_jupiter_service() -> MockJupiterServiceTrait {
        MockJupiterServiceTrait::new()
    }

    fn create_mock_solana_provider() -> MockSolanaProviderTrait {
        MockSolanaProviderTrait::new()
    }

    fn create_mock_solana_signer() -> MockSolanaSignTrait {
        MockSolanaSignTrait::new()
    }

    fn create_test_quote_response(
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        out_amount: u64,
    ) -> QuoteResponse {
        QuoteResponse {
            input_mint: input_mint.to_string(),
            output_mint: output_mint.to_string(),
            in_amount: amount,
            out_amount,
            other_amount_threshold: out_amount,
            price_impact_pct: 0.1,
            swap_mode: "ExactIn".to_string(),
            slippage_bps: 50, // 0.5%
            route_plan: vec![RoutePlan {
                swap_info: SwapInfo {
                    amm_key: "63mqrcydH89L7RhuMC3jLBojrRc2u3QWmjP4UrXsnotS".to_string(), // noboost
                    label: "Stabble Stable Swap".to_string(),
                    input_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                    output_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
                    in_amount: "1000000".to_string(),
                    out_amount: "999984".to_string(),
                    fee_amount: "10".to_string(),
                    fee_mint: "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB".to_string(),
                },
                percent: 1,
            }],
        }
    }

    fn create_test_swap_response(encoded_transaction: &str) -> SwapResponse {
        SwapResponse {
            swap_transaction: encoded_transaction.to_string(),
            last_valid_block_height: 123456789,
            prioritization_fee_lamports: Some(5000),
            compute_unit_limit: Some(20000),
            simulation_error: None,
        }
    }

    #[tokio::test]
    async fn test_execute_swap_success() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mut mock_solana_provider = create_mock_solana_provider();
        let mut mock_solana_signer = create_mock_solana_signer();

        let quote_response =
            create_test_quote_response(source_mint, destination_mint, amount, output_amount);

        let encoded_tx = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs";
        let swap_response = create_test_swap_response(encoded_tx);

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                let response = quote_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_jupiter_service
            .expect_get_swap_transaction()
            .times(1)
            .returning(move |_| {
                let response = swap_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_solana_signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_solana_provider
            .expect_send_versioned_transaction()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_solana_provider
            .expect_confirm_transaction()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(true) }));

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        assert!(
            result.is_ok(),
            "Swap should succeed, but got error: {:?}",
            result.err()
        );

        let swap_result = result.unwrap();
        assert_eq!(swap_result.source_amount, amount);
        assert_eq!(swap_result.destination_amount, output_amount);
        assert_eq!(
            swap_result.transaction_signature,
            test_signature.to_string()
        );
    }

    #[tokio::test]
    async fn test_execute_swap_get_quote_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mock_solana_provider = create_mock_solana_provider();
        let mock_solana_signer = create_mock_solana_signer();

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(crate::services::JupiterServiceError::ApiError {
                        message: "API error: insufficient liquidity".to_string(),
                    })
                })
            });

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        match result {
            Err(RelayerError::DexError(error_message)) => {
                assert!(
                    error_message.contains("Failed to get Jupiter quote")
                        && error_message.contains("insufficient liquidity"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected DexError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_get_transaction_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mock_solana_provider = create_mock_solana_provider();
        let mock_solana_signer = create_mock_solana_signer();

        let quote_response =
            create_test_quote_response(source_mint, destination_mint, amount, output_amount);

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                let response = quote_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_jupiter_service
            .expect_get_swap_transaction()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(JupiterServiceError::ApiError {
                        message: "Failed to prepare transaction: rate limit exceeded".to_string(),
                    })
                })
            });

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        match result {
            Err(RelayerError::DexError(error_message)) => {
                assert!(
                    error_message.contains("Failed to get swap transaction")
                        && error_message.contains("rate limit exceeded"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected DexError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_invalid_transaction_format() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mock_solana_provider = create_mock_solana_provider();
        let mock_solana_signer = create_mock_solana_signer();

        let quote_response =
            create_test_quote_response(source_mint, destination_mint, amount, output_amount);

        let swap_response = create_test_swap_response("invalid-transaction-format");

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                let response = quote_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_jupiter_service
            .expect_get_swap_transaction()
            .times(1)
            .returning(move |_| {
                let response = swap_response.clone();
                Box::pin(async move { Ok(response) })
            });

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        match result {
            Err(RelayerError::DexError(error_message)) => {
                assert!(
                    error_message.contains("Failed to decode swap transaction"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected DexError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_signing_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mock_solana_provider = create_mock_solana_provider();
        let mut mock_solana_signer = create_mock_solana_signer();

        let quote_response =
            create_test_quote_response(source_mint, destination_mint, amount, output_amount);

        let encoded_tx = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs";
        let swap_response = create_test_swap_response(encoded_tx);

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                let response = quote_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_jupiter_service
            .expect_get_swap_transaction()
            .times(1)
            .returning(move |_| {
                let response = swap_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_solana_signer
            .expect_sign()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(SignerError::SigningError(
                        "Failed to sign: invalid key".to_string(),
                    ))
                })
            });

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        match result {
            Err(RelayerError::DexError(error_message)) => {
                assert!(
                    error_message.contains("Failed to sign Dex transaction")
                        && error_message.contains("Failed to sign: invalid key"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected DexError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_send_transaction_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mut mock_solana_provider = create_mock_solana_provider();
        let mut mock_solana_signer = create_mock_solana_signer();

        let quote_response =
            create_test_quote_response(source_mint, destination_mint, amount, output_amount);

        let encoded_tx = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs";
        let swap_response = create_test_swap_response(encoded_tx);

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                let response = quote_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_jupiter_service
            .expect_get_swap_transaction()
            .times(1)
            .returning(move |_| {
                let response = swap_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_solana_signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_solana_provider
            .expect_send_versioned_transaction()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(SolanaProviderError::RpcError(
                        "Transaction simulation failed: Insufficient balance for spend".to_string(),
                    ))
                })
            });

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        match result {
            Err(RelayerError::ProviderError(error_message)) => {
                assert!(
                    error_message.contains("Failed to send transaction")
                        && error_message.contains("Insufficient balance"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected ProviderError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_confirm_transaction_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mut mock_solana_provider = create_mock_solana_provider();
        let mut mock_solana_signer = create_mock_solana_signer();

        let quote_response =
            create_test_quote_response(source_mint, destination_mint, amount, output_amount);

        let encoded_tx = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs";
        let swap_response = create_test_swap_response(encoded_tx);

        mock_jupiter_service
            .expect_get_quote()
            .times(1)
            .returning(move |_| {
                let response = quote_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_jupiter_service
            .expect_get_swap_transaction()
            .times(1)
            .returning(move |_| {
                let response = swap_response.clone();
                Box::pin(async move { Ok(response) })
            });

        mock_solana_signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_solana_provider
            .expect_send_versioned_transaction()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_solana_provider
            .expect_confirm_transaction()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(SolanaProviderError::RpcError(
                        "Transaction timed out".to_string(),
                    ))
                })
            });

        let dex = JupiterSwapDex::new(
            Arc::new(mock_solana_provider),
            Arc::new(mock_solana_signer),
            Arc::new(mock_jupiter_service),
            None,
        );

        let result = dex
            .execute_swap(SwapParams {
                owner_address: owner_address.to_string(),
                source_mint: source_mint.to_string(),
                destination_mint: destination_mint.to_string(),
                amount,
                slippage_percent: 0.5,
            })
            .await;

        match result {
            Err(RelayerError::ProviderError(error_message)) => {
                assert!(
                    error_message.contains("Transaction failed to confirm")
                        && error_message.contains("Transaction timed out"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected ProviderError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }
}
