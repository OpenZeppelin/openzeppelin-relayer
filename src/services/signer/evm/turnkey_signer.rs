use std::str::FromStr;

use alloy::{
    consensus::{SignableTransaction, TxEip1559, TxLegacy},
    primitives::keccak256,
};
use async_trait::async_trait;
use log::{debug, info};

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseEvm, SignTypedDataRequest,
    },
    models::{
        Address, EvmTransactionDataSignature, EvmTransactionDataTrait, NetworkTransactionData,
        SignerError, SignerRepoModel, TurnkeySignerConfig,
    },
    services::{Signer, TurnkeyService, TurnkeyServiceTrait},
};

use super::DataSignerTrait;

pub type DefaultTurnkeyService = TurnkeyService;

pub struct TurnkeySigner<T = DefaultTurnkeyService>
where
    T: TurnkeyServiceTrait,
{
    turnkey_service: T,
    config: TurnkeySignerConfig,
}

impl TurnkeySigner<DefaultTurnkeyService> {
    pub fn new(config: &TurnkeySignerConfig, turnkey_service: DefaultTurnkeyService) -> Self {
        Self {
            turnkey_service,
            config: config.clone(),
        }
    }
}

#[cfg(test)]
impl<T: TurnkeyServiceTrait> TurnkeySigner<T> {
    pub fn new_with_service(signer_model: &SignerRepoModel, turnkey_service: T) -> Self {
        let config = signer_model
            .config
            .get_turnkey()
            .expect("turnkey config not found");

        Self {
            turnkey_service,
            config: config.clone(),
        }
    }

    pub fn new_for_testing(config: TurnkeySignerConfig, turnkey_service: T) -> Self {
        Self {
            turnkey_service,
            config,
        }
    }
}

