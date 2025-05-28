//! JupiterUltraDex
//!
//! Implements the `DexStrategy` trait to perform Solana token swaps via the
//! Jupiter Ultra REST API. This module handles:
//!  1. Fetching an Ultra order from Jupiter.
//!  2. Decoding and signing the transaction.
//!  3. Serializing and executing the signed order via Jupiter Ultra.
//!  4. Returning the swap result as `SwapResult`.

use std::sync::Arc;

use super::{DexStrategy, SwapParams, SwapResult};
use crate::domain::relayer::RelayerError;
use crate::models::EncodedSerializedTransaction;
use crate::services::{
    JupiterService, JupiterServiceTrait, SolanaSignTrait, SolanaSigner, UltraExecuteRequest,
    UltraOrderRequest,
};
use async_trait::async_trait;
use log::info;
use solana_sdk::transaction::VersionedTransaction;

pub struct JupiterUltraDex<S, J>
where
    S: SolanaSignTrait + 'static,
    J: JupiterServiceTrait + 'static,
{
    signer: Arc<S>,
    jupiter_service: Arc<J>,
}

pub type DefaultJupiterUltraDex = JupiterUltraDex<SolanaSigner, JupiterService>;

impl<S, J> JupiterUltraDex<S, J>
where
    S: SolanaSignTrait + 'static,
    J: JupiterServiceTrait + 'static,
{
    pub fn new(signer: Arc<S>, jupiter_service: Arc<J>) -> Self {
        Self {
            signer,
            jupiter_service,
        }
    }
}

