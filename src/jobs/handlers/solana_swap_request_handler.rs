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
    domain::get_network_relayer,
    jobs::{handle_result, Job, JobProducer, SolanaTokenSwapRequest},
    models::AppState,
};

/// Handles incoming notification jobs from the queue.
///
/// # Arguments
/// * `job` - The notification job containing recipient and message details
/// * `context` - Application state containing notification services
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of notification processing
pub async fn solana_token_swap_request_handler(
    job: Job<SolanaTokenSwapRequest>,
    context: Data<ThinData<AppState<JobProducer>>>,
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

async fn handle_request(
    request: SolanaTokenSwapRequest,
    context: Data<ThinData<AppState<JobProducer>>>,
) -> Result<()> {
    info!("handling solana token swap request: {:?}", request);

    let relayer = get_network_relayer(request.relayer_id, &context).await?;

    Ok(())
}

#[cfg(test)]
mod tests {}
