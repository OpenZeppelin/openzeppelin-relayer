//! Retrieves a list of tokens supported by the relayer for fee payments.
//!
//! # Description
//!
//! This function queries the relayer for the tokens that are supported for fee payments. For
//! each token, it returns metadata including the token symbol, mint address, and the number
//! of decimal places supported.
//!
//! # Returns
//!
//! On success, returns a vector of [`GetSupportedTokensItem`] structures.
use log::info;

use crate::{
    constants::DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE,
    jobs::JobProducerTrait,
    models::{
        GetSupportedTokensItem, GetSupportedTokensRequestParams, GetSupportedTokensResult,
        TransactionRepoModel,
    },
    repositories::{Repository, TransactionRepository},
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::*;

impl<P, S, J, JP, TR> SolanaRpcMethodsImpl<P, S, J, JP, TR>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
{
    pub(crate) async fn get_supported_tokens_impl(
        &self,
        _params: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError> {
        info!("Processing get supported tokens request");

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
                        conversion_slippage_percentage: Some(
                            token
                                .swap_config
                                .as_ref()
                                .and_then(|config| config.slippage_percentage)
                                .unwrap_or(DEFAULT_CONVERSION_SLIPPAGE_PERCENTAGE),
                        ),
                    })
                    .collect()
            })
            .unwrap_or_default();

        info!(
            "Successfully handled request to get supported tokens: {:?}",
            tokens
        );

        Ok(GetSupportedTokensResult { tokens })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        domain::{setup_test_context, SolanaRpcMethodsImpl},
        models::{
            GetSupportedTokensRequestParams, RelayerNetworkPolicy, RelayerSolanaPolicy,
            SolanaAllowedTokensPolicy, SolanaAllowedTokensSwapConfig,
        },
        repositories::MockTransactionRepository,
    };

    #[tokio::test]
    async fn test_get_supported_tokens() {
        let (mut relayer, signer, provider, jupiter_service, _, job_producer, network) =
            setup_test_context();

        // Update relayer policy with some tokens
        relayer.policies = RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            allowed_tokens: Some(vec![
                SolanaAllowedTokensPolicy {
                    mint: "mint1".to_string(),
                    symbol: Some("TOKEN1".to_string()),
                    decimals: Some(9),
                    max_allowed_fee: Some(1000),
                    swap_config: Some(SolanaAllowedTokensSwapConfig {
                        ..Default::default()
                    }),
                },
                SolanaAllowedTokensPolicy {
                    mint: "mint2".to_string(),
                    symbol: Some("TOKEN2".to_string()),
                    decimals: Some(6),
                    max_allowed_fee: None,
                    swap_config: Some(SolanaAllowedTokensSwapConfig {
                        ..Default::default()
                    }),
                },
            ]),
            ..Default::default()
        });

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            network,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
            Arc::new(MockTransactionRepository::new()),
        );

        let result = rpc
            .get_supported_tokens_impl(GetSupportedTokensRequestParams {})
            .await;

        assert!(result.is_ok());
        let tokens = result.unwrap().tokens;
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].mint, "mint1");
        assert_eq!(tokens[0].symbol, "TOKEN1");
        assert_eq!(tokens[0].decimals, 9);
        assert_eq!(tokens[0].max_allowed_fee, Some(1000));
    }
}
