use actix_web::web::ThinData;

mod queue;
use log::info;
pub use queue::*;

mod workers;
pub use workers::*;

mod job_producer;
pub use job_producer::*;

mod job;
pub use job::*;

use crate::AppState;

pub async fn initialise_queue(mut app_state: ThinData<AppState>) -> Queue {
    let queue = Queue::setup().await;

    app_state.with_queue(queue.clone());

    setup_workers(queue.clone(), app_state).await;

    info!("Queue initialised");

    queue
}
