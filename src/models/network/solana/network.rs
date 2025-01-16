use crate::models::{error::NetworkError, SolanaNamedNetwork};
use core::{fmt, str::FromStr, time::Duration};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SolanaNetwork(SolanaNamedNetwork);

impl fmt::Debug for SolanaNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Network::")?;
        self.kind().fmt(f)
    }
}

impl Default for SolanaNetwork {
    fn default() -> Self {
        Self::from_named(SolanaNamedNetwork::default())
    }
}

impl From<SolanaNamedNetwork> for SolanaNetwork {
    fn from(id: SolanaNamedNetwork) -> Self {
        Self::from_named(id)
    }
}

impl FromStr for SolanaNetwork {
    type Err = NetworkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(network) = SolanaNamedNetwork::from_str(s) {
            Ok(Self::from_named(network))
        } else {
            Err(NetworkError::InvalidNetwork(format!(
                "Invalid network: {}, expected named network or chain ID",
                s
            )))
        }
    }
}

impl fmt::Display for SolanaNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for SolanaNetwork {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SolanaNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NetworkVisitor;

        impl serde::de::Visitor<'_> for NetworkVisitor {
            type Value = SolanaNetwork;

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
impl SolanaNetwork {
    pub const fn from_named(named: SolanaNamedNetwork) -> Self {
        Self(named)
    }

    pub const fn kind(&self) -> &SolanaNamedNetwork {
        &self.0
    }

    pub fn from_network_str(network: &str) -> Result<Self, NetworkError> {
        // Try parsing as named network first
        if let Ok(named) = SolanaNamedNetwork::from_str(network) {
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
}
