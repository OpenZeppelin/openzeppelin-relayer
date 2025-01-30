use crate::models::NetworkType;
use serde::Serialize;

use super::{RelayerNetworkPolicy, RelayerRepoModel};

#[derive(Debug, Serialize)]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub network: String,
    pub network_type: NetworkType,
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies: Option<NetworkPolicyResponse>,
    pub system_disabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum NetworkPolicyResponse {
    Evm(EvmPolicyResponse),
    Solana(SolanaPolicyResponse),
    Stellar(StellarPolicyResponse),
}

#[derive(Debug, Serialize)]
pub struct EvmPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_price_cap: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whitelist_receivers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eip1559_pricing: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct SolanaPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_blocks: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct StellarPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_account_balance: Option<String>,
}

impl From<RelayerRepoModel> for RelayerResponse {
    fn from(model: RelayerRepoModel) -> Self {
        let policies = model.policies.map(|p| match p {
            RelayerNetworkPolicy::Evm(evm) => NetworkPolicyResponse::Evm(EvmPolicyResponse {
                gas_price_cap: evm.gas_price_cap,
                whitelist_receivers: evm.whitelist_receivers,
                eip1559_pricing: evm.eip1559_pricing,
            }),
            RelayerNetworkPolicy::Solana(solana) => {
                NetworkPolicyResponse::Solana(SolanaPolicyResponse {
                    max_retries: solana.max_retries,
                    confirmation_blocks: solana.confirmation_blocks,
                })
            }
            RelayerNetworkPolicy::Stellar(stellar) => {
                NetworkPolicyResponse::Stellar(StellarPolicyResponse {
                    max_fee: stellar.max_fee,
                    min_account_balance: stellar.min_account_balance,
                })
            }
        });

        Self {
            id: model.id,
            name: model.name,
            network: model.network,
            network_type: model.network_type,
            paused: model.paused,
            policies,
            system_disabled: model.system_disabled,
        }
    }
}
