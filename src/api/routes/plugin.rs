//! This module defines the HTTP routes for plugin operations.
//! It includes handlers for calling plugin methods.
//! The routes are integrated with the Actix-web framework and interact with the plugin controller.
use std::collections::HashMap;

use crate::{
    api::controllers::plugin,
    models::{ApiError, ApiResponse, DefaultAppState, PaginationQuery, PluginCallRequest},
    repositories::PluginRepositoryTrait,
};
use actix_web::{get, post, web, HttpRequest, HttpResponse, Responder};
use url::form_urlencoded;

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

/// Extracts query parameters from the request into a HashMap.
/// Supports multiple values for the same key (e.g., ?tag=a&tag=b)
fn extract_query_params(http_req: &HttpRequest) -> HashMap<String, Vec<String>> {
    let mut query_params: HashMap<String, Vec<String>> = HashMap::new();
    let query_string = http_req.query_string();

    if query_string.is_empty() {
        return query_params;
    }

    // Parse query string to support multiple values for same key (e.g., ?tag=a&tag=b)
    // This also URL-decodes percent-encoded sequences and '+' characters.
    // Note: actix-web's Query<HashMap> only keeps the last value, so we parse manually.
    for (key, value) in form_urlencoded::parse(query_string.as_bytes()) {
        query_params
            .entry(key.into_owned())
            .or_default()
            .push(value.into_owned());
    }

    query_params
}

fn build_plugin_call_request_from_post_body(
    route: &str,
    http_req: &HttpRequest,
    body: &[u8],
) -> Result<PluginCallRequest, HttpResponse> {
    // Parse the body as generic JSON first
    let body_json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to parse request body as JSON: {}", e);
            return Err(HttpResponse::BadRequest()
                .json(ApiResponse::<()>::error(format!("Invalid JSON: {e}"))));
        }
    };

    // Check if the body already has a "params" field
    if body_json.get("params").is_some() {
        // Body already has params field, deserialize normally
        match serde_json::from_value::<PluginCallRequest>(body_json) {
            Ok(mut req) => {
                req.headers = Some(extract_headers(http_req));
                req.route = Some(route.to_string());
                Ok(req)
            }
            Err(e) => {
                tracing::error!("Failed to deserialize PluginCallRequest: {}", e);
                Err(
                    HttpResponse::BadRequest().json(ApiResponse::<()>::error(format!(
                        "Invalid request format: {e}"
                    ))),
                )
            }
        }
    } else {
        // Body doesn't have params field, wrap entire body as params
        Ok(PluginCallRequest {
            params: body_json,
            headers: Some(extract_headers(http_req)),
            route: Some(route.to_string()),
            method: None,
            query: None,
        })
    }
}

/// Calls a plugin method.
#[post("/plugins/{plugin_id}/call{route:.*}")]
async fn plugin_call(
    params: web::Path<(String, String)>,
    http_req: HttpRequest,
    body: web::Bytes,
    data: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let (plugin_id, route) = params.into_inner();

    let mut plugin_call_request =
        match build_plugin_call_request_from_post_body(&route, &http_req, body.as_ref()) {
            Ok(req) => req,
            Err(resp) => return Ok(resp),
        };
    plugin_call_request.method = Some("POST".to_string());
    plugin_call_request.query = Some(extract_query_params(&http_req));

    plugin::call_plugin(plugin_id, plugin_call_request, data).await
}

