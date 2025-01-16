use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Debug, Clone, Serialize, PartialEq, Display, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Serialize, Clone)]
pub enum RelayerNetworkPolicy {
    Evm(RelayerEvmPolicy),
    Solana(RelayerSolanaPolicy),
    Stellar(RelayerStellarPolicy),
}

#[derive(Debug, Serialize, Clone)]
pub struct RelayerEvmPolicy {
    pub gas_price_cap: Option<u64>,
    pub whitelist_receivers: Option<Vec<String>>,
    pub eip1559_pricing: Option<bool>,
    pub private_transactions: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerSolanaPolicy {
    pub max_retries: Option<u32>,
    pub confirmation_blocks: Option<u64>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RelayerStellarPolicy {
    pub max_fee: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub min_account_balance: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayerRepoModel {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: NetworkType,
    pub policies: Option<RelayerNetworkPolicy>,
}
