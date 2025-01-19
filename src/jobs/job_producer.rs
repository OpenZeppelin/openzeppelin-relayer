use crate::{
    jobs::{
        Job, NotificationSend, Queue, TransactionProcess, TransactionStatusCheck, TransactionSubmit,
    },
    models::RepositoryError,
};
use apalis::prelude::Storage;
use parking_lot::{Mutex, MutexGuard};

#[derive(Debug)]
pub struct JobProducer {
    queue: Mutex<Queue>,
}

impl Clone for JobProducer {
    fn clone(&self) -> Self {
        let queue_clone = self.queue.lock().clone();
        JobProducer {
            queue: Mutex::new(queue_clone),
        }
    }
}

impl JobProducer {
    pub fn new(queue: Queue) -> Self {
        Self {
            queue: Mutex::new(queue),
        }
    }

    pub fn get_queue(&self) -> Result<Queue, RepositoryError> {
        let queue_guard = Self::acquire_lock(&self.queue)?;
        Ok(queue_guard.clone())
    }

    fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock())
    }

    pub async fn handle_transaction(
        &self,
        transaction_process_job: Job<TransactionProcess>,
    ) -> Result<(), RepositoryError> {
        let mut queue = Self::acquire_lock(&self.queue)?;

        queue
            .transaction_queue
            .push(transaction_process_job)
            .await
            .expect("Could not store job");

        Ok(())
    }

    // pub async fn test(
    //     &self,
    //     transaction_process_job: Job<TransactionProcess>,
    // ) -> Result<(), RepositoryError> {
    //     let mut queue = Self::acquire_lock(&self.queue)?;

    //     queue
    //     .transaction_queue
    //     .push(transaction_process_job)
    //     .await
    //     .expect("Could not store job");

    //     Ok(())
    // }

    pub async fn submit_transaction(
        &mut self,
        transaction_submit_job: Job<TransactionSubmit>,
    ) -> Result<(), RepositoryError> {
        let mut queue = Self::acquire_lock(&self.queue)?;

        queue
            .submission_queue
            .push(transaction_submit_job)
            .await
            .expect("Could not store job");

        Ok(())
    }

    pub async fn check_transaction_status(
        &mut self,
        transaction_status_check_job: Job<TransactionStatusCheck>,
    ) -> Result<(), RepositoryError> {
        let mut queue = Self::acquire_lock(&self.queue)?;

        queue
            .status_queue
            .push(transaction_status_check_job)
            .await
            .expect("Could not store job");

        Ok(())
    }

    pub async fn send_notification(
        &mut self,
        notification_send_job: Job<NotificationSend>,
    ) -> Result<(), RepositoryError> {
        let mut queue = Self::acquire_lock(&self.queue)?;

        queue
            .notification_queue
            .push(notification_send_job)
            .await
            .expect("Could not store job");

        Ok(())
    }
}
