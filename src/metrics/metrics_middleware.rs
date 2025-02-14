//! This defines the Middleware to collect metrics for the application.
//! This middleware will increment the request counter for each request for each endpoint.

use crate::metrics::REQUEST_COUNTER;
use actix_web::{
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::future::{ok, LocalBoxFuture, Ready};
use std::task::{Context, Poll};

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
        ok(MetricsMiddlewareService { service })
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

        // Increment the metric with the endpoint as a label.
        REQUEST_COUNTER.with_label_values(&[&endpoint]).inc();

        let fut = self.service.call(req);
        Box::pin(async move {
            let res = fut.await?;
            Ok(res)
        })
    }
}
