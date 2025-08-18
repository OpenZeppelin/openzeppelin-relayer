//! # EVM Local Signer Implementation
//!
//! This module provides a local signer implementation for Ethereum Virtual Machine (EVM)
//! transactions and messages using the Alloy library with an in-memory private key.
//!
//! ## Features
//!
//! - Support for both legacy and EIP-1559 transaction types
//! - Message signing with standard Ethereum prefixing
//! - Implementation of the `DataSignerTrait` for EVM-specific operations
//!
//! ## Security Considerations
//!
//! This implementation stores private keys in memory and should primarily be used
//! for development and testing purposes, not production
use alloy::{
    consensus::{SignableTransaction, TxEip1559, TxLegacy},
    network::{EthereumWallet, TransactionBuilder, TxSigner},
    rpc::types::Transaction,
    signers::{
        k256::ecdsa::SigningKey, local::LocalSigner as AlloyLocalSignerClient,
        Signer as AlloySigner, SignerSync,
    },
};

use alloy::primitives::{address, Address as AlloyAddress, Bytes, FixedBytes, TxKind, U256};

use async_trait::async_trait;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseEvm, SignTypedDataRequest,
    },
    models::{
        Address, EvmTransactionData, EvmTransactionDataSignature, EvmTransactionDataTrait,
        NetworkTransactionData, Signer as SignerDomainModel, SignerError, SignerRepoModel,
        SignerType, TransactionRepoModel,
    },
    services::Signer,
};

use super::DataSignerTrait;

use alloy::rpc::types::TransactionRequest;

#[derive(Clone)]
pub struct LocalSigner {
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerDomainModel) -> Result<Self, SignerError> {
        let config = signer_model
            .config
            .get_local()
            .ok_or_else(|| SignerError::Configuration("Local config not found".to_string()))?;

        let local_signer_client = {
            let key_bytes = config.raw_key.borrow();

            AlloyLocalSignerClient::from_bytes(&FixedBytes::from_slice(&key_bytes)).map_err(
                |e| SignerError::Configuration(format!("Failed to create local signer: {}", e)),
            )?
        };

        Ok(Self {
            local_signer_client,
        })
    }
}

impl From<AlloyAddress> for Address {
    fn from(addr: AlloyAddress) -> Self {
        Address::Evm(addr.into_array())
    }
}

#[async_trait]
impl Signer for LocalSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        let address: Address = self.local_signer_client.address().into();
        Ok(address)
    }

    async fn sign_transaction(
        &self,
        transaction: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        let evm_data = transaction.get_evm_transaction_data()?;
        if evm_data.is_eip1559() {
            // Handle EIP-1559 transaction
            let mut unsigned_tx = TxEip1559::try_from(transaction)?;

            let signature = self
                .local_signer_client
                .sign_transaction(&mut unsigned_tx)
                .await
                .map_err(|e| {
                    SignerError::SigningError(format!("Failed to sign EIP-1559 transaction: {e}"))
                })?;

            let signed_tx = unsigned_tx.into_signed(signature);
            let mut signature_bytes = signature.as_bytes();

            // Adjust v value for EIP-1559 (27/28 -> 0/1)
            if signature_bytes[64] == 27 {
                signature_bytes[64] = 0;
            } else if signature_bytes[64] == 28 {
                signature_bytes[64] = 1;
            }

            let mut raw = Vec::with_capacity(signed_tx.eip2718_encoded_length());
            signed_tx.eip2718_encode(&mut raw);

            Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                hash: signed_tx.hash().to_string(),
                signature: EvmTransactionDataSignature::from(&signature_bytes),
                raw,
            }))
        } else {
            // Handle legacy transaction
            let mut unsigned_tx = TxLegacy::try_from(transaction.clone())?;

            let signature = self
                .local_signer_client
                .sign_transaction(&mut unsigned_tx)
                .await
                .map_err(|e| {
                    SignerError::SigningError(format!("Failed to sign legacy transaction: {e}"))
                })?;

            let signed_tx = unsigned_tx.into_signed(signature);
            let signature_bytes = signature.as_bytes();

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
impl DataSignerTrait for LocalSigner {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        let message = request.message.as_bytes();

        let signature = self
            .local_signer_client
            .sign_message(message)
            .await
            .map_err(|e| SignerError::SigningError(format!("Failed to sign message: {}", e)))?;

        let ste = signature.as_bytes();

        Ok(SignDataResponse::Evm(SignDataResponseEvm {
            r: hex::encode(&ste[0..32]),
            s: hex::encode(&ste[32..64]),
            v: ste[64],
            sig: hex::encode(ste),
        }))
    }

    async fn sign_typed_data(
        &self,
        _typed_data: SignTypedDataRequest,
    ) -> Result<SignDataResponse, SignerError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use secrets::SecretVec;

    use crate::models::{EvmTransactionData, LocalSignerConfig, SignerConfig, U256};

    use super::*;
    use std::str::FromStr;

    fn create_test_signer_model() -> SignerDomainModel {
        let seed = vec![1u8; 32];
        let raw_key = SecretVec::new(32, |v| v.copy_from_slice(&seed));
        SignerDomainModel {
            id: "test".to_string(),
            config: SignerConfig::Local(LocalSignerConfig { raw_key }),
        }
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
    async fn test_address_generation() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let address = signer.address().await.unwrap();

        match address {
            Address::Evm(addr) => {
                assert_eq!(addr.len(), 20); // EVM addresses are 20 bytes
            }
            _ => panic!("Expected EVM address"),
        }
    }

    #[tokio::test]
    async fn test_sign_transaction_invalid_data() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let mut tx = create_test_transaction();

        if let NetworkTransactionData::Evm(ref mut evm_tx) = tx {
            evm_tx.data = Some("invalid_hex".to_string());
        }

        let result = signer.sign_transaction(tx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_sign_data() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let request = SignDataRequest {
            message: "Test message".to_string(),
        };

        let result = signer.sign_data(request).await.unwrap();

        match result {
            SignDataResponse::Evm(sig) => {
                assert_eq!(sig.r.len(), 64); // 32 bytes in hex
                assert_eq!(sig.s.len(), 64); // 32 bytes in hex
                assert!(sig.v == 27 || sig.v == 28); // Valid v values
                assert_eq!(sig.sig.len(), 130); // 65 bytes in hex
            }
            _ => panic!("Expected EVM signature"),
        }
    }

    #[tokio::test]
    async fn test_sign_data_empty_message() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
        let request = SignDataRequest {
            message: "".to_string(),
        };

        let result = signer.sign_data(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sign_transaction_with_contract_creation() {
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
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
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
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
        let signer = LocalSigner::new(&create_test_signer_model()).unwrap();
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
