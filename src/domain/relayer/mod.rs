//! # Relayer Domain Module
//!
//! This module contains the core domain logic for the relayer service.
//! It handles transaction submission, validation, and monitoring across
//! different blockchain networks.
//! ## Architecture
//!
//! The relayer domain is organized into network-specific implementations
//! that share common interfaces for transaction handling and monitoring.

use actix_web::web::ThinData;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[cfg(test)]
use mockall::automock;

use crate::{
    jobs::JobProducerTrait,
    models::{
        transaction::request::{
            SponsoredTransactionBuildRequest, SponsoredTransactionQuoteRequest,
        },
        AppState, DecoratedSignature, DeletePendingTransactionsResponse,
        EncodedSerializedTransaction, EvmNetwork, EvmTransactionDataSignature, JsonRpcRequest,
        JsonRpcResponse, NetworkRepoModel, NetworkRpcRequest, NetworkRpcResult,
        NetworkTransactionRequest, NetworkType, NotificationRepoModel, RelayerError,
        RelayerRepoModel, RelayerStatus, SignerRepoModel, SponsoredTransactionBuildResponse,
        SponsoredTransactionQuoteResponse, TransactionError, TransactionRepoModel,
    },
    repositories::{
        ApiKeyRepositoryTrait, NetworkRepository, PluginRepositoryTrait, RelayerRepository,
        Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::{
        provider::get_network_provider, signer::EvmSignerFactory, TransactionCounterService,
    },
};

use async_trait::async_trait;
use eyre::Result;

mod evm;
mod solana;
mod stellar;
mod util;

pub use evm::*;
pub use solana::*;
pub use stellar::*;
pub use util::*;

// Re-export SwapResult from solana module for use in Stellar
pub use solana::SwapResult;

/// The `Relayer` trait defines the core functionality required for a relayer
/// in the system. Implementors of this trait are responsible for handling
/// transaction requests, managing balances, and interacting with the network.
#[async_trait]
#[cfg_attr(test, automock)]
#[allow(dead_code)]
pub trait Relayer {
    /// Processes a transaction request and returns the result.
    ///
    /// # Arguments
    ///
    /// * `tx_request` - The transaction request to be processed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `TransactionRepoModel` on success, or a
    /// `RelayerError` on failure.
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError>;

    /// Retrieves the current balance of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `BalanceResponse` on success, or a
    /// `RelayerError` on failure.
    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError>;

    /// Deletes all pending transactions.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `DeletePendingTransactionsResponse` with details
    /// about which transactions were cancelled and which failed, or a `RelayerError` on failure.
    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError>;

    /// Signs data using the relayer's credentials.
    ///
    /// # Arguments
    ///
    /// * `request` - The data to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `SignDataResponse` on success, or a
    /// `RelayerError` on failure.
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError>;

    /// Signs typed data using the relayer's credentials.
    ///
    /// # Arguments
    ///
    /// * `request` - The typed data to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `SignDataResponse` on success, or a
    /// `RelayerError` on failure.
    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError>;

    /// Executes a JSON-RPC request.
    ///
    /// # Arguments
    ///
    /// * `request` - The JSON-RPC request to be executed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `JsonRpcResponse` on success, or a
    /// `RelayerError` on failure.
    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError>;

    /// Retrieves the current status of the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` containing `RelayerStatus` on success, or a
    /// `RelayerError` on failure.
    async fn get_status(&self) -> Result<RelayerStatus, RelayerError>;

    /// Initializes the relayer.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success, or a `RelayerError` on failure.
    async fn initialize_relayer(&self) -> Result<(), RelayerError>;

    /// Runs health checks on the relayer without side effects.
    ///
    /// This method performs all necessary health checks (RPC validation, balance checks, etc.)
    /// and returns the results without updating any state or sending notifications.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - All health checks passed
    /// * `Err(Vec<HealthCheckFailure>)` - One or more health checks failed with specific reasons
    async fn check_health(&self) -> Result<(), Vec<crate::models::HealthCheckFailure>>;

    /// Validates that the relayer's balance meets the minimum required.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success, or a `RelayerError` on failure.
    async fn validate_min_balance(&self) -> Result<(), RelayerError>;

    /// Signs a transaction using the relayer's credentials.
    ///
    /// # Arguments
    ///
    /// * `unsigned_xdr` - The unsigned transaction XDR string to be signed.
    ///
    /// # Returns
    ///
    /// A `Result` containing a `SignTransactionExternalResponse` on success, or a
    /// `RelayerError` on failure.
    async fn sign_transaction(
        &self,
        request: &SignTransactionRequest,
    ) -> Result<SignTransactionExternalResponse, RelayerError>;
}

/// Solana Relayer Dex Trait
/// Subset of methods for Solana relayer
#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait SolanaRelayerDexTrait {
    /// Handles a token swap request.
    async fn handle_token_swap_request(
        &self,
        relayer_id: String,
    ) -> Result<Vec<SwapResult>, RelayerError>;
}

