// use actix_web::web::ThinData;
// use apalis::{layers::ErrorHandlingLayer, prelude::*};
// use apalis_redis::RedisStorage;
// use eyre::Result;
// use std::time::Duration;

// use crate::{jobs::Jobs, AppState};

// use super::transaction_handler_worker_handler;

// // Worker configuration
// #[derive(Debug, Clone)]
// struct WorkerConfig {
//     name: String,
//     concurrency: usize,
//     rate_limit: u64,
//     rate_limit_duration: Duration,
// }

// impl Default for WorkerConfig {
//     fn default() -> Self {
//         Self {
//             name: String::new(),
//             concurrency: 2,
//             rate_limit: 20,
//             rate_limit_duration: Duration::from_secs(1),
//         }
//     }
// }

// // Worker builder with common configuration
// struct RelayerWorkerBuilder<T>
// where
//     T: serde::Serialize + for<'de> serde::Deserialize<'de> + Send + Sync + Unpin + 'static,
// {
//     config: WorkerConfig,
//     backend: RedisStorage<T>,
//     app_state: ThinData<AppState>,
// }

// impl<T> RelayerWorkerBuilder<T>
// where
//     T: serde::Serialize + for<'de> serde::Deserialize<'de> + Send + Sync + Unpin + 'static,
// {
//     fn new(name: &str, backend: RedisStorage<T>, app_state: ThinData<AppState>) -> Self {
//         Self {
//             config: WorkerConfig {
//                 name: name.to_string(),
//                 ..Default::default()
//             },
//             backend,
//             app_state,
//         }
//     }

//     async fn build<F>(self, handler: F) -> Result<()>
//     where
//         F: Fn(T) -> Result<()> + Send + Sync + 'static,
//     {
//         WorkerBuilder::new(&self.config.name)
//             .layer(ErrorHandlingLayer::new())
//             .enable_tracing()
//             .rate_limit(self.config.rate_limit, self.config.rate_limit_duration)
//             .concurrency(self.config.concurrency)
//             .data(self.app_state)
//             .backend(self.backend.clone())
//             .build_fn(handler);
//         Ok(())
//     }
// }

// pub async fn setup_workers(app_state: ThinData<AppState>) -> Result<()> {
//     let jobs = Jobs::setup().await;

//     // Setup workers with common configuration
//     let handlers = vec![
//         RelayerWorkerBuilder::new(
//             "transaction_handler",
//             jobs.transaction_handler.clone(),
//             app_state.clone(),
//         )
//         .build(transaction_handler_worker_handler),
//         RelayerWorkerBuilder::new(
//             "transaction_sender",
//             jobs.transaction_sender.clone(),
//             app_state.clone(),
//         )
//         .build(transaction_sender_worker_handler),
//         RelayerWorkerBuilder::new(
//             "transaction_status_checker",
//             jobs.transaction_status_checker.clone(),
//             app_state.clone(),
//         )
//         .build(transaction_status_checker_worker_handler),
//         RelayerWorkerBuilder::new(
//             "notification_sender",
//             jobs.notification_sender.clone(),
//             app_state.clone(),
//         )
//         .build(notification_sender_worker_handler),
//     ];

//     // Run all workers concurrently
//     futures::future::try_join_all(handlers).await?;

//     Ok(())
// }