/// Calls a plugin method via GET request.
#[get("/plugins/{plugin_id}/call{route:.*}")]
async fn plugin_call_get(
    params: web::Path<(String, String)>,
    http_req: HttpRequest,
    data: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let (plugin_id, route) = params.into_inner();

    // Check if GET requests are allowed for this plugin
    let plugin = data
        .plugin_repository
        .get_by_id(&plugin_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Plugin with id {plugin_id} not found")))?;

    if !plugin.allow_get_invocation {
        return Ok(HttpResponse::MethodNotAllowed().json(ApiResponse::<()>::error(
            "GET requests are not enabled for this plugin. Set 'allow_get_invocation: true' in plugin configuration to enable.",
        )));
    }

    // For GET requests, use empty params object
    let plugin_call_request = PluginCallRequest {
        params: serde_json::json!({}),
        headers: Some(extract_headers(&http_req)),
        route: Some(route),
        method: Some("GET".to_string()),
        query: Some(extract_query_params(&http_req)),
    };

    plugin::call_plugin(plugin_id, plugin_call_request, data).await
}

/// Initializes the routes for the plugins module.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(plugin_call); // POST /plugins/{plugin_id}/call
    cfg.service(plugin_call_get); // GET /plugins/{plugin_id}/call
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
                raw_response: false,
                allow_get_invocation: false,
                config: None,
            },
            PluginModel {
                id: "test-plugin2".to_string(),
                path: "test-path2".to_string(),
                timeout: Duration::from_secs(69),
                emit_logs: false,
                emit_traces: false,
                raw_response: false,
                allow_get_invocation: false,
                config: None,
            },
        ])
    }

    #[actix_web::test]
    async fn test_plugin_call() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
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
    async fn test_plugin_call_without_params_wrapper() {
        // Test that body without "params" field is automatically wrapped
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "user": "alice",
                "amount": 100,
                "action": "transfer"
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let plugin_call_response: PluginCallResponse = serde_json::from_slice(&body).unwrap();
        assert!(plugin_call_response.result.is_null());
    }

    #[actix_web::test]
    async fn test_plugin_call_with_params_wrapper() {
        // Test that body with "params" field is handled correctly
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "params": {
                    "user": "alice",
                    "amount": 100
                }
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
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
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

    #[actix_web::test]
    async fn test_extract_headers_skips_non_utf8_value() {
        use actix_web::http::header::{HeaderName, HeaderValue};
        use actix_web::test::TestRequest;

        let non_utf8 = HeaderValue::from_bytes(&[0x80]).unwrap();
        let req = TestRequest::default()
            .insert_header((HeaderName::from_static("x-non-utf8"), non_utf8))
            .insert_header(("X-Ok", "ok"))
            .to_http_request();

        let headers = extract_headers(&req);

        assert_eq!(headers.get("x-ok"), Some(&vec!["ok".to_string()]));
        assert!(headers.get("x-non-utf8").is_none());
    }

    #[actix_web::test]
    async fn test_plugin_call_with_wildcard_route() {
        // Test that wildcard routes are captured correctly
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        // Test with /verify path
        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call/verify")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "params": {"action": "verify"},
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        // Test with /settle path
        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call/settle")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "params": {"action": "settle"},
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        // Test with nested path
        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call/api/v1/action")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "params": {"nested": true},
            }))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_plugin_call_get() {
        // Test GET request handling
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::get().to(mock_plugin_call))
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/test-plugin/call")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_plugin_call_get_with_query_params() {
        // Test GET request with query parameters
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::get().to(mock_plugin_call))
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/test-plugin/call?token=abc123&challenge=xyz")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_plugin_call_get_with_multiple_query_values() {
        // Test GET request with multiple values for same query parameter
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::get().to(mock_plugin_call))
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/test-plugin/call?tag=a&tag=b&tag=c")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_plugin_call_get_with_route() {
        // Test GET request with wildcard route
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::get().to(mock_plugin_call))
                        .route(web::post().to(mock_plugin_call)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/test-plugin/call/verify?token=abc")
            .to_request();
        let resp = test::call_service(&app, req).await;

        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_extract_query_params() {
        use actix_web::test::TestRequest;

        // Test basic query parameters
        let req = TestRequest::default()
            .uri("/test?foo=bar&baz=qux")
            .to_http_request();

        let query_params = extract_query_params(&req);

        assert_eq!(query_params.get("foo"), Some(&vec!["bar".to_string()]));
        assert_eq!(query_params.get("baz"), Some(&vec!["qux".to_string()]));
    }

    #[actix_web::test]
    async fn test_extract_query_params_multiple_values() {
        use actix_web::test::TestRequest;

        // Test multiple values for same key
        let req = TestRequest::default()
            .uri("/test?tag=a&tag=b&tag=c")
            .to_http_request();

        let query_params = extract_query_params(&req);

        assert_eq!(
            query_params.get("tag"),
            Some(&vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[actix_web::test]
    async fn test_extract_query_params_empty() {
        use actix_web::test::TestRequest;

        // Test empty query string
        let req = TestRequest::default().uri("/test").to_http_request();

        let query_params = extract_query_params(&req);

        assert!(query_params.is_empty());
    }

    #[actix_web::test]
    async fn test_extract_query_params_decoding_and_flags() {
        use actix_web::test::TestRequest;

        // percent decoding + '+' decoding + duplicate keys + keys without values
        let req = TestRequest::default()
            .uri("/test?foo=hello%20world&bar=a+b&tag=a%2Bb&flag&tag=c")
            .to_http_request();

        let query_params = extract_query_params(&req);

        assert_eq!(
            query_params.get("foo"),
            Some(&vec!["hello world".to_string()])
        );
        assert_eq!(query_params.get("bar"), Some(&vec!["a b".to_string()]));
        assert_eq!(
            query_params.get("tag"),
            Some(&vec!["a+b".to_string(), "c".to_string()])
        );
        assert_eq!(query_params.get("flag"), Some(&vec!["".to_string()]));
    }

    #[actix_web::test]
    async fn test_extract_query_params_decodes_keys_and_handles_empty_values() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .uri("/test?na%6De=al%69ce&empty=&=noval&tag=a&tag=")
            .to_http_request();

        let query_params = extract_query_params(&req);

        assert_eq!(query_params.get("name"), Some(&vec!["alice".to_string()]));
        assert_eq!(query_params.get("empty"), Some(&vec!["".to_string()]));
        assert_eq!(query_params.get(""), Some(&vec!["noval".to_string()]));
        assert_eq!(
            query_params.get("tag"),
            Some(&vec!["a".to_string(), "".to_string()])
        );
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_invalid_json() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();

        let result = build_plugin_call_request_from_post_body("/verify", &http_req, b"{bad");
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().status(),
            actix_web::http::StatusCode::BAD_REQUEST
        );
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_wraps_body_without_params_field() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default()
            .insert_header(("X-Custom", "v1"))
            .to_http_request();

        let body = serde_json::to_vec(&serde_json::json!({"user": "alice"})).unwrap();
        let req = build_plugin_call_request_from_post_body("/route", &http_req, &body).unwrap();

        assert_eq!(req.params, serde_json::json!({"user": "alice"}));
        assert_eq!(req.route, Some("/route".to_string()));
        assert!(req.headers.as_ref().unwrap().contains_key("x-custom"));
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_uses_params_field_when_present() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default()
            .insert_header(("X-Custom", "v1"))
            .to_http_request();

        let body = serde_json::to_vec(&serde_json::json!({"params": {"k": "v"}})).unwrap();
        let req = build_plugin_call_request_from_post_body("/route", &http_req, &body).unwrap();

        assert_eq!(req.params, serde_json::json!({"k": "v"}));
        assert_eq!(req.route, Some("/route".to_string()));
        assert!(req.headers.as_ref().unwrap().contains_key("x-custom"));
    }
}
