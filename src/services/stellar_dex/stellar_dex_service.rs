//! Multi-strategy Stellar DEX service implementation
//!
//! This module provides a DEX service that automatically selects the appropriate strategy
//! based on asset type and configured strategies. It implements `StellarDexServiceTrait`
//! and internally routes calls to the first strategy that can handle the requested asset.

use super::{
    AssetType, OrderBookService, SoroswapService, StellarDexServiceError, StellarDexServiceTrait,
    StellarQuoteResponse, SwapExecutionResult, SwapTransactionParams,
};
use crate::services::{provider::StellarProviderTrait, signer::Signer, signer::StellarSignTrait};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::debug;

/// Enum wrapper for different DEX service implementations
///
/// This enum allows storing different concrete DEX service types in a collection
/// without using dynamic dispatch (`dyn`).
#[derive(Clone)]
pub enum DexServiceWrapper<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// Order Book DEX service (for classic Stellar assets)
    OrderBook(Arc<OrderBookService<P, S>>),
    /// Soroswap DEX service (for Soroban contract tokens)
    Soroswap(Arc<SoroswapService<P>>),
}

impl<P, S> DexServiceWrapper<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    fn can_handle_asset(&self, asset_id: &str) -> bool {
        match self {
            DexServiceWrapper::OrderBook(service) => service.can_handle_asset(asset_id),
            DexServiceWrapper::Soroswap(service) => service.can_handle_asset(asset_id),
        }
    }

    fn supported_asset_types(&self) -> HashSet<AssetType> {
        match self {
            DexServiceWrapper::OrderBook(service) => service.supported_asset_types(),
            DexServiceWrapper::Soroswap(service) => service.supported_asset_types(),
        }
    }
}

/// Multi-strategy Stellar DEX service
///
/// This service maintains a list of DEX strategy implementations (one per configured strategy)
/// and automatically selects the first one that can handle a given asset when methods are called.
/// The routing logic is integrated directly into the service implementation.
///
/// Uses static dispatch via generics and an enum wrapper instead of dynamic dispatch.
pub struct StellarDexService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// List of DEX strategy implementations in priority order (matching the configured strategies)
    strategies: Vec<DexServiceWrapper<P, S>>,
}

impl<P, S> StellarDexService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    /// Create a new multi-strategy DEX service with the given strategy implementations
    ///
    /// # Arguments
    /// * `strategies` - Vector of DEX strategy implementations in priority order
    pub fn new(strategies: Vec<DexServiceWrapper<P, S>>) -> Self {
        Self { strategies }
    }

    /// Find the first strategy that can handle the given asset
    ///
    /// # Arguments
    /// * `asset_id` - Asset identifier to check
    ///
    /// # Returns
    /// `Some(strategy)` if a strategy can handle the asset, `None` otherwise
    fn find_strategy_for_asset(&self, asset_id: &str) -> Option<&DexServiceWrapper<P, S>> {
        for strategy in &self.strategies {
            if strategy.can_handle_asset(asset_id) {
                debug!(
                    asset_id = %asset_id,
                    "Selected DEX strategy that can handle asset"
                );
                return Some(strategy);
            }
        }
        None
    }
}

