//! This module defines the HTTP routes for plugin operations.
//! It includes handlers for calling plugin methods.
//! The routes are integrated with the Actix-web framework and interact with the plugin controller.
use crate::{
    api::controllers::plugin,
    jobs::JobProducer,
    models::{AppState, PluginCallRequest},
};
use actix_web::{post, web, Responder};

/// Calls a plugin method.
#[post("/plugins/{plugin_id}/call")]
async fn plugin_call(
    plugin_id: web::Path<String>,
    req: web::Json<PluginCallRequest>,
    data: web::ThinData<AppState<JobProducer>>,
) -> impl Responder {
    plugin::call_plugin(plugin_id.into_inner(), req.into_inner(), data).await
}

/// Initializes the routes for the plugins module.
pub fn init(cfg: &mut web::ServiceConfig) {
    // Register routes with literal segments before routes with path parameters
    cfg.service(plugin_call); // /plugins/{plugin_id}/call
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::plugins::PluginCallResponse;
    use actix_web::{test, App, HttpResponse};

    async fn mock_plugin_call() -> impl Responder {
        HttpResponse::Ok().json(PluginCallResponse {
            success: true,
            message: "Plugin called successfully".to_string(),
            output: String::from(""),
            error: String::from(""),
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
        assert!(plugin_call_response.success);
    }
}
