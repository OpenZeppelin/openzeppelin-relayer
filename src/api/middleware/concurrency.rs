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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, web, App, HttpResponse};
    use std::rc::Rc;
    use std::time::Duration;
    use tokio::time::sleep;

    #[actix_rt::test]
    async fn test_concurrency_limiter_allows_requests_within_limit() {
        let app = test::init_service(App::new().wrap(ConcurrencyLimiter::new(2)).route(
            "/test",
            web::get().to(|| async { HttpResponse::Ok().body("ok") }),
        ))
        .await;

        // Make 2 sequential requests - both should succeed since limit is 2
        let req1 = test::TestRequest::get().uri("/test").to_request();
        let req2 = test::TestRequest::get().uri("/test").to_request();

        let resp1 = test::call_service(&app, req1).await;
        let resp2 = test::call_service(&app, req2).await;

        assert!(resp1.status().is_success());
        assert!(resp2.status().is_success());
    }

    #[actix_rt::test]
    async fn test_concurrency_limiter_rejects_excess_requests() {
        use tokio::sync::mpsc;
        use tokio::task;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let app = Rc::new(
            test::init_service(App::new().wrap(ConcurrencyLimiter::new(1)).route(
                "/slow",
                web::get().to(move || {
                    let tx = tx.clone();
                    async move {
                        // Signal that we've started processing
                        let _ = tx.send(());
                        sleep(Duration::from_millis(100)).await;
                        HttpResponse::Ok().body("ok")
                    }
                }),
            ))
            .await,
        );

        // Start a slow request that will hold the permit
        let app_clone = Rc::clone(&app);
        let req1 = test::TestRequest::get().uri("/slow").to_request();
        let resp1_handle =
            task::spawn_local(async move { test::call_service(&*app_clone, req1).await });

        // Wait for confirmation that the first request acquired the permit
        rx.recv().await.unwrap();

        // Try to make another request while the first is still processing
        let req2 = test::TestRequest::get().uri("/slow").to_request();
        let resp2_result = test::try_call_service(&*app, req2).await;

        // Second request should be rejected with 429
        assert!(resp2_result.is_err());
        let err = resp2_result.unwrap_err();
        assert_eq!(err.as_response_error().status_code().as_u16(), 429);

        // Wait for first request to complete
        let resp1 = resp1_handle.await.unwrap();
        assert!(resp1.status().is_success());
    }

    #[actix_rt::test]
    async fn test_concurrency_limiter_releases_permits_after_completion() {
        let app = test::init_service(App::new().wrap(ConcurrencyLimiter::new(1)).route(
            "/test",
            web::get().to(|| async { HttpResponse::Ok().body("ok") }),
        ))
        .await;

        // Make first request
        let req1 = test::TestRequest::get().uri("/test").to_request();
        let resp1 = test::call_service(&app, req1).await;
        assert!(resp1.status().is_success());

        // After first request completes, permit should be released
        // Make second request - should succeed
        let req2 = test::TestRequest::get().uri("/test").to_request();
        let resp2 = test::call_service(&app, req2).await;
        assert!(resp2.status().is_success());
    }

    #[actix_rt::test]
    async fn test_concurrency_limiter_error_response_format() {
        use tokio::sync::mpsc;
        use tokio::task;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let app = Rc::new(
            test::init_service(App::new().wrap(ConcurrencyLimiter::new(1)).route(
                "/slow",
                web::get().to(move || {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send(());
                        sleep(Duration::from_millis(100)).await;
                        HttpResponse::Ok().body("ok")
                    }
                }),
            ))
            .await,
        );

        // Start a slow request
        let app_clone = Rc::clone(&app);
        let req1 = test::TestRequest::get().uri("/slow").to_request();
        let _resp1_handle =
            task::spawn_local(async move { test::call_service(&*app_clone, req1).await });

        // Wait for confirmation that the permit was acquired
        rx.recv().await.unwrap();

        // Try to make another request
        let req2 = test::TestRequest::get().uri("/slow").to_request();
        let resp2_result = test::try_call_service(&*app, req2).await;

        assert!(resp2_result.is_err());
        let err = resp2_result.unwrap_err();
        assert_eq!(err.as_response_error().status_code().as_u16(), 429);

        // Check error response body contains expected JSON
        let error_str = format!("{}", err);
        assert!(error_str.contains("Too many concurrent requests"));
        assert!(error_str.contains("max_concurrent"));

        // Verify it's valid JSON
        let json: serde_json::Value = serde_json::from_str(&error_str).unwrap();
        assert_eq!(json["max_concurrent"], 1);
    }

    #[actix_rt::test]
    async fn test_concurrency_limiter_handles_multiple_endpoints() {
        use tokio::sync::mpsc;
        use tokio::task;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let app = Rc::new(
            test::init_service(
                App::new()
                    .wrap(ConcurrencyLimiter::new(1))
                    .route(
                        "/endpoint1",
                        web::get().to(move || {
                            let tx = tx.clone();
                            async move {
                                let _ = tx.send(());
                                sleep(Duration::from_millis(100)).await;
                                HttpResponse::Ok().body("ok1")
                            }
                        }),
                    )
                    .route(
                        "/endpoint2",
                        web::get().to(|| async { HttpResponse::Ok().body("ok2") }),
                    ),
            )
            .await,
        );

        // Start request to endpoint1 that holds the permit
        let app_clone = Rc::clone(&app);
        let req1 = test::TestRequest::get().uri("/endpoint1").to_request();
        let resp1_handle =
            task::spawn_local(async move { test::call_service(&*app_clone, req1).await });

        // Wait for endpoint1 to acquire the permit
        rx.recv().await.unwrap();

        // Try to make concurrent request to endpoint2 - should be rejected (global limit)
        let req2 = test::TestRequest::get().uri("/endpoint2").to_request();
        let resp2_result = test::try_call_service(&*app, req2).await;
        assert!(
            resp2_result.is_err(),
            "Global limit should apply across endpoints"
        );
        let err = resp2_result.unwrap_err();
        assert_eq!(err.as_response_error().status_code().as_u16(), 429);

        // Wait for first request to complete
        let resp1 = resp1_handle.await.unwrap();
        assert!(resp1.status().is_success());

        // Now endpoint2 should succeed
        let req3 = test::TestRequest::get().uri("/endpoint2").to_request();
        let resp3 = test::call_service(&*app, req3).await;
        assert!(resp3.status().is_success());
    }

    #[actix_rt::test]
    async fn test_concurrency_limiter_with_zero_limit() {
        let app = test::init_service(App::new().wrap(ConcurrencyLimiter::new(0)).route(
            "/test",
            web::get().to(|| async { HttpResponse::Ok().body("ok") }),
        ))
        .await;

        // With limit of 0, all requests should be rejected
        let req = test::TestRequest::get().uri("/test").to_request();
        let resp_result = test::try_call_service(&app, req).await;

        assert!(resp_result.is_err());
        let err = resp_result.unwrap_err();
        assert_eq!(err.as_response_error().status_code().as_u16(), 429);
    }

    #[actix_rt::test]
    async fn test_concurrency_limiter_metrics_tracking() {
        use tokio::sync::mpsc;
        use tokio::sync::Barrier;
        use tokio::task;

        // Reset metrics before test
        IN_FLIGHT_REQUESTS
            .with_label_values(&["/metrics-test"])
            .set(0.0);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let barrier = Rc::new(Barrier::new(2));
        let barrier_clone = Rc::clone(&barrier);

        let app = Rc::new(
            test::init_service(App::new().wrap(ConcurrencyLimiter::new(2)).route(
                "/metrics-test",
                web::get().to(move || {
                    let tx = tx.clone();
                    let barrier = barrier_clone.clone();
                    async move {
                        let _ = tx.send(());
                        barrier.wait().await;
                        HttpResponse::Ok().body("ok")
                    }
                }),
            ))
            .await,
        );

        let initial_value = IN_FLIGHT_REQUESTS
            .with_label_values(&["/metrics-test"])
            .get();
        assert_eq!(initial_value, 0.0, "Should start at 0");

        // Start a request that will wait
        let app_clone = Rc::clone(&app);
        let req1 = test::TestRequest::get().uri("/metrics-test").to_request();
        let handle = task::spawn_local(async move { test::call_service(&*app_clone, req1).await });

        // Wait until request is processing
        rx.recv().await.unwrap();

        // Metric should be incremented while request is in flight
        let in_flight = IN_FLIGHT_REQUESTS
            .with_label_values(&["/metrics-test"])
            .get();
        assert_eq!(in_flight, 1.0, "Should be 1 while request is in flight");

        // Release the barrier to let request complete
        barrier.wait().await;

        // Wait for request to finish
        let resp = handle.await.unwrap();
        assert!(resp.status().is_success());

        // After completion, should be back to 0
        let final_value = IN_FLIGHT_REQUESTS
            .with_label_values(&["/metrics-test"])
            .get();
        assert_eq!(final_value, 0.0, "Should be back to 0 after completion");
    }
}
