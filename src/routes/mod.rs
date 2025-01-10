pub mod cat;
pub mod health;
pub mod relayer;

use actix_web::web;

pub fn configure_routes(cfg: &mut web::ServiceConfig) {
    cfg.configure(health::init)
        .configure(cat::init)
        .configure(relayer::init);
}
