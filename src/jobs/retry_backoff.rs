//! Retry policy implementation with exponential backoff for job processing.
//!
//! This module provides a configurable retry mechanism with exponential backoff
//! for handling transient failures in job processing.
//!
//! # Example
//! ```rust, ignore
//! use crate::jobs::retry_backoff::BackoffRetryPolicy;
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
/// ```rust, ignore
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestJob;

    #[tokio::test]
    async fn test_backoff_duration_calculation() {
        let policy = BackoffRetryPolicy {
            retries: 5,
            initial_backoff: Duration::from_secs(1),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
        };

        // Check first few backoff durations
        assert_eq!(policy.backoff_duration(0), Duration::from_secs(1));
        assert_eq!(policy.backoff_duration(1), Duration::from_secs(2));
        assert_eq!(policy.backoff_duration(2), Duration::from_secs(4));
        assert_eq!(policy.backoff_duration(3), Duration::from_secs(8));

        // Test max backoff limit
        let policy = BackoffRetryPolicy {
            retries: 10,
            initial_backoff: Duration::from_secs(10),
            multiplier: 3.0,
            max_backoff: Duration::from_secs(60),
        };

        // This would be 10 * 3^3 = 270 seconds, but should be capped at 60
        assert_eq!(policy.backoff_duration(3), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_retry_policy_success() {
        let mut policy = BackoffRetryPolicy::default();
        let job = TestJob;
        let ctx = ();
        let mut req = Request::new_with_ctx(job, ctx);
        let mut result: Result<(), Err> = Ok(());

        // Should return None for successful results
        assert!(policy.retry(&mut req, &mut result).is_none());
    }

    #[tokio::test]
    async fn test_retry_policy_failure_with_retries() {
        let mut policy = BackoffRetryPolicy {
            retries: 3,
            initial_backoff: Duration::from_millis(10),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(1),
        };

        let job = TestJob;
        let ctx = ();
        let mut req = Request::new_with_ctx(job, ctx);
        let mut result: Result<(), Err> =
            Err(Error::from(Box::new(std::io::Error::other("Test error"))
                as Box<dyn std::error::Error + Send + Sync>));

        // First attempt (0) should return Some(Sleep) since retries > 0
        assert!(policy.retry(&mut req, &mut result).is_some());

        // Simulate first retry
        req.parts.attempt.increment();
        assert!(policy.retry(&mut req, &mut result).is_some());

        // Simulate second retry
        req.parts.attempt.increment();
        assert!(policy.retry(&mut req, &mut result).is_some());

        // Simulate third retry - should be the last one
        req.parts.attempt.increment();
        assert!(policy.retry(&mut req, &mut result).is_none());
    }

    #[tokio::test]
    async fn test_clone_request() {
        let mut policy: BackoffRetryPolicy = BackoffRetryPolicy::default();
        let job = TestJob;
        let ctx = ();
        let req: Request<TestJob, ()> = Request::new_with_ctx(job, ctx);

        // Original request attempt should be 0
        assert_eq!(req.parts.attempt.current(), 0);

        // Cloned request should have attempt incremented to 1
        let cloned_req =
            <BackoffRetryPolicy as Policy<Request<TestJob, ()>, (), Error>>::clone_request(
                &mut policy,
                &req,
            )
            .unwrap();
        let cloned_req_attempt = cloned_req.parts.attempt.current();
        assert_eq!(cloned_req_attempt, 1);
    }

    #[test]
    fn test_default_policy() {
        let policy = BackoffRetryPolicy::default();

        assert_eq!(policy.retries, 5);
        assert_eq!(policy.initial_backoff, Duration::from_millis(1000));
        assert_eq!(policy.multiplier, 1.5);
        assert_eq!(policy.max_backoff, Duration::from_secs(60));
    }

    #[test]
    fn test_zero_initial_backoff() {
        let policy = BackoffRetryPolicy {
            retries: 3,
            initial_backoff: Duration::from_millis(0),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
        };

        // With zero initial backoff, all durations should be zero
        assert_eq!(policy.backoff_duration(0), Duration::from_millis(0));
        assert_eq!(policy.backoff_duration(1), Duration::from_millis(0));
        assert_eq!(policy.backoff_duration(2), Duration::from_millis(0));
    }

    #[test]
    fn test_multiplier_one() {
        let policy = BackoffRetryPolicy {
            retries: 3,
            initial_backoff: Duration::from_millis(500),
            multiplier: 1.0,
            max_backoff: Duration::from_secs(60),
        };

        // With multiplier of 1.0, all durations should be the same as initial
        assert_eq!(policy.backoff_duration(0), Duration::from_millis(500));
        assert_eq!(policy.backoff_duration(1), Duration::from_millis(500));
        assert_eq!(policy.backoff_duration(2), Duration::from_millis(500));
    }

    #[test]
    fn test_multiplier_less_than_one() {
        let policy = BackoffRetryPolicy {
            retries: 3,
            initial_backoff: Duration::from_millis(1000),
            multiplier: 0.5,
            max_backoff: Duration::from_secs(60),
        };

        // With multiplier < 1.0, each duration should be less than the previous
        assert_eq!(policy.backoff_duration(0), Duration::from_millis(1000));
        assert_eq!(policy.backoff_duration(1), Duration::from_millis(500));
        assert_eq!(policy.backoff_duration(2), Duration::from_millis(250));
    }

    #[tokio::test]
    async fn test_retry_policy_exhausted_retries() {
        let mut policy = BackoffRetryPolicy {
            retries: 0, // No retries allowed
            initial_backoff: Duration::from_millis(10),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(1),
        };

        let job = TestJob;
        let ctx = ();
        let mut req = Request::new_with_ctx(job, ctx);
        let mut result: Result<(), Err> =
            Err(Error::from(Box::new(std::io::Error::other("Test error"))
                as Box<dyn std::error::Error + Send + Sync>));

        // Should return None immediately because retries=0
        assert!(policy.retry(&mut req, &mut result).is_none());
    }

    #[tokio::test]
    async fn test_retry_policy_large_max_backoff() {
        let policy = BackoffRetryPolicy {
            retries: 10,
            initial_backoff: Duration::from_millis(100),
            multiplier: 10.0,                               // Large multiplier
            max_backoff: Duration::from_secs(24 * 60 * 60), // 24 hours
        };

        // Even with a large multiplier, we should never exceed max_backoff
        assert!(policy.backoff_duration(10) <= Duration::from_secs(24 * 60 * 60));
    }
}
