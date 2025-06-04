use crate::models::NetworkError;
use core::{fmt, time::Duration};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MidnightNamedNetwork {
    Testnet,
}

impl Default for MidnightNamedNetwork {
    fn default() -> Self {
        Self::Testnet
    }
}

#[allow(dead_code)]
impl MidnightNamedNetwork {
    pub const fn as_str(&self) -> &'static str {
        match self {
            MidnightNamedNetwork::Testnet => "testnet",
        }
    }

    pub const fn average_blocktime(self) -> Option<Duration> {
        Some(Duration::from_secs(match self {
            MidnightNamedNetwork::Testnet => 6,
        }))
    }

    pub const fn explorer_urls(self) -> &'static [&'static str] {
        match self {
            MidnightNamedNetwork::Testnet => &[
                "https://polkadot.js.org/apps/?rpc=wss://rpc.testnet-02.midnight.network#/explorer",
            ],
        }
    }

    pub const fn public_rpc_urls(self) -> &'static [&'static str] {
        match self {
            MidnightNamedNetwork::Testnet => &["https://rpc.testnet-02.midnight.network"],
        }
    }

    pub const fn is_testnet(&self) -> bool {
        matches!(self, MidnightNamedNetwork::Testnet)
    }
}

impl fmt::Display for MidnightNamedNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl AsRef<str> for MidnightNamedNetwork {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for MidnightNamedNetwork {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "testnet" => Ok(MidnightNamedNetwork::Testnet),
            _ => Err(NetworkError::InvalidNetwork(format!(
                "Invalid Midnight network: {}",
                s
            ))),
        }
    }
}

impl Serialize for MidnightNamedNetwork {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for MidnightNamedNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NetworkVisitor;

        impl serde::de::Visitor<'_> for NetworkVisitor {
            type Value = MidnightNamedNetwork;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("network name")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "testnet" => Ok(MidnightNamedNetwork::Testnet),
                    _ => Err(serde::de::Error::unknown_variant(
                        value,
                        &["mainnet", "testnet"],
                    )),
                }
            }
        }

        deserializer.deserialize_str(NetworkVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;
    use serde_json::json;

    #[test]
    fn test_default() {
        assert_eq!(
            serde_json::to_string(&MidnightNamedNetwork::default()).unwrap(),
            "\"testnet\""
        );
    }

    #[test]
    fn test_is_testnet() {
        assert!(MidnightNamedNetwork::Testnet.is_testnet());
    }

    #[test]
    fn test_rpc_url() {
        assert_eq!(
            MidnightNamedNetwork::Testnet.public_rpc_urls(),
            &["https://rpc.testnet-02.midnight.network"]
        );
    }

    #[test]
    fn test_explorer_url() {
        assert_eq!(
            MidnightNamedNetwork::Testnet.explorer_urls(),
            &["https://polkadot.js.org/apps/?rpc=wss://rpc.testnet-02.midnight.network#/explorer"]
        );
    }

    #[test]
    fn test_average_blocktime() {
        assert_eq!(
            MidnightNamedNetwork::Testnet.average_blocktime(),
            Some(Duration::from_secs(6))
        );
    }

    #[test]
    fn test_from_str_error() {
        // Test with an invalid network name
        let result = MidnightNamedNetwork::from_str("invalid_network");
        assert!(result.is_err());
    }

    #[test]
    fn test_midnight_named_network_display() {
        let network = MidnightNamedNetwork::Testnet;
        assert_eq!(network.to_string(), "testnet");
    }

    #[test]
    fn test_deserialize_valid_networks() {
        // Test testnet
        let json = json!("testnet");
        let result: Result<MidnightNamedNetwork, _> = serde_json::from_value(json);
        assert_eq!(result.unwrap(), MidnightNamedNetwork::Testnet);
    }

    #[test]
    fn test_deserialize_invalid_network() {
        let json = json!("invalid_network");
        let result: Result<MidnightNamedNetwork, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }
}
