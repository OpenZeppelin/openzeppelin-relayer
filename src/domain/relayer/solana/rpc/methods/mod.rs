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

#[cfg(test)]
mod test_setup;
#[cfg(test)]
use mockall::automock;

use std::sync::Arc;

#[cfg(test)]
pub use test_setup::*;

// Re-export validation types from transaction domain module
pub use crate::domain::transaction::solana::{
    SolanaTransactionValidationError, SolanaTransactionValidator,
};

use crate::{
    jobs::{JobProducer, JobProducerTrait},
    models::{NetworkRepoModel, RelayerRepoModel, TransactionRepoModel},
    repositories::{Repository, TransactionRepository, TransactionRepositoryStorage},
    services::{provider::SolanaProviderTrait, signer::SolanaSignTrait, JupiterServiceTrait},
};

use super::*;

#[cfg(test)]
use crate::{jobs::MockJobProducerTrait, repositories::MockTransactionRepository};

#[cfg(test)]
use crate::services::{
    provider::MockSolanaProviderTrait, signer::MockSolanaSignTrait, MockJupiterServiceTrait,
};
use async_trait::async_trait;

use crate::{
    models::{
        SolanaFeeEstimateRequestParams, SolanaFeeEstimateResult,
        SolanaGetFeaturesEnabledRequestParams, SolanaGetFeaturesEnabledResult,
        SolanaGetSupportedTokensRequestParams, SolanaGetSupportedTokensResult,
        SolanaPrepareTransactionRequestParams, SolanaPrepareTransactionResult,
        SolanaSignAndSendTransactionRequestParams, SolanaSignAndSendTransactionResult,
        SolanaSignTransactionRequestParams, SolanaSignTransactionResult,
        SolanaTransferTransactionRequestParams, SolanaTransferTransactionResult,
    },
    services::{provider::SolanaProvider, signer::SolanaSigner, JupiterService},
};

#[cfg_attr(test, automock)]
#[async_trait]
pub trait SolanaRpcMethods: Send + Sync {
    async fn fee_estimate(
        &self,
        request: SolanaFeeEstimateRequestParams,
    ) -> Result<SolanaFeeEstimateResult, SolanaRpcError>;
    async fn transfer_transaction(
        &self,
        request: SolanaTransferTransactionRequestParams,
    ) -> Result<SolanaTransferTransactionResult, SolanaRpcError>;
    async fn prepare_transaction(
        &self,
        request: SolanaPrepareTransactionRequestParams,
    ) -> Result<SolanaPrepareTransactionResult, SolanaRpcError>;
    async fn sign_transaction(
        &self,
        request: SolanaSignTransactionRequestParams,
    ) -> Result<SolanaSignTransactionResult, SolanaRpcError>;
    async fn sign_and_send_transaction(
        &self,
        request: SolanaSignAndSendTransactionRequestParams,
    ) -> Result<SolanaSignAndSendTransactionResult, SolanaRpcError>;
    async fn get_supported_tokens(
        &self,
        request: SolanaGetSupportedTokensRequestParams,
    ) -> Result<SolanaGetSupportedTokensResult, SolanaRpcError>;
    async fn get_features_enabled(
        &self,
        request: SolanaGetFeaturesEnabledRequestParams,
    ) -> Result<SolanaGetFeaturesEnabledResult, SolanaRpcError>;
}

pub type DefaultProvider = SolanaProvider;
pub type DefaultSigner = SolanaSigner;
pub type DefaultJupiterService = JupiterService;
pub type DefaultJobProducer = JobProducer;
pub type DefaultTransactionRepository = TransactionRepositoryStorage;

#[cfg(test)]
impl
    SolanaRpcMethodsImpl<
        MockSolanaProviderTrait,
        MockSolanaSignTrait,
        MockJupiterServiceTrait,
        MockJobProducerTrait,
        MockTransactionRepository,
    >
{
    pub fn new_mock(
        relayer: RelayerRepoModel,
        network: NetworkRepoModel,
        provider: Arc<MockSolanaProviderTrait>,
        signer: Arc<MockSolanaSignTrait>,
        jupiter_service: Arc<MockJupiterServiceTrait>,
        job_producer: Arc<MockJobProducerTrait>,
        transaction_repository: Arc<MockTransactionRepository>,
    ) -> Self {
        Self {
            relayer,
            network,
            provider,
            signer,
            jupiter_service,
            job_producer,
            transaction_repository,
        }
    }
}

