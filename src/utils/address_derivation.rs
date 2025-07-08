//! Derivation of blockchain addresses from cryptographic keys.
//!
//! This module provides utilities for deriving blockchain addresses from cryptographic
//! public keys in various formats (DER, PEM). It supports multiple blockchain networks
//! including Ethereum, Solana, and potentially others.

use super::der::extract_public_key_from_der;

#[derive(Debug, thiserror::Error)]
pub enum AddressDerivationError {
    #[error("Parse Error: {0}")]
    ParseError(String),
}

/// Derive EVM address from the DER payload.
pub fn derive_ethereum_address_from_der(der: &[u8]) -> Result<[u8; 20], AddressDerivationError> {
    let pub_key = extract_public_key_from_der(der)
        .map_err(|e| AddressDerivationError::ParseError(e.to_string()))?;

    let hash = alloy::primitives::keccak256(pub_key);

    // Take the last 20 bytes of the hash
    let address_bytes = &hash[hash.len() - 20..];

    let mut array = [0u8; 20];
    array.copy_from_slice(address_bytes);

    Ok(array)
}

/// Derive EVM address from the PEM string.
pub fn derive_ethereum_address_from_pem(pem_str: &str) -> Result<[u8; 20], AddressDerivationError> {
    let pkey =
        pem::parse(pem_str).map_err(|e| AddressDerivationError::ParseError(e.to_string()))?;
    let der = pkey.contents();
    derive_ethereum_address_from_der(der)
}

/// Derive Solana address from a PEM-encoded public key.
pub fn derive_solana_address_from_pem(pem_str: &str) -> Result<String, AddressDerivationError> {
    let pkey =
        pem::parse(pem_str).map_err(|e| AddressDerivationError::ParseError(e.to_string()))?;
    let content = pkey.contents();

    let mut array = [0u8; 32];

    match content.len() {
        32 => array.copy_from_slice(content),
        44 => array.copy_from_slice(&content[12..]),
        _ => {
            return Err(AddressDerivationError::ParseError(format!(
                "Unexpected ed25519 public key length: got {} bytes (expected 32 or 44).",
                content.len()
            )));
        }
    }

    let solana_address = bs58::encode(array).into_string();
    Ok(solana_address)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SECP256K1_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEjJaJh5wfZwvj8b3bQ4GYikqDTLXWUjMh\nkFs9lGj2N9B17zo37p4PSy99rDio0QHLadpso0rtTJDSISRW9MdOqA==\n-----END PUBLIC KEY-----\n"; // noboost

    const VALID_ED25519_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAnUV+ReQWxMZ3Z2pC/5aOPPjcc8jzOo0ZgSl7+j4AMLo=\n-----END PUBLIC KEY-----\n";

    #[test]
    fn test_derive_ethereum_address_from_pem_with_invalid_data() {
        let invalid_pem = "not-a-valid-pem";
        let result = derive_ethereum_address_from_pem(invalid_pem);
        assert!(result.is_err());

        // Verify it returns the expected error type
        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    #[test]
    fn test_derive_ethereum_address_from_pem_with_valid_secp256k1() {
        let result = derive_ethereum_address_from_pem(VALID_SECP256K1_PEM);
        assert!(result.is_ok());

        let address = result.unwrap();
        assert_eq!(address.len(), 20); // Ethereum addresses are 20 bytes

        assert_eq!(
            format!("0x{}", hex::encode(address)),
            "0xeeb8861f51b3f3f2204d64bbf7a7eb25e1b4d6cd"
        );
    }

    #[test]
    fn test_derive_ethereum_address_from_der_with_invalid_data() {
        let invalid_der = &[1, 2, 3];
        let result = derive_ethereum_address_from_der(invalid_der);
        assert!(result.is_err());

        // Verify it returns the expected error type
        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    #[test]
    fn test_derive_ethereum_address_from_der_with_valid_secp256k1() {
        let pem = pem::parse(VALID_SECP256K1_PEM).unwrap();
        let der = pem.contents();
        let result = derive_ethereum_address_from_der(der);

        assert!(result.is_ok());

        let address = result.unwrap();
        assert_eq!(address.len(), 20); // Ethereum addresses are 20 bytes

        assert_eq!(
            format!("0x{}", hex::encode(address)),
            "0xeeb8861f51b3f3f2204d64bbf7a7eb25e1b4d6cd"
        );
    }

    #[test]
    fn test_derive_solana_address_from_pem_with_invalid_data() {
        let invalid_pem = "not-a-valid-pem";
        let result = derive_solana_address_from_pem(invalid_pem);
        assert!(result.is_err());

        // Verify it returns the expected error type
        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    #[test]
    fn test_derive_solana_address_from_pem_with_valid_ed25519() {
        let result = derive_solana_address_from_pem(VALID_ED25519_PEM);
        assert!(result.is_ok());

        let address = result.unwrap();
        // Solana addresses are base58 encoded, should be around 32-44 characters
        assert!(!address.is_empty());
        assert!(address.len() >= 32 && address.len() <= 44);

        assert_eq!(address, "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5");
    }

    #[test]
    fn test_derive_solana_address_from_pem_with_invalid_key_length() {
        // Create a PEM with invalid ed25519 key length
        let invalid_ed25519_der = vec![0u8; 10]; // Too short
        let invalid_pem = pem::Pem::new("PUBLIC KEY", invalid_ed25519_der);
        let invalid_pem_str = pem::encode(&invalid_pem);

        let result = derive_solana_address_from_pem(&invalid_pem_str);
        assert!(result.is_err());

        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }
}
