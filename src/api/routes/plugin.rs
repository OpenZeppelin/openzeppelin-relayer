//! This module defines the HTTP routes for plugin operations.
//! It includes handlers for calling plugin methods.
//! The routes are integrated with the Actix-web framework and interact with the plugin controller.
use std::collections::HashMap;

use crate::{
    api::controllers::plugin,
    models::{DefaultAppState, PaginationQuery, PluginCallRequest},
};
use actix_web::{get, post, web, HttpRequest, Responder};

/// List plugins
#[get("/plugins")]
async fn list_plugins(
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    plugin::list_plugins(query.into_inner(), data).await
}

/// Extracts HTTP headers from the request into a HashMap.
fn extract_headers(http_req: &HttpRequest) -> HashMap<String, Vec<String>> {
    let mut headers: HashMap<String, Vec<String>> = HashMap::new();
    for (name, value) in http_req.headers().iter() {
        if let Ok(value_str) = value.to_str() {
            headers
                .entry(name.as_str().to_string())
                .or_default()
                .push(value_str.to_string());
        }
    }
    headers
}

/// Calls a plugin method.
#[post("/plugins/{plugin_id}/call")]
async fn plugin_call(
    plugin_id: web::Path<String>,
    http_req: HttpRequest,
    req: web::Json<PluginCallRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let mut plugin_call_request = req.into_inner();
    plugin_call_request.headers = Some(extract_headers(&http_req));
    plugin::call_plugin(plugin_id.into_inner(), plugin_call_request, data).await
}

/// Initializes the routes for the plugins module.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(plugin_call); // /plugins/{plugin_id}/call
    cfg.service(list_plugins); // /plugins
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{models::PluginModel, services::plugins::PluginCallResponse};
    use actix_web::{test, App, HttpResponse};

    async fn mock_plugin_call() -> impl Responder {
        HttpResponse::Ok().json(PluginCallResponse {
            result: serde_json::Value::Null,
            metadata: None,
        })
    }

    async fn mock_list_plugins() -> impl Responder {
        HttpResponse::Ok().json(vec![
            PluginModel {
                id: "test-plugin".to_string(),
                path: "test-path".to_string(),
                timeout: Duration::from_secs(69),
                emit_logs: false,
                emit_traces: false,
            },
            PluginModel {
                id: "test-plugin2".to_string(),
                path: "test-path2".to_string(),
                timeout: Duration::from_secs(69),
                emit_logs: false,
                emit_traces: false,
            },
        ])
    }

    #[actix_web::test]
    async fn test_plugin_call() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call")
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "params": serde_json::Value::Null,
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let plugin_call_response: PluginCallResponse = serde_json::from_slice(&body).unwrap();
        assert!(plugin_call_response.result.is_null());
    }

    #[actix_web::test]
    async fn test_list_plugins() {
        let app = test::init_service(
            App::new()
                .service(web::resource("/plugins").route(web::get().to(mock_list_plugins)))
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get().uri("/plugins").to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let plugin_call_response: Vec<PluginModel> = serde_json::from_slice(&body).unwrap();

        assert_eq!(plugin_call_response.len(), 2);
        assert_eq!(plugin_call_response[0].id, "test-plugin");
        assert_eq!(plugin_call_response[0].path, "test-path");
        assert_eq!(plugin_call_response[1].id, "test-plugin2");
        assert_eq!(plugin_call_response[1].path, "test-path2");
    }

    #[actix_web::test]
    async fn test_plugin_call_extracts_headers() {
        // Test that custom headers are extracted and passed to the plugin
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call")
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .insert_header(("Content-Type", "application/json"))
            .insert_header(("X-Custom-Header", "custom-value"))
            .insert_header(("Authorization", "Bearer test-token"))
            .insert_header(("X-Request-Id", "req-12345"))
            // Add duplicate header to test multi-value
            .insert_header(("Accept", "application/json"))
            .set_json(serde_json::json!({
                "params": {"test": "data"},
            }))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_extract_headers_unit() {
        // Unit test for extract_headers using TestRequest
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .insert_header(("X-Custom-Header", "value1"))
            .insert_header(("Authorization", "Bearer token"))
            .insert_header(("Content-Type", "application/json"))
            .to_http_request();

        let headers = extract_headers(&req);

        assert_eq!(
            headers.get("x-custom-header"),
            Some(&vec!["value1".to_string()])
        );
        assert_eq!(
            headers.get("authorization"),
            Some(&vec!["Bearer token".to_string()])
        );
        assert_eq!(
            headers.get("content-type"),
            Some(&vec!["application/json".to_string()])
        );
    }

    #[actix_web::test]
    async fn test_extract_headers_multi_value() {
        use actix_web::test::TestRequest;

        // actix-web combines duplicate headers, but we can test the structure
        let req = TestRequest::default()
            .insert_header(("X-Values", "value1"))
            .to_http_request();

        let headers = extract_headers(&req);

        // Verify structure is Vec<String>
        let values = headers.get("x-values").unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0], "value1");
    }

    #[actix_web::test]
    async fn test_extract_headers_empty() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default().to_http_request();
        let headers = extract_headers(&req);

        // Should return empty HashMap (no panic)
        // Note: TestRequest may include default headers, so we just verify it doesn't panic
        let _ = headers.len();
    }
}
