//! Midnight address handling module.
//!
//! This module provides functionality for encoding, decoding, and managing Midnight addresses.
//! It supports different network types and address formats.
//!
//! Majority of the code is directly copied from midnight-node repository (node-0.12.0):
//! <https://github.com/midnightntwrk/midnight-node/blob/29935d2f0ef1cc6cba8830f652e814b9668ab11d/util/toolkit/src/address.rs>.

use bech32::Bech32m;
use rand::Rng;
use thiserror::Error;

use midnight_node_ledger_helpers::{
    DefaultDB, NetworkId, Serializable, Wallet, WalletKind, WalletSeed, DB,
};

/// Errors that can occur during address operations
#[derive(Error, Debug)]
pub enum MidnightAddressError {
    #[error("prefix first part != 'mn'")]
    PrefixInvalidConstant,
    #[error("prefix missing type")]
    PrefixMissingType,
}

/// Represents a Midnight address with its type, network, and data
#[derive(Debug, Clone, PartialEq)]
pub struct MidnightAddress {
    /// The type of the address (e.g., "shield-addr")
    pub type_: String,
    /// The network identifier (e.g., "test", "dev", or None for mainnet)
    pub network: Option<String>,
    /// The raw address data
    pub data: Vec<u8>,
}

impl MidnightAddress {
    /// Decodes a bech32m-encoded Midnight address string into a MidnightAddress struct
    ///
    /// # Arguments
    /// * `encoded_data` - The bech32m-encoded address string
    ///
    /// # Returns
    /// * `Result<Self, MidnightAddressError>` - The decoded address or an error
    pub fn decode(encoded_data: &str) -> Result<Self, MidnightAddressError> {
        let (hrp, data) = bech32::decode(encoded_data).expect("Failed while bech32 decoding");
        let prefix_parts = hrp.as_str().split('_').collect::<Vec<&str>>();
        prefix_parts
            .first()
            .filter(|c| *c == &"mn")
            .ok_or(MidnightAddressError::PrefixInvalidConstant)?;
        let type_ = prefix_parts
            .get(1)
            .ok_or(MidnightAddressError::PrefixMissingType)?
            .to_string();
        let network = prefix_parts.get(2).map(|s| s.to_string());

        Ok(Self {
            type_,
            network,
            data,
        })
    }

    /// Encodes the MidnightAddress into a bech32m string
    ///
    /// # Returns
    /// * `String` - The bech32m-encoded address string
    pub fn encode(&self) -> String {
        let network_str = match &self.network {
            Some(network) => format!("_{network}"),
            None => "".to_string(),
        };

        bech32::encode::<Bech32m>(
            bech32::Hrp::parse(&format!("mn_{}{}", self.type_, network_str))
                .expect("Failed while bech32 parsing"),
            &self.data,
        )
        .expect("Failed while bech32 encoding")
    }

    /// Creates a MidnightAddress from a wallet and network ID
    ///
    /// # Arguments
    /// * `wallet` - The wallet to create the address from
    /// * `network` - The network ID to use
    ///
    /// # Returns
    /// * `Self` - The created MidnightAddress
    pub fn from_wallet<D: DB + Clone>(wallet: &Wallet<D>, network: NetworkId) -> Self {
        let network_str = match network {
            NetworkId::MainNet => None,
            NetworkId::DevNet => Some("dev".to_string()),
            NetworkId::TestNet => Some("test".to_string()),
            NetworkId::Undeployed => Some("undeployed".to_string()),
            _ => None,
        };

        let coin_pub_key = wallet.secret_keys.coin_public_key().0 .0;
        let mut enc_pub_key = Vec::new();
        Serializable::serialize(&wallet.secret_keys.enc_public_key(), &mut enc_pub_key)
            .expect("Failed serializing secret keys");

        Self {
            type_: "shield-addr".to_string(),
            network: network_str,
            data: [&coin_pub_key[..], &enc_pub_key[..]].concat(),
        }
    }

    /// Creates a MidnightAddress from a seed string and network ID
    ///
    /// # Arguments
    /// * `seed` - The hex-encoded seed string
    /// * `network` - The network ID to use
    ///
    /// # Returns
    /// * `Self` - The created MidnightAddress
    pub fn from_seed(seed: String, network: NetworkId) -> Self {
        let seed = hex::decode(seed).unwrap();
        let wallet: Wallet<DefaultDB> = Wallet::new(
            WalletSeed(seed.try_into().unwrap()),
            0,
            WalletKind::NoLegacy,
        );
        Self::from_wallet(&wallet, network)
    }

