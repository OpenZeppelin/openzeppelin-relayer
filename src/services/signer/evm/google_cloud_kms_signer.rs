//! Google Cloud KMS signer for EVM transactions and data.
//!
//! This module implements [`Signer`] and [`DataSignerTrait`] using Google Cloud KMS
//! to securely sign Ethereum transactions and arbitrary data without exposing private keys.
//!
//! Supports both legacy and EIP-1559 transactions, plus EIP-191 data signing.
//! EIP-712 typed data signing is not yet implemented.

use alloy::{
    consensus::{SignableTransaction, TxEip1559, TxLegacy},
    primitives::{eip191_hash_message, utils::eip191_message, PrimitiveSignature},
};
use async_trait::async_trait;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseEvm, SignTypedDataRequest,
    },
    models::{
        Address, EvmTransactionDataSignature, EvmTransactionDataTrait, NetworkTransactionData,
        SignerError,
    },
    services::{DataSignerTrait, GoogleCloudKmsEvmService, GoogleCloudKmsService, Signer},
    utils::base64_encode,
};

pub struct GoogleCloudKmsSigner {
    gcp_kms_service: GoogleCloudKmsService,
}

impl GoogleCloudKmsSigner {
    pub fn new(gcp_kms_service: GoogleCloudKmsService) -> Self {
        Self { gcp_kms_service }
    }
}

#[async_trait]
impl Signer for GoogleCloudKmsSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        let address = self.gcp_kms_service.get_evm_address().await?;
        Ok(address)
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let evm_data = transaction.get_evm_transaction_data()?;

        if evm_data.is_eip1559() {
            // Handle EIP-1559 transaction
            let unsigned_tx = TxEip1559::try_from(transaction)?;

            let payload = unsigned_tx.encoded_for_signing();
            let signed_bytes = self.gcp_kms_service.sign_payload_evm(&payload).await?;

            // Ensure we have the right signature length
            if signed_bytes.len() != 65 {
                return Err(SignerError::SigningError(format!(
                    "Invalid signature length from Google Cloud KMS: expected 65 bytes, got {}",
                    signed_bytes.len()
                )));
            }

            let signature = PrimitiveSignature::from_raw(&signed_bytes)
                .map_err(|e| SignerError::ConversionError(e.to_string()))?;

            let mut signature_bytes = signature.as_bytes();
            let signed_tx = unsigned_tx.into_signed(signature);

            // Adjust v value for EIP-1559 (27/28 -> 0/1)
            if signature_bytes[64] == 27 {
                signature_bytes[64] = 0;
            } else if signature_bytes[64] == 28 {
                signature_bytes[64] = 1;
            }

            // RLP encode the signed transaction
            let mut raw = Vec::with_capacity(signed_tx.eip2718_encoded_length());
            signed_tx.eip2718_encode(&mut raw);

            Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                hash: signed_tx.hash().to_string(),
                signature: EvmTransactionDataSignature::from(&signature_bytes),
                raw,
            }))
        } else {
            // Handle legacy transaction
            let unsigned_tx = TxLegacy::try_from(transaction)?;

            let payload = unsigned_tx.encoded_for_signing();
            let signed_bytes = self.gcp_kms_service.sign_payload_evm(&payload).await?;

            // Ensure we have the right signature length
            if signed_bytes.len() != 65 {
                return Err(SignerError::SigningError(format!(
                    "Invalid signature length from Google Cloud KMS: expected 65 bytes, got {}",
                    signed_bytes.len()
                )));
            }

            let signature = PrimitiveSignature::from_raw(&signed_bytes)
                .map_err(|e| SignerError::ConversionError(e.to_string()))?;

            let signature_bytes = signature.as_bytes();

            let signed_tx = unsigned_tx.into_signed(signature);

            let mut raw = Vec::with_capacity(signed_tx.rlp_encoded_length());
            signed_tx.rlp_encode(&mut raw);

            Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                hash: signed_tx.hash().to_string(),
                signature: EvmTransactionDataSignature::from(&signature_bytes),
                raw,
            }))
        }
    }
}

