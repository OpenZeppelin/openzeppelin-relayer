use apalis_redis::RedisStorage;
use serde::{Deserialize, Serialize};

use crate::config::ServerConfig;

use super::{Job, NotificationSend, TransactionRequest, TransactionStatusCheck, TransactionSubmit};

#[derive(Clone, Debug)]
pub struct Queue {
    pub transaction_request_queue: RedisStorage<Job<TransactionRequest>>,
    pub transaction_submission_queue: RedisStorage<Job<TransactionSubmit>>,
    pub transaction_status_queue: RedisStorage<Job<TransactionStatusCheck>>,
    pub notification_queue: RedisStorage<Job<NotificationSend>>,
}

impl Queue {
    async fn storage<T: Serialize + for<'de> Deserialize<'de>>() -> RedisStorage<T> {
        let conn = apalis_redis::connect(ServerConfig::from_env().redis_url.clone())
            .await
            .expect("Could not connect to Redis Jobs DB");
        RedisStorage::new(conn)
    }

    pub async fn setup() -> Self {
        Self {
            transaction_request_queue: Self::storage().await,
            transaction_submission_queue: Self::storage().await,
            transaction_status_queue: Self::storage().await,
            notification_queue: Self::storage().await,
        }
    }
}
