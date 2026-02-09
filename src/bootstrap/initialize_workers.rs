//! Worker initialization
//!
//! Re-exports from the queue backend module where the Apalis-specific worker
//! logic now lives alongside the Redis backend implementation.
//!
//! Also provides `initialize_queue_workers` which consolidates the entire
//! queue backend lifecycle (creation, worker init) into a single call.

use std::sync::Arc;

use actix_web::web::ThinData;
use tracing::info;

use crate::{
    jobs::queue_backend::{QueueBackend, QueueBackendStorage, WorkerHandle},
    models::DefaultAppState,
};

/// Creates the queue backend and initializes all workers in a single step.
///
/// This consolidates queue backend creation (from `QUEUE_BACKEND` env var),
/// worker initialization, and logging into a single bootstrap function,
/// keeping `main.rs` free of queue implementation details.
///
/// # Arguments
/// * `app_state` - Application state containing the job producer and configuration
///
/// # Returns
/// Vector of worker handles for all spawned workers
pub async fn initialize_queue_workers(
    app_state: ThinData<DefaultAppState>,
) -> color_eyre::Result<Vec<WorkerHandle>> {
    let backend = QueueBackendStorage::new(app_state.clone()).await?;

    let handles = backend.initialize_workers(Arc::new(app_state)).await?;

    info!(
        backend = backend.backend_type(),
        worker_count = handles.len(),
        "Initialized queue backend workers"
    );

    Ok(handles)
}
