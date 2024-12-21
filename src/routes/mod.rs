pub mod cat_routes;
pub mod health_routes;

use actix_web::web;

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.configure(health_routes::init)
        .configure(cat_routes::init);
}
