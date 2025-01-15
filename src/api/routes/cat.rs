use crate::{api::controllers::cat, models::CreateCatRequest};
use actix_web::{delete, get, post, put, web, Responder};

#[get("/cats")]
async fn get_cats() -> impl Responder {
    cat::get_cats().await
}

#[get("/cats/{id}")]
async fn get_cat(cat_id: web::Path<u32>) -> impl Responder {
    cat::get_cat(cat_id).await
}

#[post("/cats")]
async fn create_cat(cat_req: web::Json<CreateCatRequest>) -> impl Responder {
    cat::create_cat(cat_req).await
}

#[put("/cats/{id}")]
async fn update_cat(
    cat_id: web::Path<u32>,
    updated_cat: web::Json<CreateCatRequest>,
) -> impl Responder {
    cat::update_cat(cat_id, updated_cat).await
}

#[delete("/cats/{id}")]
async fn delete_cat(cat_id: web::Path<u32>) -> impl Responder {
    cat::delete_cat(cat_id).await
}

pub fn init(cfg: &mut web::ServiceConfig) {
    cfg.service(get_cats);
    cfg.service(get_cat);
    cfg.service(create_cat);
    cfg.service(update_cat);
    cfg.service(delete_cat);
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{http::StatusCode, test, App};

    #[actix_web::test]
    async fn test_get_cats_route() {
        let app = test::init_service(App::new().service(get_cats)).await;

        let req = test::TestRequest::get().uri("/cats").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_get_cat_route() {
        let app = test::init_service(App::new().service(get_cat)).await;

        let req = test::TestRequest::get().uri("/cats/1").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_create_cat_route() {
        let app = test::init_service(App::new().service(create_cat)).await;

        let cat_req = CreateCatRequest {
            name: "TestCat".to_string(),
            age: 3,
        };

        let req = test::TestRequest::post()
            .uri("/cats")
            .set_json(&cat_req)
            .to_request();

        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
