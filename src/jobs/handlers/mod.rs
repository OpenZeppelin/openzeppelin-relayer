use std::sync::Arc;

use apalis::prelude::{Attempt, Error};
use eyre::Report;

mod transaction_request_handler;
use log::info;
pub use transaction_request_handler::*;

mod transaction_submission_handler;
pub use transaction_submission_handler::*;

mod notification_handler;
pub use notification_handler::*;

mod transaction_status_handler;
pub use transaction_status_handler::*;

pub fn handle_result(
    result: Result<(), Report>,
    attempt: Attempt,
    job_type: &str,
    max_attempts: usize,
) -> Result<(), Error> {
    if result.is_ok() {
        info!("{} request handled successfully", job_type);
        return Ok(());
    }
    info!("{} request failed: {:?}", job_type, result);

    if attempt.current() >= max_attempts {
        info!("Max attempts ({}) reached, failing job", max_attempts);
        Err(Error::Abort(Arc::new("Failed to handle request".into())))?
    }

    Err(Error::Failed(Arc::new(
        "Failed to handle request. Retrying".into(),
    )))?
}
