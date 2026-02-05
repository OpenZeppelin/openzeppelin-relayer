//! Soroswap DEX Service implementation
//!
//! Uses Soroswap AMM router contract for token swaps on Soroban.
//! This service handles swaps between Soroban token contracts (C... addresses) and XLM.
//!
//! The router contract provides `get_amounts_out` for quotes and
//! `swap_exact_tokens_for_tokens` for executing swaps.

use super::{
    AssetType, PathStep, StellarDexServiceError, StellarDexServiceTrait, StellarQuoteResponse,
    SwapExecutionResult, SwapTransactionParams,
};
use crate::constants::get_default_soroswap_native_wrapper;
use crate::services::provider::StellarProviderTrait;
use async_trait::async_trait;
use soroban_rs::xdr::{ContractId, Hash, Int128Parts, ScAddress, ScSymbol, ScVal, ScVec};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, warn};

/// Soroswap AMM DEX service for Soroban token swaps
///
/// This service uses Soroswap's router contract to:
/// - Get quotes by simulating `get_amounts_out`
/// - Execute swaps via `swap_exact_tokens_for_tokens`
pub struct SoroswapService<P>
where
    P: StellarProviderTrait + Send + Sync + 'static,
{
    /// Soroswap router contract address
    router_address: String,
    /// Soroswap factory contract address (required for get_amounts_out)
    factory_address: String,
    /// Native XLM wrapper token address
    native_wrapper_address: String,
    /// Stellar provider for contract calls
    provider: Arc<P>,
    /// Network passphrase for signing (used for swap execution)
    #[allow(dead_code)]
    network_passphrase: String,
}

impl<P> SoroswapService<P>
where
    P: StellarProviderTrait + Send + Sync + 'static,
{
    /// Create a new SoroswapService instance
    ///
    /// # Arguments
    ///
    /// * `router_address` - Soroswap router contract address
    /// * `factory_address` - Soroswap factory contract address (required for get_amounts_out)
    /// * `native_wrapper_address` - Optional native XLM wrapper token address (uses default if None)
    /// * `provider` - Stellar provider for contract calls
    /// * `network_passphrase` - Network passphrase
    /// * `is_testnet` - Whether this is testnet (affects default addresses)
    pub fn new(
        router_address: String,
        factory_address: String,
        native_wrapper_address: Option<String>,
        provider: Arc<P>,
        network_passphrase: String,
        is_testnet: bool,
    ) -> Self {
        let native_wrapper = native_wrapper_address
            .unwrap_or_else(|| get_default_soroswap_native_wrapper(is_testnet).to_string());

        Self {
            router_address,
            factory_address,
            native_wrapper_address: native_wrapper,
            provider,
            network_passphrase,
        }
    }

    /// Parse a Soroban contract address (C...) to ScAddress
    fn parse_contract_address(address: &str) -> Result<ScAddress, StellarDexServiceError> {
        let contract = stellar_strkey::Contract::from_string(address).map_err(|e| {
            StellarDexServiceError::InvalidAssetIdentifier(format!(
                "Invalid Soroban contract address '{address}': {e}"
            ))
        })?;

        Ok(ScAddress::Contract(ContractId(Hash(contract.0))))
    }

    /// Build a Vec<ScVal> path for router calls
    fn build_path(
        &self,
        from_token: &str,
        to_token: &str,
    ) -> Result<ScVal, StellarDexServiceError> {
        let from_addr = Self::parse_contract_address(from_token)?;
        let to_addr = Self::parse_contract_address(to_token)?;

        // Simple direct path: [from_token, to_token]
        let path_vec: ScVec = vec![ScVal::Address(from_addr), ScVal::Address(to_addr)]
            .try_into()
            .map_err(|_| {
                StellarDexServiceError::UnknownError("Failed to create path vector".to_string())
            })?;

        Ok(ScVal::Vec(Some(path_vec)))
    }

    /// Convert i128 to ScVal::I128
    fn i128_to_scval(amount: i128) -> ScVal {
        let hi = (amount >> 64) as i64;
        let lo = amount as u64;
        ScVal::I128(Int128Parts { hi, lo })
    }

    /// Extract i128 from ScVal::I128
    fn scval_to_i128(val: &ScVal) -> Result<i128, StellarDexServiceError> {
        match val {
            ScVal::I128(parts) => {
                let result = ((parts.hi as i128) << 64) | (parts.lo as i128);
                Ok(result)
            }
            _ => Err(StellarDexServiceError::UnknownError(
                "Expected I128 value from router".to_string(),
            )),
        }
    }

    /// Extract Vec<i128> from ScVal::Vec of I128s
    fn scval_to_amounts_vec(val: &ScVal) -> Result<Vec<i128>, StellarDexServiceError> {
        match val {
            ScVal::Vec(Some(sc_vec)) => {
                let mut amounts = Vec::new();
                for item in sc_vec.iter() {
                    amounts.push(Self::scval_to_i128(item)?);
                }
                Ok(amounts)
            }
            _ => Err(StellarDexServiceError::UnknownError(
                "Expected Vec of I128 values from router".to_string(),
            )),
        }
    }

    /// Call router.get_amounts_out to get quote
    ///
    /// Returns the expected output amounts for each step in the path
    /// Soroswap's get_amounts_out requires: (factory_address, amount_in, path)
    async fn call_get_amounts_out(
        &self,
        amount_in: i128,
        path: ScVal,
    ) -> Result<Vec<i128>, StellarDexServiceError> {
        let function_name = ScSymbol::try_from("get_amounts_out").map_err(|_| {
            StellarDexServiceError::UnknownError("Failed to create function symbol".to_string())
        })?;

        // Soroswap's get_amounts_out requires factory address as first argument
        let factory_addr = Self::parse_contract_address(&self.factory_address)?;
        let args = vec![
            ScVal::Address(factory_addr),
            Self::i128_to_scval(amount_in),
            path,
        ];

        debug!(
            router = %self.router_address,
            factory = %self.factory_address,
            amount_in = amount_in,
            "Calling Soroswap router get_amounts_out"
        );

        let result = self
            .provider
            .call_contract(&self.router_address, &function_name, args)
            .await
            .map_err(|e| StellarDexServiceError::ApiError {
                message: format!("Soroswap router call failed: {e}"),
            })?;

        Self::scval_to_amounts_vec(&result)
    }
}

