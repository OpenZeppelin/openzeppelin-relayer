use core::{fmt, time::Duration};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StellarNamedNetwork {
    Mainnet,
    Testnet,
}

impl Default for StellarNamedNetwork {
    fn default() -> Self {
        Self::Mainnet
    }
}

#[allow(dead_code)]
impl StellarNamedNetwork {
    pub const fn as_str(&self) -> &'static str {
        match self {
            StellarNamedNetwork::Mainnet => "mainnet",
            StellarNamedNetwork::Testnet => "testnet",
        }
    }

    pub const fn average_blocktime(self) -> Option<Duration> {
        Some(Duration::from_secs(match self {
            StellarNamedNetwork::Mainnet => 5,
            StellarNamedNetwork::Testnet => 5,
        }))
    }

    pub const fn explorer_urls(self) -> &'static [&'static str] {
        match self {
            StellarNamedNetwork::Mainnet => &["https://stellar.expert/explorer/public"],
            StellarNamedNetwork::Testnet => &["https://stellar.expert/explorer/testnet"],
        }
    }

    pub const fn public_rpc_urls(self) -> &'static [&'static str] {
        match self {
            StellarNamedNetwork::Mainnet => &["https://horizon.stellar.org"],
            StellarNamedNetwork::Testnet => &["https://horizon-testnet.stellar.org"],
        }
    }

    pub const fn is_testnet(&self) -> bool {
        matches!(self, StellarNamedNetwork::Testnet)
    }
}

impl fmt::Display for StellarNamedNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl AsRef<str> for StellarNamedNetwork {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for StellarNamedNetwork {
    type Err = (); // Define an appropriate error type

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            // Add your specific string-to-network mappings here
            "mainnet" => Ok(StellarNamedNetwork::Mainnet),
            "testnet" => Ok(StellarNamedNetwork::Testnet),
            // Add other network mappings as needed
            _ => Err(()), // Return an error for unrecognized strings
        }
    }
}

impl Serialize for StellarNamedNetwork {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_ref())
    }
}

impl<'de> Deserialize<'de> for StellarNamedNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NetworkVisitor;

        impl serde::de::Visitor<'_> for NetworkVisitor {
            type Value = StellarNamedNetwork;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("network name")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value {
                    "mainnet" => Ok(StellarNamedNetwork::Mainnet),
                    "testnet" => Ok(StellarNamedNetwork::Testnet),
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
    fn default() {
        assert_eq!(
            serde_json::to_string(&StellarNamedNetwork::default()).unwrap(),
            "\"mainnet\""
        );
    }

    #[test]
    fn test_is_testnet() {
        assert!(!StellarNamedNetwork::Mainnet.is_testnet());
        assert!(StellarNamedNetwork::Testnet.is_testnet());
    }

    #[test]
    fn test_rpc_url() {
        assert_eq!(
            StellarNamedNetwork::Mainnet.public_rpc_urls(),
            &["https://horizon.stellar.org"]
        );
        assert_eq!(
            StellarNamedNetwork::Testnet.public_rpc_urls(),
            &["https://horizon-testnet.stellar.org"]
        );
        assert_eq!(
            StellarNamedNetwork::Testnet.public_rpc_urls(),
            &["https://horizon-testnet.stellar.org"]
        );
    }

    #[test]
    fn test_explorer_url() {
        assert_eq!(
            StellarNamedNetwork::Mainnet.explorer_urls(),
            &["https://stellar.expert/explorer/public"]
        );
        assert_eq!(
            StellarNamedNetwork::Testnet.explorer_urls(),
            &["https://stellar.expert/explorer/testnet"]
        );
    }

    #[test]
    fn test_average_blocktime() {
        assert_eq!(
            StellarNamedNetwork::Mainnet.average_blocktime(),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            StellarNamedNetwork::Testnet.average_blocktime(),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn test_from_str_error() {
        // Test with an invalid network name
        let result = StellarNamedNetwork::from_str("invalid_network");
        assert!(result.is_err());
    }

    #[test]
    fn test_stellar_named_network_display() {
        let network = StellarNamedNetwork::Mainnet;
        assert_eq!(network.to_string(), "mainnet");

        let network = StellarNamedNetwork::Testnet;
        assert_eq!(network.to_string(), "testnet");
    }

    #[test]
    fn test_deserialize_valid_networks() {
        // Test mainnet
        let json = json!("mainnet");
        let result: Result<StellarNamedNetwork, _> = serde_json::from_value(json);
        assert_eq!(result.unwrap(), StellarNamedNetwork::Mainnet);

        // Test testnet
        let json = json!("testnet");
        let result: Result<StellarNamedNetwork, _> = serde_json::from_value(json);
        assert_eq!(result.unwrap(), StellarNamedNetwork::Testnet);
    }

    #[test]
    fn test_deserialize_invalid_network() {
        let json = json!("invalid_network");
        let result: Result<StellarNamedNetwork, _> = serde_json::from_value(json);
        assert!(result.is_err());
    }
}
