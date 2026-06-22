//! GCP Pub/Sub queue backend façade and re-exports for the queue abstraction.
//!
//! Mirrors `src/queues/sqs/mod.rs`. The backend is anchored to the proven
//! Redis/Apalis semantics (repository is the system of record; scheduling is
//! store-and-run-when-due via Redis; no dead-letter the relayer reads) and uses
//! the SQS module only as a reference for the dumb-pipe plumbing.
pub use crate::queues::{
    backoff_config_for_queue, filter_relayers_for_swap, retry_delay_secs,
    status_check_retry_delay_secs, HandlerError, QueueBackend, QueueBackendError, QueueHealth,
    QueueType, WorkerContext, WorkerHandle,
};

pub mod backend;
pub mod monitoring;
pub mod schedule;
pub mod worker;
