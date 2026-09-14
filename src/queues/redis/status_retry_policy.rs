//! Retry policy for the Redis EVM status-check queue.
//!
//! Healthy, non-final checks (`NotYetFinal`) sleep for the interval stamped on the
//! job when the network configures one. Every other failure keeps the stock
//! per-request exponential backoff.

use std::time::Duration;

use apalis::{
    layers::retry::backoff::Backoff,
    prelude::{Error, Request},
};
use futures::{future::BoxFuture, FutureExt};
use tower::retry::Policy;

use crate::{
    jobs::{Job, TransactionStatusCheck},
    queues::{worker_shared::configured_evm_status_retry_delay, worker_types::NotYetFinal},
};

/// Retry policy for EVM status checks.
///
/// Healthy, non-final checks may use the delay captured in the job. Every other
/// error uses the existing per-request exponential backoff.
/// Status-check retries are unbounded (see `QueueType::max_retries`); exhaustion is not handled here.
#[derive(Clone, Debug)]
pub(crate) struct EvmStatusRetryPolicy<B> {
    backoff: B,
}

impl<B> EvmStatusRetryPolicy<B> {
    pub(crate) fn new(backoff: B) -> Self {
        Self { backoff }
    }
}

impl<Res, Ctx, B> Policy<Request<Job<TransactionStatusCheck>, Ctx>, Res, Error>
    for EvmStatusRetryPolicy<B>
where
    Ctx: Clone,
    B: Backoff,
    B::Future: Send + 'static,
{
    type Future = BoxFuture<'static, ()>;

    fn retry(
        &mut self,
        req: &mut Request<Job<TransactionStatusCheck>, Ctx>,
        result: &mut Result<Res, Error>,
    ) -> Option<Self::Future> {
        let error = match result.as_mut() {
            Ok(_) | Err(Error::Abort(_)) => return None,
            Err(error) => error,
        };

        let counter = req.parts.attempt.clone();
        if let Some(delay) = not_yet_final_delay(req, error) {
            Some(Box::pin(async move {
                tokio::time::sleep(delay).await;
                counter.increment();
            }))
        } else {
            Some(
                self.backoff
                    .next_backoff()
                    .map(move |_| {
                        counter.increment();
                    })
                    .boxed(),
            )
        }
    }

    fn clone_request(
        &mut self,
        req: &Request<Job<TransactionStatusCheck>, Ctx>,
    ) -> Option<Request<Job<TransactionStatusCheck>, Ctx>> {
        Some(req.clone())
    }
}

fn not_yet_final_delay<Ctx>(
    req: &Request<Job<TransactionStatusCheck>, Ctx>,
    error: &Error,
) -> Option<Duration> {
    let Error::Failed(inner) = error else {
        return None;
    };
    inner.as_ref().as_ref().downcast_ref::<NotYetFinal>()?;
    configured_evm_status_retry_delay(
        req.args.data.status_check_retry_delay_seconds,
        req.args.data.network_type,
    )
    .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NetworkType, TransactionStatus};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Clone, Debug)]
    struct ImmediateBackoff(Arc<AtomicUsize>);

    impl Backoff for ImmediateBackoff {
        type Future = std::future::Ready<()>;

        fn next_backoff(&mut self) -> Self::Future {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::future::ready(())
        }
    }

    fn request(
        network_type: NetworkType,
        retry_delay: Option<u64>,
    ) -> Request<Job<TransactionStatusCheck>, ()> {
        Request::new(Job::new(
            crate::jobs::JobType::TransactionStatusCheck,
            TransactionStatusCheck::new("tx", "relayer", network_type)
                .with_status_check_retry_delay_seconds(retry_delay),
        ))
    }

    fn not_yet_final_error() -> Error {
        crate::queues::HandlerError::NotYetFinal(TransactionStatus::Submitted).into()
    }

    #[test]
    fn test_not_yet_final_delay_requires_valid_evm_job() {
        let req = request(NetworkType::Evm, Some(5));
        let delay = not_yet_final_delay(&req, &not_yet_final_error()).unwrap();
        assert_eq!(delay, Duration::from_secs(5));

        let ordinary = Error::Failed(Arc::new("rpc unavailable".to_string().into()));
        assert_eq!(not_yet_final_delay(&req, &ordinary), None);
        assert_eq!(
            not_yet_final_delay(
                &request(NetworkType::Stellar, Some(100)),
                &not_yet_final_error()
            ),
            None
        );
        assert_eq!(
            not_yet_final_delay(
                &request(NetworkType::Solana, Some(100)),
                &not_yet_final_error()
            ),
            None
        );
        assert_eq!(
            not_yet_final_delay(&request(NetworkType::Evm, Some(4)), &not_yet_final_error()),
            None
        );
    }

    #[tokio::test]
    async fn test_policy_keeps_configured_checks_out_of_rpc_backoff_state() {
        tokio::time::pause();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut policy = EvmStatusRetryPolicy::new(ImmediateBackoff(calls.clone()));
        let mut req = request(NetworkType::Evm, Some(5));

        let mut rpc_error = Err::<(), _>(Error::Failed(Arc::new(
            "rpc unavailable".to_string().into(),
        )));
        policy.retry(&mut req, &mut rpc_error).unwrap().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(req.parts.attempt.current(), 1);

        let mut not_final = Err::<(), _>(not_yet_final_error());
        let started = tokio::time::Instant::now();
        let configured_sleep = policy.retry(&mut req, &mut not_final).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        configured_sleep.await;
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_secs(5));
        assert!(elapsed < Duration::from_secs(6));
        assert_eq!(req.parts.attempt.current(), 2);

        let mut rpc_error = Err::<(), _>(Error::Failed(Arc::new(
            "rpc unavailable".to_string().into(),
        )));
        policy.retry(&mut req, &mut rpc_error).unwrap().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(req.parts.attempt.current(), 3);
    }

    #[test]
    fn test_policy_does_not_retry_abort() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut req = request(NetworkType::Evm, Some(5));
        let mut policy = EvmStatusRetryPolicy::new(ImmediateBackoff(calls.clone()));
        let mut abort = Err::<(), _>(Error::Abort(Arc::new("stop".to_string().into())));
        assert!(policy.retry(&mut req, &mut abort).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