/// Subset of methods for Stellar relayer
#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait StellarRelayerDexTrait {
    /// Handles a token swap request.
    async fn handle_token_swap_request(
        &self,
        relayer_id: String,
    ) -> Result<Vec<SwapResult>, RelayerError>;
}

/// Gas abstraction trait for relayers that support fee estimation and transaction preparation.
///
/// This trait provides a REST-friendly interface for gas abstraction operations,
/// allowing clients to estimate fees and prepare transactions without using JSON-RPC.
#[async_trait]
#[allow(dead_code)]
#[cfg_attr(test, automock)]
pub trait GasAbstractionTrait {
    /// Gets a quote for a gasless transaction.
    ///
    /// # Arguments
    ///
    /// * `params` - The gasless transaction quote request parameters (network-agnostic).
    ///
    /// # Returns
    ///
    /// A `Result` containing a fee estimate result on success, or a `RelayerError` on failure.
    async fn quote_sponsored_transaction(
        &self,
        params: SponsoredTransactionQuoteRequest,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError>;

    /// Prepares a transaction with fee payments.
    ///
    /// # Arguments
    ///
    /// * `params` - The prepare transaction request parameters (network-agnostic).
    ///
    /// # Returns
    ///
    /// A `Result` containing a prepare transaction result on success, or a `RelayerError` on failure.
    async fn build_sponsored_transaction(
        &self,
        params: SponsoredTransactionBuildRequest,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError>;
}

pub enum NetworkRelayer<
    J: JobProducerTrait + 'static,
    T: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
> {
    Evm(Box<DefaultEvmRelayer<J, T, RR, NR, TCR>>),
    Solana(DefaultSolanaRelayer<J, T, RR, NR>),
    Stellar(DefaultStellarRelayer<J, T, NR, RR, TCR>),
}

#[async_trait]
impl<
        J: JobProducerTrait + 'static,
        T: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
    > Relayer for NetworkRelayer<J, T, RR, NR, TCR>
{
    async fn process_transaction_request(
        &self,
        tx_request: NetworkTransactionRequest,
    ) -> Result<TransactionRepoModel, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.process_transaction_request(tx_request).await,
            NetworkRelayer::Solana(relayer) => {
                relayer.process_transaction_request(tx_request).await
            }
            NetworkRelayer::Stellar(relayer) => {
                relayer.process_transaction_request(tx_request).await
            }
        }
    }

