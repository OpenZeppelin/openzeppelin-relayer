use crate::models::{error::NetworkError, StellarNamedNetwork};
use core::{fmt, str::FromStr, time::Duration};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StellarNetwork(StellarNamedNetwork);

impl fmt::Debug for StellarNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Network::")?;
        self.kind().fmt(f)
    }
}

impl Default for StellarNetwork {
    fn default() -> Self {
        Self::from_named(StellarNamedNetwork::default())
    }
}

impl From<StellarNamedNetwork> for StellarNetwork {
    fn from(id: StellarNamedNetwork) -> Self {
        Self::from_named(id)
    }
}

impl FromStr for StellarNetwork {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(network) = StellarNamedNetwork::from_str(s) {
            Ok(Self::from_named(network))
        } else {
            Err(NetworkError::InvalidNetwork(format!(
                "Invalid network: {}, expected named network or chain ID",
                s
            )))
        }
    }
}

impl fmt::Display for StellarNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for StellarNetwork {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StellarNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NetworkVisitor;

        impl serde::de::Visitor<'_> for NetworkVisitor {
            type Value = StellarNetwork;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("network name")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(NetworkVisitor)
    }
}

#[allow(dead_code)]
impl StellarNetwork {
    pub const fn from_named(named: StellarNamedNetwork) -> Self {
        Self(named)
    }

    pub const fn kind(&self) -> &StellarNamedNetwork {
        &self.0
    }

    pub fn from_network_str(network: &str) -> Result<Self, NetworkError> {
        if let Ok(named) = StellarNamedNetwork::from_str(network) {
            Ok(Self::from_named(named))
        } else {
            Err(NetworkError::InvalidNetwork(format!(
                "Invalid network: {}, expected named network or chain ID",
                network
            )))
        }
    }

    pub const fn average_blocktime(self) -> Option<Duration> {
        self.0.average_blocktime()
    }

    pub const fn public_rpc_urls(self) -> &'static [&'static str] {
        self.0.public_rpc_urls()
    }

    pub const fn explorer_urls(self) -> &'static [&'static str] {
        self.0.explorer_urls()
    }

    pub const fn is_testnet(self) -> bool {
        self.0.is_testnet()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_default() {
        let default_network = StellarNetwork::default();
        let expected = StellarNamedNetwork::default();
        assert_eq!(
            default_network.kind(),
            &expected,
            "Default network does not match expected underlying network"
        );
    }

    #[test]
    fn test_from_str_valid() {
        let input = "mainnet";
        let parsed_network = input.parse::<StellarNetwork>();
        println!("parsed_network: {:?}", parsed_network);
        assert!(
            parsed_network.is_ok(),
            "Parsing valid network string should succeed"
        );
        let network = parsed_network.unwrap();
        assert_eq!(
            network.to_string(),
            "mainnet",
            "Display output does not match expected network name"
        );
    }

    #[test]
    fn test_from_str_invalid() {
        let input = "InvalidNetworkName";
        let parsed_network = input.parse::<StellarNetwork>();
        assert!(
            parsed_network.is_err(),
            "Parsing an invalid network string should fail"
        );
        let err = parsed_network.unwrap_err();
        // Ensure the error message contains the expected substring.
        assert!(
            format!("{}", err).contains("Invalid network"),
            "Error message should indicate an invalid network"
        );
    }

    #[test]
    fn test_debug_format() {
        let input = "mainnet";
        let parsed_network = input.parse::<StellarNetwork>().unwrap();
        let debug_str = format!("{:?}", parsed_network);
        // Debug formatting should prefix the output with "Network::"
        assert!(
            debug_str.starts_with("Network::"),
            "Debug format should start with 'Network::'"
        );
    }

    #[test]
    fn test_display() {
        let input = "Public";
        match input.parse::<StellarNetwork>() {
            Ok(network) => {
                // If parsing succeeds, test that the Display implementation outputs "Public"
                let display_str = format!("{}", network);
                assert_eq!(
                    display_str, "Public",
                    "Display output does not match the expected network name"
                );
            }
            Err(err) => {
                // If parsing fails, check that the error message is as expected.
                let error_message = format!("{}", err);
                assert_eq!(
                    error_message,
                    "Invalid network: Invalid network: Public, expected named network or chain ID",
                    "Error message did not match expected"
                );
            }
        }
    }

    #[test]
    fn test_serialization() {
        let input = "mainnet";
        let network = input.parse::<StellarNetwork>().unwrap();
        let serialized = serde_json::to_string(&network).unwrap();
        assert_eq!(serialized, "\"mainnet\"");
    }

    #[test]
    fn test_deserialization() {
        let json_str = "\"mainnet\"";
        let network: StellarNetwork =
            serde_json::from_str(json_str).expect("Deserialization should succeed");
        assert_eq!(
            network.to_string(),
            "mainnet",
            "Deserialized network does not match expected network name"
        );
    }

    #[test]
    fn test_from_network_str() {
        // Test a valid network string.
        let network = StellarNetwork::from_network_str("mainnet");
        assert!(
            network.is_ok(),
            "from_network_str should succeed for a valid network"
        );
        let network = network.unwrap();
        assert_eq!(
            network.to_string(),
            "mainnet",
            "Network from from_network_str does not match expected output"
        );

        // Test an invalid network string.
        let invalid_network = StellarNetwork::from_network_str("UnknownNetwork");
        assert!(
            invalid_network.is_err(),
            "from_network_str should fail for an invalid network"
        );
    }

    #[test]
    fn test_helper_methods() {
        let network = "mainnet".parse::<StellarNetwork>().unwrap();

        // Test average_blocktime returns an Option<Duration>
        let blocktime = network.average_blocktime();
        // We do not assume a concrete value; just that it returns Some(_) or None.
        if let Some(duration) = blocktime {
            assert!(
                duration > Duration::from_secs(0),
                "Blocktime should be a positive duration"
            )
        }

        // Test public RPC URLs.
        let rpc_urls = network.public_rpc_urls();
        // They should be a slice of string literals.
        assert!(!rpc_urls.is_empty(), "Expected at least one public RPC URL");

        // Test explorer URLs.
        let explorer_urls = network.explorer_urls();
        assert!(
            !explorer_urls.is_empty(),
            "Expected at least one explorer URL"
        );

        // Test the is_testnet flag returns a boolean.
        let _is_testnet = network.is_testnet();
    }
}
