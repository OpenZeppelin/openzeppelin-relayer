//! Job producer module for enqueueing jobs to Redis queues.
//!
//! Provides functionality for producing various types of jobs:
//! - Transaction processing jobs
//! - Transaction submission jobs
//! - Status monitoring jobs
//! - Notification jobs

use crate::{
    jobs::{
        Job, NotificationSend, Queue, TransactionRequest, TransactionSend, TransactionStatusCheck,
    },
    models::RelayerError,
};
use apalis::prelude::Storage;
use apalis_redis::RedisError;
use async_trait::async_trait;
use log::{error, info};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Mutex;

use super::{JobType, SolanaTokenSwapRequest};

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Error, Serialize)]
pub enum JobProducerError {
    #[error("Queue error: {0}")]
    QueueError(String),
}

impl From<RedisError> for JobProducerError {
    fn from(_: RedisError) -> Self {
        JobProducerError::QueueError("Queue error".to_string())
    }
}

impl From<JobProducerError> for RelayerError {
    fn from(_: JobProducerError) -> Self {
        RelayerError::QueueError("Queue error".to_string())
    }
}

#[derive(Debug)]
pub struct JobProducer {
    queue: Mutex<Queue>,
}

impl Clone for JobProducer {
    fn clone(&self) -> Self {
        // We can't clone the Mutex directly, but we can create a new one with a cloned Queue
        // This requires getting the lock first
        let queue = self
            .queue
            .try_lock()
            .expect("Failed to lock queue for cloning")
            .clone();

        Self {
            queue: Mutex::new(queue),
        }
    }
}

