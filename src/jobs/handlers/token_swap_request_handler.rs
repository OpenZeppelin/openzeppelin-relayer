//! Unified swap request handling worker implementation.
//!
//! This module implements the token swap request handling worker that processes
//! swap jobs from the queue for all supported networks (Solana and Stellar).

use actix_web::web::ThinData;
use apalis::prelude::{Attempt, Data, Error};
use eyre::Result as EyreResult;
use tracing::{debug, info, instrument};

use crate::{
    constants::WORKER_TOKEN_SWAP_REQUEST_RETRIES,
    domain::get_network_relayer,
    jobs::{handle_result, Job, TokenSwapRequest},
    models::DefaultAppState,
    observability::request_id::set_request_id,
};

/// Handles incoming swap jobs from the queue.
///
/// # Arguments
/// * `job` - The swap job containing relayer ID
/// * `context` - Application state containing services
///
/// # Returns
/// * `Result<(), Error>` - Success or failure of swap processing
#[instrument(
    level = "debug",
    skip(job, context),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %attempt.current(),
        relayer_id = %job.data.relayer_id,
    )
)]
pub async fn token_swap_request_handler(
    job: Job<TokenSwapRequest>,
    context: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> std::result::Result<(), Error> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    debug!(relayer_id = %job.data.relayer_id, "handling token swap request");

    let result = handle_request(job.data, context).await;

    handle_result(
        result,
        attempt,
        "TokenSwapRequest",
        WORKER_TOKEN_SWAP_REQUEST_RETRIES,
    )
}

#[derive(Default, Debug, Clone)]
pub struct TokenSwapCronReminder();

/// Handles incoming swap jobs from the cron queue.
#[instrument(
    level = "info",
    skip(_job, data, relayer_id),
    fields(
        job_type = "token_swap_cron",
        attempt = %attempt.current(),
    ),
    err
)]
pub async fn token_swap_cron_handler(
    _job: TokenSwapCronReminder,
    relayer_id: Data<String>,
    data: Data<ThinData<DefaultAppState>>,
    attempt: Attempt,
) -> std::result::Result<(), Error> {
    info!(
        relayer_id = %*relayer_id,
        "handling token swap cron request"
    );

    let result = handle_request(
        TokenSwapRequest {
            relayer_id: relayer_id.to_string(),
        },
        data,
    )
    .await;

    handle_result(
        result,
        attempt,
        "TokenSwapRequest",
        WORKER_TOKEN_SWAP_REQUEST_RETRIES,
    )
}

async fn handle_request(
    request: TokenSwapRequest,
    context: Data<ThinData<DefaultAppState>>,
) -> EyreResult<()> {
    debug!(relayer_id = %request.relayer_id, "processing token swap");

    let relayer = get_network_relayer(request.relayer_id.clone(), &context).await?;

    relayer
        .handle_token_swap_request(request.relayer_id.clone())
        .await
        .map_err(|e| eyre::eyre!("Failed to handle token swap request: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {}
