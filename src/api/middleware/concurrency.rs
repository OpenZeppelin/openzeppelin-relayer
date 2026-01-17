use actix_web::{
    body::MessageBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::future::{ready, LocalBoxFuture, Ready};
use std::{
    rc::Rc,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::Semaphore;

use crate::metrics::IN_FLIGHT_REQUESTS;

/// Middleware that limits concurrent requests using a semaphore
pub struct ConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
    max_permits: usize,
}

impl ConcurrencyLimiter {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_permits: max_concurrent,
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for ConcurrencyLimiter
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = ConcurrencyLimiterService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(ConcurrencyLimiterService {
            service: Rc::new(service),
            semaphore: Arc::clone(&self.semaphore),
            max_permits: self.max_permits,
        }))
    }
}

pub struct ConcurrencyLimiterService<S> {
    service: Rc<S>,
    semaphore: Arc<Semaphore>,
    max_permits: usize,
}

impl<S, B> Service<ServiceRequest> for ConcurrencyLimiterService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let service = Rc::clone(&self.service);
        let semaphore = Arc::clone(&self.semaphore);
        let max_permits = self.max_permits;

        Box::pin(async move {
            let endpoint = req.path().to_string();

            // Try to acquire a permit
            let permit = match semaphore.try_acquire() {
                Ok(permit) => permit,
                Err(_) => {
                    // No permits available
                    let current_in_flight = max_permits;
                    tracing::warn!(
                        "Concurrency limit reached for {}: {} requests in flight",
                        endpoint,
                        current_in_flight
                    );

                    return Err(actix_web::error::ErrorTooManyRequests(
                        serde_json::json!({
                            "error": "Too many concurrent requests",
                            "max_concurrent": max_permits,
                        })
                        .to_string(),
                    ));
                }
            };

            // Increment in-flight counter
            IN_FLIGHT_REQUESTS.with_label_values(&[&endpoint]).inc();

            // Call the service
            let result = service.call(req).await;

            // Decrement in-flight counter when done
            IN_FLIGHT_REQUESTS.with_label_values(&[&endpoint]).dec();

            // Permit is automatically dropped here
            drop(permit);

            result
        })
    }
}
