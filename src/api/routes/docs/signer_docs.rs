use crate::models::{ApiResponse, SignerCreateRequest, SignerResponse, SignerUpdateRequest};

/// Signer routes implementation
///
/// Note: OpenAPI documentation for these endpoints can be found in the `openapi.rs` file
///
/// Lists all signers with pagination support.
#[utoipa::path(
  get,
  path = "/api/v1/signers",
  tag = "Signers",
  operation_id = "listSigners",
  security(
      ("bearer_auth" = [])
  ),
  params(
      ("page" = Option<usize>, Query, description = "Page number for pagination (starts at 1)"),
      ("per_page" = Option<usize>, Query, description = "Number of items per page (default: 10)")
  ),
  responses(
      (
          status = 200,
          description = "Signer list retrieved successfully",
          body = ApiResponse<Vec<SignerResponse>>
      ),
      (
          status = 400,
          description = "Bad Request",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Bad Request",
              "data": null
          })
      ),
      (
          status = 401,
          description = "Unauthorized",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Unauthorized",
              "data": null
          })
      ),
      (
          status = 500,
          description = "Internal Server Error",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Internal Server Error",
              "data": null
          })
      )
  )
)]
#[allow(dead_code)]
fn doc_list_signers() {}

/// Retrieves details of a specific signer by ID.
#[utoipa::path(
  get,
  path = "/api/v1/signers/{signer_id}",
  tag = "Signers",
  operation_id = "getSigner",
  security(
      ("bearer_auth" = [])
  ),
  params(
      ("signer_id" = String, Path, description = "Signer ID")
  ),
  responses(
      (
          status = 200,
          description = "Signer retrieved successfully",
          body = ApiResponse<SignerResponse>
      ),
      (
          status = 400,
          description = "Bad Request",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Bad Request",
              "data": null
          })
      ),
      (
          status = 401,
          description = "Unauthorized",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Unauthorized",
              "data": null
          })
      ),
      (
          status = 404,
          description = "Signer not found",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Signer not found",
              "data": null
          })
      ),
      (
          status = 500,
          description = "Internal Server Error",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Internal Server Error",
              "data": null
          })
      )
  )
)]
#[allow(dead_code)]
fn doc_get_signer() {}

/// Creates a new signer.
#[utoipa::path(
  post,
  path = "/api/v1/signers",
  tag = "Signers",
  operation_id = "createSigner",
  security(
      ("bearer_auth" = [])
  ),
  request_body = SignerCreateRequest,
  responses(
      (
          status = 201,
          description = "Signer created successfully",
          body = ApiResponse<SignerResponse>
      ),
      (
          status = 400,
          description = "Bad Request",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Bad Request",
              "data": null
          })
      ),
      (
          status = 401,
          description = "Unauthorized",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Unauthorized",
              "data": null
          })
      ),
      (
          status = 409,
          description = "Signer with this ID already exists",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Signer with this ID already exists",
              "data": null
          })
      ),
      (
          status = 500,
          description = "Internal Server Error",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Internal Server Error",
              "data": null
          })
      )
  )
)]
#[allow(dead_code)]
fn doc_create_signer() {}

/// Updates an existing signer.
#[utoipa::path(
  patch,
  path = "/api/v1/signers/{signer_id}",
  tag = "Signers",
  operation_id = "updateSigner",
  security(
      ("bearer_auth" = [])
  ),
  params(
      ("signer_id" = String, Path, description = "Signer ID")
  ),
  request_body = SignerUpdateRequest,
  responses(
      (
          status = 200,
          description = "Signer updated successfully",
          body = ApiResponse<SignerResponse>
      ),
      (
          status = 400,
          description = "Bad Request",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Bad Request",
              "data": null
          })
      ),
      (
          status = 401,
          description = "Unauthorized",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Unauthorized",
              "data": null
          })
      ),
      (
          status = 404,
          description = "Signer not found",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Signer not found",
              "data": null
          })
      ),
      (
          status = 500,
          description = "Internal Server Error",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Internal Server Error",
              "data": null
          })
      )
  )
)]
#[allow(dead_code)]
fn doc_update_signer() {}

/// Deletes a signer by ID.
#[utoipa::path(
  delete,
  path = "/api/v1/signers/{signer_id}",
  tag = "Signers",
  operation_id = "deleteSigner",
  security(
      ("bearer_auth" = [])
  ),
  params(
      ("signer_id" = String, Path, description = "Signer ID")
  ),
  responses(
      (
          status = 200,
          description = "Signer deleted successfully",
          body = ApiResponse<String>,
          example = json!({
              "success": true,
              "message": "Signer deleted successfully",
              "data": "Signer deleted successfully"
          })
      ),
      (
          status = 400,
          description = "Bad Request",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Bad Request",
              "data": null
          })
      ),
      (
          status = 401,
          description = "Unauthorized",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Unauthorized",
              "data": null
          })
      ),
      (
          status = 404,
          description = "Signer not found",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
                "message": "Signer not found",
              "data": null
          })
      ),
      (
          status = 500,
          description = "Internal Server Error",
          body = ApiResponse<String>,
          example = json!({
              "success": false,
              "message": "Internal Server Error",
              "data": null
          })
      )
  )
)]
#[allow(dead_code)]
fn doc_delete_signer() {}