#[async_trait]
impl<P, S> StellarDexServiceTrait for StellarDexService<P, S>
where
    P: StellarProviderTrait + Send + Sync + 'static,
    S: StellarSignTrait + Signer + Send + Sync + 'static,
{
    fn supported_asset_types(&self) -> HashSet<AssetType> {
        // Return the union of all supported asset types from all strategies
        let mut types = HashSet::new();
        for strategy in &self.strategies {
            types.extend(strategy.supported_asset_types());
        }
        types
    }

    fn can_handle_asset(&self, asset_id: &str) -> bool {
        // Check if any strategy can handle this asset
        self.find_strategy_for_asset(asset_id).is_some()
    }

    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        let strategy = self.find_strategy_for_asset(asset_id).ok_or_else(|| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "No configured strategy can handle asset: {asset_id}"
            ))
        })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => {
                svc.get_token_to_xlm_quote(asset_id, amount, slippage, asset_decimals)
                    .await
            }
            DexServiceWrapper::Soroswap(svc) => {
                svc.get_token_to_xlm_quote(asset_id, amount, slippage, asset_decimals)
                    .await
            }
        }
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        let strategy = self.find_strategy_for_asset(asset_id).ok_or_else(|| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "No configured strategy can handle asset: {asset_id}"
            ))
        })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => {
                svc.get_xlm_to_token_quote(asset_id, amount, slippage, asset_decimals)
                    .await
            }
            DexServiceWrapper::Soroswap(svc) => {
                svc.get_xlm_to_token_quote(asset_id, amount, slippage, asset_decimals)
                    .await
            }
        }
    }

    async fn prepare_swap_transaction(
        &self,
        params: SwapTransactionParams,
    ) -> Result<(String, StellarQuoteResponse), StellarDexServiceError> {
        let strategy = self
            .find_strategy_for_asset(&params.source_asset)
            .ok_or_else(|| {
                StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "No configured strategy can handle asset: {}",
                    params.source_asset
                ))
            })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => svc.prepare_swap_transaction(params).await,
            DexServiceWrapper::Soroswap(svc) => svc.prepare_swap_transaction(params).await,
        }
    }

    async fn execute_swap(
        &self,
        params: SwapTransactionParams,
    ) -> Result<SwapExecutionResult, StellarDexServiceError> {
        let strategy = self
            .find_strategy_for_asset(&params.source_asset)
            .ok_or_else(|| {
                StellarDexServiceError::InvalidAssetIdentifier(format!(
                    "No configured strategy can handle asset: {}",
                    params.source_asset
                ))
            })?;

        match strategy {
            DexServiceWrapper::OrderBook(svc) => svc.execute_swap(params).await,
            DexServiceWrapper::Soroswap(svc) => svc.execute_swap(params).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SignerError;
    use crate::services::provider::MockStellarProviderTrait;
    use crate::services::signer::{MockStellarSignTrait, Signer};
    use async_trait::async_trait;

    // ==================== Mock Setup ====================

    /// Combined mock that implements both StellarSignTrait and Signer
    struct MockCombinedSigner {
        stellar_mock: MockStellarSignTrait,
    }

    impl MockCombinedSigner {
        fn new() -> Self {
            Self {
                stellar_mock: MockStellarSignTrait::new(),
            }
        }
    }

    #[async_trait]
    impl StellarSignTrait for MockCombinedSigner {
        async fn sign_xdr_transaction(
            &self,
            unsigned_xdr: &str,
            network_passphrase: &str,
        ) -> Result<crate::domain::relayer::SignXdrTransactionResponseStellar, SignerError>
        {
            self.stellar_mock
                .sign_xdr_transaction(unsigned_xdr, network_passphrase)
                .await
        }
    }

    #[async_trait]
    impl Signer for MockCombinedSigner {
        async fn address(&self) -> Result<crate::models::Address, SignerError> {
            Ok(crate::models::Address::Stellar(
                "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            ))
        }

        async fn sign_transaction(
            &self,
            _transaction: crate::models::NetworkTransactionData,
        ) -> Result<crate::domain::SignTransactionResponse, SignerError> {
            Ok(crate::domain::SignTransactionResponse::Stellar(
                crate::domain::SignTransactionResponseStellar {
                    signature: crate::models::DecoratedSignature {
                        hint: soroban_rs::xdr::SignatureHint([0; 4]),
                        signature: soroban_rs::xdr::Signature(
                            soroban_rs::xdr::BytesM::try_from(vec![0u8; 64]).unwrap(),
                        ),
                    },
                },
            ))
        }
    }

    // Test constants
    const CLASSIC_ASSET: &str = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";
    const CONTRACT_ASSET: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";

    /// Create a test OrderBookService wrapped in Arc
    fn create_order_book_service(
    ) -> Arc<OrderBookService<MockStellarProviderTrait, MockCombinedSigner>> {
        let provider = Arc::new(MockStellarProviderTrait::new());
        let signer = Arc::new(MockCombinedSigner::new());
        Arc::new(
            OrderBookService::new(
                "https://horizon-testnet.stellar.org".to_string(),
                provider,
                signer,
            )
            .expect("Failed to create OrderBookService"),
        )
    }

    /// Create a test SoroswapService wrapped in Arc
    fn create_soroswap_service() -> Arc<SoroswapService<MockStellarProviderTrait>> {
        let provider = Arc::new(MockStellarProviderTrait::new());
        Arc::new(SoroswapService::new(
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA".to_string(),
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            provider,
            "Test SDF Network ; September 2015".to_string(),
        ))
    }

    /// Create a StellarDexService with both OrderBook and Soroswap strategies
    fn create_multi_strategy_service(
    ) -> StellarDexService<MockStellarProviderTrait, MockCombinedSigner> {
        let order_book = DexServiceWrapper::OrderBook(create_order_book_service());
        let soroswap = DexServiceWrapper::Soroswap(create_soroswap_service());
        StellarDexService::new(vec![order_book, soroswap])
    }

    /// Create a StellarDexService with only OrderBook strategy
    fn create_order_book_only_service(
    ) -> StellarDexService<MockStellarProviderTrait, MockCombinedSigner> {
        let order_book = DexServiceWrapper::OrderBook(create_order_book_service());
        StellarDexService::new(vec![order_book])
    }

    /// Create a StellarDexService with only Soroswap strategy
    fn create_soroswap_only_service(
    ) -> StellarDexService<MockStellarProviderTrait, MockCombinedSigner> {
        let soroswap = DexServiceWrapper::Soroswap(create_soroswap_service());
        StellarDexService::new(vec![soroswap])
    }

    /// Create a StellarDexService with no strategies
    fn create_empty_service() -> StellarDexService<MockStellarProviderTrait, MockCombinedSigner> {
        StellarDexService::new(vec![])
    }

    /// Create SwapTransactionParams for testing
    fn create_swap_params(source_asset: &str) -> SwapTransactionParams {
        SwapTransactionParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: source_asset.to_string(),
            destination_asset: "native".to_string(),
            amount: 1000000,
            slippage_percent: 1.0,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        }
    }

    // ==================== DexServiceWrapper Tests ====================

    #[test]
    fn test_wrapper_order_book_can_handle_native() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::OrderBook(create_order_book_service());
        assert!(wrapper.can_handle_asset("native"));
    }

    #[test]
    fn test_wrapper_order_book_can_handle_classic_asset() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::OrderBook(create_order_book_service());
        assert!(wrapper.can_handle_asset(CLASSIC_ASSET));
    }

    #[test]
    fn test_wrapper_order_book_cannot_handle_contract() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::OrderBook(create_order_book_service());
        assert!(!wrapper.can_handle_asset(CONTRACT_ASSET));
    }

    #[test]
    fn test_wrapper_soroswap_can_handle_native() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::Soroswap(create_soroswap_service());
        assert!(wrapper.can_handle_asset("native"));
    }

    #[test]
    fn test_wrapper_soroswap_can_handle_contract() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::Soroswap(create_soroswap_service());
        assert!(wrapper.can_handle_asset(CONTRACT_ASSET));
    }

    #[test]
    fn test_wrapper_soroswap_cannot_handle_classic_asset() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::Soroswap(create_soroswap_service());
        assert!(!wrapper.can_handle_asset(CLASSIC_ASSET));
    }

    #[test]
    fn test_wrapper_order_book_supported_asset_types() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::OrderBook(create_order_book_service());
        let types = wrapper.supported_asset_types();
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Classic));
        assert!(!types.contains(&AssetType::Contract));
    }

    #[test]
    fn test_wrapper_soroswap_supported_asset_types() {
        let wrapper: DexServiceWrapper<MockStellarProviderTrait, MockCombinedSigner> =
            DexServiceWrapper::Soroswap(create_soroswap_service());
        let types = wrapper.supported_asset_types();
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Contract));
        assert!(!types.contains(&AssetType::Classic));
    }

    // ==================== StellarDexService Constructor Tests ====================

    #[test]
    fn test_new_with_multiple_strategies() {
        let service = create_multi_strategy_service();
        assert_eq!(service.strategies.len(), 2);
    }

    #[test]
    fn test_new_with_single_strategy() {
        let service = create_order_book_only_service();
        assert_eq!(service.strategies.len(), 1);
    }

    #[test]
    fn test_new_with_empty_strategies() {
        let service = create_empty_service();
        assert_eq!(service.strategies.len(), 0);
    }

    // ==================== find_strategy_for_asset Tests ====================

    #[test]
    fn test_find_strategy_for_native_asset() {
        let service = create_multi_strategy_service();
        let strategy = service.find_strategy_for_asset("native");
        assert!(strategy.is_some());
    }

    #[test]
    fn test_find_strategy_for_classic_asset() {
        let service = create_multi_strategy_service();
        let strategy = service.find_strategy_for_asset(CLASSIC_ASSET);
        assert!(strategy.is_some());
        // Verify it's the OrderBook strategy
        assert!(matches!(strategy.unwrap(), DexServiceWrapper::OrderBook(_)));
    }

    #[test]
    fn test_find_strategy_for_contract_asset() {
        let service = create_multi_strategy_service();
        let strategy = service.find_strategy_for_asset(CONTRACT_ASSET);
        assert!(strategy.is_some());
        // Verify it's the Soroswap strategy
        assert!(matches!(strategy.unwrap(), DexServiceWrapper::Soroswap(_)));
    }

    #[test]
    fn test_find_strategy_returns_none_for_unhandled_asset() {
        let service = create_order_book_only_service();
        // Contract assets are not handled by OrderBook
        let strategy = service.find_strategy_for_asset(CONTRACT_ASSET);
        assert!(strategy.is_none());
    }

    #[test]
    fn test_find_strategy_returns_none_for_empty_service() {
        let service = create_empty_service();
        let strategy = service.find_strategy_for_asset("native");
        assert!(strategy.is_none());
    }

    #[test]
    fn test_find_strategy_priority_order() {
        // OrderBook comes first, so it should handle native assets
        let service = create_multi_strategy_service();
        let strategy = service.find_strategy_for_asset("native");
        assert!(strategy.is_some());
        // First strategy (OrderBook) should be selected for native
        assert!(matches!(strategy.unwrap(), DexServiceWrapper::OrderBook(_)));
    }

    // ==================== StellarDexServiceTrait::supported_asset_types Tests ====================

    #[test]
    fn test_supported_asset_types_union_of_all_strategies() {
        let service = create_multi_strategy_service();
        let types = service.supported_asset_types();
        // Should include Native, Classic (from OrderBook), and Contract (from Soroswap)
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Classic));
        assert!(types.contains(&AssetType::Contract));
        assert_eq!(types.len(), 3);
    }

    #[test]
    fn test_supported_asset_types_order_book_only() {
        let service = create_order_book_only_service();
        let types = service.supported_asset_types();
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Classic));
        assert!(!types.contains(&AssetType::Contract));
    }

    #[test]
    fn test_supported_asset_types_soroswap_only() {
        let service = create_soroswap_only_service();
        let types = service.supported_asset_types();
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Contract));
        assert!(!types.contains(&AssetType::Classic));
    }

    #[test]
    fn test_supported_asset_types_empty_service() {
        let service = create_empty_service();
        let types = service.supported_asset_types();
        assert!(types.is_empty());
    }

    // ==================== StellarDexServiceTrait::can_handle_asset Tests ====================

    #[test]
    fn test_can_handle_asset_native() {
        let service = create_multi_strategy_service();
        assert!(service.can_handle_asset("native"));
    }

    #[test]
    fn test_can_handle_asset_empty_string() {
        let service = create_multi_strategy_service();
        assert!(service.can_handle_asset(""));
    }

    #[test]
    fn test_can_handle_asset_classic() {
        let service = create_multi_strategy_service();
        assert!(service.can_handle_asset(CLASSIC_ASSET));
    }

    #[test]
    fn test_can_handle_asset_contract() {
        let service = create_multi_strategy_service();
        assert!(service.can_handle_asset(CONTRACT_ASSET));
    }

    #[test]
    fn test_cannot_handle_contract_with_order_book_only() {
        let service = create_order_book_only_service();
        assert!(!service.can_handle_asset(CONTRACT_ASSET));
    }

    #[test]
    fn test_cannot_handle_classic_with_soroswap_only() {
        let service = create_soroswap_only_service();
        assert!(!service.can_handle_asset(CLASSIC_ASSET));
    }

    #[test]
    fn test_cannot_handle_any_asset_with_empty_service() {
        let service = create_empty_service();
        assert!(!service.can_handle_asset("native"));
        assert!(!service.can_handle_asset(CLASSIC_ASSET));
        assert!(!service.can_handle_asset(CONTRACT_ASSET));
    }

    #[test]
    fn test_cannot_handle_invalid_asset() {
        let service = create_multi_strategy_service();
        assert!(!service.can_handle_asset("INVALID"));
        assert!(!service.can_handle_asset("random_string"));
    }

    // ==================== get_token_to_xlm_quote Error Tests ====================

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_no_strategy_error() {
        let service = create_empty_service();
        let result = service
            .get_token_to_xlm_quote("native", 1000000, 1.0, Some(7))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_unhandled_asset_error() {
        let service = create_order_book_only_service();
        // Contract assets are not handled by OrderBook
        let result = service
            .get_token_to_xlm_quote(CONTRACT_ASSET, 1000000, 1.0, Some(7))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
                assert!(msg.contains(CONTRACT_ASSET));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    // ==================== get_xlm_to_token_quote Error Tests ====================

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_no_strategy_error() {
        let service = create_empty_service();
        let result = service
            .get_xlm_to_token_quote("native", 1000000, 1.0, Some(7))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_unhandled_asset_error() {
        let service = create_soroswap_only_service();
        // Classic assets are not handled by Soroswap
        let result = service
            .get_xlm_to_token_quote(CLASSIC_ASSET, 1000000, 1.0, Some(7))
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
                assert!(msg.contains(CLASSIC_ASSET));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    // ==================== prepare_swap_transaction Error Tests ====================

    #[tokio::test]
    async fn test_prepare_swap_transaction_no_strategy_error() {
        let service = create_empty_service();
        let params = create_swap_params("native");
        let result = service.prepare_swap_transaction(params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    #[tokio::test]
    async fn test_prepare_swap_transaction_unhandled_asset_error() {
        let service = create_order_book_only_service();
        let params = create_swap_params(CONTRACT_ASSET);
        let result = service.prepare_swap_transaction(params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
                assert!(msg.contains(CONTRACT_ASSET));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    // ==================== execute_swap Error Tests ====================

    #[tokio::test]
    async fn test_execute_swap_no_strategy_error() {
        let service = create_empty_service();
        let params = create_swap_params("native");
        let result = service.execute_swap(params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    #[tokio::test]
    async fn test_execute_swap_unhandled_asset_error() {
        let service = create_soroswap_only_service();
        let params = create_swap_params(CLASSIC_ASSET);
        let result = service.execute_swap(params).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("No configured strategy can handle asset"));
                assert!(msg.contains(CLASSIC_ASSET));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }
}