pub struct SolanaRpcMethodsImpl<P, S, J, JP, TR>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
    JP: JobProducerTrait + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
{
    pub(crate) relayer: RelayerRepoModel,
    pub(crate) network: NetworkRepoModel,
    pub(crate) provider: Arc<P>,
    pub(crate) signer: Arc<S>,
    pub(crate) jupiter_service: Arc<J>,
    pub(crate) job_producer: Arc<JP>,
    pub(crate) transaction_repository: Arc<TR>,
}

pub type DefaultSolanaRpcMethodsImpl = SolanaRpcMethodsImpl<
    DefaultProvider,
    DefaultSigner,
    DefaultJupiterService,
    DefaultJobProducer,
    DefaultTransactionRepository,
>;

impl<P, S, J, JP, TR> SolanaRpcMethodsImpl<P, S, J, JP, TR>
where
    P: SolanaProviderTrait + Send + Sync + 'static,
    S: SolanaSignTrait + Send + Sync + 'static,
    J: JupiterServiceTrait + Send + Sync + 'static,
    JP: JobProducerTrait + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
{
    pub fn new(
        relayer: RelayerRepoModel,
        network: NetworkRepoModel,
        provider: Arc<P>,
        signer: Arc<S>,
        jupiter_service: Arc<J>,
        job_producer: Arc<JP>,
        transaction_repository: Arc<TR>,
    ) -> Self {
        Self {
            relayer,
            network,
            provider,
            signer,
            jupiter_service,
            job_producer,
            transaction_repository,
        }
    }
}

#[async_trait]
impl<P, S, J, JP, TR> SolanaRpcMethods for SolanaRpcMethodsImpl<P, S, J, JP, TR>
where
    P: SolanaProviderTrait + Send + Sync,
    S: SolanaSignTrait + Send + Sync,
    J: JupiterServiceTrait + Send + Sync,
    JP: JobProducerTrait + Send + Sync,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
{
    async fn fee_estimate(
        &self,
        params: SolanaFeeEstimateRequestParams,
    ) -> Result<SolanaFeeEstimateResult, SolanaRpcError> {
        self.fee_estimate_impl(params).await
    }

    async fn prepare_transaction(
        &self,
        params: SolanaPrepareTransactionRequestParams,
    ) -> Result<SolanaPrepareTransactionResult, SolanaRpcError> {
        self.prepare_transaction_impl(params).await
    }

    async fn sign_transaction(
        &self,
        params: SolanaSignTransactionRequestParams,
    ) -> Result<SolanaSignTransactionResult, SolanaRpcError> {
        self.sign_transaction_impl(params).await
    }

    async fn sign_and_send_transaction(
        &self,
        params: SolanaSignAndSendTransactionRequestParams,
    ) -> Result<SolanaSignAndSendTransactionResult, SolanaRpcError> {
        self.sign_and_send_transaction_impl(params).await
    }

    async fn transfer_transaction(
        &self,
        params: SolanaTransferTransactionRequestParams,
    ) -> Result<SolanaTransferTransactionResult, SolanaRpcError> {
        self.transfer_transaction_impl(params).await
    }

    async fn get_supported_tokens(
        &self,
        params: SolanaGetSupportedTokensRequestParams,
    ) -> Result<SolanaGetSupportedTokensResult, SolanaRpcError> {
        self.get_supported_tokens_impl(params).await
    }

    async fn get_features_enabled(
        &self,
        params: SolanaGetFeaturesEnabledRequestParams,
    ) -> Result<SolanaGetFeaturesEnabledResult, SolanaRpcError> {
        self.get_features_enabled_impl(params).await
    }
}
