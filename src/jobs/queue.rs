//! Backwards-compatible re-export for the Redis queue implementation.
//!
//! The concrete Redis queue type now lives under `jobs::queue_backend::redis_queue`.

pub use crate::jobs::queue_backend::redis_queue::*;
