use actix_web::{
    body::MessageBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::future::{ready, LocalBoxFuture, Ready};
use std::{
    rc::Rc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::timeout;

use crate::metrics::TIMEOUT_COUNTER;

/// Middleware that enforces a timeout on HTTP request handlers
pub struct TimeoutMiddleware {
    duration: Duration,
}

impl TimeoutMiddleware {
    pub fn new(seconds: u64) -> Self {
        Self {
            duration: Duration::from_secs(seconds),
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for TimeoutMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = TimeoutMiddlewareService<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(TimeoutMiddlewareService {
            service: Rc::new(service),
            duration: self.duration,
        }))
    }
}

pub struct TimeoutMiddlewareService<S> {
    service: Rc<S>,
    duration: Duration,
}

impl<S, B> Service<ServiceRequest> for TimeoutMiddlewareService<S>
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
        let duration = self.duration;

        Box::pin(async move {
            let path = req.path().to_string();
            let method = req.method().to_string();

            match timeout(duration, service.call(req)).await {
                Ok(result) => result,
                Err(_) => {
                    // Timeout occurred
                    tracing::warn!(
                        "Request timeout: {} {} exceeded {}s",
                        method,
                        path,
                        duration.as_secs()
                    );

                    // Record timeout metric
                    TIMEOUT_COUNTER
                        .with_label_values(&[path.as_str(), method.as_str(), "handler"])
                        .inc();

                    Err(actix_web::error::ErrorGatewayTimeout(
                        "Request handler timeout",
                    ))
                }
            }
        })
    }
}
