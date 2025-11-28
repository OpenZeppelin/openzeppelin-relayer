//! Derivation of blockchain addresses from cryptographic keys.
//!
//! This module provides utilities for deriving blockchain addresses from cryptographic
//! public keys in various formats (DER, PEM). It supports multiple blockchain networks
//! including Ethereum, Solana, and potentially others.

use super::der::extract_public_key_from_der;
use super::ed25519::extract_ed25519_public_key_from_der;

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
    derive_solana_address_from_der(content)
}

/// Derive Stellar address from a PEM-encoded public key.
/// Stellar uses Ed25519 keys and addresses are encoded with StrKey format (G prefix for accounts).
pub fn derive_stellar_address_from_pem(pem_str: &str) -> Result<String, AddressDerivationError> {
    let pkey =
        pem::parse(pem_str).map_err(|e| AddressDerivationError::ParseError(e.to_string()))?;
    let content = pkey.contents();
    derive_stellar_address_from_der(content)
}

/// Derive Solana address from DER/SPKI encoded Ed25519 public key.
/// Solana addresses are base58-encoded Ed25519 public keys.
///
/// Accepts:
/// - 44 bytes: Full SPKI format (12-byte header + 32-byte key)
/// - 32 bytes: Raw public key
pub fn derive_solana_address_from_der(der: &[u8]) -> Result<String, AddressDerivationError> {
    let pubkey = extract_ed25519_public_key_from_der(der)
        .map_err(|e| AddressDerivationError::ParseError(e.to_string()))?;
    Ok(bs58::encode(pubkey).into_string())
}

/// Derive Stellar address from DER/SPKI encoded Ed25519 public key.
/// Stellar addresses use StrKey encoding (G prefix for public accounts).
///
/// Accepts:
/// - 44 bytes: Full SPKI format (12-byte header + 32-byte key)
/// - 32 bytes: Raw public key
pub fn derive_stellar_address_from_der(der: &[u8]) -> Result<String, AddressDerivationError> {
    let pubkey = extract_ed25519_public_key_from_der(der)
        .map_err(|e| AddressDerivationError::ParseError(e.to_string()))?;

    use stellar_strkey::ed25519::PublicKey;
    Ok(PublicKey(pubkey).to_string())
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

    #[test]
    fn test_derive_stellar_address_from_pem_with_valid_ed25519() {
        let result = derive_stellar_address_from_pem(VALID_ED25519_PEM);
        assert!(result.is_ok());

        let address = result.unwrap();
        // Stellar addresses start with 'G' for accounts
        assert!(address.starts_with('G'));
        // Stellar addresses are base32 encoded and 56 characters long
        assert_eq!(address.len(), 56);
    }

    #[test]
    fn test_derive_stellar_address_from_pem_with_invalid_data() {
        let invalid_pem = "not-a-valid-pem";
        let result = derive_stellar_address_from_pem(invalid_pem);
        assert!(result.is_err());

        // Verify it returns the expected error type
        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    #[test]
    fn test_derive_stellar_address_from_pem_with_invalid_key_length() {
        // Create a PEM with invalid ed25519 key length
        let invalid_ed25519_der = vec![0u8; 10]; // Too short
        let invalid_pem = pem::Pem::new("PUBLIC KEY", invalid_ed25519_der);
        let invalid_pem_str = pem::encode(&invalid_pem);

        let result = derive_stellar_address_from_pem(&invalid_pem_str);
        assert!(result.is_err());

        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    // Tests for DER-based derivation functions
    fn create_ed25519_spki_der(public_key: &[u8; 32]) -> Vec<u8> {
        let mut der = vec![
            0x30, 0x2a, // SEQUENCE, 42 bytes
            0x30, 0x05, // SEQUENCE, 5 bytes
            0x06, 0x03, 0x2b, 0x65, 0x70, // OID 1.3.101.112 (Ed25519)
            0x03, 0x21, // BIT STRING, 33 bytes
            0x00, // zero unused bits
        ];
        der.extend_from_slice(public_key);
        der
    }

    // Test public key matching VALID_ED25519_PEM
    const TEST_ED25519_PUBLIC_KEY: [u8; 32] = [
        0x9d, 0x45, 0x7e, 0x45, 0xe4, 0x16, 0xc4, 0xc6, 0x77, 0x67, 0x6a, 0x42, 0xff, 0x96, 0x8e,
        0x3c, 0xf8, 0xdc, 0x73, 0xc8, 0xf3, 0x3a, 0x8d, 0x19, 0x81, 0x29, 0x7b, 0xfa, 0x3e, 0x00,
        0x30, 0xba,
    ];

    #[test]
    fn test_derive_solana_address_from_der_with_spki_format() {
        let spki_der = create_ed25519_spki_der(&TEST_ED25519_PUBLIC_KEY);
        let result = derive_solana_address_from_der(&spki_der);
        assert!(result.is_ok());

        let address = result.unwrap();
        assert_eq!(address, "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5");
    }

    #[test]
    fn test_derive_solana_address_from_der_with_raw_key() {
        let result = derive_solana_address_from_der(&TEST_ED25519_PUBLIC_KEY);
        assert!(result.is_ok());

        let address = result.unwrap();
        assert_eq!(address, "BavUBpkD77FABnevMkBVqV8BDHv7gX8sSoYYJY9WU9L5");
    }

    #[test]
    fn test_derive_solana_address_from_der_with_invalid_length() {
        let invalid_der = vec![0u8; 10];
        let result = derive_solana_address_from_der(&invalid_der);
        assert!(result.is_err());
        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    #[test]
    fn test_derive_stellar_address_from_der_with_spki_format() {
        let spki_der = create_ed25519_spki_der(&TEST_ED25519_PUBLIC_KEY);
        let result = derive_stellar_address_from_der(&spki_der);
        assert!(result.is_ok());

        let address = result.unwrap();
        assert!(address.starts_with('G'));
        assert_eq!(address.len(), 56);
    }

    #[test]
    fn test_derive_stellar_address_from_der_with_raw_key() {
        let result = derive_stellar_address_from_der(&TEST_ED25519_PUBLIC_KEY);
        assert!(result.is_ok());

        let address = result.unwrap();
        assert!(address.starts_with('G'));
        assert_eq!(address.len(), 56);
    }

    #[test]
    fn test_derive_stellar_address_from_der_with_invalid_length() {
        let invalid_der = vec![0u8; 50];
        let result = derive_stellar_address_from_der(&invalid_der);
        assert!(result.is_err());
        assert!(matches!(result, Err(AddressDerivationError::ParseError(_))));
    }

    #[test]
    fn test_der_and_pem_produce_same_addresses() {
        // Verify that DER and PEM functions produce the same results
        let pem = pem::parse(VALID_ED25519_PEM).unwrap();
        let der = pem.contents();

        let solana_from_pem = derive_solana_address_from_pem(VALID_ED25519_PEM).unwrap();
        let solana_from_der = derive_solana_address_from_der(der).unwrap();
        assert_eq!(solana_from_pem, solana_from_der);

        let stellar_from_pem = derive_stellar_address_from_pem(VALID_ED25519_PEM).unwrap();
        let stellar_from_der = derive_stellar_address_from_der(der).unwrap();
        assert_eq!(stellar_from_pem, stellar_from_der);
    }
}
