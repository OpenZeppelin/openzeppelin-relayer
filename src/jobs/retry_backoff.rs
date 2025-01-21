//! Retry policy implementation with exponential backoff for job processing.
//!
//! This module provides a configurable retry mechanism with exponential backoff
//! for handling transient failures in job processing.
//!
//! # Example
//! ```rust
//! use crate::jobs::workers::retry_backoff::BackoffRetryPolicy;
//! use std::time::Duration;
//!
//! let policy = BackoffRetryPolicy {
//!     retries: 5,
//!     initial_backoff: Duration::from_secs(1),
//!     multiplier: 2.0,
//!     max_backoff: Duration::from_secs(60),
//! };
//! ```

use apalis::prelude::*;
use std::time::Duration;
use tokio::time::{sleep, Sleep};
use tower::retry::Policy;

type Req<T, Ctx> = Request<T, Ctx>;
type Err = Error;

/// A retry policy that implements exponential backoff.
///
/// This policy will retry failed operations with increasing delays between attempts,
/// using an exponential backoff algorithm with a configurable multiplier.
///
/// # Fields
///
/// * `retries` - Maximum number of retry attempts
/// * `initial_backoff` - Initial delay duration before first retry
/// * `multiplier` - Factor by which the delay increases after each attempt
/// * `max_backoff` - Maximum delay duration between retries
///
/// # Example
///
/// ```rust
/// let policy = BackoffRetryPolicy {
///     retries: 5,
///     initial_backoff: Duration::from_secs(1),
///     multiplier: 2.0,
///     max_backoff: Duration::from_secs(60),
/// };
/// ```
#[derive(Clone, Debug)]
pub struct BackoffRetryPolicy {
    /// Maximum number of retry attempts
    pub retries: usize,
    /// Initial delay duration before first retry
    pub initial_backoff: Duration,
    /// Factor by which the delay increases after each attempt
    pub multiplier: f64,
    /// Maximum delay duration between retries
    pub max_backoff: Duration,
}

/// Default configuration for retry policy
impl Default for BackoffRetryPolicy {
    fn default() -> Self {
        Self {
            retries: 5,
            initial_backoff: Duration::from_millis(1000),
            multiplier: 1.5,
            max_backoff: Duration::from_secs(60),
        }
    }
}

impl BackoffRetryPolicy {
    fn backoff_duration(&self, attempt: usize) -> Duration {
        let backoff =
            self.initial_backoff.as_millis() as f64 * self.multiplier.powi(attempt as i32);
        Duration::from_millis(backoff.min(self.max_backoff.as_millis() as f64) as u64)
    }
}

impl<T, Res, Ctx> Policy<Req<T, Ctx>, Res, Err> for BackoffRetryPolicy
where
    T: Clone,
    Ctx: Clone,
{
    type Future = Sleep;

    fn retry(
        &mut self,
        req: &mut Req<T, Ctx>,
        result: &mut Result<Res, Err>,
    ) -> Option<Self::Future> {
        let attempt = req.parts.attempt.current();

        match result {
            Ok(_) => None,
            Err(_) if (self.retries - attempt > 0) => Some(sleep(self.backoff_duration(attempt))),
            Err(_) => None,
        }
    }

    fn clone_request(&mut self, req: &Req<T, Ctx>) -> Option<Req<T, Ctx>> {
        let req = req.clone();
        req.parts.attempt.increment();
        Some(req)
    }
}
