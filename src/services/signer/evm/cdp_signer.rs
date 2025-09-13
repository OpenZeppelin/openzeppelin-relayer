//! # EVM CDP Signer Implementation
//!
//! This module provides an EVM signer implementation that uses the CDP API
//! for secure key management and transaction signing operations.
//!
//! ## Features
//!
//! - Secure signing of EVM transactions (both legacy and EIP-1559)
//! - Message signing with EIP-191 prefixing
//! - Remote key management through CDP's secure infrastructure
//!
//! ## Security Notes
//!
//! Private keys never leave the CDP service, providing enhanced security
//! compared to local key storage solutions.
use std::str::FromStr;

use alloy::{
    consensus::{SignableTransaction, TxEip1559, TxLegacy},
    primitives::{eip191_hash_message, keccak256},
};
use async_trait::async_trait;
use log::{debug, info};

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseEvm, SignTypedDataRequest,
    },
    models::{
        Address, CdpSignerConfig, EvmTransactionDataSignature, EvmTransactionDataTrait,
        NetworkTransactionData, SignerError, SignerRepoModel,
    },
    services::{signer::Signer, CdpService, CdpServiceTrait},
};

use super::DataSignerTrait;

pub type DefaultCdpService = CdpService;

pub struct CdpSigner<T = DefaultCdpService>
where
    T: CdpServiceTrait,
{
    cdp_service: T,
}

impl CdpSigner<DefaultCdpService> {
    pub fn new(config: CdpSignerConfig) -> Result<Self, SignerError> {
        let cdp_service = DefaultCdpService::new(config).map_err(|e| {
            SignerError::Configuration(format!("Failed to create CDP service: {}", e))
        })?;
        Ok(Self { cdp_service })
    }
}

#[cfg(test)]
impl<T: CdpServiceTrait> CdpSigner<T> {
    pub fn new_with_service(cdp_service: T) -> Self {
        Self { cdp_service }
    }

    pub fn new_for_testing(cdp_service: T) -> Self {
        Self::new_with_service(cdp_service)
    }
}

#[async_trait]
impl<T: CdpServiceTrait> Signer for CdpSigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let address = self
            .cdp_service
            .account_address()
            .await
            .map_err(SignerError::CdpError)?;

        Ok(address)
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let evm_data = transaction.get_evm_transaction_data()?;

        // Prepare data for signing based on transaction type
        let (unsigned_tx_bytes, is_eip1559) = if evm_data.is_eip1559() {
            let tx = TxEip1559::try_from(transaction)?;
            (tx.encoded_for_signing(), true)
        } else {
            let tx = TxLegacy::try_from(transaction)?;
            (tx.encoded_for_signing(), false)
        };

        // Sign the data with CDP service
        let signed_bytes = self
            .cdp_service
            .sign_evm_transaction(&unsigned_tx_bytes)
            .await
            .map_err(SignerError::CdpError)?;

        // Process the signed transaction
        let mut signed_bytes_slice = signed_bytes.as_slice();

        // Parse the signed transaction and extract components
        let (hash, signature_bytes) = if is_eip1559 {
            let signed_tx =
                alloy::consensus::Signed::<TxEip1559>::eip2718_decode(&mut signed_bytes_slice)
                    .map_err(|e| {
                        SignerError::SigningError(format!(
                            "Failed to decode signed transaction: {}",
                            e
                        ))
                    })?;

            let sig = signed_tx.signature();
            let mut sig_bytes = sig.as_bytes();

            // Adjust v value for EIP-1559 (27/28 -> 0/1)
            if sig_bytes[64] == 27 {
                sig_bytes[64] = 0;
            } else if sig_bytes[64] == 28 {
                sig_bytes[64] = 1;
            }

            (signed_tx.hash().to_string(), sig_bytes)
        } else {
            let signed_tx =
                alloy::consensus::Signed::<TxLegacy>::eip2718_decode(&mut signed_bytes_slice)
                    .map_err(|e| {
                        SignerError::SigningError(format!(
                            "Failed to decode signed transaction: {}",
                            e
                        ))
                    })?;

            let sig = signed_tx.signature();
            (signed_tx.hash().to_string(), sig.as_bytes())
        };

        Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
            hash,
            signature: EvmTransactionDataSignature::from(&signature_bytes),
            raw: signed_bytes,
        }))
    }
}

#[async_trait]
impl<T: CdpServiceTrait> DataSignerTrait for CdpSigner<T> {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let message_string = request.message.to_string();

        // Sign the prefixed message
        let signature_bytes = self
            .cdp_service
            .sign_evm_message(message_string)
            .await
            .map_err(SignerError::CdpError)?;

