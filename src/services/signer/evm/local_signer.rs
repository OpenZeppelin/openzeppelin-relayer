use alloy::signers::{
    k256::ecdsa::SigningKey, local::LocalSigner as AlloyLocalSignerClient, Signer as AlloySigner,
    SignerSync,
};

use alloy::primitives::{Address as AlloyAddress, FixedBytes};

use async_trait::async_trait;

use crate::{
    domain::{SignDataRequest, SignDataResponse, SignDataResponseEvm, SignTypedDataRequest},
    models::{Address, SignerRepoModel, TransactionRepoModel},
    services::{Signer, SignerError},
};

use super::DataSignerTrait;

pub struct LocalSigner {
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    pub fn new(signer_model: &SignerRepoModel) -> Self {
        let raw_key = signer_model.raw_key.as_ref().expect("keystore not found");

        // transforms the key into alloy wallet
        let key_bytes = FixedBytes::from_slice(&raw_key);
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
        _transaction: TransactionRepoModel,
    ) -> Result<Vec<u8>, SignerError> {
        todo!()
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
