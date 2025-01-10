use actix_web::{get, web, HttpResponse};

#[get("/health")]
async fn health() -> Result<HttpResponse, actix_web::Error> {
    Ok(HttpResponse::Ok().body("OK"))
}

pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(health);
}
