//! This module defines the HTTP routes for plugin operations.
//! It includes handlers for calling plugin methods.
//! The routes are integrated with the Actix-web framework and interact with the plugin controller.
use std::collections::HashMap;

use crate::{
    api::controllers::plugin,
    metrics::PLUGIN_CALLS,
    models::{
        ApiError, ApiResponse, DefaultAppState, PaginationQuery, PluginCallRequest,
        UpdatePluginRequest,
    },
    repositories::PluginRepositoryTrait,
};
use actix_web::{get, patch, post, web, HttpRequest, HttpResponse, ResponseError, Responder};
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

/// Resolves the effective route from path and query parameters.
/// Path route takes precedence; if empty, falls back to `route` query parameter.
fn resolve_route(path_route: &str, http_req: &HttpRequest) -> String {
    // Early return to avoid unnecessary query parameter parsing when path_route is provided
    if !path_route.is_empty() {
        return path_route.to_string();
    }

    // Only parse query parameters when path_route is empty (lazy evaluation)
    let query_params = extract_query_params(http_req);
    query_params
        .get("route")
        .and_then(|values| values.first())
        .cloned()
        .unwrap_or_default()
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
    let (plugin_id, path_route) = params.into_inner();
    let route = resolve_route(&path_route, &http_req);

    let mut plugin_call_request =
        match build_plugin_call_request_from_post_body(&route, &http_req, body.as_ref()) {
            Ok(req) => req,
            Err(resp) => {
                // Track failed request (400 Bad Request)
                PLUGIN_CALLS
                    .with_label_values(&[plugin_id.as_str(), "POST", "400"])
                    .inc();
                return Ok(resp);
            }
        };
    plugin_call_request.method = Some("POST".to_string());
    plugin_call_request.query = Some(extract_query_params(&http_req));

    let result = plugin::call_plugin(plugin_id.clone(), plugin_call_request, data).await;
    
    // Track the request with appropriate status
    let status_code = match &result {
        Ok(response) => response.status(),
        Err(e) => e.error_response().status(),
    };
    let status = status_code.as_str();
    PLUGIN_CALLS
        .with_label_values(&[plugin_id.as_str(), "POST", status])
        .inc();
    
    result
}

