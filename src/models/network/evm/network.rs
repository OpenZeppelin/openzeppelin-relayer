use crate::models::{error::NetworkError, EvmNamedNetwork};
use core::{cmp::Ordering, fmt, str::FromStr, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvmNetwork(EvmNetworkKind);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EvmNetworkKind {
    Named(EvmNamedNetwork),
    Id(u64),
}

impl fmt::Debug for EvmNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Network::")?;
        self.kind().fmt(f)
    }
}

impl Default for EvmNetwork {
    fn default() -> Self {
        Self::from_named(EvmNamedNetwork::default())
    }
}

impl From<EvmNamedNetwork> for EvmNetwork {
    fn from(id: EvmNamedNetwork) -> Self {
        Self::from_named(id)
    }
}

impl From<u64> for EvmNetwork {
    fn from(id: u64) -> Self {
        Self::from_id(id)
    }
}

impl From<EvmNetwork> for u64 {
    fn from(chain: EvmNetwork) -> Self {
        chain.id()
    }
}

impl TryFrom<EvmNetwork> for EvmNamedNetwork {
    type Error = <EvmNamedNetwork as TryFrom<u64>>::Error;

    #[inline]
    fn try_from(chain: EvmNetwork) -> Result<Self, Self::Error> {
        match *chain.kind() {
            EvmNetworkKind::Named(chain) => Ok(chain),
            EvmNetworkKind::Id(id) => id.try_into(),
        }
    }
}

impl FromStr for EvmNetwork {
    type Err = core::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(chain) = EvmNamedNetwork::from_str(s) {
            Ok(Self::from_named(chain))
        } else {
            s.parse::<u64>().map(Self::from_id)
        }
    }
}

impl fmt::Display for EvmNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            EvmNetworkKind::Named(network) => network.fmt(f),
            EvmNetworkKind::Id(id) => id.fmt(f),
        }
    }
}

impl PartialEq<u64> for EvmNetwork {
    fn eq(&self, other: &u64) -> bool {
        self.id().eq(other)
    }
}

impl PartialOrd<u64> for EvmNetwork {
    fn partial_cmp(&self, other: &u64) -> Option<Ordering> {
        self.id().partial_cmp(other)
    }
}

impl serde::Serialize for EvmNetwork {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.kind() {
            EvmNetworkKind::Named(network) => network.serialize(serializer),
            EvmNetworkKind::Id(id) => id.serialize(serializer),
        }
    }
}

impl<'de> serde::Deserialize<'de> for EvmNetwork {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NetworkVisitor;

        impl serde::de::Visitor<'_> for NetworkVisitor {
            type Value = EvmNetwork;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("chain name or ID")
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                if v.is_negative() {
                    Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Signed(v),
                        &self,
                    ))
                } else {
                    Ok(EvmNetwork::from_id(v as u64))
                }
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(EvmNetwork::from_id(value))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(NetworkVisitor)
    }
}

impl EvmNetwork {
    #[allow(non_snake_case)]
    pub const fn Named(named: EvmNamedNetwork) -> Self {
        Self::from_named(named)
    }

    #[allow(non_snake_case)]
    pub const fn Id(id: u64) -> Self {
        Self::from_id_unchecked(id)
    }

    pub const fn from_named(named: EvmNamedNetwork) -> Self {
        Self(EvmNetworkKind::Named(named))
    }

    pub fn from_id(id: u64) -> Self {
        if let Ok(named) = EvmNamedNetwork::try_from(id) {
            Self::from_named(named)
        } else {
            Self::from_id_unchecked(id)
        }
    }

    pub const fn from_id_unchecked(id: u64) -> Self {
        Self(EvmNetworkKind::Id(id))
    }

    pub fn from_network_str(network: &str) -> Result<Self, NetworkError> {
        // Try parsing as named network first
        if let Ok(named) = EvmNamedNetwork::from_str(network) {
            Ok(Self::from_named(named))
        } else {
            Err(NetworkError::InvalidNetwork(format!(
                "Invalid network: {}, expected named network or chain ID",
                network
            )))
        }
    }

    pub fn get_rpc_url(&self, custom_rpc_urls: Option<Vec<String>>) -> Option<String> {
        custom_rpc_urls
            .as_ref()
            .and_then(|urls| urls.first().cloned())
            .or_else(|| {
                self.public_rpc_urls()
                    .and_then(|urls| urls.first().cloned())
                    .map(String::from)
            })
    }

    pub const fn kind(&self) -> &EvmNetworkKind {
        &self.0
    }

    pub const fn into_kind(self) -> EvmNetworkKind {
        self.0
    }

    pub const fn is_ethereum(&self) -> bool {
        matches!(self.named(), Some(named) if named.is_ethereum())
    }

    pub const fn is_optimism(self) -> bool {
        matches!(self.named(), Some(named) if named.is_optimism())
    }

    pub const fn is_arbitrum(self) -> bool {
        matches!(self.named(), Some(named) if named.is_arbitrum())
    }

    pub const fn is_rollup(self) -> bool {
        matches!(self.named(), Some(named) if named.is_rollup())
    }

    pub const fn is_testnet(self) -> bool {
        matches!(self.named(), Some(named) if named.is_testnet())
    }

    pub const fn named(self) -> Option<EvmNamedNetwork> {
        match *self.kind() {
            EvmNetworkKind::Named(named) => Some(named),
            EvmNetworkKind::Id(_) => None,
        }
    }

    /// Returns the recommended number of confirmations needed for this network.
    pub const fn required_confirmations(self) -> u64 {
        match self.named() {
            Some(named) => named.required_confirmations(),
            None => 1, // Default for unknown networks
        }
    }

