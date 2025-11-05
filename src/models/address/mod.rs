use std::fmt;

mod midnight;
pub use midnight::*;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Address {
    /// Ethereum-like address (20 bytes)
    Evm([u8; 20]),
    /// Stellar address (Base32-encoded string)
    Stellar(String),
    /// Solana address (Base58-encoded string)
    Solana(String),
    /// Midnight address (bech32m-encoded string with type and network)
    Midnight(String),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Evm(addr) => write!(f, "0x{}", hex::encode(addr)),
            Address::Stellar(addr) => write!(f, "{}", addr),
            Address::Solana(addr) => write!(f, "{}", addr),
            Address::Midnight(addr) => write!(f, "{}", addr),
        }
    }
}

impl Address {
    /// Validates an address based on the type
    #[allow(dead_code)]
    pub fn validate(&self) -> bool {
        match self {
            Address::Evm(addr) => addr.len() == 20,
            Address::Stellar(addr) => {
                addr.len() <= 56 && addr.chars().all(|c| c.is_ascii_alphanumeric() || c == '=')
            }
            Address::Solana(addr) => {
                addr.len() <= 44 && addr.chars().all(|c| c.is_ascii_alphanumeric())
            }
            Address::Midnight(addr) => {
                addr.len() <= 56 && addr.chars().all(|c| c.is_ascii_alphanumeric())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::address::midnight::MidnightAddress;
    use midnight_node_ledger_helpers::NetworkId;

    #[test]
    fn test_evm_address_display() {
        let address = Address::Evm([
            200, 52, 220, 220, 154, 7, 77, 187, 173, 204, 113, 88, 71, 137, 174, 75, 70, 61, 177,
            22,
        ]);
        assert_eq!(
            address.to_string(),
            "0xc834dcdc9a074dbbadcc71584789ae4b463db116"
        );
    }

    #[test]
    fn test_midnight_address_display() {
        let seed = "b49408db310c043ab736fb57a98e15c8cedbed4c38450df3755ac9726ee14d0c";
        let midnight_addr =
            MidnightAddress::from_seed(seed.to_string(), NetworkId::TestNet).encode();
        let address = Address::Midnight(midnight_addr);
        assert_eq!(
            address.to_string(),
            "mn_shield-addr_test1j9qa8ekx5n5xlfa77ev8jjgmdu0akarkd8wkuugxxss258yqkgfvmkv8nh7z53lkrct9mc5pkxwppamklqrg9xnsc53f092jpg9t4ycgc96m5"
        );
    }
}
