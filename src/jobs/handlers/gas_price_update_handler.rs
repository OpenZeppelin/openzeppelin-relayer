//! Gas price cache update worker handler
//!
//! This module defines the periodic worker used to refresh cached EVM gas
//! prices. It is scheduled via a Cron-based backend and registered per active
//! EVM network with gas price caching enabled. Each tick resolves the target
//! network by its `chain_id`, constructs a provider, and delegates the
//! one-shot update to the `GasPriceManager`.
use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use log::info;

use crate::services::provider::evm::EvmProviderTrait;
use crate::{
    jobs::handle_result, models::DefaultAppState, repositories::network::NetworkRepository,
    services::gas::manager::GasPriceManagerTrait,
};

/// Marker job representing a scheduled gas price update tick.
#[derive(Default, Debug, Clone)]
pub struct GasPriceTick;

/// Processes a scheduled gas price update tick for a specific EVM network.
///
/// - `job`: Tick marker sent by the Cron backend
/// - `chain_id`: Worker-scoped EVM `chain_id` for which the update runs
/// - `data`: Thin application state
/// - `attempt`: Current processing attempt provided by the worker runtime
///
/// Returns success or an error that determines retry behavior.
pub async fn gas_price_update_handler(
    job: GasPriceTick,
    chain_id: Data<u64>,
    data: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling gas price update tick: {:?}", job);

    let result = handle_tick(*chain_id, data).await;

    handle_result(result, attempt, "GasPriceUpdate", 3)
}

/// Performs a one-shot gas price cache refresh for the provided EVM `chain_id`.
///
/// Resolves the network, initializes a provider, fetches components, and stores
/// them in the manager cache.
async fn handle_tick(chain_id: u64, context: Data<ThinData<DefaultAppState>>) -> Result<()> {
    let network_repo = context.network_repository();
    let network_model = network_repo
        .get_by_chain_id(crate::models::NetworkType::Evm, chain_id)
        .await?
        .ok_or_else(|| eyre::eyre!("Network with chain_id {} not found", chain_id))?;

    let evm_network = crate::models::EvmNetwork::try_from(network_model)?;

    // Configure manager with network cache settings if present
    if let Some(cfg) = &evm_network.gas_price_cache {
        context
            .gas_price_manager
            .configure_network(chain_id, cfg.clone());
        if !cfg.enabled {
            return Ok(());
        }
    } else {
        return Ok(());
    }

    let provider = crate::services::get_network_provider(&evm_network, None)?;

    // Read jsonrpc gas price directly
    let gas_price = provider.get_gas_price().await?;

    // Base fee and fee history
    let block = provider.get_block_by_number().await?;
    let base_fee: u128 = block.header.base_fee_per_gas.unwrap_or(0).into();
    let fee_history = provider
        .get_fee_history(
            4,
            alloy::rpc::types::BlockNumberOrTag::Latest,
            vec![30.0, 50.0, 85.0, 99.0],
        )
        .await?;

    // Update cache from components
    context
        .gas_price_manager
        .update_from_components(chain_id, gas_price, base_fee, fee_history)
        .await;

    Ok(())
}
