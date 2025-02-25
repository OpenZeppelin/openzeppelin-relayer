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
