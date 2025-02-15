use crate::metrics::{update_system_metrics, REGISTRY};
use actix_web::{get, web, HttpResponse, Responder};
use prometheus::{Encoder, TextEncoder};

#[get("/metrics")]
async fn list_metrics() -> impl Responder {
    // Gather the metric families from the registry and extract metric names.
    let metric_families = REGISTRY.gather();
    let metric_names: Vec<String> = metric_families
        .iter()
        .map(|mf| mf.get_name().to_string())
        .collect();
    HttpResponse::Ok().json(metric_names)
}

#[get("/metrics/{metric_name}")]
async fn metric_detail(path: web::Path<String>) -> impl Responder {
    let metric_name = path.into_inner();
    let metric_families = REGISTRY.gather();

    for mf in metric_families {
        if mf.get_name() == metric_name {
            let encoder = TextEncoder::new();
            let mut buffer = Vec::new();
            if let Err(e) = encoder.encode(&[mf], &mut buffer) {
                return HttpResponse::InternalServerError().body(format!("Encoding error: {}", e));
            }
            return HttpResponse::Ok()
                .content_type(encoder.format_type())
                .body(buffer);
        }
    }
    HttpResponse::NotFound().body("Metric not found")
}

#[get("/debug/metrics/scrape")]
async fn scrape_metrics() -> impl Responder {
    update_system_metrics();
    match crate::metrics::gather_metrics() {
        Ok(body) => HttpResponse::Ok().content_type("text/plain;").body(body),
        Err(e) => HttpResponse::InternalServerError().body(format!("Error: {}", e)),
    }
}

pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(list_metrics);
    cfg.service(metric_detail);
    cfg.service(scrape_metrics);
}
