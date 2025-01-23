use alloy::signers::{k256::ecdsa::SigningKey, local::LocalSigner as AlloyLocalSignerClient};

use alloy::primitives::{Address as AlloyAddress, FixedBytes};

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;

use crate::{
    models::{Address, SignerRepoModel, TransactionRepoModel},
    services::{Signer, SignerError},
};

use super::EvmSignerTrait;

pub struct LocalSigner {
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    pub fn new(signer_model: SignerRepoModel) -> Self {
        let raw_key = signer_model.raw_key.expect("keystore not found");

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
impl EvmSignerTrait for LocalSigner {
    async fn sign_data(&self, _data: Bytes) -> Result<Vec<u8>, SignerError> {
        todo!()
    }

    async fn sign_typed_data(&self, _typed_data: Value) -> Result<Vec<u8>, SignerError> {
        todo!()
    }
}
