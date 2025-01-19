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

pub async fn initialise_workers(app_state: ThinData<AppState>) {
    setup_workers(app_state).await;
}
