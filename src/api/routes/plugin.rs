//! This module defines the HTTP routes for plugin operations.
//! It includes handlers for calling plugin methods.
//! The routes are integrated with the Actix-web framework and interact with the plugin controller.
use crate::{
    api::controllers::plugin,
    models::{DefaultAppState, PaginationQuery, PluginCallRequest},
};
use actix_web::{get, post, web, HttpRequest, Responder};
use relayer_macros::require_permissions;

/// List plugins
#[require_permissions(["plugins:get:all"])]
#[get("/plugins")]
async fn list_plugins(
    raw_request: HttpRequest,
    query: web::Query<PaginationQuery>,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    plugin::list_plugins(query.into_inner(), data).await
}

/// Calls a plugin method.
#[require_permissions(["plugins:execute:{plugin_id}"])]
#[post("/plugins/{plugin_id}/call")]
async fn plugin_call(
    plugin_id: web::Path<String>,
    req: web::Json<PluginCallRequest>,
    raw_request: HttpRequest,
    data: web::ThinData<DefaultAppState>,
) -> impl Responder {
    plugin::call_plugin(plugin_id.into_inner(), req.into_inner(), data).await
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
}