    async fn get_balance(&self) -> Result<BalanceResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.get_balance().await,
            NetworkRelayer::Solana(relayer) => relayer.get_balance().await,
            NetworkRelayer::Stellar(relayer) => relayer.get_balance().await,
        }
    }

    async fn delete_pending_transactions(
        &self,
    ) -> Result<DeletePendingTransactionsResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.delete_pending_transactions().await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.delete_pending_transactions().await,
        }
    }

    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_data(request).await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.sign_data(request).await,
        }
    }

    async fn sign_typed_data(
        &self,
        request: SignTypedDataRequest,
    ) -> Result<SignDataResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.sign_typed_data(request).await,
            NetworkRelayer::Solana(_) => solana_not_supported_relayer(),
            NetworkRelayer::Stellar(relayer) => relayer.sign_typed_data(request).await,
        }
    }

    async fn rpc(
        &self,
        request: JsonRpcRequest<NetworkRpcRequest>,
    ) -> Result<JsonRpcResponse<NetworkRpcResult>, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.rpc(request).await,
            NetworkRelayer::Solana(relayer) => relayer.rpc(request).await,
            NetworkRelayer::Stellar(relayer) => relayer.rpc(request).await,
        }
    }

    async fn get_status(&self) -> Result<RelayerStatus, RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.get_status().await,
            NetworkRelayer::Solana(relayer) => relayer.get_status().await,
            NetworkRelayer::Stellar(relayer) => relayer.get_status().await,
        }
    }

    async fn validate_min_balance(&self) -> Result<(), RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.validate_min_balance().await,
            NetworkRelayer::Solana(relayer) => relayer.validate_min_balance().await,
            NetworkRelayer::Stellar(relayer) => relayer.validate_min_balance().await,
        }
    }

    async fn initialize_relayer(&self) -> Result<(), RelayerError> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.initialize_relayer().await,
            NetworkRelayer::Solana(relayer) => relayer.initialize_relayer().await,
            NetworkRelayer::Stellar(relayer) => relayer.initialize_relayer().await,
        }
    }

    async fn check_health(&self) -> Result<(), Vec<crate::models::HealthCheckFailure>> {
        match self {
            NetworkRelayer::Evm(relayer) => relayer.check_health().await,
            NetworkRelayer::Solana(relayer) => relayer.check_health().await,
            NetworkRelayer::Stellar(relayer) => relayer.check_health().await,
        }
    }

    async fn sign_transaction(
        &self,
        request: &SignTransactionRequest,
    ) -> Result<SignTransactionExternalResponse, RelayerError> {
        match self {
            NetworkRelayer::Evm(_) => Err(RelayerError::NotSupported(
                "sign_transaction not supported for EVM".to_string(),
            )),
            NetworkRelayer::Solana(relayer) => relayer.sign_transaction(request).await,
            NetworkRelayer::Stellar(relayer) => relayer.sign_transaction(request).await,
        }
    }
}

#[async_trait]
impl<
        J: JobProducerTrait + 'static,
        T: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
    > GasAbstractionTrait for NetworkRelayer<J, T, RR, NR, TCR>
{
    async fn quote_sponsored_transaction(
        &self,
        params: SponsoredTransactionQuoteRequest,
    ) -> Result<SponsoredTransactionQuoteResponse, RelayerError> {
        match params {
            SponsoredTransactionQuoteRequest::Solana(params) => match self {
                NetworkRelayer::Solana(relayer) => {
                    relayer
                        .quote_sponsored_transaction(SponsoredTransactionQuoteRequest::Solana(
                            params,
                        ))
                        .await
                }
                NetworkRelayer::Stellar(_) => Err(RelayerError::ValidationError(
                    "Solana request type does not match Stellar relayer type".to_string(),
                )),
                NetworkRelayer::Evm(_) => Err(RelayerError::NotSupported(
                    "Gas abstraction not supported for EVM relayers".to_string(),
                )),
            },
            SponsoredTransactionQuoteRequest::Stellar(params) => match self {
                NetworkRelayer::Stellar(relayer) => {
                    relayer
                        .quote_sponsored_transaction(SponsoredTransactionQuoteRequest::Stellar(
                            params,
                        ))
                        .await
                }
                NetworkRelayer::Solana(_) => Err(RelayerError::ValidationError(
                    "Stellar request type does not match Solana relayer type".to_string(),
                )),
                NetworkRelayer::Evm(_) => Err(RelayerError::NotSupported(
                    "Gas abstraction not supported for EVM relayers".to_string(),
                )),
            },
        }
    }

    async fn build_sponsored_transaction(
        &self,
        params: SponsoredTransactionBuildRequest,
    ) -> Result<SponsoredTransactionBuildResponse, RelayerError> {
        match params {
            SponsoredTransactionBuildRequest::Solana(params) => match self {
                NetworkRelayer::Solana(relayer) => {
                    relayer
                        .build_sponsored_transaction(SponsoredTransactionBuildRequest::Solana(
                            params,
                        ))
                        .await
                }
                NetworkRelayer::Stellar(_) => Err(RelayerError::ValidationError(
                    "Solana request type does not match Stellar relayer type".to_string(),
                )),
                NetworkRelayer::Evm(_) => Err(RelayerError::NotSupported(
                    "Gas abstraction not supported for EVM relayers".to_string(),
                )),
            },
            SponsoredTransactionBuildRequest::Stellar(params) => match self {
                NetworkRelayer::Stellar(relayer) => {
                    relayer
                        .build_sponsored_transaction(SponsoredTransactionBuildRequest::Stellar(
                            params,
                        ))
                        .await
                }
                NetworkRelayer::Solana(_) => Err(RelayerError::ValidationError(
                    "Stellar request type does not match Solana relayer type".to_string(),
                )),
                NetworkRelayer::Evm(_) => Err(RelayerError::NotSupported(
                    "Gas abstraction not supported for EVM relayers".to_string(),
                )),
            },
        }
    }
}

