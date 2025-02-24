//! This defines the Middleware to collect metrics for the application.
//! This middleware will increment the request counter for each request for each endpoint.

use crate::metrics::{ERROR_COUNTER, RAW_REQUEST_COUNTER, REQUEST_COUNTER, REQUEST_LATENCY};
use actix_web::{
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::future::{LocalBoxFuture, Ready};
use std::{
    task::{Context, Poll},
    time::Instant,
};

pub struct MetricsMiddleware;

/// Trait implementation for the MetricsMiddleware.
impl<S, B> Transform<S, ServiceRequest> for MetricsMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = MetricsMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        futures::future::ready(Ok(MetricsMiddlewareService { service }))
    }
}

pub struct MetricsMiddlewareService<S> {
    service: S,
}

/// Trait implementation for the MetricsMiddlewareService.
impl<S, B> Service<ServiceRequest> for MetricsMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    // Poll the service
    fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    // Call function to increment the request counter.
    fn call(&self, req: ServiceRequest) -> Self::Future {
        // Get the registered routes for the request.

        // If not available, fall back to the raw path.
        let endpoint = req
            .match_pattern()
            .unwrap_or_else(|| req.path().to_string());

        // Get the HTTP method.
        let method = req.method().to_string();

        // Capture the raw URI.
        let raw_uri = req.path().to_string();

        // Start timer for latency.
        let start_time = Instant::now();

        let fut = self.service.call(req);
        Box::pin(async move {
            let res = fut.await;
            // Compute elapsed time in seconds.
            let elapsed = start_time.elapsed().as_secs_f64();

            // Status code for success and error.
            let status = match &res {
                Ok(response) => response.response().status().to_string(),
                Err(e) => e.as_response_error().status_code().to_string(),
            };

            // Add latency in histogram
            REQUEST_LATENCY
                .with_label_values(&[&endpoint, &method, &status])
                .observe(elapsed);

            match &res {
                Ok(_) => {
                    REQUEST_COUNTER
                        .with_label_values(&[&endpoint, &method, &status])
                        .inc();
                }
                Err(_) => {
                    // Increment the error counter.
                    ERROR_COUNTER
                        .with_label_values(&[&endpoint, &method, &status])
                        .inc();
                }
            }
            // May be cardinality explosion here, but it's useful for debugging.
            RAW_REQUEST_COUNTER
                .with_label_values(&[&raw_uri, &method, &status])
                .inc();
            res
        })
    }
}