#[async_trait]
#[cfg_attr(test, automock)]
pub trait JobProducerTrait: Send + Sync {
    async fn produce_transaction_request_job(
        &self,
        transaction_process_job: TransactionRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;

    async fn produce_submit_transaction_job(
        &self,
        transaction_submit_job: TransactionSend,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;

    async fn produce_check_transaction_status_job(
        &self,
        transaction_status_check_job: TransactionStatusCheck,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;

    async fn produce_send_notification_job(
        &self,
        notification_send_job: NotificationSend,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;

    async fn produce_solana_token_swap_request_job(
        &self,
        solana_swap_request_job: SolanaTokenSwapRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;
}

impl JobProducer {
    pub fn new(queue: Queue) -> Self {
        Self {
            queue: Mutex::new(queue.clone()),
        }
    }

    pub async fn get_queue(&self) -> Result<Queue, JobProducerError> {
        let queue = self.queue.lock().await;

        Ok(queue.clone())
    }
}

#[async_trait]
impl JobProducerTrait for JobProducer {
    async fn produce_transaction_request_job(
        &self,
        transaction_process_job: TransactionRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        info!(
            "Producing transaction request job: {:?}",
            transaction_process_job
        );
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionRequest, transaction_process_job);

        match scheduled_on {
            Some(scheduled_on) => {
                queue
                    .transaction_request_queue
                    .schedule(job, scheduled_on)
                    .await?;
            }
            None => {
                queue.transaction_request_queue.push(job).await?;
            }
        }
        info!("Transaction job produced successfully");

        Ok(())
    }

    async fn produce_submit_transaction_job(
        &self,
        transaction_submit_job: TransactionSend,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionSend, transaction_submit_job);

        match scheduled_on {
            Some(on) => {
                queue.transaction_submission_queue.schedule(job, on).await?;
            }
            None => {
                queue.transaction_submission_queue.push(job).await?;
            }
        }
        info!("Transaction Submit job produced successfully");

        Ok(())
    }

    async fn produce_check_transaction_status_job(
        &self,
        transaction_status_check_job: TransactionStatusCheck,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(
            JobType::TransactionStatusCheck,
            transaction_status_check_job,
        );
        match scheduled_on {
            Some(on) => {
                queue.transaction_status_queue.schedule(job, on).await?;
            }
            None => {
                queue.transaction_status_queue.push(job).await?;
            }
        }
        info!("Transaction Status Check job produced successfully");
        Ok(())
    }

    async fn produce_send_notification_job(
        &self,
        notification_send_job: NotificationSend,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::NotificationSend, notification_send_job);

        match scheduled_on {
            Some(on) => {
                queue.notification_queue.schedule(job, on).await?;
            }
            None => {
                queue.notification_queue.push(job).await?;
            }
        }

        info!("Notification Send job produced successfully");
        Ok(())
    }

    async fn produce_solana_token_swap_request_job(
        &self,
        solana_swap_request_job: SolanaTokenSwapRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::SolanaTokenSwapRequest, solana_swap_request_job);

        match scheduled_on {
            Some(on) => {
                queue
                    .solana_token_swap_request_queue
                    .schedule(job, on)
                    .await?;
            }
            None => {
                queue.solana_token_swap_request_queue.push(job).await?;
            }
        }

        info!("Solana token swap job produced successfully");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionResponse, TransactionResponse, TransactionStatus, WebhookNotification,
        WebhookPayload, U256,
    }; // Define a simplified queue for testing without using complex mocks
    #[derive(Clone, Debug)]
    struct TestRedisStorage<T> {
        pub push_called: bool,
        pub schedule_called: bool,
        _phantom: std::marker::PhantomData<T>,
    }

    impl<T> TestRedisStorage<T> {
        fn new() -> Self {
            Self {
                push_called: false,
                schedule_called: false,
                _phantom: std::marker::PhantomData,
            }
        }

        async fn push(&mut self, _job: T) -> Result<(), JobProducerError> {
            self.push_called = true;
            Ok(())
        }

        async fn schedule(&mut self, _job: T, _timestamp: i64) -> Result<(), JobProducerError> {
            self.schedule_called = true;
            Ok(())
        }
    }

    // A test version of the Queue
    #[derive(Clone, Debug)]
    struct TestQueue {
        pub transaction_request_queue: TestRedisStorage<Job<TransactionRequest>>,
        pub transaction_submission_queue: TestRedisStorage<Job<TransactionSend>>,
        pub transaction_status_queue: TestRedisStorage<Job<TransactionStatusCheck>>,
        pub notification_queue: TestRedisStorage<Job<NotificationSend>>,
        pub solana_token_swap_request_queue: TestRedisStorage<Job<SolanaTokenSwapRequest>>,
    }

    impl TestQueue {
        fn new() -> Self {
            Self {
                transaction_request_queue: TestRedisStorage::new(),
                transaction_submission_queue: TestRedisStorage::new(),
                transaction_status_queue: TestRedisStorage::new(),
                notification_queue: TestRedisStorage::new(),
                solana_token_swap_request_queue: TestRedisStorage::new(),
            }
        }
    }

    // A test version of JobProducer
    struct TestJobProducer {
        queue: Mutex<TestQueue>,
    }

    impl TestJobProducer {
        fn new() -> Self {
            Self {
                queue: Mutex::new(TestQueue::new()),
            }
        }

        async fn get_queue(&self) -> TestQueue {
            self.queue.lock().await.clone()
        }
    }

    #[async_trait]
    impl JobProducerTrait for TestJobProducer {
        async fn produce_transaction_request_job(
            &self,
            transaction_process_job: TransactionRequest,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(JobType::TransactionRequest, transaction_process_job);

            match scheduled_on {
                Some(scheduled_on) => {
                    queue
                        .transaction_request_queue
                        .schedule(job, scheduled_on)
                        .await?;
                }
                None => {
                    queue.transaction_request_queue.push(job).await?;
                }
            }

            Ok(())
        }

        async fn produce_submit_transaction_job(
            &self,
            transaction_submit_job: TransactionSend,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(JobType::TransactionSend, transaction_submit_job);

            match scheduled_on {
                Some(on) => {
                    queue.transaction_submission_queue.schedule(job, on).await?;
                }
                None => {
                    queue.transaction_submission_queue.push(job).await?;
                }
            }

            Ok(())
        }

        async fn produce_check_transaction_status_job(
            &self,
            transaction_status_check_job: TransactionStatusCheck,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(
                JobType::TransactionStatusCheck,
                transaction_status_check_job,
            );

            match scheduled_on {
                Some(on) => {
                    queue.transaction_status_queue.schedule(job, on).await?;
                }
                None => {
                    queue.transaction_status_queue.push(job).await?;
                }
            }

            Ok(())
        }

        async fn produce_send_notification_job(
            &self,
            notification_send_job: NotificationSend,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(JobType::NotificationSend, notification_send_job);

            match scheduled_on {
                Some(on) => {
                    queue.notification_queue.schedule(job, on).await?;
                }
                None => {
                    queue.notification_queue.push(job).await?;
                }
            }

            Ok(())
        }

        async fn produce_solana_token_swap_request_job(
            &self,
            solana_token_swap_request_job: SolanaTokenSwapRequest,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(
                JobType::SolanaTokenSwapRequest,
                solana_token_swap_request_job,
            );

            match scheduled_on {
                Some(on) => {
                    queue
                        .solana_token_swap_request_queue
                        .schedule(job, on)
                        .await?;
                }
                None => {
                    queue.solana_token_swap_request_queue.push(job).await?;
                }
            }

            Ok(())
        }
    }

    #[tokio::test]
    async fn test_job_producer_operations() {
        let producer = TestJobProducer::new();

        // Test transaction request job
        let request = TransactionRequest::new("tx123", "relayer-1");
        let result = producer
            .produce_transaction_request_job(request, None)
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.transaction_request_queue.push_called);

        // Test scheduled job
        let producer = TestJobProducer::new();
        let request = TransactionRequest::new("tx123", "relayer-1");
        let result = producer
            .produce_transaction_request_job(request, Some(1000))
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.transaction_request_queue.schedule_called);
    }

    #[tokio::test]
    async fn test_submit_transaction_job() {
        let producer = TestJobProducer::new();

        // Test submit transaction job
        let submit_job = TransactionSend::submit("tx123", "relayer-1");
        let result = producer
            .produce_submit_transaction_job(submit_job, None)
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.transaction_submission_queue.push_called);
    }

    #[tokio::test]
    async fn test_check_status_job() {
        let producer = TestJobProducer::new();

        // Test status check job
        let status_job = TransactionStatusCheck::new("tx123", "relayer-1");
        let result = producer
            .produce_check_transaction_status_job(status_job, None)
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue.push_called);
    }

    #[tokio::test]
    async fn test_notification_job() {
        let producer = TestJobProducer::new();

        // Create a simple notification for testing
        let notification = WebhookNotification::new(
            "test_event".to_string(),
            WebhookPayload::Transaction(TransactionResponse::Evm(Box::new(
                EvmTransactionResponse {
                    id: "tx123".to_string(),
                    hash: Some("0x123".to_string()),
                    status: TransactionStatus::Confirmed,
                    status_reason: None,
                    created_at: "2025-01-27T15:31:10.777083+00:00".to_string(),
                    sent_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                    confirmed_at: Some("2025-01-27T15:31:10.777083+00:00".to_string()),
                    gas_price: Some(1000000000),
                    gas_limit: Some(21000),
                    nonce: Some(1),
                    value: U256::from(1000000000000000000_u64),
                    from: "0xabc".to_string(),
                    to: Some("0xdef".to_string()),
                    relayer_id: "relayer-1".to_string(),
                    data: None,
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                    signature: None,
                    speed: None,
                },
            ))),
        );
        let job = NotificationSend::new("notification-1".to_string(), notification);

        let result = producer.produce_send_notification_job(job, None).await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.notification_queue.push_called);
    }

    #[test]
    fn test_job_producer_error_conversion() {
        // Test error conversion without using specific Redis error types
        let job_error = JobProducerError::QueueError("Test error".to_string());
        let relayer_error: RelayerError = job_error.into();

        match relayer_error {
            RelayerError::QueueError(msg) => {
                assert_eq!(msg, "Queue error");
            }
            _ => panic!("Unexpected error type"),
        }
    }
}
