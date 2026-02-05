mod stellar_relayer;
pub use stellar_relayer::*;

mod gas_abstraction;
mod token_swap;

pub mod xdr_utils;
pub use xdr_utils::*;

pub use crate::services::stellar_dex::StellarDexServiceTrait;

use std::sync::Arc;

use crate::{
    constants::{
        get_default_soroswap_factory, get_default_soroswap_router, STELLAR_HORIZON_MAINNET_URL,
        STELLAR_HORIZON_TESTNET_URL,
    },
    jobs::JobProducerTrait,
    models::{
        NetworkRepoModel, NetworkType, RelayerError, RelayerRepoModel, SignerRepoModel,
        StellarNetwork, StellarSwapStrategy, TransactionRepoModel,
    },
    repositories::{
        NetworkRepository, RelayerRepository, Repository, TransactionCounterTrait,
        TransactionRepository,
    },
    services::{
        provider::get_network_provider,
        signer::StellarSignerFactory,
        stellar_dex::{DexServiceWrapper, OrderBookService, SoroswapService, StellarDexService},
        TransactionCounterService,
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

    // Create signer once and wrap in Arc for shared use
    let stellar_signer = Arc::new(StellarSignerFactory::create_stellar_signer(&signer.into())?);

    let transaction_counter_service = Arc::new(TransactionCounterService::new(
        relayer.id.clone(),
        relayer.address.clone(),
        transaction_counter_store,
    ));

    // Create DEX services based on configured strategies
    let horizon_url = network.horizon_url.clone().unwrap_or_else(|| {
        if network.is_testnet() {
            STELLAR_HORIZON_TESTNET_URL.to_string()
        } else {
            STELLAR_HORIZON_MAINNET_URL.to_string()
        }
    });
    let provider_arc = Arc::new(provider.clone());
    let signer_arc = stellar_signer.clone();

    // Get strategies from policy (default to OrderBook if none specified)
    let strategies = relayer
        .policies
        .get_stellar_policy()
        .get_swap_config()
        .and_then(|config| {
            if config.strategies.is_empty() {
                None
            } else {
                Some(config.strategies.clone())
            }
        })
        .unwrap_or_else(|| vec![StellarSwapStrategy::OrderBook]);

    // Create DEX services for each strategy
    // Type parameters are inferred from provider and signer_arc
    let mut dex_services: Vec<DexServiceWrapper<_, _>> = Vec::new();
    for strategy in &strategies {
        match strategy {
            StellarSwapStrategy::OrderBook => {
                let order_book_service = Arc::new(
                    OrderBookService::new(
                        horizon_url.clone(),
                        provider_arc.clone(),
                        signer_arc.clone(),
                    )
                    .map_err(|e| {
                        RelayerError::NetworkConfiguration(format!(
                            "Failed to create OrderBook DEX service: {e}"
                        ))
                    })?,
                );
                dex_services.push(DexServiceWrapper::OrderBook(order_book_service));
            }
            StellarSwapStrategy::Soroswap => {
                // Get Soroswap router address from server config, falling back to default
                let router_address =
                    crate::config::ServerConfig::get_stellar_soroswap_router_address()
                        .unwrap_or_else(|| {
                            get_default_soroswap_router(network.is_testnet()).to_string()
                        });

                // Get Soroswap factory address from server config, falling back to default
                let factory_address =
                    crate::config::ServerConfig::get_stellar_soroswap_factory_address()
                        .unwrap_or_else(|| {
                            get_default_soroswap_factory(network.is_testnet()).to_string()
                        });

                // Get native wrapper address from server config if configured
                let native_wrapper_address =
                    crate::config::ServerConfig::get_stellar_soroswap_native_wrapper_address();

                let soroswap_service = Arc::new(SoroswapService::new(
                    router_address,
                    factory_address,
                    native_wrapper_address,
                    provider_arc.clone(),
                    network.passphrase.clone(),
                    network.is_testnet(),
                ));
                dex_services.push(DexServiceWrapper::Soroswap(soroswap_service));
                tracing::info!("Soroswap DEX service initialized");
            }
        }
    }

    // Create multi-strategy DEX service with the configured strategies
    let dex_service = Arc::new(StellarDexService::new(dex_services));

    let relayer = DefaultStellarRelayer::<J, TR, NR, RR, TCR>::new(
        relayer,
        stellar_signer.clone(),
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
