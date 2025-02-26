//! Retrieves a list of features enabled by the relayer.
//!
//! # Deprecated
//!
//! This method is deprecated. It is recommended to use more fine-grained methods for feature
//! detection.
//!
//! # Description
//!
//! This function returns a list of enabled features on the relayer.
//!
//! # Returns
//!
//! On success, returns a vector of strings where each string represents an enabled feature
//! (e.g., "gasless").
use crate::{
    jobs::JobProducerTrait,
    models::{GetFeaturesEnabledRequestParams, GetFeaturesEnabledResult},
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::*;

impl<P, S, J, JP> SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
{
    pub(crate) async fn get_features_enabled_impl(
        &self,
        _params: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError> {
        // gasless is enabled out of the box to be compliant with the spec
        Ok(GetFeaturesEnabledResult {
            features: vec!["gasless".to_string()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_features_enabled() {
        let (relayer, signer, provider, jupiter_service, _, job_producer) = setup_test_context();

        let rpc = SolanaRpcMethodsImpl::new_mock(
            relayer,
            Arc::new(provider),
            Arc::new(signer),
            Arc::new(jupiter_service),
            Arc::new(job_producer),
        );

        let result = rpc
            .get_features_enabled_impl(GetFeaturesEnabledRequestParams {})
            .await;

        assert!(result.is_ok(), "Should return Ok result");

        let features = result.unwrap().features;
        assert_eq!(features.len(), 1, "Should return exactly one feature");
        assert_eq!(
            features[0], "gasless",
            "Should return 'gasless' as enabled feature"
        );

        println!("Enabled features: {:?}", features);
    }
}