    pub const fn id(self) -> u64 {
        match *self.kind() {
            EvmNetworkKind::Named(named) => named as u64,
            EvmNetworkKind::Id(id) => id,
        }
    }

    pub const fn average_blocktime(self) -> Option<Duration> {
        match self.kind() {
            EvmNetworkKind::Named(named) => named.average_blocktime(),
            EvmNetworkKind::Id(_) => None,
        }
    }

    pub const fn is_legacy(self) -> bool {
        match self.kind() {
            EvmNetworkKind::Named(named) => named.is_legacy(),
            EvmNetworkKind::Id(_) => false,
        }
    }

    pub const fn explorer_urls(self) -> Option<&'static [&'static str]> {
        match self.kind() {
            EvmNetworkKind::Named(named) => named.explorer_urls(),
            EvmNetworkKind::Id(_) => None,
        }
    }

    pub const fn public_rpc_urls(self) -> Option<&'static [&'static str]> {
        match self.kind() {
            EvmNetworkKind::Named(named) => named.public_rpc_urls(),
            EvmNetworkKind::Id(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unused_imports)]
    #[test]
    fn test_id() {
        assert_eq!(EvmNetwork::from_id(1234).id(), 1234);
        assert!(EvmNetwork::from_id(1).is_ethereum());
    }

    #[test]
    fn test_named_id() {
        assert_eq!(
            EvmNetwork::from_named(EvmNamedNetwork::Sepolia).id(),
            11155111
        );
    }

    #[test]
    fn test_display_named_chain() {
        assert_eq!(
            EvmNetwork::from_named(EvmNamedNetwork::Mainnet).to_string(),
            "mainnet"
        );
    }

    #[test]
    fn test_display_id_chain() {
        assert_eq!(EvmNetwork::from_id(1234).to_string(), "1234");
    }

    #[test]
    fn test_from_str_named_chain() {
        let result = EvmNetwork::from_str("mainnet");
        let expected = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_from_str_named_chain_error() {
        let result = EvmNetwork::from_str("chain");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_str_id_chain() {
        let result = EvmNetwork::from_str("1234");
        let expected = EvmNetwork::from_id(1234);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    fn test_default() {
        let default = EvmNetwork::default();
        let expected = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert_eq!(default, expected);
    }

    #[test]
    fn test_serde() {
        let re = r#"["arbitrum","amoy","gnosis"]"#;
        let expected = [
            EvmNetwork::from_named(EvmNamedNetwork::Arbitrum),
            EvmNetwork::from_id(80002),
            EvmNetwork::from_named(EvmNamedNetwork::Gnosis),
        ];
        assert_eq!(serde_json::to_string(&expected).unwrap(), re);
    }

    #[test]
    fn test_from_network_str_mainnet() {
        let network = EvmNetwork::from_network_str("mainnet");
        assert!(matches!(
            network,
            Ok(EvmNetwork(EvmNetworkKind::Named(EvmNamedNetwork::Mainnet)))
        ));
    }

    #[test]
    #[should_panic(expected = "Invalid network")]
    fn test_from_network_str_invalid() {
        EvmNetwork::from_network_str("invalid-network").unwrap();
    }

    #[test]
    fn test_eq_with_u64() {
        let network = EvmNetwork::from_id(10);
        assert_eq!(network, 10u64);
        assert_ne!(network, 11u64);
    }

    #[test]
    fn test_partial_cmp_with_u64() {
        let network = EvmNetwork::from_id(10);
        assert!(network < 20u64);
        assert!(network > 5u64);
        assert_eq!(network.partial_cmp(&10u64), Some(std::cmp::Ordering::Equal));
    }

    #[test]
    fn test_is_ethereum() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert!(network.is_ethereum());
        assert!(!network.is_optimism());
        assert!(!network.is_arbitrum());
        assert!(!network.is_testnet());
    }

    #[test]
    fn test_is_arbitrum() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Arbitrum);
        assert!(network.is_arbitrum());
        assert!(!network.is_optimism());
        assert!(!network.is_ethereum());
    }

    #[test]
    fn test_is_optimism() {
        let network = EvmNetwork::from_named(EvmNamedNetwork::Optimism);
        assert!(network.is_optimism());
        assert!(!network.is_arbitrum());
        assert!(!network.is_ethereum());
    }

    #[test]
    fn test_is_testnet() {
        let sepolia = EvmNetwork::from_named(EvmNamedNetwork::Sepolia);
        assert!(sepolia.is_testnet());
        let mainnet = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert!(!mainnet.is_testnet());
    }

    #[test]
    fn test_average_blocktime_known() {
        let mainnet = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert!(mainnet.average_blocktime().is_some());
    }

    #[test]
    fn test_average_blocktime_unknown() {
        let custom = EvmNetwork::from_id(1234567);
        assert!(custom.average_blocktime().is_none());
    }

    #[test]
    fn test_is_legacy() {
        let mainnet = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert!(!mainnet.is_legacy());
        let custom = EvmNetwork::from_id(1234);
        assert!(!custom.is_legacy());
    }

    #[test]
    fn test_explorer_urls() {
        let mainnet = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert!(mainnet.explorer_urls().is_some());
        let custom = EvmNetwork::from_id(9999);
        assert!(custom.explorer_urls().is_none());
    }

    #[test]
    fn test_public_rpc_urls() {
        let mainnet = EvmNetwork::from_named(EvmNamedNetwork::Mainnet);
        assert!(mainnet.public_rpc_urls().is_some());
        let custom = EvmNetwork::from_id(9999);
        assert!(custom.public_rpc_urls().is_none());
    }
}
