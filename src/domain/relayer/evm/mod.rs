/// The `evm` module provides functionality for interacting with
/// Ethereum Virtual Machine (EVM) based blockchains. It includes
/// the `evm_relayer` submodule which contains the core logic for
/// relaying transactions and events between different EVM networks.
mod evm_relayer;
mod nonce;
mod rpc_utils;
mod validations;

pub use evm_relayer::*;
pub use rpc_utils::*;
pub use validations::*;

use crate::{
    jobs::{JobProducerError, JobProducerTrait, TransactionStatusCheck},
    models::{EvmNetwork, TransactionRepoModel},
    utils::calculate_scheduled_timestamp,
};

/// Enqueues the first status check for an EVM transaction, timed by the
/// network's `status_check` settings.
async fn schedule_initial_status_check<J: JobProducerTrait>(
    job_producer: &J,
    tx: &TransactionRepoModel,
    network: &EvmNetwork,
) -> Result<(), JobProducerError> {
    job_producer
        .produce_check_transaction_status_job(
            TransactionStatusCheck::for_evm_network(&tx.id, &tx.relayer_id, network),
            Some(calculate_scheduled_timestamp(
                network.status_check_initial_delay_seconds() as i64,
            )),
        )
        .await
}
