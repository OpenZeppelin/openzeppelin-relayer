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
}