#[async_trait]
impl<S, J> DexStrategy for JupiterUltraDex<S, J>
where
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
{
    async fn execute_swap(&self, params: SwapParams) -> Result<SwapResult, RelayerError> {
        info!("Executing Jupiter swap using ultra api: {:?}", params);

        let order = self
            .jupiter_service
            .get_ultra_order(UltraOrderRequest {
                input_mint: params.source_mint.clone(),
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
                RelayerError::DexError(format!("Failed to sign Dex swap transaction: {}", e))
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
            mint: params.source_mint,
            source_amount: params.amount,
            destination_amount: order.out_amount,
            transaction_signature: response.signature.unwrap_or_default(),
            error: response.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::SignerError,
        services::{
            MockJupiterServiceTrait, MockSolanaSignTrait, RoutePlan, SwapEvents, SwapInfo,
            UltraExecuteResponse, UltraOrderResponse,
        },
    };
    use mockall::predicate;
    use solana_sdk::signature::Signature;
    use std::str::FromStr;

    fn create_mock_jupiter_service() -> MockJupiterServiceTrait {
        MockJupiterServiceTrait::new()
    }

    fn create_mock_solana_signer() -> MockSolanaSignTrait {
        MockSolanaSignTrait::new()
    }

    fn create_test_ultra_order_response(
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        out_amount: u64,
    ) -> UltraOrderResponse {
        UltraOrderResponse {
            input_mint: input_mint.to_string(),
            output_mint: output_mint.to_string(),
            in_amount: amount,
            out_amount,
            other_amount_threshold: out_amount,
            price_impact_pct: 0.1,
            swap_mode: "ExactIn".to_string(),
            slippage_bps: 50, // 0.5%
            route_plan: vec![RoutePlan {
                percent: 100,
                swap_info: SwapInfo {
                    amm_key: "test_amm_key".to_string(),
                    label: "Test".to_string(),
                    input_mint: input_mint.to_string(),
                    output_mint: output_mint.to_string(),
                    in_amount: amount.to_string(),
                    out_amount: out_amount.to_string(),
                    fee_amount: "1000".to_string(),
                    fee_mint: input_mint.to_string(),
                },
            }],
            prioritization_fee_lamports: 5000,
            transaction: Some("AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAQAKEZhsMunBegjHhwObzSrJeKhnl3sehIwqA8OCTejBJ/Z+O7sAR2gDS0+R1HXkqqjr0Wo3+auYeJQtq0il4DAumgiiHZpJZ1Uy9xq1yiOta3BcBOI7Dv+jmETs0W7Leny+AsVIwZWPN51bjn3Xk4uSzTFeAEom3HHY/EcBBpOfm7HkzWyukBvmNY5l9pnNxB/lTC52M7jy0Pxg6NhYJ37e1WXRYOFdoHOThs0hoFy/UG3+mVBbkR4sB9ywdKopv6IHO9+wuF/sV/02h9w+AjIBszK2bmCBPIrCZH4mqBdRcBFVAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABPS2wOQQj9KmokeOrgrMWdshu07fURwWLPYC0eDAkB+1Jh0UqsxbwO7GNdqHBaH3CjnuNams8L+PIsxs5JAZ16jJclj04kifG7PRApFI4NgwtaE5na/xCEBI572Nvp+FmsH4P9uc5VDeldVYzceVRhzPQ3SsaI7BOphAAiCnjaBgMGRm/lIRcy/+ytunLDm+e8jOW7xfcSayxDmzpAAAAAtD/6J/XX9kp0wJsfKVh53ksJqzbfyd1RSzIap7OM5ejnStls42Wf0xNRAChL93gEW4UQqPNOSYySLu5vwwX4aQR51VvyMcBu7nTFbs5oFQf9sbLeo/SOUQKxzaJWvBOPBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkGtJJ5s3DlXjsp517KoA8Lg71wC+tMHoDO9HDeQbotrwUMAAUCwFwVAAwACQOhzhsAAAAAAAoGAAQAIgcQAQEPOxAIAAUGAgQgIg8PDQ8hEg4JExEGARQUFAgQKAgmKgEDFhgXFSUnJCkQIywQIysIHSIqAh8DHhkbGhwLL8EgmzNB1pyBBwMAAAA6AWQAAU9kAQIvAABkAgNAQg8AAAAAAE3WYgAAAAAADwAAEAMEAAABCQMW8exZwhONJLLrrr9eKTOouI7XVrRLBjytPl3cL6rziwS+v7vCBB+8CQctooGHnRbQ3aoExfOLSH0uJhZijTPAKrJbYSJJ5hP1VwRmY2FlBkRkC2JtQsJRwDIR3Tbag/HLEdZxTPfqLWdCCyd0nco65bHdIoy/ByorMycoLzADMiYs".to_string()),
            request_id: "test-request-id".to_string(),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_success() {
        // Arrange
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        // Create mocks
        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mut mock_solana_signer = create_mock_solana_signer();

        let expected_order =
            create_test_ultra_order_response(source_mint, destination_mint, amount, output_amount);

        // Expected execute response
        let expected_execute_response = UltraExecuteResponse {
            signature: Some(test_signature.to_string()),
            status: "success".to_string(),
            slot: Some("123456789".to_string()),
            error: None,
            code: 0,
            total_input_amount: Some("1000000".to_string()),
            total_output_amount: Some("1000000".to_string()),
            input_amount_result: Some("1000000".to_string()),
            output_amount_result: Some("1000000".to_string()),
            swap_events: Some(vec![SwapEvents {
                input_mint: "mock_input_mint".to_string(),
                output_mint: "mock_output_mint".to_string(),
                input_amount: "1000000".to_string(),
                output_amount: "1000000".to_string(),
            }]),
        };

        mock_jupiter_service
            .expect_get_ultra_order()
            .with(predicate::function(move |req: &UltraOrderRequest| {
                req.input_mint == source_mint
                    && req.output_mint == destination_mint
                    && req.amount == amount
                    && req.taker == owner_address
            }))
            .times(1)
            .returning(move |_| {
                let order = expected_order.clone();
                Box::pin(async move { Ok(order) })
            });

        mock_solana_signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_jupiter_service
            .expect_execute_ultra_order()
            .with(predicate::function(move |req: &UltraExecuteRequest| {
                req.request_id == "test-request-id"
            }))
            .times(1)
            .returning(move |_| {
                let response = expected_execute_response.clone();
                Box::pin(async move { Ok(response) })
            });

        let dex =
            JupiterUltraDex::new(Arc::new(mock_solana_signer), Arc::new(mock_jupiter_service));

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
    async fn test_execute_swap_get_order_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mock_solana_signer = create_mock_solana_signer();

        mock_jupiter_service
            .expect_get_ultra_order()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(crate::services::JupiterServiceError::ApiError {
                        message: "API error: insufficient liquidity".to_string(),
                    })
                })
            });

        let dex =
            JupiterUltraDex::new(Arc::new(mock_solana_signer), Arc::new(mock_jupiter_service));

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
                    error_message.contains("Failed to get Jupiter Ultra order")
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
    async fn test_execute_swap_missing_transaction() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mock_solana_signer = create_mock_solana_signer();

        let mut order_response =
            create_test_ultra_order_response(source_mint, destination_mint, amount, output_amount);
        order_response.transaction = None; // Missing transaction

        mock_jupiter_service
            .expect_get_ultra_order()
            .times(1)
            .returning(move |_| {
                let order = order_response.clone();
                Box::pin(async move { Ok(order) })
            });

        let dex =
            JupiterUltraDex::new(Arc::new(mock_solana_signer), Arc::new(mock_jupiter_service));

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
                    error_message.contains("Failed to get transaction from Jupiter order"),
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
        let mock_solana_signer = create_mock_solana_signer();

        let mut order_response =
            create_test_ultra_order_response(source_mint, destination_mint, amount, output_amount);
        order_response.transaction = Some("invalid-transaction-format".to_string()); // Invalid format

        mock_jupiter_service
            .expect_get_ultra_order()
            .times(1)
            .returning(move |_| {
                let order = order_response.clone();
                Box::pin(async move { Ok(order) })
            });

        let dex =
            JupiterUltraDex::new(Arc::new(mock_solana_signer), Arc::new(mock_jupiter_service));

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
        let mut mock_solana_signer = create_mock_solana_signer();

        let expected_order =
            create_test_ultra_order_response(source_mint, destination_mint, amount, output_amount);

        mock_jupiter_service
            .expect_get_ultra_order()
            .times(1)
            .returning(move |_| {
                let order = expected_order.clone();
                Box::pin(async move { Ok(order) })
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

        let dex =
            JupiterUltraDex::new(Arc::new(mock_solana_signer), Arc::new(mock_jupiter_service));

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
                    error_message.contains("Failed to sign Dex swap transaction")
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
    async fn test_execute_swap_execution_error() {
        let source_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC
        let destination_mint = "So11111111111111111111111111111111111111112"; // SOL
        let amount = 1000000; // 1 USDC
        let output_amount = 24860952; // ~0.025 SOL
        let owner_address = "BFzfNx3UdatqpBX4zzJH9Cp7GQZpwc3Fg1aPgYbSgZyf";
        let test_signature = Signature::from_str("2jg9xbGLtZRsiJBrDWQnz33JuLjDkiKSZuxZPdjJ3qrJbMeTEerXFAKynkPW63J88nq63cvosDNRsg9VqHtGixvP").unwrap();

        let mut mock_jupiter_service = create_mock_jupiter_service();
        let mut mock_solana_signer = create_mock_solana_signer();

        let expected_order =
            create_test_ultra_order_response(source_mint, destination_mint, amount, output_amount);

        mock_jupiter_service
            .expect_get_ultra_order()
            .times(1)
            .returning(move |_| {
                let order = expected_order.clone();
                Box::pin(async move { Ok(order) })
            });

        mock_solana_signer
            .expect_sign()
            .times(1)
            .returning(move |_| Box::pin(async move { Ok(test_signature) }));

        mock_jupiter_service
            .expect_execute_ultra_order()
            .times(1)
            .returning(move |_| {
                Box::pin(async move {
                    Err(crate::services::JupiterServiceError::ApiError {
                        message: "Execution failed: price slippage too high".to_string(),
                    })
                })
            });

        let dex =
            JupiterUltraDex::new(Arc::new(mock_solana_signer), Arc::new(mock_jupiter_service));

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
                    error_message.contains("Failed to execute order")
                        && error_message.contains("price slippage too high"),
                    "Error message did not contain expected substrings: {}",
                    error_message
                );
            }
            Err(e) => panic!("Expected DexError but got different error: {:?}", e),
            Ok(_) => panic!("Expected error but got Ok"),
        }
    }
}