        // Ensure we have the right signature length
        if signature_bytes.len() != 65 {
            return Err(SignerError::SigningError(format!(
                "Invalid signature length from CDP: expected 65 bytes, got {}",
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
        Err(SignerError::NotImplemented(
            "EIP-712 typed data signing not yet implemented for CDP".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        models::{CdpSignerConfig, SecretString},
        services::{CdpError, MockCdpServiceTrait},
    };
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_address() {
        let mut mock_service = MockCdpServiceTrait::new();

        mock_service
            .expect_account_address()
            .times(1)
            .returning(|| {
                Box::pin(async {
                    Ok(Address::Evm([
                        200, 52, 220, 220, 154, 7, 77, 187, 173, 204, 113, 88, 71, 137, 174, 75,
                        70, 61, 177, 22,
                    ]))
                })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
        let result = signer.address().await.unwrap();

        match result {
            Address::Evm(addr) => {
                assert_eq!(
                    hex::encode(addr),
                    "c834dcdc9a074dbbadcc71584789ae4b463db116"
                );
            }
            _ => panic!("Expected EVM address"),
        }
    }

    #[tokio::test]
    async fn test_sign_data() {
        let mut mock_service = MockCdpServiceTrait::new();
        let test_message = "Test message";

        let r = [1u8; 32];
        let s = [2u8; 32];
        let v = 27u8;
        let mut mock_sig = Vec::with_capacity(65);
        mock_sig.extend_from_slice(&r);
        mock_sig.extend_from_slice(&s);
        mock_sig.push(v);

        mock_service
            .expect_sign_evm_message()
            .times(1)
            .returning(move |_| {
                let sig = mock_sig.clone();
                Box::pin(async { Ok(sig) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
        let request = SignDataRequest {
            message: test_message.to_string(),
        };

        let result = signer.sign_data(request).await.unwrap();

        match result {
            SignDataResponse::Evm(sig) => {
                assert_eq!(
                    sig.r,
                    "0101010101010101010101010101010101010101010101010101010101010101"
                );
                assert_eq!(
                    sig.s,
                    "0202020202020202020202020202020202020202020202020202020202020202"
                );
                assert_eq!(sig.v, 27);
                assert_eq!(sig.sig, "01010101010101010101010101010101010101010101010101010101010101010202020202020202020202020202020202020202020202020202020202020202".to_string() + "1b");
            }
            _ => panic!("Expected EVM signature"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction() {
        let mut mock_service = MockCdpServiceTrait::new();

        let tx_data = crate::models::EvmTransactionData {
            from: "0x7f5f4552091a69125d5dfcb7b8c2658029395bdf".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: None,
            gas_limit: Some(21000),
            nonce: Some(0),
            value: crate::models::U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
            max_fee_per_gas: Some(1),
            max_priority_fee_per_gas: Some(1),
            speed: None,
        };

        mock_service
            .expect_sign_evm_transaction()
            .returning(move |_| {
                let test = hex::decode("02f86d83aa36a70184442b657e84e946e47982520894b726167dc2ef2ac582f0a3de4c08ac4abb90626a0180c001a0f6b2cfef2b4d31f4af9a6d851c022f3ae89571e1eee6ec5d05889eaf50c4244da0369a720cf91e1327b9fff17d9291e042a22172e92c1db5e76f4b0ebf7fae9ed2").unwrap();
                Box::pin(async { Ok(test) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let result = signer
            .sign_transaction(NetworkTransactionData::Evm(tx_data))
            .await
            .unwrap();

        match result {
            SignTransactionResponse::Evm(signed_tx) => {
                assert_eq!(
                    signed_tx.signature.r,
                    "f6b2cfef2b4d31f4af9a6d851c022f3ae89571e1eee6ec5d05889eaf50c4244d"
                );
                assert_eq!(
                    signed_tx.signature.s,
                    "369a720cf91e1327b9fff17d9291e042a22172e92c1db5e76f4b0ebf7fae9ed2"
                );
                assert_eq!(signed_tx.signature.v, 1);
                assert_eq!(
                    signed_tx.hash,
                    "0xc2e3533e19d6cf2318a1415bcbe2df3977707c5000dc2b9cd04b99e5aeee2b58"
                );
            }
            _ => panic!("Expected EVM signed transaction"),
        }
    }

    #[tokio::test]
    async fn test_sign_data_error_handling() {
        let mut mock_service = MockCdpServiceTrait::new();
        let test_message = "Test message";

        // Set up mock to return an error
        mock_service
            .expect_sign_evm_message()
            .times(1)
            .returning(move |_| {
                Box::pin(async { Err(CdpError::SigningError("Mock signing error".into())) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
        let request = SignDataRequest {
            message: test_message.to_string(),
        };

        let result = signer.sign_data(request).await;

        assert!(result.is_err());
        match result {
            Err(SignerError::CdpError(err)) => {
                assert_eq!(err.to_string(), "Signing error: Mock signing error");
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_data_invalid_signature_length() {
        let mut mock_service = MockCdpServiceTrait::new();
        let test_message = "Test message";

        mock_service
            .expect_sign_evm_message()
            .times(1)
            .returning(move |_| {
                let invalid_sig = vec![1u8; 64]; // Only 64 bytes instead of 65
                Box::pin(async { Ok(invalid_sig) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);
        let request = SignDataRequest {
            message: test_message.to_string(),
        };

        // Verify that we get the expected error about signature length
        let result = signer.sign_data(request).await;
        assert!(result.is_err());
        match result {
            Err(SignerError::SigningError(msg)) => {
                assert!(msg.contains("Invalid signature length"));
                assert!(msg.contains("expected 65 bytes"));
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_typed_data_not_implemented() {
        let mock_service = MockCdpServiceTrait::new();
        let signer = CdpSigner::new_for_testing(mock_service);

        let request = SignTypedDataRequest {
            domain_separator: "test-domain".to_string(),
            hash_struct_message: "test-struct".to_string(),
        };

        let result = signer.sign_typed_data(request).await;
        assert!(result.is_err());
        match result {
            Err(SignerError::NotImplemented(_)) => {}
            _ => panic!("Expected NotImplemented error variant"),
        }
    }

    #[tokio::test]
    async fn test_sign_legacy_transaction() {
        let mut mock_service = MockCdpServiceTrait::new();

        // Create a legacy transaction (with gas_price instead of max_fee_per_gas)
        let tx_data = crate::models::EvmTransactionData {
            from: "0x7f5f4552091a69125d5dfcb7b8c2658029395bdf".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: Some(20_000_000_000),
            gas_limit: Some(21000),
            nonce: Some(0),
            value: crate::models::U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
            max_fee_per_gas: None,          // Not used in legacy transactions
            max_priority_fee_per_gas: None, // Not used in legacy transactions
            speed: None,
        };

        mock_service
            .expect_sign_evm_transaction()
            .returning(move |_| {
                let test = hex::decode("f86c808504a817c80082520894742d35cc6634c0532925a3b844bc454e4438f44f880de0b6b3a76400008025a0a37376a614e7c1c1605614b126467b0cf6ecb72e9a8d918e69d6048c4db42e89a01a87f2753e120205ae26681ad7d5158b6d8371424ca825ab4b773fb71e0c45fa").unwrap();
                Box::pin(async { Ok(test) })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let result = signer
            .sign_transaction(NetworkTransactionData::Evm(tx_data))
            .await
            .unwrap();

        match result {
            SignTransactionResponse::Evm(signed_tx) => {
                assert_eq!(
                    signed_tx.signature.r,
                    "a37376a614e7c1c1605614b126467b0cf6ecb72e9a8d918e69d6048c4db42e89"
                );
                assert_eq!(
                    signed_tx.signature.s,
                    "1a87f2753e120205ae26681ad7d5158b6d8371424ca825ab4b773fb71e0c45fa"
                );
                assert_eq!(signed_tx.signature.v, 27);
            }
            _ => panic!("Expected EVM signed transaction"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_error_handling() {
        let mut mock_service = MockCdpServiceTrait::new();

        let tx_data = crate::models::EvmTransactionData {
            from: "0x7f5f4552091a69125d5dfcb7b8c2658029395bdf".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: None,
            gas_limit: Some(21000),
            nonce: Some(0),
            value: crate::models::U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
            max_fee_per_gas: Some(1),
            max_priority_fee_per_gas: Some(1),
            speed: None,
        };

        mock_service
            .expect_sign_evm_transaction()
            .returning(move |_| {
                Box::pin(async {
                    Err(CdpError::SigningError(
                        "Mock transaction signing error".into(),
                    ))
                })
            });

        let signer = CdpSigner::new_for_testing(mock_service);

        let result = signer
            .sign_transaction(NetworkTransactionData::Evm(tx_data))
            .await;

        assert!(result.is_err());
        match result {
            Err(SignerError::CdpError(err)) => {
                assert_eq!(
                    err.to_string(),
                    "Signing error: Mock transaction signing error"
                );
            }
            _ => panic!("Expected SigningError error variant"),
        }
    }
}
