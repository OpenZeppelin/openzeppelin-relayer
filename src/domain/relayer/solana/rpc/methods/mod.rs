//! # Solana RPC Methods Module
//!
//! This module defines the `SolanaRpcMethods` trait which provides an asynchronous interface
//! for various Solana-specific RPC operations. These operations include fee estimation,
//! transaction processing (transfer, prepare, sign, and send), token retrieval, and feature
//! queries.
mod fee_estimate;
mod get_features_enabled;
mod get_supported_tokens;
mod prepare_transaction;
mod sign_and_send_transaction;
mod sign_transaction;
mod transfer_transaction;
mod utils;
mod validations;

#[cfg(test)]
mod test_setup;
#[cfg(test)]
use mockall::automock;

use std::sync::Arc;

#[cfg(test)]
pub use test_setup::*;
pub use validations::*;

use crate::{
    jobs::{JobProducer, JobProducerTrait},
    models::RelayerRepoModel,
    services::{JupiterServiceTrait, SolanaProviderTrait, SolanaSignTrait},
};

use super::*;

#[cfg(test)]
use crate::jobs::MockJobProducerTrait;

#[cfg(test)]
use crate::services::{MockJupiterServiceTrait, MockSolanaProviderTrait, MockSolanaSignTrait};
use async_trait::async_trait;

use crate::{
    models::{
        FeeEstimateRequestParams, FeeEstimateResult, GetFeaturesEnabledRequestParams,
        GetFeaturesEnabledResult, GetSupportedTokensRequestParams, GetSupportedTokensResult,
        PrepareTransactionRequestParams, PrepareTransactionResult,
        SignAndSendTransactionRequestParams, SignAndSendTransactionResult,
        SignTransactionRequestParams, SignTransactionResult, TransferTransactionRequestParams,
        TransferTransactionResult,
    },
    services::{JupiterService, SolanaProvider, SolanaSigner},
};

#[cfg_attr(test, automock)]
#[async_trait]
pub trait SolanaRpcMethods: Send + Sync {
    async fn fee_estimate(
        &self,
        request: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError>;
    async fn transfer_transaction(
        &self,
        request: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError>;
    async fn prepare_transaction(
        &self,
        request: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError>;
    async fn sign_transaction(
        &self,
        request: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError>;
    async fn sign_and_send_transaction(
        &self,
        request: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError>;
    async fn get_supported_tokens(
        &self,
        request: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError>;
    async fn get_features_enabled(
        &self,
        request: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError>;
}

pub type DefaultProvider = SolanaProvider;
pub type DefaultSigner = SolanaSigner;
pub type DefaultJupiterService = JupiterService;
pub type DefaultJobProducer = JobProducer;

#[cfg(test)]
impl
    SolanaRpcMethodsImpl<
        MockSolanaProviderTrait,
        MockSolanaSignTrait,
        MockJupiterServiceTrait,
        MockJobProducerTrait,
    >
{
    pub fn new_mock(
        relayer: RelayerRepoModel,
        provider: Arc<MockSolanaProviderTrait>,
        signer: Arc<MockSolanaSignTrait>,
        jupiter_service: Arc<MockJupiterServiceTrait>,
        job_producer: Arc<MockJobProducerTrait>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
            jupiter_service,
            job_producer,
        }
    }
}

pub struct SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
    JP: JobProducerTrait + Send + Sync + 'static,
{
    pub(crate) relayer: RelayerRepoModel,
    pub(crate) provider: Arc<P>,
    pub(crate) signer: Arc<S>,
    pub(crate) jupiter_service: Arc<J>,
    pub(crate) job_producer: Arc<JP>,
}

pub type DefaultSolanaRpcMethodsImpl =
    SolanaRpcMethodsImpl<DefaultProvider, DefaultSigner, DefaultJupiterService, DefaultJobProducer>;

impl<P, S, J, JP> SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
    JP: JobProducerTrait + Send + Sync + 'static,
{
    pub fn new(
        relayer: RelayerRepoModel,
        provider: Arc<P>,
        signer: Arc<S>,
        jupiter_service: Arc<J>,
        job_producer: Arc<JP>,
    ) -> Self {
        Self {
            relayer,
            provider,
            signer,
            jupiter_service,
            job_producer,
        }
    }
}

#[async_trait]
impl<P, S, J, JP> SolanaRpcMethods for SolanaRpcMethodsImpl<P, S, J, JP>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
{
    async fn fee_estimate(
        &self,
        params: FeeEstimateRequestParams,
    ) -> Result<FeeEstimateResult, SolanaRpcError> {
        self.fee_estimate_impl(params).await
    }

    async fn prepare_transaction(
        &self,
        params: PrepareTransactionRequestParams,
    ) -> Result<PrepareTransactionResult, SolanaRpcError> {
        self.prepare_transaction_impl(params).await
    }

    async fn sign_transaction(
        &self,
        params: SignTransactionRequestParams,
    ) -> Result<SignTransactionResult, SolanaRpcError> {
        self.sign_transaction_impl(params).await
    }

    async fn sign_and_send_transaction(
        &self,
        params: SignAndSendTransactionRequestParams,
    ) -> Result<SignAndSendTransactionResult, SolanaRpcError> {
        self.sign_and_send_transaction_impl(params).await
    }

    async fn transfer_transaction(
        &self,
        params: TransferTransactionRequestParams,
    ) -> Result<TransferTransactionResult, SolanaRpcError> {
        self.transfer_transaction_impl(params).await
    }

    async fn get_supported_tokens(
        &self,
        params: GetSupportedTokensRequestParams,
    ) -> Result<GetSupportedTokensResult, SolanaRpcError> {
        self.get_supported_tokens_impl(params).await
    }

    async fn get_features_enabled(
        &self,
        params: GetFeaturesEnabledRequestParams,
    ) -> Result<GetFeaturesEnabledResult, SolanaRpcError> {
        self.get_features_enabled_impl(params).await
    }
}
