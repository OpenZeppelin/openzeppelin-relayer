mod stellar_relayer;
pub use stellar_relayer::*;

pub mod xdr_utils;
pub use xdr_utils::*;

// DEX functionality moved to services/stellar_dex
pub use crate::services::stellar_dex::StellarDexServiceTrait;

use std::sync::Arc;

use crate::{
    constants::{STELLAR_HORIZON_MAINNET_URL, STELLAR_HORIZON_TESTNET_URL},
    jobs::JobProducerTrait,
    models::{
        NetworkRepoModel, NetworkType, RelayerError, RelayerRepoModel, SignerRepoModel,
        StellarNetwork, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, RelayerRepository, Repository, TransactionCounterTrait,
        TransactionRepository,
    },
    services::{
        provider::get_network_provider, signer::StellarSignerFactory,
        stellar_dex::OrderBookService, TransactionCounterService,
    },
};

/// Function to create a Stellar relayer instance
pub async fn create_stellar_relayer<
    J: JobProducerTrait + 'static,
    TR: TransactionRepository + Repository<TransactionRepoModel, String> + Send + Sync + 'static,
    NR: NetworkRepository + Repository<NetworkRepoModel, String> + Send + Sync + 'static,
    RR: RelayerRepository + Repository<RelayerRepoModel, String> + Send + Sync + 'static,
    TCR: TransactionCounterTrait + Send + Sync + 'static,
>(
    relayer: RelayerRepoModel,
    signer: SignerRepoModel,
    relayer_repository: Arc<RR>,
    network_repository: Arc<NR>,
    transaction_repository: Arc<TR>,
    job_producer: Arc<J>,
    transaction_counter_store: Arc<TCR>,
) -> Result<DefaultStellarRelayer<J, TR, NR, RR, TCR>, RelayerError> {
    let network_repo = network_repository
        .get_by_name(NetworkType::Stellar, &relayer.network)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            RelayerError::NetworkConfiguration(format!("Network {} not found", relayer.network))
        })?;

    let network = StellarNetwork::try_from(network_repo.clone())?;
    let provider = get_network_provider(&network, relayer.custom_rpc_urls.clone())
        .map_err(|e| RelayerError::NetworkConfiguration(e.to_string()))?;
    let signer_service = StellarSignerFactory::create_stellar_signer(&signer.into())?;
    let transaction_counter_service = Arc::new(TransactionCounterService::new(
        relayer.id.clone(),
        relayer.address.clone(),
        transaction_counter_store,
    ));

    // Create DEX service for swap operations using Horizon API
    let horizon_url = network.horizon_url.clone().unwrap_or_else(|| {
        if network.is_testnet() {
            STELLAR_HORIZON_TESTNET_URL.to_string()
        } else {
            STELLAR_HORIZON_MAINNET_URL.to_string()
        }
    });
    let dex_service = Arc::new(OrderBookService::new(horizon_url).map_err(|e| {
        RelayerError::NetworkConfiguration(format!("Failed to create DEX service: {}", e))
    })?);

    let relayer = DefaultStellarRelayer::<J, TR, NR, RR, TCR>::new(
        relayer,
        signer_service,
        provider,
        StellarRelayerDependencies::new(
            relayer_repository,
            network_repository,
            transaction_repository,
            transaction_counter_service,
            job_producer,
        ),
        dex_service,
    )
    .await?;

    Ok(relayer)
}
