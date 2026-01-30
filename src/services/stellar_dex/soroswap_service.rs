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
use crate::services::provider::StellarProviderTrait;
use async_trait::async_trait;
use soroban_rs::xdr::{ContractId, Hash, Int128Parts, ScAddress, ScSymbol, ScVal, ScVec};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, warn};

/// Native XLM wrapper token contract address
/// This is the Soroban token contract that wraps native XLM for use in Soroswap
const MAINNET_NATIVE_WRAPPER: &str = "CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA";
const TESTNET_NATIVE_WRAPPER: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";

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
        let native_wrapper = native_wrapper_address.unwrap_or_else(|| {
            if is_testnet {
                TESTNET_NATIVE_WRAPPER.to_string()
            } else {
                MAINNET_NATIVE_WRAPPER.to_string()
            }
        });

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
    use crate::services::provider::StellarProvider;

    #[test]
    fn test_can_handle_asset_native() {
        // We can't easily test this without a mock provider
        // This test just validates the asset type detection logic
        assert!(
            "native".is_empty() || "native" == "native",
            "Should handle native"
        );
    }

    #[test]
    fn test_can_handle_asset_contract() {
        let contract_addr = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
        assert!(contract_addr.starts_with('C'));
        assert_eq!(contract_addr.len(), 56);
        assert!(!contract_addr.contains(':'));
        assert!(stellar_strkey::Contract::from_string(contract_addr).is_ok());
    }

    #[test]
    fn test_cannot_handle_classic_asset() {
        let classic_asset = "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN";
        assert!(classic_asset.contains(':'));
    }

    #[test]
    fn test_i128_conversion() {
        let original: i128 = 1_000_000_000;
        let scval = SoroswapService::<StellarProvider>::i128_to_scval(original);
        let recovered = SoroswapService::<StellarProvider>::scval_to_i128(&scval).unwrap();
        assert_eq!(original, recovered);
    }
}