    /// Generates a random 32-byte seed and returns it as a hex string
    ///
    /// # Returns
    /// * `String` - A hex-encoded random seed
    pub fn generate_random_seed() -> String {
        let mut seed = [0u8; 32];
        rand::rng().fill(&mut seed);
        hex::encode(seed)
    }
}

impl TryFrom<&MidnightAddress> for NetworkId {
    type Error = String;

    /// Attempts to convert a MidnightAddress into a NetworkId
    ///
    /// # Arguments
    /// * `value` - The MidnightAddress to convert
    ///
    /// # Returns
    /// * `Result<Self, String>` - The NetworkId or an error string
    fn try_from(value: &MidnightAddress) -> Result<Self, Self::Error> {
        match value.network {
            Some(ref network) => match network.as_str() {
                "dev" => Ok(NetworkId::DevNet),
                "test" => Ok(NetworkId::TestNet),
                "undeployed" => Ok(NetworkId::Undeployed),
                _ => Err(network.to_string()),
            },
            None => Ok(NetworkId::MainNet),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bech32::{Bech32m, Hrp};

    #[test]
    fn test_parse() {
        let encoded_str = bech32::encode::<Bech32m>(
            Hrp::parse("mn_shield-addr_test").expect("Failed while bech32 parsing"),
            &[1, 2, 3],
        )
        .expect("Failed while bech32 encoding");
        let address =
            MidnightAddress::decode(&encoded_str).expect("Failed while decoding `MidnightAddress");
        assert_eq!(address.type_, "shield-addr".to_string());
        assert_eq!(address.network, Some("test".to_string()));
        assert_eq!(address.data, vec![1u8, 2u8, 3u8]);
    }

    #[test]
    fn test_from_seed() {
        let seed = "b49408db310c043ab736fb57a98e15c8cedbed4c38450df3755ac9726ee14d0c";
        let address = MidnightAddress::from_seed(seed.to_string(), NetworkId::TestNet);
        assert_eq!(address.type_, "shield-addr".to_string());
        assert_eq!(address.network, Some("test".to_string()));
        assert_eq!(address.encode(), "mn_shield-addr_test1quc5snkchyepu6rpn5sn85cmnjfk2kzynwtf3lapt9t8q0qlw97sxqypw479uxdvf48386urhyndrty9vmpkjlydmdcur78rr3lw345kg5r4fgc2");
        let decoded_address = MidnightAddress::decode(&address.encode()).unwrap();
        assert_eq!(decoded_address.type_, "shield-addr".to_string());
        assert_eq!(decoded_address.network, Some("test".to_string()));
        assert_eq!(decoded_address.data, address.data);
    }

    #[test]
    fn test_from_wallet() {
        let wallet = Wallet::<DefaultDB>::new(
            WalletSeed(
                hex::decode("b49408db310c043ab736fb57a98e15c8cedbed4c38450df3755ac9726ee14d0c")
                    .unwrap()
                    .try_into()
                    .unwrap(),
            ),
            0,
            WalletKind::NoLegacy,
        );
        let address = MidnightAddress::from_wallet(&wallet, NetworkId::TestNet);
        assert_eq!(address.type_, "shield-addr".to_string());
        assert_eq!(address.network, Some("test".to_string()));
        assert_eq!(address.encode(), "mn_shield-addr_test1quc5snkchyepu6rpn5sn85cmnjfk2kzynwtf3lapt9t8q0qlw97sxqypw479uxdvf48386urhyndrty9vmpkjlydmdcur78rr3lw345kg5r4fgc2");
    }

    #[test]
    fn test_encode_decode() {
        let address =
            MidnightAddress::from_seed(MidnightAddress::generate_random_seed(), NetworkId::TestNet);
        let encoded_address = address.encode();
        let decoded_address = MidnightAddress::decode(&encoded_address).unwrap();
        assert_eq!(address, decoded_address);
    }

    #[test]
    fn test_try_from() {
        let address =
            MidnightAddress::from_seed(MidnightAddress::generate_random_seed(), NetworkId::TestNet);
        let network_id = NetworkId::try_from(&address).unwrap();
        assert_eq!(network_id, NetworkId::TestNet);
    }

    #[test]
    fn test_random_seed() {
        let seed = MidnightAddress::generate_random_seed();
        assert_eq!(seed.len(), 64);
    }
}