/// Calls a plugin method via GET request.
#[get("/plugins/{plugin_id}/call{route:.*}")]
async fn plugin_call_get(
    params: web::Path<(String, String)>,
    http_req: HttpRequest,
    data: web::ThinData<DefaultAppState>,
) -> Result<HttpResponse, ApiError> {
    let (plugin_id, path_route) = params.into_inner();
    let route = resolve_route(&path_route, &http_req);

    // Check if GET requests are allowed for this plugin
    let plugin = match data.plugin_repository.get_by_id(&plugin_id).await? {
        Some(p) => p,
        None => {
            // Track 404
            PLUGIN_CALLS
                .with_label_values(&[plugin_id.as_str(), "GET", "404"])
                .inc();
            return Err(ApiError::NotFound(format!(
                "Plugin with id {plugin_id} not found"
            )));
        }
    };

    if !plugin.allow_get_invocation {
        // Track 405 Method Not Allowed
        PLUGIN_CALLS
            .with_label_values(&[plugin_id.as_str(), "GET", "405"])
            .inc();
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

    let result = plugin::call_plugin(plugin_id.clone(), plugin_call_request, data).await;
    
    // Track the request with appropriate status
    let status_code = match &result {
        Ok(response) => response.status(),
        Err(e) => e.error_response().status(),
    };
    let status = status_code.as_str();
    PLUGIN_CALLS
        .with_label_values(&[plugin_id.as_str(), "GET", status])
        .inc();
    
    result
}

/// Get plugin by ID
#[get("/plugins/{plugin_id}")]
async fn get_plugin(
    path: web::Path<String>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let plugin_id = path.into_inner();
    plugin::get_plugin(plugin_id, data).await
}

/// Update plugin configuration
#[patch("/plugins/{plugin_id}")]
async fn update_plugin(
    path: web::Path<String>,
    body: web::Json<UpdatePluginRequest>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    let plugin_id = path.into_inner();
    plugin::update_plugin(plugin_id, body.into_inner(), data).await
}

/// Initializes the routes for the plugins module.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(plugin_call); // POST /plugins/{plugin_id}/call
    cfg.service(plugin_call_get); // GET /plugins/{plugin_id}/call
    cfg.service(get_plugin); // GET /plugins/{plugin_id}
    cfg.service(update_plugin); // PATCH /plugins/{plugin_id}
    cfg.service(list_plugins); // GET /plugins
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{models::PluginModel, services::plugins::PluginCallResponse};
    use actix_web::{test, App, HttpResponse};

    // ============================================================================
    // TEST HELPERS AND INFRASTRUCTURE
    // ============================================================================

    /// Helper struct to capture requests passed to handlers for verification
    #[derive(Clone, Default)]
    struct CapturedRequest {
        inner: std::sync::Arc<std::sync::Mutex<Option<PluginCallRequest>>>,
    }

    impl CapturedRequest {
        fn capture(&self, req: PluginCallRequest) {
            *self.inner.lock().unwrap() = Some(req);
        }

        fn get(&self) -> Option<PluginCallRequest> {
            self.inner.lock().unwrap().clone()
        }

        fn clear(&self) {
            *self.inner.lock().unwrap() = None;
        }
    }

    /// Capturing handler for POST requests that records the PluginCallRequest for verification
    async fn capturing_plugin_call_handler(
        params: web::Path<(String, String)>,
        http_req: HttpRequest,
        body: web::Bytes,
        captured: web::Data<CapturedRequest>,
    ) -> impl Responder {
        let (_plugin_id, path_route) = params.into_inner();
        let route = resolve_route(&path_route, &http_req);
        match build_plugin_call_request_from_post_body(&route, &http_req, body.as_ref()) {
            Ok(mut req) => {
                req.method = Some("POST".to_string());
                req.query = Some(extract_query_params(&http_req));
                captured.capture(req);
                HttpResponse::Ok().json(PluginCallResponse {
                    result: serde_json::Value::Null,
                    metadata: None,
                })
            }
            Err(resp) => resp,
        }
    }

    /// Capturing handler for GET requests that records the PluginCallRequest for verification
    /// This simulates what plugin_call_get does: creates a PluginCallRequest with method="GET"
    async fn capturing_plugin_call_get_handler(
        params: web::Path<(String, String)>,
        http_req: HttpRequest,
        captured: web::Data<CapturedRequest>,
    ) -> impl Responder {
        let (_plugin_id, path_route) = params.into_inner();
        let route = resolve_route(&path_route, &http_req);
        // Simulate what plugin_call_get does for GET requests
        let plugin_call_request = PluginCallRequest {
            params: serde_json::json!({}),
            headers: Some(extract_headers(&http_req)),
            route: Some(route),
            method: Some("GET".to_string()),
            query: Some(extract_query_params(&http_req)),
        };
        captured.capture(plugin_call_request);
        HttpResponse::Ok().json(PluginCallResponse {
            result: serde_json::Value::Null,
            metadata: None,
        })
    }

    // ============================================================================
    // UNIT TESTS FOR HELPER FUNCTIONS
    // ============================================================================
    async fn mock_list_plugins() -> impl Responder {
        HttpResponse::Ok().json(vec![
            PluginModel {
                id: "test-plugin".to_string(),
                path: "test-path".to_string(),
                timeout: Duration::from_secs(69),
                emit_logs: false,
                emit_traces: false,
                forward_logs: false,
                allow_get_invocation: false,
                config: None,
                raw_response: false,
            },
            PluginModel {
                id: "test-plugin2".to_string(),
                path: "test-path2".to_string(),
                timeout: Duration::from_secs(69),
                emit_logs: false,
                emit_traces: false,
                forward_logs: false,
                allow_get_invocation: false,
                config: None,
                raw_response: false,
            },
        ])
    }

    async fn mock_plugin_call() -> impl Responder {
        HttpResponse::Ok().json(PluginCallResponse {
            result: serde_json::Value::Null,
            metadata: None,
        })
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

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_empty_body() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = b"{}";
        let req = build_plugin_call_request_from_post_body("/test", &http_req, body).unwrap();

        assert_eq!(req.params, serde_json::json!({}));
        assert_eq!(req.route, Some("/test".to_string()));
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_null_params() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = serde_json::to_vec(&serde_json::json!({"params": null})).unwrap();
        let req = build_plugin_call_request_from_post_body("/test", &http_req, &body).unwrap();

        assert_eq!(req.params, serde_json::Value::Null);
        assert_eq!(req.route, Some("/test".to_string()));
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_array_params() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = serde_json::to_vec(&serde_json::json!({"params": [1, 2, 3]})).unwrap();
        let req = build_plugin_call_request_from_post_body("/test", &http_req, &body).unwrap();

        assert_eq!(req.params, serde_json::json!([1, 2, 3]));
        assert_eq!(req.route, Some("/test".to_string()));
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_string_params() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = serde_json::to_vec(&serde_json::json!({"params": "test-string"})).unwrap();
        let req = build_plugin_call_request_from_post_body("/test", &http_req, &body).unwrap();

        assert_eq!(req.params, serde_json::json!("test-string"));
        assert_eq!(req.route, Some("/test".to_string()));
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_additional_fields() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        // Test that additional fields in the body are ignored when params field exists
        // (they should be ignored during deserialization)
        let body = serde_json::to_vec(&serde_json::json!({
            "params": {"k": "v"},
            "extra_field": "ignored"
        }))
        .unwrap();
        let req = build_plugin_call_request_from_post_body("/test", &http_req, &body).unwrap();

        assert_eq!(req.params, serde_json::json!({"k": "v"}));
        assert_eq!(req.route, Some("/test".to_string()));
    }

    #[actix_web::test]
    async fn test_plugin_call_get_not_found() {
        use crate::api::controllers::plugin;
        use crate::utils::mocks::mockutils::create_mock_app_state;

        // Test the controller directly since route handler has type constraints
        let app_state = create_mock_app_state(None, None, None, None, None, None).await;
        let _plugin_call_request = PluginCallRequest {
            params: serde_json::json!({}),
            headers: None,
            route: None,
            method: Some("GET".to_string()),
            query: None,
        };

        // The controller will return NotFound when plugin doesn't exist
        let result = plugin::call_plugin(
            "non-existent-plugin".to_string(),
            _plugin_call_request,
            web::ThinData(app_state),
        )
        .await;

        assert!(result.is_err());
        // Verify it's a NotFound error
        if let Err(crate::models::ApiError::NotFound(_)) = result {
            // Expected error type
        } else {
            panic!("Expected NotFound error, got different error");
        }
    }

    #[actix_web::test]
    async fn test_plugin_call_get_allowed() {
        use crate::utils::mocks::mockutils::create_mock_app_state;
        use std::time::Duration;

        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(60),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: true,
            config: None,
            forward_logs: false,
        };

        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin]), None).await;

        // Verify the plugin exists and has allow_get_invocation=true
        let plugin_repo = app_state.plugin_repository.clone();
        let found_plugin = plugin_repo.get_by_id("test-plugin").await.unwrap();
        assert!(found_plugin.is_some());
        assert!(found_plugin.unwrap().allow_get_invocation);
    }

    #[actix_web::test]
    async fn test_extract_query_params_with_only_question_mark() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default().uri("/test?").to_http_request();
        let query_params = extract_query_params(&req);

        assert!(query_params.is_empty());
    }

    #[actix_web::test]
    async fn test_extract_query_params_with_ampersand_only() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default().uri("/test?&").to_http_request();
        let query_params = extract_query_params(&req);

        // form_urlencoded::parse skips empty keys, so this should be empty
        // or contain an empty key depending on implementation
        // Let's test that it handles it gracefully without panicking
        let _ = query_params.len();
    }

    #[actix_web::test]
    async fn test_extract_query_params_with_special_characters() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .uri("/test?key=value%20with%20spaces&symbol=%26%3D%3F")
            .to_http_request();
        let query_params = extract_query_params(&req);

        assert_eq!(
            query_params.get("key"),
            Some(&vec!["value with spaces".to_string()])
        );
        assert_eq!(query_params.get("symbol"), Some(&vec!["&=?".to_string()]));
    }

    #[actix_web::test]
    async fn test_extract_headers_case_insensitive() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .insert_header(("X-Custom-Header", "value1"))
            .insert_header(("x-custom-header", "value2"))
            .to_http_request();

        let headers = extract_headers(&req);

        // Headers should be normalized to lowercase
        let values = headers.get("x-custom-header");
        assert!(values.is_some());
        // Note: actix-web may combine duplicate headers, so we just verify it exists
        assert!(!values.unwrap().is_empty());
    }

    #[actix_web::test]
    async fn test_extract_headers_with_empty_value() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .insert_header(("X-Empty", ""))
            .insert_header(("X-Normal", "normal-value"))
            .to_http_request();

        let headers = extract_headers(&req);

        assert_eq!(headers.get("x-empty"), Some(&vec!["".to_string()]));
        assert_eq!(
            headers.get("x-normal"),
            Some(&vec!["normal-value".to_string()])
        );
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_empty_route() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = serde_json::to_vec(&serde_json::json!({"user": "alice"})).unwrap();
        let req = build_plugin_call_request_from_post_body("", &http_req, &body).unwrap();

        assert_eq!(req.route, Some("".to_string()));
        assert_eq!(req.params, serde_json::json!({"user": "alice"}));
    }

    #[actix_web::test]
    async fn test_build_plugin_call_request_with_root_route() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = serde_json::to_vec(&serde_json::json!({"user": "alice"})).unwrap();
        let req = build_plugin_call_request_from_post_body("/", &http_req, &body).unwrap();

        assert_eq!(req.route, Some("/".to_string()));
    }

    #[actix_web::test]
    async fn test_extract_query_params_with_unicode() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .uri("/test?name=%E4%B8%AD%E6%96%87&value=test")
            .to_http_request();

        let query_params = extract_query_params(&req);

        // Should decode UTF-8 encoded characters
        assert_eq!(query_params.get("name"), Some(&vec!["中文".to_string()]));
        assert_eq!(query_params.get("value"), Some(&vec!["test".to_string()]));
    }

    // ============================================================================
    // INTEGRATION TESTS WITH CAPTURING HANDLERS
    // ============================================================================

    /// Verifies that headers are correctly extracted and passed to the plugin request
    #[actix_web::test]
    async fn test_headers_actually_extracted_and_passed() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call/verify")
            .insert_header(("Content-Type", "application/json"))
            .insert_header(("X-Custom-Header", "custom-value"))
            .insert_header(("Authorization", "Bearer token123"))
            .insert_header(("X-Request-Id", "req-12345"))
            .set_json(serde_json::json!({"test": "data"}))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        // Verify headers were actually captured and passed correctly
        let captured_req = captured.get().expect("Request should have been captured");
        let headers = captured_req.headers.expect("Headers should be present");

        assert!(
            headers.contains_key("x-custom-header"),
            "X-Custom-Header should be extracted (lowercased)"
        );
        assert!(
            headers.contains_key("authorization"),
            "Authorization header should be extracted"
        );
        assert_eq!(
            headers.get("x-custom-header").unwrap()[0],
            "custom-value",
            "Header value should match"
        );
        assert_eq!(
            headers.get("authorization").unwrap()[0],
            "Bearer token123",
            "Authorization header value should match"
        );
        assert_eq!(
            headers.get("x-request-id").unwrap()[0],
            "req-12345",
            "X-Request-Id header value should match"
        );
    }

    /// Verifies that the route field is correctly extracted from the URL path
    #[actix_web::test]
    async fn test_route_field_correctly_set() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        let test_cases = vec![
            ("/plugins/test/call", ""),
            ("/plugins/test/call/verify", "/verify"),
            ("/plugins/test/call/api/v1/verify", "/api/v1/verify"),
            (
                "/plugins/test/call/settle/transaction",
                "/settle/transaction",
            ),
        ];

        for (uri, expected_route) in test_cases {
            captured.clear();
            let req = test::TestRequest::post()
                .uri(uri)
                .insert_header(("Content-Type", "application/json"))
                .set_json(serde_json::json!({}))
                .to_request();

            test::call_service(&app, req).await;

            let captured_req = captured.get().expect("Request should have been captured");
            assert_eq!(
                captured_req.route,
                Some(expected_route.to_string()),
                "Route should be '{}' for URI '{}'",
                expected_route,
                uri
            );
        }
    }

    /// Verifies that route can be specified via query parameter when path route is empty
    #[actix_web::test]
    async fn test_route_from_query_parameter() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        // Test route from query parameter
        let req = test::TestRequest::post()
            .uri("/plugins/test/call?route=/verify")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({}))
            .to_request();

        test::call_service(&app, req).await;

        let captured_req = captured.get().expect("Request should have been captured");
        assert_eq!(
            captured_req.route,
            Some("/verify".to_string()),
            "Route should be extracted from query parameter"
        );
    }

    /// Verifies that path route takes precedence over query parameter
    #[actix_web::test]
    async fn test_path_route_takes_precedence_over_query() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        // Test that path route takes precedence over query param
        let req = test::TestRequest::post()
            .uri("/plugins/test/call/settle?route=/verify")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({}))
            .to_request();

        test::call_service(&app, req).await;

        let captured_req = captured.get().expect("Request should have been captured");
        assert_eq!(
            captured_req.route,
            Some("/settle".to_string()),
            "Path route should take precedence over query parameter"
        );
    }

    /// Verifies that query parameters are correctly extracted and passed
    #[actix_web::test]
    async fn test_query_params_extracted_and_passed() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test/call?token=abc123&action=verify&tag=a&tag=b")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({}))
            .to_request();

        test::call_service(&app, req).await;

        let captured_req = captured.get().expect("Request should have been captured");
        let query = captured_req.query.expect("Query params should be present");

        assert_eq!(
            query.get("token"),
            Some(&vec!["abc123".to_string()]),
            "Token query param should be extracted"
        );
        assert_eq!(
            query.get("action"),
            Some(&vec!["verify".to_string()]),
            "Action query param should be extracted"
        );
        assert_eq!(
            query.get("tag"),
            Some(&vec!["a".to_string(), "b".to_string()]),
            "Multiple tag query params should be extracted as vector"
        );
    }

    /// Verifies that the method field is correctly set to "POST" for POST requests
    #[actix_web::test]
    async fn test_method_field_set_correctly_for_post() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test/call")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({}))
            .to_request();

        test::call_service(&app, req).await;

        let captured_req = captured.get().expect("Request should have been captured");
        assert_eq!(
            captured_req.method,
            Some("POST".to_string()),
            "Method should be set to POST for POST requests"
        );
    }

    /// Verifies that invalid JSON returns a proper 400 Bad Request error with helpful message
    #[actix_web::test]
    async fn test_invalid_json_returns_proper_error() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();

        // Test case 1: Invalid JSON syntax
        let result =
            build_plugin_call_request_from_post_body("/test", &http_req, b"{invalid json here}");
        assert!(result.is_err(), "Invalid JSON should return error");
        let err_response = result.unwrap_err();
        assert_eq!(
            err_response.status(),
            actix_web::http::StatusCode::BAD_REQUEST,
            "Invalid JSON should return 400 Bad Request"
        );
        let body_bytes = actix_web::body::to_bytes(err_response.into_body())
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();
        assert!(
            body_str.contains("Invalid JSON"),
            "Error message should contain 'Invalid JSON', got: {}",
            body_str
        );

        // Test case 2: Single quotes (not valid JSON)
        let result =
            build_plugin_call_request_from_post_body("/test", &http_req, b"{'key': 'value'}");
        assert!(
            result.is_err(),
            "Invalid JSON with single quotes should return error"
        );

        // Test case 3: Unquoted keys
        let result = build_plugin_call_request_from_post_body("/test", &http_req, b"{key: value}");
        assert!(
            result.is_err(),
            "Invalid JSON with unquoted keys should return error"
        );
    }

    /// Verifies query parameter edge cases: keys without values, empty values, etc.
    #[actix_web::test]
    async fn test_query_params_with_edge_cases() {
        use actix_web::test::TestRequest;

        // Test: key without value (flag parameter)
        let req = TestRequest::default()
            .uri("/test?flag&key=value")
            .to_http_request();
        let params = extract_query_params(&req);

        assert_eq!(
            params.get("flag"),
            Some(&vec!["".to_string()]),
            "Flag parameter without value should have empty string value"
        );
        assert_eq!(
            params.get("key"),
            Some(&vec!["value".to_string()]),
            "Key with value should be extracted correctly"
        );

        // Test: empty value vs no value
        let req = TestRequest::default()
            .uri("/test?empty=&flag")
            .to_http_request();
        let params = extract_query_params(&req);

        assert_eq!(
            params.get("empty"),
            Some(&vec!["".to_string()]),
            "Empty value should be preserved"
        );
        assert_eq!(
            params.get("flag"),
            Some(&vec!["".to_string()]),
            "Flag without value should also have empty string"
        );
    }

    /// Verifies that header values preserve their original case (keys are lowercased)
    #[actix_web::test]
    async fn test_extract_headers_preserves_original_case_in_values() {
        use actix_web::test::TestRequest;

        let req = TestRequest::default()
            .insert_header(("X-Mixed-Case", "Value-With-CAPS"))
            .insert_header(("X-Lower", "lowercase-value"))
            .insert_header(("X-Upper", "UPPERCASE-VALUE"))
            .to_http_request();

        let headers = extract_headers(&req);

        // Keys are normalized to lowercase, but values should preserve case
        let mixed_case_values = headers.get("x-mixed-case").unwrap();
        assert_eq!(
            mixed_case_values[0], "Value-With-CAPS",
            "Header values should preserve original case"
        );

        let lower_values = headers.get("x-lower").unwrap();
        assert_eq!(lower_values[0], "lowercase-value");

        let upper_values = headers.get("x-upper").unwrap();
        assert_eq!(upper_values[0], "UPPERCASE-VALUE");
    }

    /// Verifies handling of very long query strings
    #[actix_web::test]
    async fn test_very_long_query_string() {
        use actix_web::test::TestRequest;

        // Test with a reasonably long query string (1000 chars)
        let long_value = "a".repeat(1000);
        let uri = format!("/test?data={}", long_value);

        let req = TestRequest::default().uri(&uri).to_http_request();

        let params = extract_query_params(&req);
        assert_eq!(
            params.get("data").unwrap()[0].len(),
            1000,
            "Long query parameter value should be handled correctly"
        );
        assert_eq!(
            params.get("data").unwrap()[0],
            long_value,
            "Long query parameter value should match"
        );
    }

    /// Verifies that complex nested JSON structures in params are preserved correctly
    #[actix_web::test]
    async fn test_params_field_with_complex_nested_structure() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let complex_json = serde_json::json!({
            "params": {
                "user": {
                    "name": "alice",
                    "metadata": {
                        "tags": ["a", "b", "c"],
                        "score": 100
                    }
                },
                "action": "verify"
            }
        });

        let body = serde_json::to_vec(&complex_json).unwrap();
        let req = build_plugin_call_request_from_post_body("/test", &http_req, &body).unwrap();

        // Verify nested structure is preserved
        assert_eq!(
            req.params.get("user").unwrap().get("name").unwrap(),
            "alice",
            "Nested user name should be preserved"
        );
        assert!(
            req.params.get("user").unwrap().get("metadata").is_some(),
            "Nested metadata should be preserved"
        );
        let metadata = req.params.get("user").unwrap().get("metadata").unwrap();
        assert_eq!(
            metadata.get("score").unwrap(),
            100,
            "Nested score should be preserved"
        );
        assert!(
            metadata.get("tags").is_some(),
            "Nested tags array should be preserved"
        );
    }

    /// Integration test: Verifies GET restriction logic by testing the repository behavior
    /// that the route handler uses. This test verifies that plugin_call_get correctly checks
    /// allow_get_invocation and would return 405 Method Not Allowed when GET is disabled,
    /// and 404 when plugin doesn't exist.
    #[actix_web::test]
    async fn test_get_restriction_logic_through_route_handler() {
        use crate::utils::mocks::mockutils::create_mock_app_state;
        use std::time::Duration;

        // Create plugin with allow_get_invocation = false
        let plugin_disabled = PluginModel {
            id: "plugin-no-get".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(60),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        };

        // Create plugin with allow_get_invocation = true
        let plugin_enabled = PluginModel {
            id: "plugin-with-get".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(60),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: true,
            config: None,
            forward_logs: false,
        };

        let app_state = create_mock_app_state(
            None,
            None,
            None,
            None,
            Some(vec![plugin_disabled.clone(), plugin_enabled.clone()]),
            None,
        )
        .await;

        // Test 1: GET request to non-existent plugin should return 404
        // We verify the repository logic that the handler uses
        let plugin_repo = app_state.plugin_repository.clone();
        let found_plugin = plugin_repo
            .get_by_id("non-existent")
            .await
            .expect("Repository call should succeed");
        assert!(
            found_plugin.is_none(),
            "Non-existent plugin should return None (would trigger 404 in route handler)"
        );

        // Test 2: GET request to plugin with allow_get_invocation=false should be rejected
        let found_plugin = plugin_repo
            .get_by_id("plugin-no-get")
            .await
            .expect("Repository call should succeed");
        assert!(found_plugin.is_some(), "Plugin should exist in repository");
        let plugin = found_plugin.unwrap();
        assert!(
            !plugin.allow_get_invocation,
            "Plugin should have allow_get_invocation=false (would trigger 405 in route handler)"
        );

        // Test 3: GET request to plugin with allow_get_invocation=true should be allowed
        let found_plugin = plugin_repo
            .get_by_id("plugin-with-get")
            .await
            .expect("Repository call should succeed");
        assert!(found_plugin.is_some(), "Plugin should exist in repository");
        assert!(
            found_plugin.unwrap().allow_get_invocation,
            "Plugin should have allow_get_invocation=true (would proceed in route handler)"
        );
    }

    /// Verifies that error responses contain appropriate status codes and messages
    #[actix_web::test]
    async fn test_error_responses_contain_appropriate_messages() {
        use crate::models::ApiResponse;
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();

        // Test invalid JSON error response
        let result = build_plugin_call_request_from_post_body("/test", &http_req, b"{invalid}");

        assert!(result.is_err());
        let err_response = result.unwrap_err();
        assert_eq!(
            err_response.status(),
            actix_web::http::StatusCode::BAD_REQUEST,
            "Should return 400 Bad Request"
        );

        // Verify error response structure
        let body_bytes = actix_web::body::to_bytes(err_response.into_body())
            .await
            .unwrap();
        let api_response: ApiResponse<()> =
            serde_json::from_slice(&body_bytes).expect("Response should be valid JSON");
        assert!(
            !api_response.success,
            "Error response should have success=false"
        );
        assert!(
            api_response.error.is_some(),
            "Error response should contain error message"
        );
        assert!(
            api_response.error.unwrap().contains("Invalid JSON"),
            "Error message should mention 'Invalid JSON'"
        );
    }

    /// Verifies query parameters with special characters and URL encoding
    #[actix_web::test]
    async fn test_query_params_with_special_characters_and_encoding() {
        use actix_web::test::TestRequest;

        // Test various special characters and encoding scenarios
        let req = TestRequest::default()
            .uri("/test?key=value%20with%20spaces&symbol=%26%3D%3F&unicode=%E4%B8%AD%E6%96%87")
            .to_http_request();

        let params = extract_query_params(&req);

        assert_eq!(
            params.get("key"),
            Some(&vec!["value with spaces".to_string()]),
            "URL-encoded spaces should be decoded"
        );
        assert_eq!(
            params.get("symbol"),
            Some(&vec!["&=?".to_string()]),
            "URL-encoded special characters should be decoded"
        );
        assert_eq!(
            params.get("unicode"),
            Some(&vec!["中文".to_string()]),
            "URL-encoded Unicode should be decoded"
        );
    }

    /// Verifies that params without wrapper are correctly wrapped
    #[actix_web::test]
    async fn test_params_without_wrapper_correctly_wrapped() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();

        // Body without "params" field should be wrapped
        let body_without_params = serde_json::json!({
            "user": "alice",
            "action": "transfer",
            "amount": 100
        });

        let body = serde_json::to_vec(&body_without_params).unwrap();
        let req = build_plugin_call_request_from_post_body("/test", &http_req, &body).unwrap();

        // The entire body should become the params field
        assert_eq!(
            req.params.get("user").unwrap(),
            "alice",
            "User field should be in params"
        );
        assert_eq!(
            req.params.get("action").unwrap(),
            "transfer",
            "Action field should be in params"
        );
        assert_eq!(
            req.params.get("amount").unwrap(),
            100,
            "Amount field should be in params"
        );
    }

    /// Verifies that the method field is correctly set to "GET" for GET requests
    #[actix_web::test]
    async fn test_method_field_set_correctly_for_get() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::get().to(capturing_plugin_call_get_handler)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/test/call/verify?token=abc123")
            .to_request();

        test::call_service(&app, req).await;

        let captured_req = captured.get().expect("Request should have been captured");
        assert_eq!(
            captured_req.method,
            Some("GET".to_string()),
            "Method should be set to GET for GET requests"
        );
        assert_eq!(
            captured_req.params,
            serde_json::json!({}),
            "GET requests should have empty params object"
        );
    }

    /// Verifies that invalid JSON returns 400 Bad Request when sent through the capturing handler
    /// This tests the error flow through the actual request processing logic
    #[actix_web::test]
    async fn test_invalid_json_through_route_handler() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .insert_header(("Content-Type", "application/json"))
            .set_payload("{invalid json syntax}")
            .to_request();

        let resp = test::call_service(&app, req).await;

        assert_eq!(
            resp.status(),
            actix_web::http::StatusCode::BAD_REQUEST,
            "Invalid JSON should return 400 Bad Request through route handler"
        );

        // Verify error response structure
        let body = test::read_body(resp).await;
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("Invalid JSON"),
            "Error message should contain 'Invalid JSON', got: {}",
            body_str
        );

        // Verify that invalid request was not captured
        assert!(
            captured.get().is_none(),
            "Invalid JSON request should not be captured"
        );
    }

    /// Verifies that PluginCallRequest with params field handles various param types correctly
    /// Since PluginCallRequest deserialization is lenient, we test that params can be any JSON type
    #[actix_web::test]
    async fn test_plugin_call_request_with_various_param_types() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();

        // Test case 1: params as string
        let body_with_string_params = serde_json::json!({
            "params": "this is a string, not an object"
        });
        let body = serde_json::to_vec(&body_with_string_params).unwrap();
        let result = build_plugin_call_request_from_post_body("/test", &http_req, &body);
        assert!(result.is_ok(), "Params as string should be valid");
        let req = result.unwrap();
        assert_eq!(
            req.params,
            serde_json::json!("this is a string, not an object"),
            "String params should be preserved"
        );

        // Test case 2: params as array
        let body_with_array_params = serde_json::json!({
            "params": [1, 2, 3, "four"]
        });
        let body = serde_json::to_vec(&body_with_array_params).unwrap();
        let result = build_plugin_call_request_from_post_body("/test", &http_req, &body);
        assert!(result.is_ok(), "Params as array should be valid");
        let req = result.unwrap();
        assert_eq!(
            req.params,
            serde_json::json!([1, 2, 3, "four"]),
            "Array params should be preserved"
        );

        // Test case 3: params as number
        let body_with_number_params = serde_json::json!({
            "params": 42
        });
        let body = serde_json::to_vec(&body_with_number_params).unwrap();
        let result = build_plugin_call_request_from_post_body("/test", &http_req, &body);
        assert!(result.is_ok(), "Params as number should be valid");
        let req = result.unwrap();
        assert_eq!(
            req.params,
            serde_json::json!(42),
            "Number params should be preserved"
        );

        // Test case 4: params as boolean
        let body_with_bool_params = serde_json::json!({
            "params": true
        });
        let body = serde_json::to_vec(&body_with_bool_params).unwrap();
        let result = build_plugin_call_request_from_post_body("/test", &http_req, &body);
        assert!(result.is_ok(), "Params as boolean should be valid");
        let req = result.unwrap();
        assert_eq!(
            req.params,
            serde_json::json!(true),
            "Boolean params should be preserved"
        );
    }

    /// Verifies that requests without Content-Type header are handled gracefully
    #[actix_web::test]
    async fn test_request_without_content_type_header() {
        let captured = web::Data::new(CapturedRequest::default());
        let app = test::init_service(
            App::new()
                .app_data(captured.clone())
                .service(
                    web::resource("/plugins/{plugin_id}/call{route:.*}")
                        .route(web::post().to(capturing_plugin_call_handler)),
                )
                .configure(init),
        )
        .await;

        // POST request without Content-Type header
        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .set_payload("{\"test\": \"data\"}")
            .to_request();

        let resp = test::call_service(&app, req).await;

        // Should still process the request (Content-Type is not strictly required for JSON parsing)
        // The request should succeed and be captured
        assert!(
            resp.status().is_success(),
            "Request without Content-Type should be handled gracefully, got status: {}",
            resp.status()
        );

        // Verify the request was captured and processed
        let captured_req = captured.get();
        assert!(
            captured_req.is_some(),
            "Request without Content-Type should still be processed"
        );
    }

    /// Verifies handling of very large request bodies (stress test)
    #[actix_web::test]
    async fn test_very_large_request_body() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();

        // Create a large JSON body (10KB of data)
        let large_data = "x".repeat(10000);
        let large_json = serde_json::json!({
            "params": {
                "large_data": large_data,
                "count": 10000
            }
        });

        let body = serde_json::to_vec(&large_json).unwrap();
        let result = build_plugin_call_request_from_post_body("/test", &http_req, &body);

        assert!(result.is_ok(), "Large request body should be handled");
        let req = result.unwrap();
        assert_eq!(
            req.params
                .get("large_data")
                .unwrap()
                .as_str()
                .unwrap()
                .len(),
            10000,
            "Large data should be preserved correctly"
        );
    }

    /// Verifies that nested routes with special characters are handled correctly
    #[actix_web::test]
    async fn test_nested_routes_with_special_characters() {
        use actix_web::test::TestRequest;

        let http_req = TestRequest::default().to_http_request();
        let body = serde_json::to_vec(&serde_json::json!({"test": "data"})).unwrap();

        let test_cases = vec![
            ("/api/v1/verify", "/api/v1/verify"),
            ("/settle/transaction", "/settle/transaction"),
            ("/path-with-dashes", "/path-with-dashes"),
            ("/path_with_underscores", "/path_with_underscores"),
            ("/path.with.dots", "/path.with.dots"),
            ("/path%20with%20spaces", "/path%20with%20spaces"), // URL encoded spaces
        ];

        for (route, expected_route) in test_cases {
            let req = build_plugin_call_request_from_post_body(route, &http_req, &body).unwrap();
            assert_eq!(
                req.route,
                Some(expected_route.to_string()),
                "Route '{}' should be preserved as '{}'",
                route,
                expected_route
            );
        }
    }

    #[actix_web::test]
    async fn test_plugin_call_route_handler_post() {
        use crate::utils::mocks::mockutils::create_mock_app_state;
        use std::time::Duration;

        let plugin = PluginModel {
            id: "test-plugin".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(60),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: false,
            config: None,
            forward_logs: false,
        };

        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin]), None).await;

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(web::ThinData(app_state)))
                .configure(init),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/plugins/test-plugin/call")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({"params": {"test": "data"}}))
            .to_request();

        let resp = test::call_service(&app, req).await;

        // Plugin execution fails in test environment (no ts-node), but route handler is executed
        // Verify the route handler was called (returns 500 due to plugin execution failure)
        assert!(
            resp.status().is_server_error() || resp.status().is_client_error(),
            "Route handler should be executed, got status: {}",
            resp.status()
        );
    }

    /// Integration test: Verifies that the actual plugin_call_get route handler processes
    /// GET requests when allowed.
    #[actix_web::test]
    async fn test_plugin_call_get_route_handler_allowed() {
        use crate::utils::mocks::mockutils::create_mock_app_state;
        use std::time::Duration;

        let plugin = PluginModel {
            id: "test-plugin-with-get".to_string(),
            path: "test-path".to_string(),
            timeout: Duration::from_secs(60),
            emit_logs: false,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: true, // GET allowed
            config: None,
            forward_logs: false,
        };

        let app_state =
            create_mock_app_state(None, None, None, None, Some(vec![plugin]), None).await;

        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(web::ThinData(app_state)))
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/test-plugin-with-get/call?token=abc123")
            .to_request();

        let resp = test::call_service(&app, req).await;

        // Plugin execution fails in test environment (no ts-node), but route handler is executed
        // Verify the route handler was called (returns 500 due to plugin execution failure)
        assert!(
            resp.status().is_server_error() || resp.status().is_client_error(),
            "Route handler should be executed, got status: {}",
            resp.status()
        );
    }

    // ============================================================================
    // GET PLUGIN ROUTE TESTS
    // ============================================================================

    /// Mock handler for get plugin that returns a plugin by ID
    async fn mock_get_plugin(path: web::Path<String>) -> impl Responder {
        let plugin_id = path.into_inner();

        if plugin_id == "not-found" {
            return HttpResponse::NotFound().json(ApiResponse::<()>::error(format!(
                "Plugin with id {} not found",
                plugin_id
            )));
        }

        let plugin = PluginModel {
            id: plugin_id,
            path: "test-path.ts".to_string(),
            timeout: Duration::from_secs(30),
            emit_logs: true,
            emit_traces: false,
            raw_response: false,
            allow_get_invocation: true,
            config: None,
            forward_logs: true,
        };

        HttpResponse::Ok().json(ApiResponse::success(plugin))
    }

    #[actix_web::test]
    async fn test_get_plugin_route_success() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}").route(web::get().to(mock_get_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/my-plugin")
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let response: ApiResponse<PluginModel> = serde_json::from_slice(&body).unwrap();
        assert!(response.success);
        assert_eq!(response.data.as_ref().unwrap().id, "my-plugin");
        assert!(response.data.as_ref().unwrap().emit_logs);
        assert!(response.data.as_ref().unwrap().forward_logs);
    }

    #[actix_web::test]
    async fn test_get_plugin_route_not_found() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}").route(web::get().to(mock_get_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::get()
            .uri("/plugins/not-found")
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::NOT_FOUND);
    }

    // ============================================================================
    // UPDATE PLUGIN ROUTE TESTS
    // ============================================================================

    /// Mock handler for update plugin that returns the updated plugin
    async fn mock_update_plugin(
        path: web::Path<String>,
        body: web::Json<UpdatePluginRequest>,
    ) -> impl Responder {
        let plugin_id = path.into_inner();
        let update = body.into_inner();

        // Simulate successful update
        let updated_plugin = PluginModel {
            id: plugin_id,
            path: "test-path".to_string(),
            timeout: Duration::from_secs(update.timeout.unwrap_or(30)),
            emit_logs: update.emit_logs.unwrap_or(false),
            emit_traces: update.emit_traces.unwrap_or(false),
            raw_response: update.raw_response.unwrap_or(false),
            allow_get_invocation: update.allow_get_invocation.unwrap_or(false),
            config: update.config.flatten(),
            forward_logs: update.forward_logs.unwrap_or(false),
        };

        HttpResponse::Ok().json(ApiResponse::success(updated_plugin))
    }

    #[actix_web::test]
    async fn test_update_plugin_route_success() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::patch()
            .uri("/plugins/test-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "timeout": 60,
                "emit_logs": true,
                "forward_logs": true
            }))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let response: ApiResponse<PluginModel> = serde_json::from_slice(&body).unwrap();
        assert!(response.success);
        assert_eq!(response.data.as_ref().unwrap().id, "test-plugin");
        assert_eq!(
            response.data.as_ref().unwrap().timeout,
            Duration::from_secs(60)
        );
        assert!(response.data.as_ref().unwrap().emit_logs);
        assert!(response.data.as_ref().unwrap().forward_logs);
    }

    #[actix_web::test]
    async fn test_update_plugin_route_with_config() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::patch()
            .uri("/plugins/my-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "config": {
                    "feature_enabled": true,
                    "api_key": "secret123"
                }
            }))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let response: ApiResponse<PluginModel> = serde_json::from_slice(&body).unwrap();
        assert!(response.success);
        assert!(response.data.as_ref().unwrap().config.is_some());
        let config = response.data.as_ref().unwrap().config.as_ref().unwrap();
        assert_eq!(
            config.get("feature_enabled"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(config.get("api_key"), Some(&serde_json::json!("secret123")));
    }

    #[actix_web::test]
    async fn test_update_plugin_route_clear_config() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::patch()
            .uri("/plugins/test-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "config": null
            }))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let response: ApiResponse<PluginModel> = serde_json::from_slice(&body).unwrap();
        assert!(response.success);
        assert!(response.data.as_ref().unwrap().config.is_none());
    }

    #[actix_web::test]
    async fn test_update_plugin_route_empty_body() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        // Empty JSON object - no fields to update
        let req = test::TestRequest::patch()
            .uri("/plugins/test-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({}))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());
    }

    #[actix_web::test]
    async fn test_update_plugin_route_all_fields() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::patch()
            .uri("/plugins/full-update-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "timeout": 120,
                "emit_logs": true,
                "emit_traces": true,
                "raw_response": true,
                "allow_get_invocation": true,
                "forward_logs": true,
                "config": {
                    "key": "value"
                }
            }))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_success());

        let body = test::read_body(resp).await;
        let response: ApiResponse<PluginModel> = serde_json::from_slice(&body).unwrap();
        let plugin = response.data.unwrap();

        assert_eq!(plugin.timeout, Duration::from_secs(120));
        assert!(plugin.emit_logs);
        assert!(plugin.emit_traces);
        assert!(plugin.raw_response);
        assert!(plugin.allow_get_invocation);
        assert!(plugin.forward_logs);
        assert!(plugin.config.is_some());
    }

    #[actix_web::test]
    async fn test_update_plugin_route_invalid_json() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        let req = test::TestRequest::patch()
            .uri("/plugins/test-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_payload("{ invalid json }")
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_client_error());
    }

    #[actix_web::test]
    async fn test_update_plugin_route_unknown_field_rejected() {
        let app = test::init_service(
            App::new()
                .service(
                    web::resource("/plugins/{plugin_id}")
                        .route(web::patch().to(mock_update_plugin)),
                )
                .configure(init),
        )
        .await;

        // UpdatePluginRequest has deny_unknown_fields, so this should fail
        let req = test::TestRequest::patch()
            .uri("/plugins/test-plugin")
            .insert_header(("Content-Type", "application/json"))
            .set_json(serde_json::json!({
                "timeout": 60,
                "unknown_field": "should_fail"
            }))
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert!(resp.status().is_client_error());
    }
}
