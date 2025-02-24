use alloy::{
    consensus::{SignableTransaction, TxLegacy},
    network::{EthereumWallet, TransactionBuilder, TxSigner},
    rpc::types::Transaction,
    signers::{
        k256::ecdsa::SigningKey, local::LocalSigner as AlloyLocalSignerClient,
        Signer as AlloySigner, SignerSync,
    },
};

use alloy::primitives::{address, Address as AlloyAddress, FixedBytes, U256};

use async_trait::async_trait;

use crate::{
    domain::{
        SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTransactionResponse,
        SignTransactionResponseEvm, SignTypedDataRequest,
    },
    models::{
        Address, EvmTransactionDataSignature, NetworkTransactionData, SignerError, SignerRepoModel,
        SignerType, TransactionRepoModel,
    },
    services::Signer,
};

use super::DataSignerTrait;

use alloy::rpc::types::TransactionRequest;

pub struct LocalSigner {
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Self {
        let raw_key = signer_model.raw_key.as_ref().expect("keystore not found");

        // transforms the key into alloy wallet
        let key_bytes = FixedBytes::from_slice(raw_key);
        let local_signer_client =
            AlloyLocalSignerClient::from_bytes(&key_bytes).expect("failed to create signer");

        Self {
            local_signer_client,
        }
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
        let mut unsigned_tx = TxLegacy::try_from(transaction)?;

        let signature = self
            .local_signer_client
            .sign_transaction(&mut unsigned_tx)
            .await
            .map_err(|e| SignerError::SigningError(format!("Failed to sign transaction: {e}")))?;

        let signed_tx = unsigned_tx.into_signed(signature);
        let signature_bytes = signature.as_bytes();

        let mut raw = Vec::new();
        signed_tx.rlp_encode(&mut raw);

        Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
            hash: signed_tx.hash().to_string(),
            signature: EvmTransactionDataSignature::from(&signature_bytes),
            raw,
        }))
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
    use super::*;
    use crate::models::{
        EvmTransactionData, NetworkTransactionData, SignerType, TransactionStatus,
    };

    #[tokio::test]
    async fn test_sign_transaction() {
        let signer_model = SignerRepoModel {
            id: "test".to_string(),
            signer_type: SignerType::Local,
            path: None,
            raw_key: Some(vec![1u8; 32]),
            passphrase: None,
        };

        let signer = LocalSigner::new(&signer_model);

        let transaction = NetworkTransactionData::Evm(EvmTransactionData {
            from: "0x742d35Cc6634C0532925a3b844Bc454e4438f44e".to_string(),
            to: Some("0x742d35Cc6634C0532925a3b844Bc454e4438f44f".to_string()),
            gas_price: 20000000000,
            gas_limit: 21000,
            nonce: 0,
            value: U256::from(1000000000000000000u64),
            data: Some("0x".to_string()),
            chain_id: 1,
            hash: None,
            signature: None,
            raw: None,
        });

        let result = signer.sign_transaction(transaction).await.unwrap();
        match result {
            SignTransactionResponse::Evm(evm) => {
                assert!(!evm.hash.is_empty());
                assert!(!evm.raw.is_empty());
                assert!(!evm.signature.sig.is_empty());
            }
            _ => panic!("Wrong variant"),
        }
    }
}
