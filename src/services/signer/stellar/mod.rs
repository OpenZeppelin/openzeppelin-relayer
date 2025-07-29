// openzeppelin-relayer/src/services/signer/stellar/mod.rs
//! Stellar signer implementation (local keystore)

mod local_signer;
mod vault_signer;

use async_trait::async_trait;
use local_signer::*;
use vault_signer::*;

use crate::{
    domain::{SignDataRequest, SignDataResponse, SignTransactionResponse, SignTypedDataRequest},
    models::{
        Address, NetworkTransactionData, Signer as SignerDomainModel, SignerConfig,
        SignerRepoModel, SignerType, TransactionRepoModel, VaultSignerConfig,
    },
    services::{
        signer::{SignerError, SignerFactoryError},
        Signer, VaultConfig, VaultService,
    },
};

use super::DataSignerTrait;

pub enum StellarSigner {
    Local(Box<LocalSigner>),
    Vault(VaultSigner<VaultService>),
}

#[async_trait]
impl Signer for StellarSigner {
    async fn address(&self) -> Result<Address, SignerError> {
        match self {
            Self::Local(s) => s.address().await,
            Self::Vault(s) => s.address().await,
        }
    }

    async fn sign_transaction(
        &self,
        tx: NetworkTransactionData,
    ) -> Result<SignTransactionResponse, SignerError> {
        match self {
            Self::Local(s) => s.sign_transaction(tx).await,
            Self::Vault(s) => s.sign_transaction(tx).await,
        }
    }
}

pub struct StellarSignerFactory;

impl StellarSignerFactory {
    pub fn create_stellar_signer(
        m: &SignerDomainModel,
    ) -> Result<StellarSigner, SignerFactoryError> {
        let signer = match &m.config {
            SignerConfig::Local(_) => {
                let local_signer = LocalSigner::new(m)?;
                StellarSigner::Local(Box::new(local_signer))
            }
            SignerConfig::Vault(config) => {
                let vault_config = VaultConfig::new(
                    config.address.clone(),
                    config.role_id.clone(),
                    config.secret_id.clone(),
                    config.namespace.clone(),
                    config
                        .mount_point
                        .clone()
                        .unwrap_or_else(|| "secret".to_string()),
                    None,
                );
                let vault_service = VaultService::new(vault_config);

                StellarSigner::Vault(VaultSigner::new(
                    m.id.clone(),
                    config.clone(),
                    vault_service,
                ))
            }
            SignerConfig::AwsKms(_) => {
                return Err(SignerFactoryError::UnsupportedType("AWS KMS".into()))
            }
            SignerConfig::VaultTransit(_) => {
                return Err(SignerFactoryError::UnsupportedType("Vault Transit".into()))
            }
            SignerConfig::Turnkey(_) => {
                return Err(SignerFactoryError::UnsupportedType("Turnkey".into()))
            }
            SignerConfig::GoogleCloudKms(_) => {
                return Err(SignerFactoryError::UnsupportedType(
                    "Google Cloud KMS".into(),
                ))
            }
        };
        Ok(signer)
    }
}