#[async_trait]
impl<P> StellarDexServiceTrait for SoroswapService<P>
where
    P: StellarProviderTrait + Send + Sync + 'static,
{
    fn supported_asset_types(&self) -> HashSet<AssetType> {
        // Soroswap supports Soroban contract tokens and Native XLM (via wrapper)
        HashSet::from([AssetType::Native, AssetType::Contract])
    }

    fn can_handle_asset(&self, asset_id: &str) -> bool {
        // Handle native XLM (will use wrapper)
        if asset_id == "native" || asset_id.is_empty() {
            return true;
        }

        // Handle Soroban contract tokens (C... format, 56 chars)
        if asset_id.starts_with('C')
            && asset_id.len() == 56
            && !asset_id.contains(':')
            && stellar_strkey::Contract::from_string(asset_id).is_ok()
        {
            return true;
        }

        false
    }

    async fn get_token_to_xlm_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        _asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        // For native XLM, return 1:1
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * 100.0) as u32,
                path: None,
            });
        }

        // Build path: [token, native_wrapper]
        let path = self.build_path(asset_id, &self.native_wrapper_address)?;

        // Call router to get quote
        let amounts = self.call_get_amounts_out(amount as i128, path).await?;

        // Last amount is the output
        let out_amount = amounts
            .last()
            .copied()
            .ok_or_else(|| StellarDexServiceError::NoPathFound)?;

        if out_amount <= 0 {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // Safe conversion from i128 to u64 - we already checked out_amount > 0 above
        let out_amount_u64 = u64::try_from(out_amount).map_err(|_| {
            StellarDexServiceError::UnknownError(format!(
                "Output amount {out_amount} exceeds u64::MAX"
            ))
        })?;

        // Calculate price impact (simplified - assumes 1:1 expected ratio)
        // TODO: Use pool reserves for accurate price impact calculation
        let price_impact = if amount > 0 && out_amount_u64 > 0 {
            let expected_ratio = 1.0;
            let actual_ratio = out_amount_u64 as f64 / amount as f64;
            ((expected_ratio - actual_ratio).abs() / expected_ratio * 100.0).min(100.0)
        } else {
            0.0
        };

        debug!(
            asset = %asset_id,
            in_amount = amount,
            out_amount = out_amount_u64,
            "Soroswap quote: token -> XLM"
        );

        Ok(StellarQuoteResponse {
            input_asset: asset_id.to_string(),
            output_asset: "native".to_string(),
            in_amount: amount,
            out_amount: out_amount_u64,
            price_impact_pct: price_impact,
            slippage_bps: (slippage * 100.0) as u32,
            path: Some(vec![
                PathStep {
                    asset_code: Some(asset_id.to_string()),
                    asset_issuer: None,
                    amount,
                },
                PathStep {
                    asset_code: Some("native".to_string()),
                    asset_issuer: None,
                    amount: out_amount_u64,
                },
            ]),
        })
    }

    async fn get_xlm_to_token_quote(
        &self,
        asset_id: &str,
        amount: u64,
        slippage: f32,
        _asset_decimals: Option<u8>,
    ) -> Result<StellarQuoteResponse, StellarDexServiceError> {
        // For native XLM, return 1:1
        if asset_id == "native" || asset_id.is_empty() {
            return Ok(StellarQuoteResponse {
                input_asset: "native".to_string(),
                output_asset: "native".to_string(),
                in_amount: amount,
                out_amount: amount,
                price_impact_pct: 0.0,
                slippage_bps: (slippage * 100.0) as u32,
                path: None,
            });
        }

        // Build path: [native_wrapper, token]
        let path = self.build_path(&self.native_wrapper_address, asset_id)?;

        // Call router to get quote
        let amounts = self.call_get_amounts_out(amount as i128, path).await?;

        // Last amount is the output
        let out_amount = amounts
            .last()
            .copied()
            .ok_or_else(|| StellarDexServiceError::NoPathFound)?;

        if out_amount <= 0 {
            return Err(StellarDexServiceError::NoPathFound);
        }

        // Safe conversion from i128 to u64 - we already checked out_amount > 0 above
        let out_amount_u64 = u64::try_from(out_amount).map_err(|_| {
            StellarDexServiceError::UnknownError(format!(
                "Output amount {out_amount} exceeds u64::MAX"
            ))
        })?;

        // Calculate price impact (simplified - assumes 1:1 expected ratio)
        // TODO: Use pool reserves for accurate price impact calculation
        let price_impact = if amount > 0 && out_amount_u64 > 0 {
            let expected_ratio = 1.0;
            let actual_ratio = out_amount_u64 as f64 / amount as f64;
            ((expected_ratio - actual_ratio).abs() / expected_ratio * 100.0).min(100.0)
        } else {
            0.0
        };

        debug!(
            asset = %asset_id,
            in_amount = amount,
            out_amount = out_amount_u64,
            "Soroswap quote: XLM -> token"
        );

        Ok(StellarQuoteResponse {
            input_asset: "native".to_string(),
            output_asset: asset_id.to_string(),
            in_amount: amount,
            out_amount: out_amount_u64,
            price_impact_pct: price_impact,
            slippage_bps: (slippage * 100.0) as u32,
            path: Some(vec![
                PathStep {
                    asset_code: Some("native".to_string()),
                    asset_issuer: None,
                    amount,
                },
                PathStep {
                    asset_code: Some(asset_id.to_string()),
                    asset_issuer: None,
                    amount: out_amount_u64,
                },
            ]),
        })
    }

    async fn prepare_swap_transaction(
        &self,
        params: SwapTransactionParams,
    ) -> Result<(String, StellarQuoteResponse), StellarDexServiceError> {
        // For gas abstraction, we don't actually need to prepare a swap transaction
        // The FeeForwarder contract handles the token transfer
        // This method is mainly used for the relayer's own token swaps

        warn!("Soroswap prepare_swap_transaction is not yet fully implemented");

        // Get the quote first
        let quote = if params.destination_asset == "native" {
            self.get_token_to_xlm_quote(
                &params.source_asset,
                params.amount,
                params.slippage_percent,
                params.source_asset_decimals,
            )
            .await?
        } else if params.source_asset == "native" {
            self.get_xlm_to_token_quote(
                &params.destination_asset,
                params.amount,
                params.slippage_percent,
                params.destination_asset_decimals,
            )
            .await?
        } else {
            return Err(StellarDexServiceError::InvalidAssetIdentifier(
                "Soroswap currently only supports swaps involving native XLM".to_string(),
            ));
        };

        // TODO: Build the actual swap transaction XDR
        // For now, return empty transaction as placeholder
        let placeholder_xdr = String::new();

        Ok((placeholder_xdr, quote))
    }

    async fn execute_swap(
        &self,
        _params: SwapTransactionParams,
    ) -> Result<SwapExecutionResult, StellarDexServiceError> {
        // TODO: Implement actual swap execution
        // This requires building and submitting a Soroswap swap transaction

        warn!("Soroswap execute_swap is not yet implemented");

        Err(StellarDexServiceError::UnknownError(
            "Soroswap swap execution is not yet implemented".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{
        STELLAR_SOROSWAP_MAINNET_NATIVE_WRAPPER, STELLAR_SOROSWAP_TESTNET_NATIVE_WRAPPER,
    };
    use crate::services::provider::MockStellarProviderTrait;
    use futures::FutureExt;

    fn create_mock_provider() -> Arc<MockStellarProviderTrait> {
        Arc::new(MockStellarProviderTrait::new())
    }

    fn create_test_service(
        provider: Arc<MockStellarProviderTrait>,
        is_testnet: bool,
    ) -> SoroswapService<MockStellarProviderTrait> {
        SoroswapService::new(
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(), // router
            "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA".to_string(), // factory
            None,
            provider,
            "Test SDF Network ; September 2015".to_string(),
            is_testnet,
        )
    }

    // ==================== Constructor Tests ====================

    #[test]
    fn test_new_testnet_uses_testnet_native_wrapper() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        assert_eq!(
            service.native_wrapper_address,
            STELLAR_SOROSWAP_TESTNET_NATIVE_WRAPPER
        );
    }

    #[test]
    fn test_new_mainnet_uses_mainnet_native_wrapper() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, false);
        assert_eq!(
            service.native_wrapper_address,
            STELLAR_SOROSWAP_MAINNET_NATIVE_WRAPPER
        );
    }

    #[test]
    fn test_new_with_custom_native_wrapper() {
        let provider = create_mock_provider();
        let custom_wrapper = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHK3M".to_string();
        let service = SoroswapService::new(
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA".to_string(),
            Some(custom_wrapper.clone()),
            provider,
            "Test SDF Network ; September 2015".to_string(),
            true,
        );
        assert_eq!(service.native_wrapper_address, custom_wrapper);
    }

    // ==================== parse_contract_address Tests ====================

    #[test]
    fn test_parse_contract_address_valid() {
        let addr = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
        let result = SoroswapService::<MockStellarProviderTrait>::parse_contract_address(addr);
        assert!(result.is_ok());
        match result.unwrap() {
            ScAddress::Contract(_) => {}
            _ => panic!("Expected Contract address"),
        }
    }

    #[test]
    fn test_parse_contract_address_invalid_format() {
        let addr = "INVALID_ADDRESS";
        let result = SoroswapService::<MockStellarProviderTrait>::parse_contract_address(addr);
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("Invalid Soroban contract address"));
            }
            _ => panic!("Expected InvalidAssetIdentifier error"),
        }
    }

    #[test]
    fn test_parse_contract_address_stellar_account_not_contract() {
        // A valid Stellar account address (G...) but not a contract (C...)
        let addr = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let result = SoroswapService::<MockStellarProviderTrait>::parse_contract_address(addr);
        assert!(result.is_err());
    }

    // ==================== can_handle_asset Tests ====================

    #[test]
    fn test_can_handle_asset_native() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        assert!(service.can_handle_asset("native"));
    }

    #[test]
    fn test_can_handle_asset_empty_string() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        assert!(service.can_handle_asset(""));
    }

    #[test]
    fn test_can_handle_asset_valid_contract() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        let contract_addr = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
        assert!(service.can_handle_asset(contract_addr));
    }

    #[test]
    fn test_cannot_handle_classic_asset() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        let classic_asset = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";
        assert!(!service.can_handle_asset(classic_asset));
    }

    #[test]
    fn test_cannot_handle_short_address() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        assert!(!service.can_handle_asset("CSHORT"));
    }

    #[test]
    fn test_cannot_handle_non_c_prefix() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        // Stellar account address (G prefix)
        let addr = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        assert!(!service.can_handle_asset(addr));
    }

    #[test]
    fn test_cannot_handle_invalid_contract_checksum() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        // Valid format but invalid checksum
        let invalid_addr = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(!service.can_handle_asset(invalid_addr));
    }

    // ==================== supported_asset_types Tests ====================

    #[test]
    fn test_supported_asset_types() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        let types = service.supported_asset_types();
        assert!(types.contains(&AssetType::Native));
        assert!(types.contains(&AssetType::Contract));
        assert_eq!(types.len(), 2);
    }

    // ==================== i128 Conversion Tests ====================

    #[test]
    fn test_i128_to_scval_and_back_positive() {
        let original: i128 = 1_000_000_000;
        let scval = SoroswapService::<MockStellarProviderTrait>::i128_to_scval(original);
        let recovered = SoroswapService::<MockStellarProviderTrait>::scval_to_i128(&scval).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_i128_to_scval_and_back_zero() {
        let original: i128 = 0;
        let scval = SoroswapService::<MockStellarProviderTrait>::i128_to_scval(original);
        let recovered = SoroswapService::<MockStellarProviderTrait>::scval_to_i128(&scval).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_i128_to_scval_and_back_negative() {
        let original: i128 = -1_000_000_000;
        let scval = SoroswapService::<MockStellarProviderTrait>::i128_to_scval(original);
        let recovered = SoroswapService::<MockStellarProviderTrait>::scval_to_i128(&scval).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_i128_to_scval_and_back_large_positive() {
        let original: i128 = i128::MAX / 2;
        let scval = SoroswapService::<MockStellarProviderTrait>::i128_to_scval(original);
        let recovered = SoroswapService::<MockStellarProviderTrait>::scval_to_i128(&scval).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_i128_to_scval_and_back_large_negative() {
        let original: i128 = i128::MIN / 2;
        let scval = SoroswapService::<MockStellarProviderTrait>::i128_to_scval(original);
        let recovered = SoroswapService::<MockStellarProviderTrait>::scval_to_i128(&scval).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn test_scval_to_i128_wrong_type() {
        let scval = ScVal::Bool(true);
        let result = SoroswapService::<MockStellarProviderTrait>::scval_to_i128(&scval);
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::UnknownError(msg) => {
                assert!(msg.contains("Expected I128 value"));
            }
            _ => panic!("Expected UnknownError"),
        }
    }

    // ==================== scval_to_amounts_vec Tests ====================

    #[test]
    fn test_scval_to_amounts_vec_valid() {
        let amounts: Vec<i128> = vec![100, 200, 300];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let scval = ScVal::Vec(Some(sc_vec));

        let result =
            SoroswapService::<MockStellarProviderTrait>::scval_to_amounts_vec(&scval).unwrap();
        assert_eq!(result, vec![100, 200, 300]);
    }

    #[test]
    fn test_scval_to_amounts_vec_empty() {
        let sc_vec: ScVec = vec![].try_into().unwrap();
        let scval = ScVal::Vec(Some(sc_vec));

        let result =
            SoroswapService::<MockStellarProviderTrait>::scval_to_amounts_vec(&scval).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_scval_to_amounts_vec_wrong_type() {
        let scval = ScVal::Bool(true);
        let result = SoroswapService::<MockStellarProviderTrait>::scval_to_amounts_vec(&scval);
        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::UnknownError(msg) => {
                assert!(msg.contains("Expected Vec of I128 values"));
            }
            _ => panic!("Expected UnknownError"),
        }
    }

    #[test]
    fn test_scval_to_amounts_vec_none() {
        let scval = ScVal::Vec(None);
        let result = SoroswapService::<MockStellarProviderTrait>::scval_to_amounts_vec(&scval);
        assert!(result.is_err());
    }

    #[test]
    fn test_scval_to_amounts_vec_mixed_types() {
        // Vec containing a non-I128 value
        let sc_vec: ScVec = vec![ScVal::Bool(true)].try_into().unwrap();
        let scval = ScVal::Vec(Some(sc_vec));

        let result = SoroswapService::<MockStellarProviderTrait>::scval_to_amounts_vec(&scval);
        assert!(result.is_err());
    }

    // ==================== build_path Tests ====================

    #[test]
    fn test_build_path_valid() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        let from = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
        let to = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";

        let result = service.build_path(from, to);
        assert!(result.is_ok());
        match result.unwrap() {
            ScVal::Vec(Some(vec)) => {
                assert_eq!(vec.len(), 2);
            }
            _ => panic!("Expected Vec"),
        }
    }

    #[test]
    fn test_build_path_invalid_from() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        let result = service.build_path(
            "INVALID",
            "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_build_path_invalid_to() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);
        let result = service.build_path(
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
            "INVALID",
        );
        assert!(result.is_err());
    }

    // ==================== Async Quote Tests ====================

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_native_returns_1_to_1() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);

        let quote = service
            .get_token_to_xlm_quote("native", 1_000_000, 0.5, None)
            .await
            .unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, 1_000_000);
        assert_eq!(quote.out_amount, 1_000_000);
        assert_eq!(quote.price_impact_pct, 0.0);
        assert_eq!(quote.slippage_bps, 50);
        assert!(quote.path.is_none());
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_empty_returns_1_to_1() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);

        let quote = service
            .get_token_to_xlm_quote("", 1_000_000, 1.0, None)
            .await
            .unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, quote.out_amount);
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_native_returns_1_to_1() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);

        let quote = service
            .get_xlm_to_token_quote("native", 1_000_000, 0.5, None)
            .await
            .unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.in_amount, 1_000_000);
        assert_eq!(quote.out_amount, 1_000_000);
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_empty_returns_1_to_1() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);

        let quote = service
            .get_xlm_to_token_quote("", 500_000, 0.25, None)
            .await
            .unwrap();

        assert_eq!(quote.input_asset, "native");
        assert_eq!(quote.output_asset, "native");
        assert_eq!(quote.slippage_bps, 25);
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_with_mock_provider() {
        let mut mock = MockStellarProviderTrait::new();

        // Build expected output - amounts vec with input and output
        let amounts: Vec<i128> = vec![1_000_000, 950_000];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let quote = service
            .get_token_to_xlm_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(quote.in_amount, 1_000_000);
        assert_eq!(quote.out_amount, 950_000);
        assert_eq!(quote.output_asset, "native");
        assert!(quote.path.is_some());
        assert_eq!(quote.path.as_ref().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_get_xlm_to_token_quote_with_mock_provider() {
        let mut mock = MockStellarProviderTrait::new();

        let amounts: Vec<i128> = vec![1_000_000, 1_050_000];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let quote = service
            .get_xlm_to_token_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await
            .unwrap();

        assert_eq!(quote.in_amount, 1_000_000);
        assert_eq!(quote.out_amount, 1_050_000);
        assert_eq!(quote.input_asset, "native");
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_empty_amounts_returns_no_path() {
        let mut mock = MockStellarProviderTrait::new();

        // Return empty amounts vec
        let sc_vec: ScVec = vec![].try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let result = service
            .get_token_to_xlm_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::NoPathFound => {}
            e => panic!("Expected NoPathFound error, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_zero_output_returns_no_path() {
        let mut mock = MockStellarProviderTrait::new();

        let amounts: Vec<i128> = vec![1_000_000, 0];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let result = service
            .get_token_to_xlm_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::NoPathFound => {}
            e => panic!("Expected NoPathFound error, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_negative_output_returns_no_path() {
        let mut mock = MockStellarProviderTrait::new();

        let amounts: Vec<i128> = vec![1_000_000, -100];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let result = service
            .get_token_to_xlm_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::NoPathFound => {}
            e => panic!("Expected NoPathFound error, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_token_to_xlm_quote_provider_error() {
        let mut mock = MockStellarProviderTrait::new();

        mock.expect_call_contract().returning(|_, _, _| {
            async move {
                Err(crate::services::provider::ProviderError::Other(
                    "Connection failed".to_string(),
                ))
            }
            .boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let result = service
            .get_token_to_xlm_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::ApiError { message } => {
                assert!(message.contains("router call failed"));
            }
            e => panic!("Expected ApiError, got {:?}", e),
        }
    }

    // ==================== prepare_swap_transaction Tests ====================

    #[tokio::test]
    async fn test_prepare_swap_transaction_token_to_native() {
        let mut mock = MockStellarProviderTrait::new();

        let amounts: Vec<i128> = vec![1_000_000, 950_000];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let params = SwapTransactionParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            destination_asset: "native".to_string(),
            amount: 1_000_000,
            slippage_percent: 0.5,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: None,
        };

        let (xdr, quote) = service.prepare_swap_transaction(params).await.unwrap();

        assert!(xdr.is_empty()); // Placeholder
        assert_eq!(quote.out_amount, 950_000);
    }

    #[tokio::test]
    async fn test_prepare_swap_transaction_native_to_token() {
        let mut mock = MockStellarProviderTrait::new();

        let amounts: Vec<i128> = vec![1_000_000, 1_050_000];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let params = SwapTransactionParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: "native".to_string(),
            destination_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
                .to_string(),
            amount: 1_000_000,
            slippage_percent: 0.5,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: None,
            destination_asset_decimals: Some(7),
        };

        let (xdr, quote) = service.prepare_swap_transaction(params).await.unwrap();

        assert!(xdr.is_empty()); // Placeholder
        assert_eq!(quote.out_amount, 1_050_000);
    }

    #[tokio::test]
    async fn test_prepare_swap_transaction_token_to_token_not_supported() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);

        let params = SwapTransactionParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".to_string(),
            destination_asset: "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA"
                .to_string(),
            amount: 1_000_000,
            slippage_percent: 0.5,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: Some(7),
            destination_asset_decimals: Some(7),
        };

        let result = service.prepare_swap_transaction(params).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::InvalidAssetIdentifier(msg) => {
                assert!(msg.contains("only supports swaps involving native XLM"));
            }
            e => panic!("Expected InvalidAssetIdentifier, got {:?}", e),
        }
    }

    // ==================== execute_swap Tests ====================

    #[tokio::test]
    async fn test_execute_swap_not_implemented() {
        let provider = create_mock_provider();
        let service = create_test_service(provider, true);

        let params = SwapTransactionParams {
            source_account: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            source_asset: "native".to_string(),
            destination_asset: "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"
                .to_string(),
            amount: 1_000_000,
            slippage_percent: 0.5,
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            source_asset_decimals: None,
            destination_asset_decimals: Some(7),
        };

        let result = service.execute_swap(params).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            StellarDexServiceError::UnknownError(msg) => {
                assert!(msg.contains("not yet implemented"));
            }
            e => panic!("Expected UnknownError, got {:?}", e),
        }
    }

    // ==================== Price Impact Calculation Tests ====================

    #[tokio::test]
    async fn test_price_impact_calculation() {
        let mut mock = MockStellarProviderTrait::new();

        // 10% price impact: in 1_000_000, out 900_000
        let amounts: Vec<i128> = vec![1_000_000, 900_000];
        let sc_vals: Vec<ScVal> = amounts
            .iter()
            .map(|&a| SoroswapService::<MockStellarProviderTrait>::i128_to_scval(a))
            .collect();
        let sc_vec: ScVec = sc_vals.try_into().unwrap();
        let result_scval = ScVal::Vec(Some(sc_vec));

        mock.expect_call_contract().returning(move |_, _, _| {
            let result = result_scval.clone();
            async move { Ok(result) }.boxed()
        });

        let provider = Arc::new(mock);
        let service = create_test_service(provider, true);

        let quote = service
            .get_token_to_xlm_quote(
                "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                1_000_000,
                0.5,
                None,
            )
            .await
            .unwrap();

        // Price impact should be around 10%
        assert!(quote.price_impact_pct > 9.0 && quote.price_impact_pct < 11.0);
    }
}