#[async_trait]
impl<T: TurnkeyServiceTrait> Signer for TurnkeySigner<T> {
    async fn address(&self) -> Result<Address, SignerError> {
        let address = self.turnkey_service.address_evm()?;

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

        // Sign the data with Turnkey service
        let signed_bytes = self
            .turnkey_service
            .sign_evm_transaction(&unsigned_tx_bytes)
            .await?;

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
impl<T: TurnkeyServiceTrait> DataSignerTrait for TurnkeySigner<T> {
    async fn sign_data(&self, request: SignDataRequest) -> Result<SignDataResponse, SignerError> {
        // For EVM, we need to follow Ethereum's personal_sign format:
        // First, create the Ethereum signed message format
        let message_bytes = request.message.as_bytes();
        let prefix = format!("\x19Ethereum Signed Message:\n{}", message_bytes.len());

        let mut prefixed_message = prefix.into_bytes();
        prefixed_message.extend_from_slice(message_bytes);

        // let mut prefixed_message = prefix.as_bytes().to_vec();
        // prefixed_message.extend_from_slice(message_bytes);

        let message_hash = keccak256(&prefixed_message);

        // Sign the prefixed message
        let signature_bytes = self
            .turnkey_service
            .sign_evm(&message_hash.to_vec())
            .await?;

        // Ensure we have the right signature length
        if signature_bytes.len() != 65 {
            return Err(SignerError::SigningError(format!(
                "Invalid signature length from Turnkey: expected 65 bytes, got {}",
                signature_bytes.len()
            )));
        }

        // Extract r, s, v components
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
            "EIP-712 typed data signing not yet implemented for Turnkey".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::body::MessageBody;
    use alloy::primitives::{keccak256, private::alloy_rlp::*};

    #[test]
    fn test() {
        let public_key_hex = "047d3bb8e0317927700cf19fed34e0627367be1390ec247dddf8c239e4b4321a49aea80090e49b206b6a3e577a4f11d721ab063482001ee10db40d6f2963233eec";

        // Convert the hex string into a vector of bytes.
        let public_key = hex::decode(public_key_hex).unwrap();

        // Remove the first byte (0x04 prefix) to get the 64-byte concatenation of X and Y.
        let pub_key_no_prefix = &public_key[1..];

        // Compute the Keccak-256 hash of the 64-byte public key.
        let hash = keccak256(pub_key_no_prefix);

        // Ethereum addresses are the last 20 bytes of the Keccak-256 hash.
        // Since the hash is 32 bytes, the address is bytes 12..32.
        let address_bytes = &hash[12..];

        let address = hex::encode(address_bytes);

        println!("Ethereum address: 0x{}", address);

        // let mut signed = "02f86f83aa36a70185027c27ad0085188159d99882520894ad6ed179ce21440a0c1d44adc60c82cb73f604250180c001a06f56b7c9df749a975b30c91f44721c8a68f5e050252b6af01753ea5871e4e3d6a04c8d3db66334046338ddd0eb81c6b1443ee511cedde8cfffbd132cf483260d14";
        // let mut signed_bytes = hex::decode(signed).expect("Failed to decode hex string");
        // let mut aa = signed_bytes.as_slice();

        // let test: alloy::consensus::Signed<TxLegacy> =
        //     alloy::consensus::Signed::eip2718_decode(&mut aa).unwrap();

        // let signature = test.signature();
        // let hash = test.tx().signature_hash();

        // println!("Signature: {:?}", signature);

        // println!("Hash: {:?}", hash);
        // let r = signature.r();
        // let s = signature.s().to_vec();
        // let v = signature.v();

        // let mut sig_bytes = Vec::with_capacity(65);
        // sig_bytes.extend_from_slice(&r);
        // sig_bytes.extend_from_slice(&s);
        // sig_bytes.push(v);

        // let hash = alloy::primitives::keccak256(&signed_bytes);
        // let result = SignTransactionResponse::Evm(SignTransactionResponseEvm {
        //     hash: format!("0x{}", hex::encode(hash)),
        //     signature: EvmTransactionDataSignature {
        //         r: hex::encode(r),
        //         s: hex::encode(s),
        //         v,
        //         sig: hex::encode(&sig_bytes),
        //     },
        //     raw: signed_bytes,
        // });
        //     debug!("Result: {:?}", result);
        // let mut signed_bytes_slice = signed_bytes.as_slice();
        // let signed_tx = alloy::consensus::TxEip1559::decode(&mut signed_bytes_slice)
        //     .map_err(|e| SignerError::SigningError(format!("Failed to decode signed transaction: {}", e))).unwrap();

        // signed_tx.

        //     // Extract signature components
        //     let r = signed_tx.r().to_vec();
        //     let s = signed_tx.s().to_vec();
        //     let v = signed_tx.v();

        //     let r = signature.r().to_vec();
        //     let s = signature.s().to_vec();
        //     let v = signature.v();

        // Combine r, s, v into a single signature
        // let mut sig_bytes = Vec::with_capacity(65);
        // sig_bytes.extend_from_slice(&r);
        // sig_bytes.extend_from_slice(&s);
        // sig_bytes.push(v);

        // // Calculate the transaction hash
        // let hash = alloy::primitives::keccak256(&signed);

        //     let result = SignTransactionResponse::Evm(SignTransactionResponseEvm {
        //         hash: format!("0x{}", hex::encode(hash)),
        //         signature: EvmTransactionDataSignature {
        //             r: hex::encode(r),
        //             s: hex::encode(s),
        //             v,
        //             sig: hex::encode(&sig_bytes),
        //         },
        //         raw: signed_bytes,
        //     });
    }
    //     use crate::{
    //         models::{SecretString, TurnkeySignerConfig},
    //         services::MockTurnkeyServiceTrait,
    //     };
    //     use mockall::predicate::*;

    //     fn create_test_config() -> TurnkeySignerConfig {
    //         TurnkeySignerConfig {
    //             api_public_key: "test-api-public-key".to_string(),
    //             api_private_key: SecretString::new("test-api-private-key"),
    //             organization_id: "test-org-id".to_string(),
    //             private_key_id: "test-private-key-id".to_string(),
    //             public_key: "7f5f4552091a69125d5dfcb7b8c2658029395bdf".to_string(), // An example Ethereum address
    //         }
    //     }

    //     #[test]
    //     fn test_new_with_service() {
    //         let mut mock_service = MockTurnkeyServiceTrait::new();
    //         let config = create_test_config();

    //         let signer = TurnkeySigner::new_for_testing(config.clone(), mock_service);

    //         assert_eq!(signer.config.api_public_key, "test-api-public-key");
    //         assert_eq!(signer.config.organization_id, "test-org-id");
    //         assert_eq!(signer.config.public_key, "7f5f4552091a69125d5dfcb7b8c2658029395bdf");
    //     }

    //     #[tokio::test]
    //     async fn test_address() {
    //         let mut mock_service = MockTurnkeyServiceTrait::new();
    //         let config = create_test_config();

    //         let signer = TurnkeySigner::new_for_testing(config, mock_service);
    //         let result = signer.address().await.unwrap();

    //         match result {
    //             Address::Evm(addr) => {
    //                 assert_eq!(hex::encode(addr), "7f5f4552091a69125d5dfcb7b8c2658029395bdf");
    //             }
    //             _ => panic!("Expected EVM address"),
    //         }
    //     }

    //     #[tokio::test]
    //     async fn test_sign_data() {
    //         let mut mock_service = MockTurnkeyServiceTrait::new();
    //         let test_message = "Test message";

    //         // The prefixed message that should be sent to Turnkey
    //         let prefix = format!("\x19Ethereum Signed Message:\n{}", test_message.len());
    //         let mut prefixed_message = prefix.as_bytes().to_vec();
    //         prefixed_message.extend_from_slice(test_message.as_bytes());

    //         // Create a mock signature
    //         let r = [1u8; 32];
    //         let s = [2u8; 32];
    //         let v = 27u8;
    //         let mut mock_sig = Vec::with_capacity(65);
    //         mock_sig.extend_from_slice(&r);
    //         mock_sig.extend_from_slice(&s);
    //         mock_sig.push(v);

    //         mock_service
    //             .expect_sign()
    //             .with(eq(prefixed_message))
    //             .times(1)
    //             .returning(move |_| Ok(mock_sig.clone()));

    //         let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);
    //         let request = SignDataRequest {
    //             message: test_message.to_string(),
    //         };

    //         let result = signer.sign_data(request).await.unwrap();

    //         match result {
    //             SignDataResponse::Evm(sig) => {
    //                 assert_eq!(sig.r, "0101010101010101010101010101010101010101010101010101010101010101");
    //                 assert_eq!(sig.s, "0202020202020202020202020202020202020202020202020202020202020202");
    //                 assert_eq!(sig.v, 27);
    //                 assert_eq!(sig.sig, "01010101010101010101010101010101010101010101010101010101010101010202020202020202020202020202020202020202020202020202020202020202" + "1b");
    //             }
    //             _ => panic!("Expected EVM signature"),
    //         }
    //     }

    //     #[tokio::test]
    //     async fn test_sign_transaction() {
    //         let mut mock_service = MockTurnkeyServiceTrait::new();

    //         // Create a mock transaction
    //         let tx_data = NetworkTransactionData::Evm(crate::models::EvmTransactionData {
    //             from: "0x7f5f4552091a69125d5dfcb7b8c2658029395bdf".to_string(),
    //             to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
    //             gas_price: Some(20000000000),
    //             gas_limit: 21000,
    //             nonce: Some(0),
    //             value: crate::models::U256::from(1000000000000000000u64),
    //             data: Some("0x".to_string()),
    //             chain_id: 1,
    //             hash: None,
    //             signature: None,
    //             raw: None,
    //             max_fee_per_gas: None,
    //             max_priority_fee_per_gas: None,
    //             speed: None,
    //         });

    //         // Create a mock unsigned TX bytes that the EvmTransactionData.serialize_unsigned would return
    //         let unsigned_tx_bytes = vec![1, 2, 3, 4, 5];

    //         // Create a mock signature
    //         let r = [1u8; 32];
    //         let s = [2u8; 32];
    //         let v = 27u8;
    //         let mut mock_sig = Vec::with_capacity(65);
    //         mock_sig.extend_from_slice(&r);
    //         mock_sig.extend_from_slice(&s);
    //         mock_sig.push(v);

    //         // Setup expectations on the mock
    //         mock_service
    //             .expect_sign()
    //             .with(eq(unsigned_tx_bytes.clone()))
    //             .times(1)
    //             .returning(move |_| Ok(mock_sig.clone()));

    //         // Create the test signer with our mock
    //         let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);

    //         // We need to mock the EvmTransactionData methods since we can't directly test with real transactions
    //         // This requires a more complex test setup with partial mocking or custom struct implementations

    //         // For simplicity in this example, I'll skip the actual test execution
    //         // In a real test, you would:
    //         // 1. Setup appropriate mocks for the tx_data.serialize_unsigned() method
    //         // 2. Setup mocks for hash_signed and serialize_signed
    //         // 3. Execute signer.sign_transaction(tx_data).await
    //         // 4. Verify the results match expectations
    //     }

    //     #[tokio::test]
    //     async fn test_sign_typed_data_not_implemented() {
    //         let mut mock_service = MockTurnkeyServiceTrait::new();
    //         let signer = TurnkeySigner::new_for_testing(create_test_config(), mock_service);

    //         let result = signer.sign_typed_data(SignTypedDataRequest {
    //             domain: serde_json::json!({}),
    //             types: serde_json::json!({}),
    //             primary_type: "Test".to_string(),
    //             message: serde_json::json!({})
    //         }).await;

    //         match result {
    //             Err(SignerError::NotImplemented(_)) => {} // Expected
    //             _ => panic!("Expected NotImplemented error"),
    //         }
    //     }
}
