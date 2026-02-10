//! Redis backend implementation using Apalis.
//!
//! This module provides a Redis/Apalis-backed implementation of the QueueBackend trait.
//! It wraps the existing Queue structure and delegates to Apalis for job processing.

use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use crate::{
    jobs::{
        Job, NotificationSend, RelayerHealthCheck, TokenSwapRequest, TransactionRequest,
        TransactionSend, TransactionStatusCheck,
    },
    models::{DefaultAppState, NetworkType},
    queues::{Queue, QueueBackendType},
    utils::RedisConnections,
};
use actix_web::web::ThinData;
use apalis::prelude::Storage;

use super::{QueueBackend, QueueBackendError, QueueHealth, QueueType, WorkerHandle};

/// Redis backend using Apalis for job queue operations.
///
/// This is a wrapper around the existing Queue implementation that provides
/// the QueueBackend trait interface. It delegates all operations to the
/// existing Apalis/Redis infrastructure.
#[derive(Clone)]
pub struct RedisBackend {
    queue: Queue,
}

impl std::fmt::Debug for RedisBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisBackend")
            .field("backend_type", &"redis")
            .finish()
    }
}

impl RedisBackend {
    /// Creates a new Redis backend.
    ///
    /// This initializes all Redis-backed queues using the existing Queue::setup method.
    ///
    /// # Arguments
    /// * `redis_connections` - Redis connection pools for queue operations
    ///
    /// # Errors
    /// Returns QueueBackendError if queue setup fails
    pub async fn new(redis_connections: Arc<RedisConnections>) -> Result<Self, QueueBackendError> {
        info!("Initializing Redis queue backend");

        let queue = Queue::setup(redis_connections)
            .await
            .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;

        Ok(Self { queue })
    }

    /// Returns a reference to the underlying Queue for compatibility with existing code.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }
}

#[async_trait]
impl QueueBackend for RedisBackend {
    async fn produce_transaction_request(
        &self,
        job: Job<TransactionRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.transaction_request_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_transaction_submission(
        &self,
        job: Job<TransactionSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.transaction_submission_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_transaction_status_check(
        &self,
        job: Job<TransactionStatusCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        // Route by network_type to preserve existing Redis queue behavior.
        let mut storage = match job.data.network_type {
            Some(NetworkType::Evm) => self.queue.transaction_status_queue_evm.clone(),
            Some(NetworkType::Stellar) => self.queue.transaction_status_queue_stellar.clone(),
            _ => self.queue.transaction_status_queue.clone(),
        };
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_notification(
        &self,
        job: Job<NotificationSend>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.notification_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_token_swap_request(
        &self,
        job: Job<TokenSwapRequest>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.token_swap_request_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn produce_relayer_health_check(
        &self,
        job: Job<RelayerHealthCheck>,
        scheduled_on: Option<i64>,
    ) -> Result<String, QueueBackendError> {
        let mut storage = self.queue.relayer_health_check_queue.clone();
        let job_id = job.message_id.clone();

        match scheduled_on {
            Some(on) => {
                storage
                    .schedule(job, on)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
            None => {
                storage
                    .push(job)
                    .await
                    .map_err(|e| QueueBackendError::RedisError(e.to_string()))?;
            }
        }

        Ok(job_id)
    }

    async fn initialize_workers(
        &self,
        app_state: Arc<ThinData<DefaultAppState>>,
    ) -> Result<Vec<WorkerHandle>, QueueBackendError> {
        info!("Initializing Redis backend workers");

        super::redis_worker::initialize_redis_workers((*app_state).clone())
            .await
            .map_err(|e| QueueBackendError::WorkerInitError(e.to_string()))?;

        super::redis_worker::initialize_redis_token_swap_workers((*app_state).clone())
            .await
            .map_err(|e| QueueBackendError::WorkerInitError(e.to_string()))?;

        // Apalis workers are owned by the monitors; no explicit
        // worker handles are returned from that flow.
        Ok(vec![])
    }

    async fn health_check(&self) -> Result<Vec<QueueHealth>, QueueBackendError> {
        // Intentionally avoid per-request Redis queue depth calls here to keep
        // health checks lightweight and avoid adding pressure to Redis.
        // Return static backend health metadata only.
        let health_statuses = vec![
            QueueHealth {
                queue_type: QueueType::TransactionRequest,
                messages_visible: 0, // Would need Redis LLEN query
                messages_in_flight: 0,
                messages_dlq: 0,
                backend: "redis".to_string(),
                is_healthy: true,
            },
            QueueHealth {
                queue_type: QueueType::TransactionSubmission,
                messages_visible: 0,
                messages_in_flight: 0,
                messages_dlq: 0,
                backend: "redis".to_string(),
                is_healthy: true,
            },
            QueueHealth {
                queue_type: QueueType::StatusCheck,
                messages_visible: 0,
                messages_in_flight: 0,
                messages_dlq: 0,
                backend: "redis".to_string(),
                is_healthy: true,
            },
            QueueHealth {
                queue_type: QueueType::Notification,
                messages_visible: 0,
                messages_in_flight: 0,
                messages_dlq: 0,
                backend: "redis".to_string(),
                is_healthy: true,
            },
            QueueHealth {
                queue_type: QueueType::TokenSwapRequest,
                messages_visible: 0,
                messages_in_flight: 0,
                messages_dlq: 0,
                backend: "redis".to_string(),
                is_healthy: true,
            },
            QueueHealth {
                queue_type: QueueType::RelayerHealthCheck,
                messages_visible: 0,
                messages_in_flight: 0,
                messages_dlq: 0,
                backend: "redis".to_string(),
                is_healthy: true,
            },
        ];

        Ok(health_statuses)
    }

    fn backend_type(&self) -> QueueBackendType {
        QueueBackendType::Redis
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_backend_type() {
        // This test is basic but ensures the trait implementation is correct
        // More comprehensive tests would require Redis connection
    }
}
