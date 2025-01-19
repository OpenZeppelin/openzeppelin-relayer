use crate::{
    jobs::{
        Job, NotificationSend, Queue, TransactionRequest, TransactionStatusCheck, TransactionSubmit,
    },
    models::RelayerError,
    ApiError,
};
use apalis::prelude::Storage;
use apalis_redis::RedisError;
use log::{error, info};
use parking_lot::{Mutex, MutexGuard};
use thiserror::Error;

use super::JobType;

#[derive(Debug, Error)]
pub enum JobProducerError {
    #[error("Queue error: {0}")]
    QueueError(String),
}

impl From<RedisError> for JobProducerError {
    fn from(error: RedisError) -> Self {
        match error {
            _ => JobProducerError::QueueError("Redis error".to_string()),
        }
    }
}

impl From<JobProducerError> for RelayerError {
    fn from(error: JobProducerError) -> Self {
        match error {
            JobProducerError::QueueError(msg) => RelayerError::QueueError(msg),
        }
    }
}

#[derive(Debug)]
pub struct JobProducer {
    queue: Mutex<Queue>,
}

impl JobProducer {
    pub fn new(queue: Queue) -> Self {
        Self {
            queue: Mutex::new(queue),
        }
    }

    pub fn get_queue(&self) -> Result<Queue, JobProducerError> {
        let queue_guard = Self::acquire_lock(&self.queue)?;
        Ok(queue_guard.clone())
    }

    fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, JobProducerError> {
        Ok(lock.lock())
    }

    pub async fn produce_transaction_request_job(&self) -> Result<(), JobProducerError> {
        let transaction_request_job = TransactionRequest::new("", "");
        info!(
            "Producing transaction request job: {:?}",
            transaction_request_job
        );

        let mut queue = Self::acquire_lock(&self.queue)?;
        let job = Job::new(JobType::TransactionRequest, transaction_request_job);

        queue.transaction_request_queue.push(job).await?;

        info!("Transaction job produced successfully");

        Ok(())
    }

    pub async fn produce_submit_transaction_job(
        &mut self,
        transaction_submit_job: TransactionSubmit,
    ) -> Result<(), JobProducerError> {
        let mut queue = Self::acquire_lock(&self.queue)?;
        let job = Job::new(JobType::TransactionSubmit, transaction_submit_job);

        queue.transaction_submission_queue.push(job).await?;
        info!("Transaction Submit job produced successfully");

        Ok(())
    }

    pub async fn produce_check_transaction_status_job(
        &mut self,
        transaction_status_check_job: TransactionStatusCheck,
    ) -> Result<(), JobProducerError> {
        let mut queue = Self::acquire_lock(&self.queue)?;
        let job = Job::new(
            JobType::TransactionStatusCheck,
            transaction_status_check_job,
        );

        queue.transaction_status_queue.push(job).await?;
        info!("Transaction Status Check job produced successfully");

        Ok(())
    }

    pub async fn produce_send_notification_job(
        &mut self,
        notification_send_job: NotificationSend,
    ) -> Result<(), JobProducerError> {
        let mut queue = Self::acquire_lock(&self.queue)?;
        let job = Job::new(JobType::TransactionStatusCheck, notification_send_job);

        queue.notification_queue.push(job).await?;

        info!("Notification Send job produced successfully");

        Ok(())
    }
}
