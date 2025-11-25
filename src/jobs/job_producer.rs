//! Job producer module for enqueueing jobs to Redis queues.
//!
//! Provides functionality for producing various types of jobs:
//! - Transaction processing jobs
//! - Transaction submission jobs
//! - Status monitoring jobs
//! - Notification jobs

use crate::{
    jobs::{
        Job, NotificationSend, Queue, RelayerHealthCheck, TransactionRequest, TransactionSend,
        TransactionStatusCheck,
    },
    models::RelayerError,
    observability::request_id::get_request_id,
};
use apalis::prelude::Storage;
use apalis_redis::RedisError;
use async_trait::async_trait;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, error};

use super::{JobType, TokenSwapRequest};

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Error, Serialize, Clone)]
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

    async fn produce_token_swap_request_job(
        &self,
        swap_request_job: TokenSwapRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;

    async fn produce_relayer_health_check_job(
        &self,
        relayer_health_check_job: RelayerHealthCheck,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError>;

    async fn get_queue(&self) -> Result<Queue, JobProducerError>;
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
    async fn get_queue(&self) -> Result<Queue, JobProducerError> {
        let queue = self.queue.lock().await;

        Ok(queue.clone())
    }

    async fn produce_transaction_request_job(
        &self,
        transaction_process_job: TransactionRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        debug!(
            "Producing transaction request job: {:?}",
            transaction_process_job
        );
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionRequest, transaction_process_job)
            .with_request_id(get_request_id());

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
        debug!("Transaction job produced successfully");

        Ok(())
    }

    async fn produce_submit_transaction_job(
        &self,
        transaction_submit_job: TransactionSend,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionSend, transaction_submit_job)
            .with_request_id(get_request_id());

        match scheduled_on {
            Some(on) => {
                queue.transaction_submission_queue.schedule(job, on).await?;
            }
            None => {
                queue.transaction_submission_queue.push(job).await?;
            }
        }
        debug!("Transaction Submit job produced successfully");

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
            transaction_status_check_job.clone(),
        )
        .with_request_id(get_request_id());

        // Route to the appropriate queue based on network type
        use crate::models::NetworkType;
        let status_queue = match transaction_status_check_job.network_type {
            Some(NetworkType::Evm) => &mut queue.transaction_status_queue_evm,
            Some(NetworkType::Stellar) => &mut queue.transaction_status_queue_stellar,
            _ => &mut queue.transaction_status_queue, // Generic queue or legacy messages without network_type
        };

        match scheduled_on {
            Some(on) => {
                status_queue.schedule(job, on).await?;
            }
            None => {
                status_queue.push(job).await?;
            }
        }
        debug!(
            network_type = ?transaction_status_check_job.network_type,
            "Transaction Status Check job produced successfully"
        );
        Ok(())
    }

    async fn produce_send_notification_job(
        &self,
        notification_send_job: NotificationSend,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::NotificationSend, notification_send_job)
            .with_request_id(get_request_id());

        match scheduled_on {
            Some(on) => {
                queue.notification_queue.schedule(job, on).await?;
            }
            None => {
                queue.notification_queue.push(job).await?;
            }
        }

        debug!("Notification Send job produced successfully");
        Ok(())
    }

    async fn produce_token_swap_request_job(
        &self,
        swap_request_job: TokenSwapRequest,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job =
            Job::new(JobType::TokenSwapRequest, swap_request_job).with_request_id(get_request_id());

        match scheduled_on {
            Some(on) => {
                queue.token_swap_request_queue.schedule(job, on).await?;
            }
            None => {
                queue.token_swap_request_queue.push(job).await?;
            }
        }

        debug!("Token swap job produced successfully");
        Ok(())
    }

    async fn produce_relayer_health_check_job(
        &self,
        relayer_health_check_job: RelayerHealthCheck,
        scheduled_on: Option<i64>,
    ) -> Result<(), JobProducerError> {
        let job = Job::new(
            JobType::RelayerHealthCheck,
            relayer_health_check_job.clone(),
        )
        .with_request_id(get_request_id());

        let mut queue = self.queue.lock().await;

        match scheduled_on {
            Some(scheduled_on) => {
                queue
                    .relayer_health_check_queue
                    .schedule(job, scheduled_on)
                    .await?;
            }
            None => {
                queue.relayer_health_check_queue.push(job).await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionResponse, TransactionResponse, TransactionStatus, WebhookNotification,
        WebhookPayload, U256,
    };
    use crate::utils::calculate_scheduled_timestamp;

    #[derive(Clone, Debug)]
    // Define a simplified queue for testing without using complex mocks
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
        pub transaction_status_queue_evm: TestRedisStorage<Job<TransactionStatusCheck>>,
        pub transaction_status_queue_stellar: TestRedisStorage<Job<TransactionStatusCheck>>,
        pub notification_queue: TestRedisStorage<Job<NotificationSend>>,
        pub token_swap_request_queue: TestRedisStorage<Job<TokenSwapRequest>>,
        pub relayer_health_check_queue: TestRedisStorage<Job<RelayerHealthCheck>>,
    }

    impl TestQueue {
        fn new() -> Self {
            Self {
                transaction_request_queue: TestRedisStorage::new(),
                transaction_submission_queue: TestRedisStorage::new(),
                transaction_status_queue: TestRedisStorage::new(),
                transaction_status_queue_evm: TestRedisStorage::new(),
                transaction_status_queue_stellar: TestRedisStorage::new(),
                notification_queue: TestRedisStorage::new(),
                token_swap_request_queue: TestRedisStorage::new(),
                relayer_health_check_queue: TestRedisStorage::new(),
            }
        }
    }

    // A test version of JobProducer
    struct TestJobProducer {
        queue: Mutex<TestQueue>,
    }

    impl Clone for TestJobProducer {
        fn clone(&self) -> Self {
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
        async fn get_queue(&self) -> Result<Queue, JobProducerError> {
            unimplemented!("get_queue not used in tests")
        }

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
                transaction_status_check_job.clone(),
            );

            // Route to the appropriate queue based on network type
            use crate::models::NetworkType;
            let status_queue = match transaction_status_check_job.network_type {
                Some(NetworkType::Evm) => &mut queue.transaction_status_queue_evm,
                Some(NetworkType::Stellar) => &mut queue.transaction_status_queue_stellar,
                Some(NetworkType::Solana) => &mut queue.transaction_status_queue, // Use default queue
                None => &mut queue.transaction_status_queue, // Legacy messages without network_type
            };

            match scheduled_on {
                Some(on) => {
                    status_queue.schedule(job, on).await?;
                }
                None => {
                    status_queue.push(job).await?;
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

        async fn produce_token_swap_request_job(
            &self,
            swap_request_job: TokenSwapRequest,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(JobType::TokenSwapRequest, swap_request_job);

            match scheduled_on {
                Some(on) => {
                    queue.token_swap_request_queue.schedule(job, on).await?;
                }
                None => {
                    queue.token_swap_request_queue.push(job).await?;
                }
            }

            Ok(())
        }

        async fn produce_relayer_health_check_job(
            &self,
            relayer_health_check_job: RelayerHealthCheck,
            scheduled_on: Option<i64>,
        ) -> Result<(), JobProducerError> {
            let mut queue = self.queue.lock().await;
            let job = Job::new(JobType::RelayerHealthCheck, relayer_health_check_job);

            match scheduled_on {
                Some(scheduled_on) => {
                    queue
                        .relayer_health_check_queue
                        .schedule(job, scheduled_on)
                        .await?;
                }
                None => {
                    queue.relayer_health_check_queue.push(job).await?;
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
        let scheduled_timestamp = calculate_scheduled_timestamp(10); // Schedule for 10 seconds from now
        let result = producer
            .produce_transaction_request_job(request, Some(scheduled_timestamp))
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
        use crate::models::NetworkType;
        let producer = TestJobProducer::new();

        // Test status check job for EVM
        let status_job = TransactionStatusCheck::new("tx123", "relayer-1", NetworkType::Evm);
        let result = producer
            .produce_check_transaction_status_job(status_job, None)
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue_evm.push_called);
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

    #[tokio::test]
    async fn test_relayer_health_check_job() {
        let producer = TestJobProducer::new();

        // Test immediate health check job
        let health_check = RelayerHealthCheck::new("relayer-1".to_string());
        let result = producer
            .produce_relayer_health_check_job(health_check, None)
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.relayer_health_check_queue.push_called);

        // Test scheduled health check job
        let producer = TestJobProducer::new();
        let health_check = RelayerHealthCheck::new("relayer-1".to_string());
        let scheduled_timestamp = calculate_scheduled_timestamp(60);
        let result = producer
            .produce_relayer_health_check_job(health_check, Some(scheduled_timestamp))
            .await;
        assert!(result.is_ok());

        let queue = producer.get_queue().await;
        assert!(queue.relayer_health_check_queue.schedule_called);
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

    #[tokio::test]
    async fn test_get_queue() {
        let producer = TestJobProducer::new();

        // Get the queue
        let queue = producer.get_queue().await;

        // Verify the queue is valid and has the expected structure
        assert!(!queue.transaction_request_queue.push_called);
        assert!(!queue.transaction_request_queue.schedule_called);
        assert!(!queue.transaction_submission_queue.push_called);
        assert!(!queue.notification_queue.push_called);
        assert!(!queue.token_swap_request_queue.push_called);
        assert!(!queue.relayer_health_check_queue.push_called);
    }

    #[tokio::test]
    async fn test_produce_relayer_health_check_job_immediate() {
        let producer = TestJobProducer::new();

        // Test immediate health check job (no scheduling)
        let health_check = RelayerHealthCheck::new("relayer-1".to_string());
        let result = producer
            .produce_relayer_health_check_job(health_check, None)
            .await;

        // Should succeed
        assert!(result.is_ok());

        // Verify the job was pushed (not scheduled)
        let queue = producer.get_queue().await;
        assert!(queue.relayer_health_check_queue.push_called);
        assert!(!queue.relayer_health_check_queue.schedule_called);

        // Other queues should not be affected
        assert!(!queue.transaction_request_queue.push_called);
        assert!(!queue.transaction_submission_queue.push_called);
        assert!(!queue.transaction_status_queue.push_called);
        assert!(!queue.notification_queue.push_called);
        assert!(!queue.token_swap_request_queue.push_called);
    }

    #[tokio::test]
    async fn test_produce_relayer_health_check_job_scheduled() {
        let producer = TestJobProducer::new();

        // Test scheduled health check job
        let health_check = RelayerHealthCheck::new("relayer-2".to_string());
        let scheduled_timestamp = calculate_scheduled_timestamp(300); // 5 minutes from now
        let result = producer
            .produce_relayer_health_check_job(health_check, Some(scheduled_timestamp))
            .await;

        // Should succeed
        assert!(result.is_ok());

        // Verify the job was scheduled (not pushed)
        let queue = producer.get_queue().await;
        assert!(queue.relayer_health_check_queue.schedule_called);
        assert!(!queue.relayer_health_check_queue.push_called);

        // Other queues should not be affected
        assert!(!queue.transaction_request_queue.push_called);
        assert!(!queue.transaction_submission_queue.push_called);
        assert!(!queue.transaction_status_queue.push_called);
        assert!(!queue.notification_queue.push_called);
        assert!(!queue.token_swap_request_queue.push_called);
    }

    #[tokio::test]
    async fn test_produce_relayer_health_check_job_multiple_relayers() {
        let producer = TestJobProducer::new();

        // Produce health check jobs for multiple relayers
        let relayer_ids = vec!["relayer-1", "relayer-2", "relayer-3"];

        for relayer_id in &relayer_ids {
            let health_check = RelayerHealthCheck::new(relayer_id.to_string());
            let result = producer
                .produce_relayer_health_check_job(health_check, None)
                .await;
            assert!(result.is_ok());
        }

        // Verify jobs were produced
        let queue = producer.get_queue().await;
        assert!(queue.relayer_health_check_queue.push_called);
    }

    #[tokio::test]
    async fn test_status_check_routes_to_evm_queue() {
        use crate::models::NetworkType;
        let producer = TestJobProducer::new();

        let status_job = TransactionStatusCheck::new("tx-evm", "relayer-1", NetworkType::Evm);
        let result = producer
            .produce_check_transaction_status_job(status_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue_evm.push_called);
        assert!(!queue.transaction_status_queue_stellar.push_called);
        assert!(!queue.transaction_status_queue.push_called);
    }

    #[tokio::test]
    async fn test_status_check_routes_to_stellar_queue() {
        use crate::models::NetworkType;
        let producer = TestJobProducer::new();

        let status_job =
            TransactionStatusCheck::new("tx-stellar", "relayer-2", NetworkType::Stellar);
        let result = producer
            .produce_check_transaction_status_job(status_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue_stellar.push_called);
        assert!(!queue.transaction_status_queue_evm.push_called);
        assert!(!queue.transaction_status_queue.push_called);
    }

    #[tokio::test]
    async fn test_status_check_routes_to_default_queue_for_solana() {
        use crate::models::NetworkType;
        let producer = TestJobProducer::new();

        let status_job = TransactionStatusCheck::new("tx-solana", "relayer-3", NetworkType::Solana);
        let result = producer
            .produce_check_transaction_status_job(status_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue.push_called);
        assert!(!queue.transaction_status_queue_evm.push_called);
        assert!(!queue.transaction_status_queue_stellar.push_called);
    }

    #[tokio::test]
    async fn test_status_check_scheduled_evm() {
        use crate::models::NetworkType;
        let producer = TestJobProducer::new();

        let status_job =
            TransactionStatusCheck::new("tx-evm-scheduled", "relayer-1", NetworkType::Evm);
        let scheduled_timestamp = calculate_scheduled_timestamp(30);
        let result = producer
            .produce_check_transaction_status_job(status_job, Some(scheduled_timestamp))
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue_evm.schedule_called);
        assert!(!queue.transaction_status_queue_evm.push_called);
    }

    #[tokio::test]
    async fn test_submit_transaction_scheduled() {
        let producer = TestJobProducer::new();

        let submit_job = TransactionSend::submit("tx-scheduled", "relayer-1");
        let scheduled_timestamp = calculate_scheduled_timestamp(15);
        let result = producer
            .produce_submit_transaction_job(submit_job, Some(scheduled_timestamp))
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_submission_queue.schedule_called);
        assert!(!queue.transaction_submission_queue.push_called);
    }

    #[tokio::test]
    async fn test_notification_job_scheduled() {
        let producer = TestJobProducer::new();

        let notification = WebhookNotification::new(
            "test_scheduled_event".to_string(),
            WebhookPayload::Transaction(TransactionResponse::Evm(Box::new(
                EvmTransactionResponse {
                    id: "tx-notify-scheduled".to_string(),
                    hash: Some("0xabc123".to_string()),
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
        let job = NotificationSend::new("notification-scheduled".to_string(), notification);

        let scheduled_timestamp = calculate_scheduled_timestamp(5);
        let result = producer
            .produce_send_notification_job(job, Some(scheduled_timestamp))
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.notification_queue.schedule_called);
        assert!(!queue.notification_queue.push_called);
    }

    #[tokio::test]
    async fn test_solana_swap_job_immediate() {
        let producer = TestJobProducer::new();

        let swap_job = TokenSwapRequest::new("relayer-solana".to_string());
        let result = producer
            .produce_token_swap_request_job(swap_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.token_swap_request_queue.push_called);
        assert!(!queue.token_swap_request_queue.schedule_called);
    }

    #[tokio::test]
    async fn test_token_swap_job_scheduled() {
        let producer = TestJobProducer::new();

        let swap_job = TokenSwapRequest::new("relayer-solana".to_string());
        let scheduled_timestamp = calculate_scheduled_timestamp(20);
        let result = producer
            .produce_token_swap_request_job(swap_job, Some(scheduled_timestamp))
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.token_swap_request_queue.schedule_called);
        assert!(!queue.token_swap_request_queue.push_called);
    }

    #[tokio::test]
    async fn test_transaction_send_cancel_job() {
        let producer = TestJobProducer::new();

        let cancel_job = TransactionSend::cancel("tx-cancel", "relayer-1", "user requested");
        let result = producer
            .produce_submit_transaction_job(cancel_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_submission_queue.push_called);
    }

    #[tokio::test]
    async fn test_transaction_send_resubmit_job() {
        let producer = TestJobProducer::new();

        let resubmit_job = TransactionSend::resubmit("tx-resubmit", "relayer-1");
        let result = producer
            .produce_submit_transaction_job(resubmit_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_submission_queue.push_called);
    }

    #[tokio::test]
    async fn test_transaction_send_resend_job() {
        let producer = TestJobProducer::new();

        let resend_job = TransactionSend::resend("tx-resend", "relayer-1");
        let result = producer
            .produce_submit_transaction_job(resend_job, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_submission_queue.push_called);
    }

    #[tokio::test]
    async fn test_multiple_jobs_different_queues() {
        let producer = TestJobProducer::new();

        // Produce different types of jobs
        let request = TransactionRequest::new("tx1", "relayer-1");
        producer
            .produce_transaction_request_job(request, None)
            .await
            .unwrap();

        let submit = TransactionSend::submit("tx2", "relayer-1");
        producer
            .produce_submit_transaction_job(submit, None)
            .await
            .unwrap();

        use crate::models::NetworkType;
        let status = TransactionStatusCheck::new("tx3", "relayer-1", NetworkType::Evm);
        producer
            .produce_check_transaction_status_job(status, None)
            .await
            .unwrap();

        // Verify all queues were used
        let queue = producer.get_queue().await;
        assert!(queue.transaction_request_queue.push_called);
        assert!(queue.transaction_submission_queue.push_called);
        assert!(queue.transaction_status_queue_evm.push_called);
    }

    #[test]
    fn test_job_producer_clone() {
        let producer = TestJobProducer::new();
        let cloned_producer = producer.clone();

        // Both should be valid instances
        // The clone creates a new Mutex with a cloned Queue
        assert!(std::ptr::addr_of!(producer) != std::ptr::addr_of!(cloned_producer));
    }

    #[tokio::test]
    async fn test_transaction_request_with_metadata() {
        let producer = TestJobProducer::new();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("retry_count".to_string(), "3".to_string());

        let request = TransactionRequest::new("tx-meta", "relayer-1").with_metadata(metadata);

        let result = producer
            .produce_transaction_request_job(request, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_request_queue.push_called);
    }

    #[tokio::test]
    async fn test_status_check_with_metadata() {
        use crate::models::NetworkType;
        let producer = TestJobProducer::new();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("attempt".to_string(), "2".to_string());

        let status =
            TransactionStatusCheck::new("tx-status-meta", "relayer-1", NetworkType::Stellar)
                .with_metadata(metadata);

        let result = producer
            .produce_check_transaction_status_job(status, None)
            .await;

        assert!(result.is_ok());
        let queue = producer.get_queue().await;
        assert!(queue.transaction_status_queue_stellar.push_called);
    }

    #[tokio::test]
    async fn test_scheduled_jobs_with_different_delays() {
        let producer = TestJobProducer::new();

        // Test with various scheduling delays
        let delays = vec![1, 10, 60, 300, 3600]; // 1s, 10s, 1m, 5m, 1h

        for (idx, delay) in delays.iter().enumerate() {
            let request = TransactionRequest::new(format!("tx-delay-{}", idx), "relayer-1");
            let timestamp = calculate_scheduled_timestamp(*delay);

            let result = producer
                .produce_transaction_request_job(request, Some(timestamp))
                .await;

            assert!(
                result.is_ok(),
                "Failed to schedule job with delay {}",
                delay
            );
        }
    }

    #[test]
    fn test_job_producer_error_display() {
        let error = JobProducerError::QueueError("Test queue error".to_string());
        let error_string = error.to_string();

        assert!(error_string.contains("Queue error"));
        assert!(error_string.contains("Test queue error"));
    }

    #[test]
    fn test_job_producer_error_to_relayer_error() {
        let job_error = JobProducerError::QueueError("Connection failed".to_string());
        let relayer_error: RelayerError = job_error.into();

        match relayer_error {
            RelayerError::QueueError(msg) => {
                assert_eq!(msg, "Queue error");
            }
            _ => panic!("Expected QueueError variant"),
        }
    }
}
