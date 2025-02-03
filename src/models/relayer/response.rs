use crate::models::NetworkType;
use serde::{Deserialize, Serialize};

use super::{RelayerNetworkPolicy, RelayerRepoModel};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub network: String,
    pub network_type: NetworkType,
    pub paused: bool,
    pub policies: NetworkPolicyResponse,
    pub address: String,
    pub system_disabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum NetworkPolicyResponse {
    Evm(EvmPolicyResponse),
    Solana(SolanaPolicyResponse),
    Stellar(StellarPolicyResponse),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct EvmPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_price_cap: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: bool,
    pub private_transactions: bool,
    pub min_balance: u128,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SolanaPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_blocks: Option<u64>,
    pub min_balance: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct StellarPolicyResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee: Option<u32>,
    pub min_balance: u64,
}

impl From<RelayerRepoModel> for RelayerResponse {
    fn from(model: RelayerRepoModel) -> Self {
        let policies = match model.policies {
            RelayerNetworkPolicy::Evm(evm) => NetworkPolicyResponse::Evm(EvmPolicyResponse {
                gas_price_cap: evm.gas_price_cap,
                whitelist_receivers: evm.whitelist_receivers,
                eip1559_pricing: evm.eip1559_pricing,
                min_balance: evm.min_balance,
                private_transactions: evm.private_transactions,
            }),
            RelayerNetworkPolicy::Solana(solana) => {
                NetworkPolicyResponse::Solana(SolanaPolicyResponse {
                    max_retries: solana.max_retries,
                    confirmation_blocks: solana.confirmation_blocks,
                    min_balance: solana.min_balance,
                })
            }
            RelayerNetworkPolicy::Stellar(stellar) => {
                NetworkPolicyResponse::Stellar(StellarPolicyResponse {
                    max_fee: stellar.max_fee,
                    min_balance: stellar.min_balance,
                })
            }
        };

        Self {
            id: model.id,
            name: model.name,
            network: model.network,
            network_type: model.network_type,
            paused: model.paused,
            policies,
            address: model.address,
            system_disabled: model.system_disabled,
        }
    }
}
