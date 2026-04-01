//! Transaction request handler for processing incoming transaction jobs.
//!
//! Handles the validation and preparation of transactions before they are
//! submitted to the network
use actix_web::web::ThinData;
use chrono::Utc;
use tracing::instrument;

use crate::{
    constants::WORKER_TRANSACTION_REQUEST_RETRIES,
    domain::{get_relayer_transaction, get_transaction_by_id, Transaction},
    jobs::{handle_result, Job, TransactionRequest},
    metrics::{observe_processing_time, STAGE_PREPARE_DURATION, STAGE_REQUEST_QUEUE_DWELL},
    models::DefaultAppState,
    observability::request_id::set_request_id,
    queues::{HandlerError, WorkerContext},
};

#[instrument(
    level = "debug",
    skip(job, state, ctx),
    fields(
        request_id = ?job.request_id,
        job_id = %job.message_id,
        job_type = %job.job_type.to_string(),
        attempt = %ctx.attempt,
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        task_id = %ctx.task_id,
    )
)]
pub async fn transaction_request_handler(
    job: Job<TransactionRequest>,
    state: ThinData<DefaultAppState>,
    ctx: WorkerContext,
) -> Result<(), HandlerError> {
    if let Some(request_id) = job.request_id.clone() {
        set_request_id(request_id);
    }

    tracing::debug!(
        tx_id = %job.data.transaction_id,
        relayer_id = %job.data.relayer_id,
        "handling transaction request"
    );

    let result = handle_request(job.data, &state).await;

    handle_result(
        result,
        &ctx,
        "Transaction Request",
        WORKER_TRANSACTION_REQUEST_RETRIES,
    )
}

async fn handle_request(
    request: TransactionRequest,
    state: &ThinData<DefaultAppState>,
) -> eyre::Result<()> {
    let relayer_transaction = get_relayer_transaction(request.relayer_id, state).await?;

    let transaction = get_transaction_by_id(request.transaction_id.clone(), state).await?;

    // Measure time from transaction creation to request handler start.
    // On first attempt this approximates queue dwell time. On retries it
    // includes cumulative retry backoff since created_at is unchanged.
    let relayer_id = transaction.relayer_id.clone();
    let network_type = transaction.network_type.to_string();
    if let Ok(created_time) = chrono::DateTime::parse_from_rfc3339(&transaction.created_at) {
        let dwell_secs =
            (Utc::now() - created_time.with_timezone(&Utc)).num_milliseconds() as f64 / 1000.0;
        observe_processing_time(
            &relayer_id,
            &network_type,
            STAGE_REQUEST_QUEUE_DWELL,
            dwell_secs,
        );
    }

    tracing::debug!(
        tx_id = %transaction.id,
        relayer_id = %transaction.relayer_id,
        status = ?transaction.status,
        "preparing transaction"
    );

    let prepare_start = std::time::Instant::now();
    let prepared = relayer_transaction.prepare_transaction(transaction).await?;
    let prepare_duration = prepare_start.elapsed().as_secs_f64();

    observe_processing_time(
        &relayer_id,
        &network_type,
        STAGE_PREPARE_DURATION,
        prepare_duration,
    );

    tracing::debug!(
        tx_id = %prepared.id,
        relayer_id = %prepared.relayer_id,
        status = ?prepared.status,
        prepare_duration_ms = prepare_start.elapsed().as_millis(),
        "transaction prepared"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queues::WorkerContext;

    #[tokio::test]
    async fn test_handler_result_processing() {
        let request = TransactionRequest::new("tx123", "relayer-1");
        let job = Job::new(crate::jobs::JobType::TransactionRequest, request);
        let ctx = WorkerContext::new(0, "test-task".into());

        assert_eq!(job.data.transaction_id, "tx123");
        assert_eq!(job.data.relayer_id, "relayer-1");
        assert_eq!(ctx.attempt, 0);
    }
}
