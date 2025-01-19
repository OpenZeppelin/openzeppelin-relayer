use apalis::prelude::Storage;

use crate::jobs::{
    Job, NotificationSend, Queue, TransactionProcess, TransactionStatusCheck, TransactionSubmit,
};

pub struct JobProducer {
    queue: Queue,
}

impl JobProducer {
    pub fn new(queue: Queue) -> Self {
        Self { queue }
    }

    pub async fn handle_transaction(&mut self, transaction_process_job: Job<TransactionProcess>) {
        self.queue
            .transaction_queue
            .push(transaction_process_job)
            .await
            .expect("Could not store job");
    }

    pub async fn submit_transaction(&mut self, transaction_submit_job: Job<TransactionSubmit>) {
        self.queue
            .submission_queue
            .clone()
            .push(transaction_submit_job)
            .await
            .expect("Could not store job");
    }

    pub async fn check_transaction_status(
        &mut self,
        transaction_status_check_job: Job<TransactionStatusCheck>,
    ) {
        self.queue
            .status_queue
            .clone()
            .push(transaction_status_check_job)
            .await
            .expect("Could not store job");
    }

    pub async fn send_notification(&mut self, notification_send_job: Job<NotificationSend>) {
        self.queue
            .notification_queue
            .clone()
            .push(notification_send_job)
            .await
            .expect("Could not store job");
    }
}
