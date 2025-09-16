use crate::observability::request_id::set_request_id;
use actix_web::{
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    http::header::{HeaderName, HeaderValue},
    Error, HttpMessage,
};
use futures::future::LocalBoxFuture;
use std::future::{ready, Ready};
use tracing_actix_web::RequestId as ActixRequestId;

/// Middleware that adds request ID tracking to all HTTP requests
pub struct RequestIdMiddleware;

impl<S, B> Transform<S, ServiceRequest> for RequestIdMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = RequestIdMiddlewareService<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RequestIdMiddlewareService { service }))
    }
}

pub struct RequestIdMiddlewareService<S> {
    service: S,
}

impl<S, B> Service<ServiceRequest> for RequestIdMiddlewareService<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // Priority order: incoming header -> ActixRequestId -> new UUID
        let rid = req
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                req.extensions()
                    .get::<ActixRequestId>()
                    .map(|r| r.to_string())
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        set_request_id(rid.clone());

        let fut = self.service.call(req);

        Box::pin(async move {
            let mut res = fut.await?;
            // Use safe conversion to avoid panic on invalid header values
            if let Ok(header_value) = HeaderValue::from_str(&rid) {
                res.headers_mut()
                    .insert(HeaderName::from_static("x-request-id"), header_value);
            }
            Ok(res)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, web, App, HttpResponse};
    use uuid::Uuid;

    #[actix_rt::test]
    async fn echoes_incoming_x_request_id_header() {
        let app = test::init_service(App::new().wrap(RequestIdMiddleware).route(
            "/",
            web::get().to(|| async { HttpResponse::Ok().body("ok") }),
        ))
        .await;

        let req = test::TestRequest::get()
            .uri("/")
            .insert_header(("x-request-id", "test-req-id-123"))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let hdr = resp
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        assert_eq!(hdr.as_deref(), Some("test-req-id-123"));
    }

    #[actix_rt::test]
    async fn generates_uuid_when_header_absent() {
        let app = test::init_service(
            App::new()
                .wrap(RequestIdMiddleware)
                .route("/", web::get().to(|| async { HttpResponse::Ok().finish() })),
        )
        .await;

        let req = test::TestRequest::get().uri("/").to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let hdr = resp
            .headers()
            .get("x-request-id")
            .expect("x-request-id header should be present")
            .to_str()
            .expect("header should be valid ascii");

        // Ensure it's a valid UUID (version is not strictly asserted here)
        let parsed = Uuid::try_parse(hdr).expect("x-request-id should be a UUID");
        // Sanity: UUID should not be nil
        assert_ne!(parsed, Uuid::nil());
    }

    #[actix_rt::test]
    async fn preserves_header_on_internal_error() {
        let app = test::init_service(App::new().wrap(RequestIdMiddleware).route(
            "/",
            web::get().to(|| async {
                // Simulate 500 error
                HttpResponse::InternalServerError().finish()
            }),
        ))
        .await;

        let req = test::TestRequest::get()
            .uri("/")
            .insert_header(("x-request-id", "err-req-id-999"))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(
            resp.status(),
            actix_web::http::StatusCode::INTERNAL_SERVER_ERROR
        );

        let hdr = resp
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok());
        assert_eq!(hdr, Some("err-req-id-999"));
    }
}
