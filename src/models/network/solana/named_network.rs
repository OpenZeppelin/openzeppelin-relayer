use core::{fmt, time::Duration};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

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
    type Err = (); // Define an appropriate error type

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // Add your specific string-to-network mappings here
            "mainnet" => Ok(SolanaNamedNetwork::MainnetBeta),
            "testnet" => Ok(SolanaNamedNetwork::Testnet),
            "devnet" => Ok(SolanaNamedNetwork::Devnet),
            // Add other network mappings as needed
            _ => Err(()), // Return an error for unrecognized strings
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
    fn is_testnet() {
        assert_eq!(SolanaNamedNetwork::MainnetBeta.is_testnet(), false);
        assert_eq!(SolanaNamedNetwork::Testnet.is_testnet(), true);
    }

    #[test]
    fn rpc_url() {
        assert_eq!(
            SolanaNamedNetwork::MainnetBeta.public_rpc_urls(),
            &["https://api.mainnet-beta.solana.com"]
        );
    }
}
