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
    models::{RelayerError, RelayerRepoModel, SignerRepoModel},
    repositories::{
        InMemoryRelayerRepository, InMemoryTransactionCounter, InMemoryTransactionRepository,
        RelayerRepositoryStorage,
    },
    services::{get_solana_network_provider, JupiterService, SolanaSignerFactory},
};

pub fn create_solana_relayer(
    relayer: RelayerRepoModel,
    signer: SignerRepoModel,
    relayer_repository: Arc<RelayerRepositoryStorage<InMemoryRelayerRepository>>,
    transaction_repository: Arc<InMemoryTransactionRepository>,
    transaction_counter_store: Arc<InMemoryTransactionCounter>,
    job_producer: Arc<JobProducer>,
) -> Result<SolanaRelayer, RelayerError> {
    let provider = Arc::new(get_solana_network_provider(
        &relayer.network,
        relayer.custom_rpc_urls.clone(),
    )?);
    let signer_service = Arc::new(SolanaSignerFactory::create_solana_signer(&signer)?);
    let jupiter_service = JupiterService::new_from_network(relayer.network.as_str());
    let rpc_methods = SolanaRpcMethodsImpl::new(
        relayer.clone(),
        provider.clone(),
        signer_service.clone(),
        Arc::new(jupiter_service),
        job_producer.clone(),
    );
    let rpc_handler = Arc::new(SolanaRpcHandler::new(rpc_methods));
    let relayer = SolanaRelayer::new(
        relayer,
        signer_service,
        relayer_repository,
        provider,
        rpc_handler,
        transaction_repository,
        job_producer,
    )?;

    Ok(relayer)
}
