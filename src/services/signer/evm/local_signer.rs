use std::str::FromStr;

use alloy::{
    network::{EthereumWallet, NetworkWallet},
    primitives::FixedBytes,
    signers::{
        k256::{ecdsa::SigningKey, Secp256k1},
        local::LocalSigner as AlloyLocalSignerClient,
    },
};
use async_trait::async_trait;

use crate::{
    models::{Address, SignerRepoModel, TransactionRepoModel},
    services::{Signer, SignerError},
};

// TODO introduce Address model
// TODO
// Example local implementation
pub struct LocalSigner {
    signer_model: SignerRepoModel,
    local_signer_client: AlloyLocalSignerClient<SigningKey>,
}

impl LocalSigner {
    fn new(signer_model: SignerRepoModel) -> Self {
        let local_signer_client = AlloyLocalSignerClient::from_str("").unwrap();
        Self {
            signer_model,
            local_signer_client,
        }
    }
}

// #[async_trait]
// impl Signer for LocalSigner {
//   async fn address(&self) -> Result<Address, SignerError> {
//       // Implementation
//       let address: Address::Evm = self.local_signer_client.address();
//   }

//   async fn sign_transaction(
//       &self,
//       transaction: TransactionRepoModel,
//   ) -> Result<Vec<u8>, SignerError> {
//       // Implementation
//       todo!()
//   }
// }
