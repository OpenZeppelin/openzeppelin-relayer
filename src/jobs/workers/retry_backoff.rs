use apalis::prelude::*;
use std::time::Duration;
use tokio::time::{sleep, Sleep};
use tower::retry::Policy;

type Req<T, Ctx> = Request<T, Ctx>;
type Err = Error;

#[derive(Clone, Debug)]
pub struct BackoffRetryPolicy {
    pub retries: usize,
    pub initial_backoff: Duration,
    pub multiplier: f64,
    pub max_backoff: Duration,
}

impl Default for BackoffRetryPolicy {
    fn default() -> Self {
        Self {
            retries: 25,
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
