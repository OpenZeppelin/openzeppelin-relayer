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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, web, App, HttpResponse};
    use std::time::Duration;

    #[actix_web::test]
    async fn test_request_completes_before_timeout() {
        let app = test::init_service(App::new().wrap(TimeoutMiddleware::new(5)).route(
            "/fast",
            web::get().to(|| async { HttpResponse::Ok().body("OK") }),
        ))
        .await;

        let req = test::TestRequest::get().uri("/fast").to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_request_exceeds_timeout_returns_error() {
        let app = test::init_service(App::new().wrap(TimeoutMiddleware::new(1)).route(
            "/slow",
            web::get().to(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                HttpResponse::Ok().body("OK")
            }),
        ))
        .await;

        let req = test::TestRequest::get().uri("/slow").to_request();
        let result = test::try_call_service(&app, req).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.as_response_error().status_code(),
            actix_web::http::StatusCode::GATEWAY_TIMEOUT
        );
    }

    #[actix_web::test]
    async fn test_request_just_under_timeout_succeeds() {
        let app = test::init_service(App::new().wrap(TimeoutMiddleware::new(2)).route(
            "/almost",
            web::get().to(|| async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                HttpResponse::Ok().body("OK")
            }),
        ))
        .await;

        let req = test::TestRequest::get().uri("/almost").to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_timeout_middleware_new_sets_duration() {
        let middleware = TimeoutMiddleware::new(10);
        assert_eq!(middleware.duration, Duration::from_secs(10));
    }

    #[actix_web::test]
    async fn test_post_request_timeout() {
        let app = test::init_service(App::new().wrap(TimeoutMiddleware::new(1)).route(
            "/slow-post",
            web::post().to(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                HttpResponse::Ok().body("OK")
            }),
        ))
        .await;

        let req = test::TestRequest::post()
            .uri("/slow-post")
            .set_payload("test body")
            .to_request();
        let result = test::try_call_service(&app, req).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.as_response_error().status_code(),
            actix_web::http::StatusCode::GATEWAY_TIMEOUT
        );
    }

    #[actix_web::test]
    async fn test_multiple_requests_independent_timeouts() {
        let app = test::init_service(
            App::new()
                .wrap(TimeoutMiddleware::new(2))
                .route(
                    "/fast",
                    web::get().to(|| async { HttpResponse::Ok().body("OK") }),
                )
                .route(
                    "/slow",
                    web::get().to(|| async {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        HttpResponse::Ok().body("OK")
                    }),
                ),
        )
        .await;

        // Fast request should succeed
        let req = test::TestRequest::get().uri("/fast").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        // Slow request should timeout
        let req = test::TestRequest::get().uri("/slow").to_request();
        let result = test::try_call_service(&app, req).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.as_response_error().status_code(),
            actix_web::http::StatusCode::GATEWAY_TIMEOUT
        );
    }
}
