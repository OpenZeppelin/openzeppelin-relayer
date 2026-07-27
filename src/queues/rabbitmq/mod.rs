//! RabbitMQ (AMQP 0-9-1) queue backend façade and re-exports.
//!
//! A deliberate sibling of the Pub/Sub backend (`src/queues/pubsub/`): each of
//! the 8 queue types maps to one durable classic queue published via the default
//! exchange; deferred jobs and retry backoff are held in the Redis sorted-set
//! scheduler shared with Pub/Sub (`src/queues/schedule.rs`), so broker queues
//! only ever carry already-due jobs. RabbitMQ's semantics shed two of Pub/Sub's
//! hardest parts: no lease/ack-deadline management (an unacked delivery stays
//! reserved while the consumer channel lives) and no external depth read
//! (passive `queue_declare` returns the count over the live connection).
pub use crate::queues::{
    backoff_config_for_queue, filter_relayers_for_swap, retry_delay_secs,
    status_check_retry_delay_secs, HandlerError, QueueBackend, QueueBackendError, QueueHealth,
    QueueType, WorkerContext, WorkerHandle,
};

pub mod backend;
pub mod worker;
