use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path, header, body_json};
use crate::services::vault::{VaultService, VaultConfig, VaultServiceTrait, VaultError};
use serde_json::json;
use crate::utils::base64_encode;

#[tokio::test]
async fn test_vault_service_retrieve_secret_success() {
    // Setup Wiremock mock server
    let mock_server = MockServer::start().await;

    // Mock AppRole authentication
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .and(body_json(json!({
            "role_id": "test-role-id",
            "secret_id": "test-secret-id"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": null,
                    "wrap_info": null,
                    "warnings": null,
                    "auth": {
                        "client_token": "test-token",
                        "accessor": "test-accessor",
                        "policies": ["default"],
                        "token_policies": ["default"],
                        "metadata": {
                            "role_name": "test-role"
                        },
                        "lease_duration": 3600,
                        "renewable": true,
                        "entity_id": "test-entity-id",
                        "token_type": "service",
                        "orphan": true
                    }
                }))
        )
        .mount(&mock_server)
        .await;

    // Mock KV2 secret retrieval
    Mock::given(method("GET"))
        .and(path("/v1/test-mount/data/my-secret"))
        .and(header("X-Vault-Token", "test-token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": {
                        "data": {
                            "value": "super-secret-value"
                        },
                        "metadata": {
                            "created_time": "2024-01-01T00:00:00Z",
                            "deletion_time": "",
                            "destroyed": false,
                            "version": 1
                        }
                    },
                    "wrap_info": null,
                    "warnings": null,
                    "auth": null
                }))
        )
        .mount(&mock_server)
        .await;

    // Configure service pointing to mock server
    let config = VaultConfig::new(
        mock_server.uri(),
        "test-role-id".to_string(),
        "test-secret-id".to_string(),
        None,
        "test-mount".to_string(),
        Some(60),
    );

    let vault_service = VaultService::new(config);

    // Execute method under test
    let secret = vault_service.retrieve_secret("my-secret").await.unwrap();

    // Verify the retrieved secret is correct
    assert_eq!(secret, "super-secret-value");
}

#[tokio::test]
async fn test_vault_service_sign_success() {
    let mock_server = MockServer::start().await;

    // Mock AppRole authentication
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .and(body_json(json!({
            "role_id": "test-role-id",
            "secret_id": "test-secret-id"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": null,
                    "wrap_info": null,
                    "warnings": null,
                    "auth": {
                        "client_token": "test-token",
                        "accessor": "test-accessor",
                        "policies": ["default"],
                        "token_policies": ["default"],
                        "metadata": {
                            "role_name": "test-role"
                        },
                        "lease_duration": 3600,
                        "renewable": true,
                        "entity_id": "test-entity-id",
                        "token_type": "service",
                        "orphan": true
                    }
                }))
        )
        .mount(&mock_server)
        .await;

    let message = b"hello world";
    let encoded_message = base64_encode(message);

    // Mock Transit signing
    Mock::given(method("POST"))
        .and(path("/v1/test-mount/sign/my-signing-key"))
        .and(header("X-Vault-Token", "test-token"))
        .and(body_json(json!({
            "input": encoded_message
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": {
                        "signature": "vault:v1:fake-signature",
                        "key_version": 1
                    },
                    "wrap_info": null,
                    "warnings": null,
                    "auth": null
                }))
        )
        .mount(&mock_server)
        .await;

    let config = VaultConfig::new(
        mock_server.uri(),
        "test-role-id".to_string(),
        "test-secret-id".to_string(),
        None,
        "test-mount".to_string(),
        Some(60),
    );

    let vault_service = VaultService::new(config);
    let signature = vault_service.sign("my-signing-key", message).await.unwrap();

    assert_eq!(signature, "vault:v1:fake-signature");
}

#[tokio::test]
async fn test_vault_service_retrieve_secret_failure() {
    let mock_server = MockServer::start().await;

    // Mock AppRole authentication
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .and(body_json(json!({
            "role_id": "test-role-id",
            "secret_id": "test-secret-id"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": null,
                    "wrap_info": null,
                    "warnings": null,
                    "auth": {
                        "client_token": "test-token",
                        "accessor": "test-accessor",
                        "policies": ["default"],
                        "token_policies": ["default"],
                        "metadata": {
                            "role_name": "test-role"
                        },
                        "lease_duration": 3600,
                        "renewable": true,
                        "entity_id": "test-entity-id",
                        "token_type": "service",
                        "orphan": true
                    }
                }))
        )
        .mount(&mock_server)
        .await;

    // Mock error when retrieving secret
    Mock::given(method("GET"))
        .and(path("/v1/test-mount/data/my-secret"))
        .and(header("X-Vault-Token", "test-token"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({
                    "errors": ["secret not found:"]
                }))
        )
        .mount(&mock_server)
        .await;

    let config = VaultConfig::new(
        mock_server.uri(),
        "test-role-id".to_string(),
        "test-secret-id".to_string(),
        None,
        "test-mount".to_string(),
        Some(60),
    );

    let vault_service = VaultService::new(config);

    // Method should return an error
    let result = vault_service.retrieve_secret("my-secret").await;
    assert!(result.is_err());

    if let Err(e) = result {
        assert!(matches!(e, VaultError::ClientError(_)));
        assert!(e.to_string().contains("secret not found"));
    }
}

#[tokio::test]
async fn test_vault_service_sign_failure() {
    let mock_server = MockServer::start().await;

    // Mock AppRole authentication
    Mock::given(method("POST"))
        .and(path("/v1/auth/approle/login"))
        .and(body_json(json!({
            "role_id": "test-role-id",
            "secret_id": "test-secret-id"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "request_id": "test-request-id",
                    "lease_id": "",
                    "renewable": false,
                    "lease_duration": 0,
                    "data": null,
                    "wrap_info": null,
                    "warnings": null,
                    "auth": {
                        "client_token": "test-token",
                        "accessor": "test-accessor",
                        "policies": ["default"],
                        "token_policies": ["default"],
                        "metadata": {
                            "role_name": "test-role"
                        },
                        "lease_duration": 3600,
                        "renewable": true,
                        "entity_id": "test-entity-id",
                        "token_type": "service",
                        "orphan": true
                    }
                }))
        )
        .mount(&mock_server)
        .await;

    let message = b"hello world";
    let encoded_message = base64_encode(message);

    // Mock signing error
    Mock::given(method("POST"))
        .and(path("/v1/test-mount/sign/my-signing-key"))
        .and(header("X-Vault-Token", "test-token"))
        .and(body_json(json!({
            "input": encoded_message
        })))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({
                    "errors": ["1 error occurred:\n\t* signing key not found"]
                }))
        )
        .mount(&mock_server)
        .await;

    let config = VaultConfig::new(
        mock_server.uri(),
        "test-role-id".to_string(),
        "test-secret-id".to_string(),
        None,
        "test-mount".to_string(),
        Some(60),
    );

    let vault_service = VaultService::new(config);
    let result = vault_service.sign("my-signing-key", message).await;
    assert!(result.is_err());

    if let Err(e) = result {
        assert!(matches!(e, VaultError::SigningError(_)));
        assert!(e.to_string().contains("signing key not found"));
    }
}
