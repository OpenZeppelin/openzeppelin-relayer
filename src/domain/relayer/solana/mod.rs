/// Module for Solana relayer functionality
mod solana_relayer;
use std::sync::Arc;

pub use solana_relayer::*;

/// Module for Solana RPC functionality
mod rpc;
pub use rpc::*;

mod dex;
pub use dex::*;

mod token;
pub use token::*;

use crate::{
    jobs::JobProducer,
    models::{NetworkType, RelayerError, RelayerRepoModel, SignerRepoModel, SolanaNetwork},
    repositories::{
        InMemoryNetworkRepository, InMemoryRelayerRepository, InMemoryTransactionRepository,
        RelayerRepositoryStorage,
    },
    services::{get_network_provider, JupiterService, SolanaSignerFactory},
};

/// Function to create a Solana relayer instance
pub async fn create_solana_relayer(
    relayer: RelayerRepoModel,
    signer: SignerRepoModel,
    relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
    network_repository: Arc<InMemoryNetworkRepository>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    job_producer: Arc<JobProducer>,
) -> Result<DefaultSolanaRelayer, RelayerError> {
    let network_repo = network_repository
        .get(NetworkType::Solana, &relayer.network)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            RelayerError::NetworkConfiguration(format!("Network {} not found", relayer.network))
        })?;

    let network = SolanaNetwork::try_from(network_repo)?;
    let provider = Arc::new(get_network_provider(
        &network,
        relayer.custom_rpc_urls.clone(),
    )?);
    let signer_service = Arc::new(SolanaSignerFactory::create_solana_signer(&signer)?);
    let jupiter_service = Arc::new(JupiterService::new_from_network(relayer.network.as_str()));
    let rpc_methods = SolanaRpcMethodsImpl::new(
        relayer.clone(),
        provider.clone(),
        signer_service.clone(),
        jupiter_service.clone(),
        job_producer.clone(),
    );
    let rpc_handler = Arc::new(SolanaRpcHandler::new(rpc_methods));
    let dex_service = create_network_dex_generic(
        &relayer,
        provider.clone(),
        signer_service.clone(),
        jupiter_service.clone(),
    )?;

    let relayer = DefaultSolanaRelayer::new(
        relayer,
        signer_service,
        relayer_repository,
        network_repository,
        provider,
        rpc_handler,
        transaction_repository,
        job_producer,
        Arc::new(dex_service),
    )
    .await?;

    Ok(relayer)
}
