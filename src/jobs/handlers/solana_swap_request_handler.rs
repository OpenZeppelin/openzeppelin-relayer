//! Solana swap request handling worker implementation.
//!
//! This module implements the solana token swap request handling worker that processes
//! notification jobs from the queue.

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, *};
use eyre::Result;
use log::info;

use crate::{
    constants::WORKER_DEFAULT_MAXIMUM_RETRIES,
    domain::{create_solana_relayer, get_relayer_by_id, SolanaRelayerDexTrait},
    jobs::{handle_result, Job, SolanaTokenSwapRequest},
    models::DefaultAppState,
    repositories::Repository,
};

/// Handles incoming swap jobs from the queue.
///
/// # Arguments
/// * `job` - The notification job containing recipient and message details
/// * `context` - Application state containing notification services
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of notification processing
pub async fn solana_token_swap_request_handler(
    job: Job<SolanaTokenSwapRequest>,
    context: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling solana token swap request: {:?}", job.data);

    let result = handle_request(job.data, context).await;

    handle_result(
        result,
        attempt,
        "SolanaTokenSwapRequest",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

#[derive(Default, Debug, Clone)]
pub struct CronReminder();

/// Handles incoming swap jobs from the cron queue.
pub async fn solana_token_swap_cron_handler(
    job: CronReminder,
    relayer_id: Data<String>,
    data: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> Result<(), Error> {
    info!("handling solana token swap cron request: {:?}", job);

    let result = handle_request(
        SolanaTokenSwapRequest {
            relayer_id: relayer_id.to_string(),
        },
        data,
    )
    .await;

    handle_result(
        result,
        attempt,
        "SolanaTokenSwapRequest",
        WORKER_DEFAULT_MAXIMUM_RETRIES,
    )
}

async fn handle_request(
    request: SolanaTokenSwapRequest,
    context: Data<ThinData<DefaultAppState>>,
) -> Result<()> {
    info!("handling solana token swap request: {:?}", request);

    let relayer_model = get_relayer_by_id(request.relayer_id.clone(), &context).await?;
    let signer_model = context
        .signer_repository
        .get_by_id(relayer_model.signer_id.clone())
        .await?;

    let relayer = create_solana_relayer(
        relayer_model,
        signer_model,
        context.relayer_repository(),
        context.network_repository(),
        context.transaction_repository(),
        context.job_producer(),
    )
    .await?;

    relayer
        .handle_token_swap_request(request.relayer_id.clone())
        .await
        .map_err(|e| eyre::eyre!("Failed to handle solana token swap request: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {}
