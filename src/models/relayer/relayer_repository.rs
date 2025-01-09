use serde::Serialize;
use strum::Display;

#[derive(Debug, Clone, Serialize, PartialEq, Display)]
pub enum NetworkType {
    Evm,
    Stellar,
    Solana,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayerRepoModel {
    pub id: String,
    pub name: String,
    pub network: String,
    pub paused: bool,
    pub network_type: NetworkType,
}
