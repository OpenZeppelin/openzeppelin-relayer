use crate::{
    models::{ApiError, CreateCatRequest},
    services::CatService,
};
use actix_web::{web, HttpResponse};

type CatResult = Result<HttpResponse, ApiError>;

pub async fn get_cats() -> CatResult {
    let cats = CatService::get_all_cats();
    Ok(HttpResponse::Ok().json(&cats))
}

pub async fn get_cat(cat_id: web::Path<u32>) -> CatResult {
    let cat_id = cat_id.into_inner();
    match CatService::find_cat_by_id(cat_id) {
        Some(cat) => Ok(HttpResponse::Ok().json(cat)),
        None => Err(ApiError::NotFound("Cat not found".to_string())),
    }
}

pub async fn create_cat(cat_req: web::Json<CreateCatRequest>) -> HttpResponse {
    let create_request = cat_req.into_inner();
    let new_cat = CatService::create_cat(create_request);
    HttpResponse::Ok().json(new_cat)
}

pub async fn update_cat(
    cat_id: web::Path<u32>,
    updated_cat: web::Json<CreateCatRequest>,
) -> CatResult {
    let cat_id = cat_id.into_inner();
    let cat_request = updated_cat.into_inner();

    match CatService::update_cat(cat_id, cat_request) {
        Some(updated) => Ok(HttpResponse::Ok().json(updated)),
        None => Err(ApiError::NotFound("Cat not found".to_string())),
    }
}

pub async fn delete_cat(cat_id: web::Path<u32>) -> Result<HttpResponse, ApiError> {
    let cat_id = cat_id.into_inner();

    if CatService::delete_cat(cat_id) {
        Ok(HttpResponse::Ok().body("Cat deleted"))
    } else {
        Err(ApiError::NotFound("Cat not found".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_web::test]
    async fn test_get_cats() {
        let resp = get_cats().await.unwrap();
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_get_cat() {
        // Test existing cat
        let cat_id = web::Path::from(1u32);
        let resp = get_cat(cat_id).await.unwrap();
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        // Test non-existent cat
        // Test non-existent cat
        let cat_id = web::Path::from(999u32);
        let resp = get_cat(cat_id).await;
        assert!(resp.is_err());
    }

    #[actix_web::test]
    async fn test_create_cat() {
        let cat_req = CreateCatRequest {
            name: "TestCat".to_string(),
            age: 3,
        };
        let json_req = web::Json(cat_req);

        let resp = create_cat(json_req).await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
    }

    #[actix_web::test]
    async fn test_update_cat() {
        let cat_id = web::Path::from(1u32);
        let updated_cat = CreateCatRequest {
            name: "UpdatedCat".to_string(),
            age: 4,
        };
        let json_req = web::Json(updated_cat);

        // Test existing cat
        let resp = update_cat(cat_id, json_req).await.unwrap();
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        // Test non-existent cat
        let cat_id = web::Path::from(999u32);
        let updated_cat = CreateCatRequest {
            name: "UpdatedCat".to_string(),
            age: 4,
        };
        let json_req = web::Json(updated_cat);
        let resp = update_cat(cat_id, json_req).await;
        assert!(resp.is_err());
    }

    #[actix_web::test]
    async fn test_delete_cat() {
        // Test existing cat
        let cat_id = web::Path::from(1u32);
        let resp = delete_cat(cat_id).await.unwrap();
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);

        // Test non-existent cat
        let cat_id = web::Path::from(999u32);
        let resp = delete_cat(cat_id).await;
        assert!(resp.is_err());
    }
}
