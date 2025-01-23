use std::str::FromStr;

use alloy::signers::{k256::ecdsa::SigningKey, local::LocalSigner as AlloyLocalSignerClient};

use alloy::primitives::Address as AlloyAddress;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;

use crate::{
    models::{Address, SignerRepoModel, TransactionRepoModel},
    services::{Signer, SignerError},
};

use super::EvmSignerTrait;

pub struct LocalSigner {
    signer_model: SignerRepoModel,
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    pub fn new(signer_model: SignerRepoModel) -> Self {
        let local_signer_client = AlloyLocalSignerClient::from_str("").unwrap();
        Self {
            signer_model,
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
        let addr = self.local_signer_client.address();
        let address: Address = addr.into();
        Ok(address)
    }

    async fn sign_transaction(
        &self,
        transaction: TransactionRepoModel,
    ) -> Result<Vec<u8>, SignerError> {
        todo!()
    }
}

#[async_trait]
impl EvmSignerTrait for LocalSigner {
    async fn sign_data(&self, data: Bytes) -> Result<Vec<u8>, SignerError> {
        todo!()
    }

    async fn sign_typed_data(&self, typed_data: Value) -> Result<Vec<u8>, SignerError> {
        todo!()
    }
}
