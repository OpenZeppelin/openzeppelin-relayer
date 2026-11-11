//! This defines the Middleware to collect metrics for the application.
//! This middleware will increment the request counter for each request for each endpoint.

use crate::metrics::{ERROR_COUNTER, REQUEST_COUNTER, REQUEST_LATENCY};
use actix_web::{
    dev::{Service, ServiceRequest, ServiceResponse, Transform},
    Error,
};
use futures::future::{LocalBoxFuture, Ready};
use std::{
    task::{Context, Poll},
    time::Instant,
};

/// Label value shared by every request that matches no registered route.
pub const UNMATCHED_ROUTE_LABEL: &str = "unmatched";

/// Returns the endpoint label for a request: its matched route pattern (for example
/// `/api/v1/relayers/{relayer_id}/transactions/{transaction_id}`), never the raw path.
///
/// Labelling by raw path creates a new, never-released series for every resource id
/// (and for every path a scanner probes), so the registry and each scrape grow without
/// bound. Requests that match no route share [`UNMATCHED_ROUTE_LABEL`].
pub fn route_label(req: &ServiceRequest) -> String {
    req.match_pattern()
        .unwrap_or_else(|| UNMATCHED_ROUTE_LABEL.to_string())
}

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
        let endpoint = route_label(&req);

        // Get the HTTP method.
        let method = req.method().to_string();

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
            res
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::REGISTRY;
    use actix_web::{test, web, App, HttpResponse};

    /// Every label value recorded so far, across every registered metric.
    fn label_values() -> Vec<String> {
        REGISTRY
            .gather()
            .iter()
            .flat_map(|family| family.get_metric().iter())
            .flat_map(|metric| metric.get_label().iter())
            .map(|label| label.value().to_string())
            .collect()
    }

    #[actix_rt::test]
    async fn test_requests_for_different_ids_share_one_route_series() {
        let pattern = "/cardinality-test/relayers/{relayer_id}/transactions/{transaction_id}";
        let app = test::init_service(App::new().wrap(MetricsMiddleware).route(
            pattern,
            web::get().to(|| async { HttpResponse::Ok().finish() }),
        ))
        .await;

        let before = REQUEST_COUNTER
            .with_label_values(&[pattern, "GET", "200 OK"])
            .get();
        for tx in ["card-tx-a1", "card-tx-b2", "card-tx-c3"] {
            let uri = format!("/cardinality-test/relayers/card-relayer-7f/transactions/{tx}");
            let req = test::TestRequest::get().uri(&uri).to_request();
            assert!(test::call_service(&app, req).await.status().is_success());
        }

        let after = REQUEST_COUNTER
            .with_label_values(&[pattern, "GET", "200 OK"])
            .get();
        assert_eq!(after - before, 3.0);
        let values = label_values();
        assert!(
            !values
                .iter()
                .any(|v| v.contains("card-tx-") || v.contains("card-relayer-7f")),
            "no metric may carry a resource id: {values:?}"
        );
    }

    #[actix_rt::test]
    async fn test_unmatched_paths_share_one_label() {
        let app = test::init_service(App::new().wrap(MetricsMiddleware).route(
            "/known",
            web::get().to(|| async { HttpResponse::Ok().finish() }),
        ))
        .await;

        for uri in ["/unmatched-probe-1", "/unmatched-probe-2/x"] {
            let req = test::TestRequest::get().uri(uri).to_request();
            assert_eq!(test::call_service(&app, req).await.status().as_u16(), 404);
        }

        let values = label_values();
        assert!(values.iter().any(|v| v == UNMATCHED_ROUTE_LABEL));
        assert!(
            !values.iter().any(|v| v.contains("unmatched-probe")),
            "unmatched paths must not become labels: {values:?}"
        );
    }
}
