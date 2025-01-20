use crate::{
    jobs::{
        Job, NotificationSend, Queue, TransactionRequest, TransactionStatusCheck, TransactionSubmit,
    },
    models::RelayerError,
};
use apalis::prelude::Storage;
use apalis_redis::RedisError;
use log::{error, info};
use thiserror::Error;
use tokio::sync::Mutex;

use super::JobType;

#[derive(Debug, Error)]
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

    pub async fn produce_transaction_request_job(
        &self,
        transaction_process_job: TransactionRequest,
    ) -> Result<(), JobProducerError> {
        info!(
            "Producing transaction request job: {:?}",
            transaction_process_job
        );
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionRequest, transaction_process_job);
        queue.transaction_request_queue.push(job).await?;

        info!("Transaction job produced successfully");

        Ok(())
    }

    pub async fn produce_submit_transaction_job(
        &self,
        transaction_submit_job: TransactionSubmit,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionSubmit, transaction_submit_job);

        // queue.transaction_submission_queue.push(job).await?;
        queue.transaction_submission_queue.push(job).await?;
        info!("Transaction Submit job produced successfully");

        Ok(())
    }

    pub async fn produce_check_transaction_status_job(
        &self,
        transaction_status_check_job: TransactionStatusCheck,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(
            JobType::TransactionStatusCheck,
            transaction_status_check_job,
        );

        queue.transaction_status_queue.push(job).await?;
        info!("Transaction Status Check job produced successfully");

        Ok(())
    }

    pub async fn produce_send_notification_job(
        &self,
        notification_send_job: NotificationSend,
    ) -> Result<(), JobProducerError> {
        let mut queue = self.queue.lock().await;
        let job = Job::new(JobType::TransactionStatusCheck, notification_send_job);

        queue.notification_queue.push(job).await?;

        info!("Notification Send job produced successfully");

        Ok(())
    }
}