#[async_trait]
impl DataSignerTrait for GoogleCloudKmsSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let eip191_message = eip191_message(&request.message);

        let signature_bytes = self
            .gcp_kms_service
            .sign_payload_evm(&eip191_message)
            .await?;

        // Ensure we have the right signature length
        if signature_bytes.len() != 65 {
            return Err(SignerError::SigningError(format!(
                "Invalid signature length from Google Cloud KMS: expected 65 bytes, got {}",
                signature_bytes.len()
            )));
        }

        let r = hex::encode(&signature_bytes[0..32]);
        let s = hex::encode(&signature_bytes[32..64]);
        let v = signature_bytes[64];

        Ok(SignDataResponse::Evm(SignDataResponseEvm {
            r,
            s,
            v,
            sig: hex::encode(&signature_bytes),
        }))
    }

    async fn sign_typed_data(
        &self,
        _typed_data: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        // EIP-712 typed data signing requires specific handling
        // This is a placeholder that you'll need to implement based on your needs
        Err(SignerError::NotImplemented(
            "EIP-712 typed data signing not yet implemented for Google Cloud KMS".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        EvmTransactionData, GoogleCloudKmsSignerConfig, GoogleCloudKmsSignerKeyConfig,
        GoogleCloudKmsSignerServiceAccountConfig, SecretString, U256,
    };
    use wiremock::matchers::{header_exists, method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_mock_gcp_signer(mock_server: &MockServer) -> GoogleCloudKmsSigner {
        let base_url = mock_server.uri();

        let config = GoogleCloudKmsSignerConfig {
            service_account: GoogleCloudKmsSignerServiceAccountConfig {
                project_id: "test-project".to_string(),
                private_key_id: SecretString::new("test-private-key-id"),
                private_key: SecretString::new("-----BEGIN EXAMPLE PRIVATE KEY-----\nFAKEKEYDATA\n-----END EXAMPLE PRIVATE KEY-----\n"),
                client_email: SecretString::new("test-service-account@example.com"),
                client_id: "test-client-id".to_string(),
                auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
                token_uri: "https://oauth2.googleapis.com/token".to_string(),
                client_x509_cert_url: "https://www.googleapis.com/robot/v1/metadata/x509/test-service-account%40example.com".to_string(),
                auth_provider_x509_cert_url: "https://www.googleapis.com/oauth2/v1/certs".to_string(),
                universe_domain: base_url,
            },
            key: GoogleCloudKmsSignerKeyConfig {
                location: "global".to_string(),
                key_id: "test-key".to_string(),
                key_ring_id: "test-ring".to_string(),
                key_version: 1,
            },
        };

        let service = GoogleCloudKmsService::new(&config).unwrap();
        GoogleCloudKmsSigner::new(service)
    }

    #[tokio::test]
    async fn test_address_evm() {
        let mock_server = MockServer::start().await;

        // Mock the public key endpoint
        Mock::given(method("GET"))
            .and(path_regex(
                r"/v1/projects/.*/locations/.*/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*",
            ))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "pem": "-----BEGIN PUBLIC KEY-----\nMFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEjJaJh5wfZwvj8b3bQ4GYikqDTLXWUjMh\nkFs9lGj2N9B17zo37p4PSy99rDio0QHLadpso0rtTJDSISRW9MdOqA==\n-----END PUBLIC KEY-----\n", // noboost
                "algorithm": "ECDSA_SECP256K1_SHA256"
            })))
            .mount(&mock_server)
            .await;

        let signer = setup_mock_gcp_signer(&mock_server).await;
        let result = signer.address().await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_data() {
        use alloy::primitives::utils::eip191_message;
        use k256::ecdsa::{signature::DigestSigner, SigningKey};
        use k256::pkcs8::EncodePublicKey;
        use sha3::{Digest, Keccak256};

        let mock_server = MockServer::start().await;

        // Generate a test keypair
        let signing_key = SigningKey::from_slice(
            &hex::decode("c85ef7d79691fe79573b1a7064c19c1a9819ebdbd1faaab1a8ec92344438aaf4")
                .unwrap(),
        )
        .unwrap();
        let verifying_key = signing_key.verifying_key();

        // Get the public key in PEM format
        let public_key_der = verifying_key.to_public_key_der().unwrap();
        let public_key_pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            base64_encode(public_key_der.as_bytes())
        );

        // Mock the public key endpoint with our generated public key
        Mock::given(method("GET"))
            .and(path_regex(
                r"/v1/projects/.*/locations/.*/keyRings/.*/cryptoKeys/.*/cryptoKeyVersions/.*",
            ))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "pem": public_key_pem,
                "algorithm": "ECDSA_SECP256K1_SHA256"
            })))
            .mount(&mock_server)
            .await;

        // The test message
        let test_message = "Test message";
        let eip191_msg = eip191_message(test_message.as_bytes());

        // Sign the message with the private key
        let (signature, _recovery_id) = signing_key
            .sign_digest_recoverable(Keccak256::new_with_prefix(&eip191_msg))
            .unwrap();

        // Convert to DER format for the mock response
        let der_signature = signature.to_der();
        let signature_base64 = base64_encode(der_signature.as_bytes());

        // Mock the sign endpoint with the valid signature
        Mock::given(method("POST"))
            .and(path_regex(
                r"/v1/projects/.*/locations/.*/keyRings/.*/cryptoKeys/.*:asymmetricSign",
            ))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "signature": signature_base64
            })))
            .mount(&mock_server)
            .await;

        let signer = setup_mock_gcp_signer(&mock_server).await;
        let request = SignDataRequest {
            message: test_message.to_string(),
        };

        let result = signer.sign_data(request).await;

        assert!(result.is_ok());
        if let Ok(SignDataResponse::Evm(response)) = result {
            // Check that we have the expected components
            assert_eq!(response.r.len(), 64); // 32 bytes hex encoded
            assert_eq!(response.s.len(), 64); // 32 bytes hex encoded
            assert!(response.v == 27 || response.v == 28); // Valid v value
            assert_eq!(response.sig.len(), 130); // 65 bytes hex encoded
        } else {
            panic!("Expected EVM response");
        }
    }
}
