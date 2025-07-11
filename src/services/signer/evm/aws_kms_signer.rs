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
    services::{AwsKmsClient, AwsKmsEvmService, AwsKmsService, DataSignerTrait, Signer},
};

pub type DefaultAwsKmsService = AwsKmsService<AwsKmsClient>;

pub struct AwsKmsSigner<T = DefaultAwsKmsService>
where
    T: AwsKmsEvmService,
{
    aws_kms_service: T,
}

impl AwsKmsSigner<DefaultAwsKmsService> {
    pub fn new(aws_kms_service: AwsKmsService) -> Self {
        Self { aws_kms_service }
    }
}

#[cfg(test)]
impl<T: AwsKmsEvmService> AwsKmsSigner<T> {
    pub fn new_for_testing(aws_kms_service: T) -> Self {
        Self { aws_kms_service }
    }
}

#[async_trait]
impl<T: AwsKmsEvmService> Signer for AwsKmsSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let address = self.aws_kms_service.get_evm_address().await?;

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

            // Prepare transaction for signing
            let payload = unsigned_tx.encoded_for_signing();

            // Sign payload
            let signed_bytes = self.aws_kms_service.sign_payload_evm(&payload).await?;

            // Ensure we have the right signature length
            if signed_bytes.len() != 65 {
                return Err(SignerError::SigningError(format!(
                    "Invalid signature length from AWS KMS: expected 65 bytes, got {}",
                    signed_bytes.len()
                )));
            }

            // Construct primitive signature
            let signature = PrimitiveSignature::from_raw(&signed_bytes)
                .map_err(|e| SignerError::ConversionError(e.to_string()))?;

            // Extract signature array bytes
            let mut signature_bytes = signature.as_bytes();

            // Construct a signed transaction
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

            // Prepare transaction for signing
            let payload = unsigned_tx.encoded_for_signing();

            let signed_bytes = self.aws_kms_service.sign_payload_evm(&payload).await?;

            // Ensure we have the right signature length
            if signed_bytes.len() != 65 {
                return Err(SignerError::SigningError(format!(
                    "Invalid signature length from AWS KMS: expected 65 bytes, got {}",
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
impl<T: AwsKmsEvmService> DataSignerTrait for AwsKmsSigner<T> {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let eip191_message = eip191_message(&request.message);

        let signature_bytes = self
            .aws_kms_service
            .sign_payload_evm(&eip191_message)
            .await?;

        // Ensure we have the right signature length
        if signature_bytes.len() != 65 {
            return Err(SignerError::SigningError(format!(
                "Invalid signature length from AWS KMS: expected 65 bytes, got {}",
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
            "EIP-712 typed data signing not yet implemented for AWS KMS".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{AwsKmsSignerConfig, EvmTransactionData, U256},
        services::{tests::setup_mock_kms_client, MockAwsKmsClient},
    };

    fn setup_mock_aws_signer() -> AwsKmsSigner<AwsKmsService<MockAwsKmsClient>> {
        let (client, _) = setup_mock_kms_client();
        let service = AwsKmsService::new_for_testing(
            client,
            AwsKmsSignerConfig {
                region: Some("us-east-1".to_string()),
                key_id: "test-key-id".to_string(),
            },
        );
        AwsKmsSigner::new_for_testing(service)
    }

    fn create_test_transaction() -> NetworkTransactionData {
        NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20000000000),
            gas_limit: Some(21000),
            nonce: Some(0),
            value: U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            speed: None,
        })
    }

    #[tokio::test]
    async fn test_address_evm() {
        let signer = setup_mock_aws_signer();

        let result = signer.address().await;
        assert!(result.is_ok());
        match result.unwrap() {
            Address::Evm(addr) => {
                assert_eq!(addr.len(), 20); // EVM addresses are 20 bytes
            }
            _ => panic!("Expected EVM address"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_invalid_data() {
        let signer = setup_mock_aws_signer();
        let mut tx = create_test_transaction();

        if let NetworkTransactionData::Evm(ref mut evm_tx) = tx {
            evm_tx.data = Some("invalid_hex".to_string());
        }

        let result = signer.sign_transaction(tx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_data() {
        let signer = setup_mock_aws_signer();
        let request = SignDataRequest {
            message: "Test message".to_string(),
        };

        let result = signer.sign_data(request).await.unwrap();

        match result {
            SignDataResponse::Evm(sig) => {
                assert_eq!(sig.r.len(), 64); // 32 bytes in hex
                assert_eq!(sig.s.len(), 64); // 32 bytes in hex
                assert!(sig.v == 27 || sig.v == 28, "Invalid v: {}", sig.v); // Valid v values
                assert_eq!(sig.sig.len(), 130); // 65 bytes in hex
            }
            _ => panic!("Expected EVM signature"),
        }
    }

    #[tokio::test]
    async fn test_sign_data_empty_message() {
        let signer = setup_mock_aws_signer();
        let request = SignDataRequest {
            message: "".to_string(),
        };

        let result = signer.sign_data(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_with_contract_creation() {
        let signer = setup_mock_aws_signer();
        let mut tx = create_test_transaction();

        if let NetworkTransactionData::Evm(ref mut evm_tx) = tx {
            evm_tx.to = None;
            evm_tx.data = Some("0x6080604000".to_string()); // Minimal valid hex string for test
        }

        let result = signer.sign_transaction(tx).await.unwrap();
        match result {
            SignTransactionResponse::Evm(signed_tx) => {
                assert!(!signed_tx.hash.is_empty());
                assert!(!signed_tx.raw.is_empty());
                assert!(!signed_tx.signature.sig.is_empty());
            }
            _ => panic!("Expected EVM transaction response"),
        }
    }

    #[tokio::test]
    async fn test_sign_eip1559_transaction() {
        let signer = setup_mock_aws_signer();
        let mut tx = create_test_transaction();

        // Convert to EIP-1559 transaction by setting max_fee_per_gas and max_priority_fee_per_gas
        if let NetworkTransactionData::Evm(ref mut evm_tx) = tx {
            evm_tx.gas_price = None;
            evm_tx.max_fee_per_gas = Some(30_000_000_000);
            evm_tx.max_priority_fee_per_gas = Some(2_000_000_000);
        }

        let result = signer.sign_transaction(tx).await;
        assert!(result.is_ok());

        match result.unwrap() {
            SignTransactionResponse::Evm(signed_tx) => {
                assert!(!signed_tx.hash.is_empty());
                assert!(!signed_tx.raw.is_empty());
                assert!(!signed_tx.signature.sig.is_empty());
                // Verify signature components
                assert_eq!(signed_tx.signature.r.len(), 64); // 32 bytes in hex
                assert_eq!(signed_tx.signature.s.len(), 64); // 32 bytes in hex
                assert!(signed_tx.signature.v == 0 || signed_tx.signature.v == 1);
                // EIP-1559 v values
            }
            _ => panic!("Expected EVM transaction response"),
        }
    }

    #[tokio::test]
    async fn test_sign_eip1559_transaction_with_contract_creation() {
        let signer = setup_mock_aws_signer();
        let mut tx = create_test_transaction();

        if let NetworkTransactionData::Evm(ref mut evm_tx) = tx {
            evm_tx.to = None;
            evm_tx.data = Some("0x6080604000".to_string()); // Minimal valid hex string for test
            evm_tx.gas_price = None;
            evm_tx.max_fee_per_gas = Some(30_000_000_000);
            evm_tx.max_priority_fee_per_gas = Some(2_000_000_000);
        }

        let result = signer.sign_transaction(tx).await;
        assert!(result.is_ok());

        match result.unwrap() {
            SignTransactionResponse::Evm(signed_tx) => {
                assert!(!signed_tx.hash.is_empty());
                assert!(!signed_tx.raw.is_empty());
                assert!(!signed_tx.signature.sig.is_empty());
            }
            _ => panic!("Expected EVM transaction response"),
        }
    }
}
