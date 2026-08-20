use std::{sync::Arc, time::Duration};

use apalis::{
    layers::retry::{backoff::Backoff, RetryPolicyError},
    prelude::{Error, Request},
};
use futures::{future::BoxFuture, FutureExt};
use tower::retry::Policy;

use crate::{
    jobs::{Job, TransactionStatusCheck},
    queues::{worker_shared::configured_status_retry_delay, NotYetFinal},
};

/// Retry policy for EVM status checks.
///
/// Healthy, non-final checks may use the delay captured in the job. Every other
/// error uses the existing per-request exponential backoff.
#[derive(Clone, Debug)]
pub(crate) struct EvmStatusRetryPolicy<B> {
    retries: usize,
    backoff: B,
}

impl<B> EvmStatusRetryPolicy<B> {
    pub(crate) fn new(retries: usize, backoff: B) -> Self {
        Self { retries, backoff }
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
        let attempt = req.parts.attempt.current();
        let error = match result.as_mut() {
            Ok(_) | Err(Error::Abort(_)) => return None,
            Err(error) => error,
        };

        if self.retries == 0 {
            *error = Error::Abort(Arc::new(Box::new(RetryPolicyError::ZeroRetries(
                error.clone(),
            ))));
            return None;
        }

        if self.retries < attempt {
            *error = Error::Abort(Arc::new(Box::new(RetryPolicyError::OutOfRetries {
                current_attempt: attempt,
                inner: error.clone(),
            })));
            return None;
        }

        let counter = req.parts.attempt.clone();
        if let Some(delay) = configured_retry_delay(req, error) {
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

fn configured_retry_delay<Ctx>(
    req: &Request<Job<TransactionStatusCheck>, Ctx>,
    error: &Error,
) -> Option<Duration> {
    let is_not_yet_final = match error {
        Error::Failed(inner) => inner
            .as_ref()
            .as_ref()
            .downcast_ref::<NotYetFinal>()
            .is_some(),
        _ => false,
    };

    if !is_not_yet_final {
        return None;
    }

    configured_status_retry_delay(
        req.args.data.status_retry_delay_seconds,
        req.args.data.network_type,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NetworkType;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        retry_delay: Option<u32>,
    ) -> Request<Job<TransactionStatusCheck>, ()> {
        Request::new(Job::new(
            crate::jobs::JobType::TransactionStatusCheck,
            TransactionStatusCheck::new("tx", "relayer", network_type)
                .with_status_retry_delay_seconds(retry_delay.map(u64::from)),
        ))
    }

    fn not_yet_final_error() -> Error {
        crate::queues::HandlerError::NotYetFinal.into()
    }

    #[test]
    fn configured_delay_requires_valid_evm_not_yet_final_job() {
        let req = request(NetworkType::Evm, Some(2));
        let delay = configured_retry_delay(&req, &not_yet_final_error()).unwrap();
        assert!(delay >= Duration::from_secs(2));
        assert!(delay < Duration::from_secs(3));

        let ordinary = Error::Failed(Arc::new("rpc unavailable".to_string().into()));
        assert_eq!(configured_retry_delay(&req, &ordinary), None);
        assert_eq!(
            configured_retry_delay(
                &request(NetworkType::Stellar, Some(2)),
                &not_yet_final_error()
            ),
            None
        );
        for delay in [None, Some(0), Some(101)] {
            assert_eq!(
                configured_retry_delay(&request(NetworkType::Evm, delay), &not_yet_final_error()),
                None
            );
        }
    }

    #[tokio::test]
    async fn policy_keeps_configured_checks_out_of_rpc_backoff_state() {
        tokio::time::pause();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut policy = EvmStatusRetryPolicy::new(usize::MAX, ImmediateBackoff(calls.clone()));
        let mut req = request(NetworkType::Evm, Some(2));

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
        assert!(elapsed >= Duration::from_secs(2));
        assert!(elapsed < Duration::from_secs(3));
        assert_eq!(req.parts.attempt.current(), 2);

        let mut rpc_error = Err::<(), _>(Error::Failed(Arc::new(
            "rpc unavailable".to_string().into(),
        )));
        policy.retry(&mut req, &mut rpc_error).unwrap().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(req.parts.attempt.current(), 3);
    }

    #[test]
    fn policy_preserves_abort_and_exhaustion_semantics() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut req = request(NetworkType::Evm, Some(2));
        let mut policy = EvmStatusRetryPolicy::new(1, ImmediateBackoff(calls.clone()));
        let mut abort = Err::<(), _>(Error::Abort(Arc::new("stop".to_string().into())));
        assert!(policy.retry(&mut req, &mut abort).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        req.parts.attempt.increment();
        req.parts.attempt.increment();
        let mut exhausted = Err::<(), _>(not_yet_final_error());
        assert!(policy.retry(&mut req, &mut exhausted).is_none());
        assert!(matches!(exhausted, Err(Error::Abort(_))));

        let mut zero_policy = EvmStatusRetryPolicy::new(0, ImmediateBackoff(calls));
        let mut zero = Err::<(), _>(not_yet_final_error());
        assert!(zero_policy
            .retry(&mut request(NetworkType::Evm, Some(2)), &mut zero)
            .is_none());
        assert!(matches!(zero, Err(Error::Abort(_))));
    }
}