impl<
        J: JobProducerTrait + 'static,
        T: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
    > NetworkRelayer<J, T, RR, NR, TCR>
{
    /// Handles a token swap request for supported networks (Solana and Stellar).
    ///
    /// # Returns
    ///
    /// A `Result` containing a `Vec<SwapResult>` on success, or a `RelayerError` on failure.
    /// Returns `NotSupported` error for EVM networks.
    pub async fn handle_token_swap_request(
        &self,
        relayer_id: String,
    ) -> Result<Vec<SwapResult>, RelayerError> {
        match self {
            NetworkRelayer::Evm(_) => Err(RelayerError::NotSupported(
                "Token swap not supported for EVM relayers".to_string(),
            )),
            NetworkRelayer::Solana(relayer) => relayer.handle_token_swap_request(relayer_id).await,
            NetworkRelayer::Stellar(relayer) => relayer.handle_token_swap_request(relayer_id).await,
        }
    }
}

#[async_trait]
pub trait RelayerFactoryTrait<
    J: JobProducerTrait + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
    SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
    PR: PluginRepositoryTrait + Send + Sync + 'static,
    AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
>
{
    async fn create_relayer(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        state: &ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<NetworkRelayer<J, TR, RR, NR, TCR>, RelayerError>;
}

pub struct RelayerFactory;

#[async_trait]
impl<
        J: JobProducerTrait + 'static,
        TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
        RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
        NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
        NFR: Repository<NotificationRepoModel, String> + Send + Sync + 'static,
        SR: Repository<SignerRepoModel, String> + Send + Sync + 'static,
        TCR: TransactionCounterTrait + Send + Sync + 'static,
        PR: PluginRepositoryTrait + Send + Sync + 'static,
        AKR: ApiKeyRepositoryTrait + Send + Sync + 'static,
    > RelayerFactoryTrait<J, RR, TR, NR, NFR, SR, TCR, PR, AKR> for RelayerFactory
{
    async fn create_relayer(
        relayer: RelayerRepoModel,
        signer: SignerRepoModel,
        state: &ThinData<AppState<J, RR, TR, NR, NFR, SR, TCR, PR, AKR>>,
    ) -> Result<NetworkRelayer<J, TR, RR, NR, TCR>, RelayerError> {
        match relayer.network_type {
            NetworkType::Evm => {
                let network_repo = state
                    .network_repository()
                    .get_by_name(NetworkType::Evm, &relayer.network)
                    .await
                    .ok()
                    .flatten()
                    .ok_or_else(|| {
                        RelayerError::NetworkConfiguration(format!(
                            "Network {} not found",
                            relayer.network
                        ))
                    })?;

                let network = EvmNetwork::try_from(network_repo)?;

                let evm_provider = get_network_provider(&network, relayer.custom_rpc_urls.clone())?;
                let signer_service = EvmSignerFactory::create_evm_signer(signer.into()).await?;
                let transaction_counter_service = Arc::new(TransactionCounterService::new(
                    relayer.id.clone(),
                    relayer.address.clone(),
                    state.transaction_counter_store(),
                ));
                let relayer = DefaultEvmRelayer::new(
                    relayer,
                    signer_service,
                    evm_provider,
                    network,
                    state.relayer_repository(),
                    state.network_repository(),
                    state.transaction_repository(),
                    transaction_counter_service,
                    state.job_producer(),
                )?;

                Ok(NetworkRelayer::Evm(Box::new(relayer)))
            }
            NetworkType::Solana => {
                let solana_relayer = create_solana_relayer(
                    relayer,
                    signer,
                    state.relayer_repository(),
                    state.network_repository(),
                    state.transaction_repository(),
                    state.job_producer(),
                )
                .await?;
                Ok(NetworkRelayer::Solana(solana_relayer))
            }
            NetworkType::Stellar => {
                let stellar_relayer = create_stellar_relayer(
                    relayer,
                    signer,
                    state.relayer_repository(),
                    state.network_repository(),
                    state.transaction_repository(),
                    state.job_producer(),
                    state.transaction_counter_store(),
                )
                .await?;
                Ok(NetworkRelayer::Stellar(stellar_relayer))
            }
        }
    }
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignDataRequest {
    pub message: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignDataResponseEvm {
    pub r: String,
    pub s: String,
    pub v: u8,
    pub sig: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignDataResponseSolana {
    pub signature: String,
    pub public_key: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum SignDataResponse {
    Evm(SignDataResponseEvm),
    Solana(SignDataResponseSolana),
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SignTypedDataRequest {
    pub domain_separator: String,
    pub hash_struct_message: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SignTransactionRequestStellar {
    pub unsigned_xdr: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SignTransactionRequestSolana {
    pub transaction: EncodedSerializedTransaction,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum SignTransactionRequest {
    Stellar(SignTransactionRequestStellar),
    Evm(Vec<u8>),
    Solana(SignTransactionRequestSolana),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SignTransactionResponseEvm {
    pub hash: String,
    pub signature: EvmTransactionDataSignature,
    pub raw: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SignTransactionResponseStellar {
    pub signature: DecoratedSignature,
}

#[derive(Debug, Serialize, Deserialize, ToSchema, Clone)]
pub struct SignTransactionResponseSolana {
    pub transaction: EncodedSerializedTransaction,
    pub signature: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignXdrTransactionResponseStellar {
    pub signed_xdr: String,
    pub signature: DecoratedSignature,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum SignTransactionResponse {
    Evm(SignTransactionResponseEvm),
    Solana(SignTransactionResponseSolana),
    Stellar(SignTransactionResponseStellar),
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SignTransactionResponseStellar)]
pub struct SignTransactionExternalResponseStellar {
    pub signed_xdr: String,
    pub signature: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
#[schema(as = SignTransactionResponse)]
pub enum SignTransactionExternalResponse {
    Stellar(SignTransactionExternalResponseStellar),
    Evm(Vec<u8>),
    Solana(SignTransactionResponseSolana),
}

impl SignTransactionResponse {
    pub fn into_evm(self) -> Result<SignTransactionResponseEvm, TransactionError> {
        match self {
            SignTransactionResponse::Evm(e) => Ok(e),
            _ => Err(TransactionError::InvalidType(
                "Expected EVM signature".to_string(),
            )),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BalanceResponse {
    pub balance: u128,
    #[schema(example = "wei")]
    pub unit: String,
}
