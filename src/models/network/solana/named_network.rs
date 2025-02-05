use core::{fmt, time::Duration};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::models::NetworkError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SolanaNamedNetwork {
    MainnetBeta,
    Testnet,
    Devnet,
}

impl Default for SolanaNamedNetwork {
    fn default() -> Self {
        Self::MainnetBeta
    }
}

#[allow(dead_code)]
impl SolanaNamedNetwork {
    pub fn as_str(&self) -> &'static str {
        match self {
            SolanaNamedNetwork::MainnetBeta => "mainnet-beta",
            SolanaNamedNetwork::Testnet => "testnet",
            SolanaNamedNetwork::Devnet => "devnet",
        }
    }

    pub const fn is_testnet(&self) -> bool {
        matches!(
            self,
            SolanaNamedNetwork::Testnet | SolanaNamedNetwork::Devnet
        )
    }

    pub const fn public_rpc_urls(&self) -> &'static [&'static str] {
        match self {
            SolanaNamedNetwork::MainnetBeta => &["https://api.mainnet-beta.solana.com"],
            SolanaNamedNetwork::Testnet => &["https://api.testnet.solana.com"],
            SolanaNamedNetwork::Devnet => &["https://api.devnet.solana.com"],
        }
    }

    pub const fn explorer_urls(&self) -> &'static [&'static str] {
        match self {
            SolanaNamedNetwork::MainnetBeta => &["https://explorer.solana.com"],
            SolanaNamedNetwork::Testnet => &["https://explorer.solana.com?cluster=testnet"],
            SolanaNamedNetwork::Devnet => &["https://explorer.solana.com?cluster=devnet"],
        }
    }

    pub const fn average_blocktime(self) -> Option<Duration> {
        use SolanaNamedNetwork::*;

        Some(Duration::from_millis(match self {
            MainnetBeta => 400,
            Testnet => 400,
            Devnet => 400,
        }))
    }
}

impl fmt::Display for SolanaNamedNetwork {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl AsRef<str> for SolanaNamedNetwork {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for SolanaNamedNetwork {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "mainnet-beta" => Ok(SolanaNamedNetwork::MainnetBeta),
            "testnet" => Ok(SolanaNamedNetwork::Testnet),
            "devnet" => Ok(SolanaNamedNetwork::Devnet),
            _ => Err(NetworkError::InvalidNetwork(format!(
                "Invalid Solana network: {}",
                s
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default() {
        assert_eq!(
            serde_json::to_string(&SolanaNamedNetwork::default()).unwrap(),
            "\"mainnet_beta\""
        );
    }

    #[test]
    fn test_is_testnet() {
        assert!(!SolanaNamedNetwork::MainnetBeta.is_testnet());
        assert!(SolanaNamedNetwork::Testnet.is_testnet());
        assert!(SolanaNamedNetwork::Devnet.is_testnet());
    }

    #[test]
    fn test_rpc_url() {
        assert_eq!(
            SolanaNamedNetwork::MainnetBeta.public_rpc_urls(),
            &["https://api.mainnet-beta.solana.com"]
        );
        assert_eq!(
            SolanaNamedNetwork::Devnet.public_rpc_urls(),
            &["https://api.devnet.solana.com"]
        );
        assert_eq!(
            SolanaNamedNetwork::Testnet.public_rpc_urls(),
            &["https://api.testnet.solana.com"]
        );
    }

    #[test]
    fn test_explorer_url() {
        assert_eq!(
            SolanaNamedNetwork::MainnetBeta.explorer_urls(),
            &["https://explorer.solana.com"]
        );
        assert_eq!(
            SolanaNamedNetwork::Devnet.explorer_urls(),
            &["https://explorer.solana.com?cluster=devnet"]
        );
        assert_eq!(
            SolanaNamedNetwork::Testnet.explorer_urls(),
            &["https://explorer.solana.com?cluster=testnet"]
        );
    }

    #[test]
    fn test_average_blocktime() {
        assert_eq!(
            SolanaNamedNetwork::MainnetBeta.average_blocktime(),
            Some(Duration::from_millis(400))
        );
        assert_eq!(
            SolanaNamedNetwork::Devnet.average_blocktime(),
            Some(Duration::from_millis(400))
        );
        assert_eq!(
            SolanaNamedNetwork::Testnet.average_blocktime(),
            Some(Duration::from_millis(400))
        );
    }

    #[test]
    fn test_from_str() {
        assert_eq!(
            SolanaNamedNetwork::from_str("mainnet-beta").unwrap(),
            SolanaNamedNetwork::MainnetBeta
        );
        assert_eq!(
            SolanaNamedNetwork::from_str("testnet").unwrap(),
            SolanaNamedNetwork::Testnet
        );
        assert_eq!(
            SolanaNamedNetwork::from_str("devnet").unwrap(),
            SolanaNamedNetwork::Devnet
        );

        assert!(matches!(
            "invalid".parse::<SolanaNamedNetwork>(),
            Err(NetworkError::InvalidNetwork(_))
        ));
    }

    #[test]
    fn test_solana_named_network_display() {
        let network = SolanaNamedNetwork::MainnetBeta;
        assert_eq!(network.to_string(), "mainnet-beta");

        let network = SolanaNamedNetwork::Testnet;
        assert_eq!(network.to_string(), "testnet");

        let network = SolanaNamedNetwork::Devnet;
        assert_eq!(network.to_string(), "devnet");
    }
}
