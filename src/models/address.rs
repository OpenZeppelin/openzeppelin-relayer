use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// Ethereum-like address (20 bytes)
    Evm([u8; 20]),
    /// Stellar address (Base32-encoded string)
    Stellar(String),
    /// Solana address (Base58-encoded string)
    Solana(String),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Evm(addr) => write!(f, "0x{}", hex::encode(addr)),
            Address::Stellar(addr) => write!(f, "{}", addr),
            Address::Solana(addr) => write!(f, "{}", addr),
        }
    }
}

impl Address {
    /// Validates an address based on the type
    pub fn validate(&self) -> bool {
        match self {
            Address::Evm(addr) => addr.len() == 20,
            Address::Stellar(addr) => {
                addr.len() <= 56 && addr.chars().all(|c| c.is_ascii_alphanumeric() || c == '=')
            }
            Address::Solana(addr) => {
                addr.len() <= 44 && addr.chars().all(|c| c.is_ascii_alphanumeric())
            }
        }
    }
}
