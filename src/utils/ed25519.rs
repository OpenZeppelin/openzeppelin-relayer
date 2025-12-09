//! Ed25519 (EdDSA) operations for cryptographic keys.
//!
//! This module provides utilities for parsing and extracting information from
//! Ed25519 public keys in various formats (DER/SPKI, raw bytes).

use ed25519_dalek::{pkcs8::DecodePublicKey, VerifyingKey};

#[derive(Debug, thiserror::Error)]
pub enum Ed25519Error {
    #[error("Parse Error: {0}")]
    ParseError(String),
}

/// Extract raw 32-byte Ed25519 public key from DER/SPKI encoded format.
///
/// AWS KMS and other providers return Ed25519 public keys in SPKI format.
/// This function uses proper ASN.1 parsing via ed25519-dalek's pkcs8 support
/// to extract the raw 32-byte public key from the SPKI structure.
///
/// This function accepts:
/// - SPKI/DER encoded public keys (any valid length)
/// - 32 bytes: Raw public key (already extracted)
pub fn extract_ed25519_public_key_from_der(der: &[u8]) -> Result<[u8; 32], Ed25519Error> {
    // If already 32 bytes, assume it's a raw public key
    if der.len() == 32 {
        let mut array = [0u8; 32];
        array.copy_from_slice(der);
        return Ok(array);
    }

    // Otherwise, parse as SPKI/DER format using ed25519-dalek
    let verifying_key = VerifyingKey::from_public_key_der(der)
        .map_err(|e| Ed25519Error::ParseError(format!("ASN.1 parse error: {e}")))?;

    Ok(verifying_key.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Well-known test Ed25519 public key (32 bytes)
    const TEST_ED25519_PUBLIC_KEY: [u8; 32] = [
        0x9d, 0x45, 0x7e, 0x45, 0xe4, 0x16, 0xc4, 0xc6, 0x77, 0x67, 0x6a, 0x42, 0xff, 0x96, 0x8e,
        0x3c, 0xf8, 0xdc, 0x73, 0xc8, 0xf3, 0x3a, 0x8d, 0x19, 0x81, 0x29, 0x7b, 0xfa, 0x3e, 0x00,
        0x30, 0xba,
    ];

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

    #[test]
    fn test_extract_ed25519_public_key_from_der_spki_format() {
        let spki_der = create_ed25519_spki_der(&TEST_ED25519_PUBLIC_KEY);
        assert_eq!(spki_der.len(), 44);

        let result = extract_ed25519_public_key_from_der(&spki_der);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TEST_ED25519_PUBLIC_KEY);
    }

    #[test]
    fn test_extract_ed25519_public_key_from_der_raw_format() {
        let result = extract_ed25519_public_key_from_der(&TEST_ED25519_PUBLIC_KEY);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TEST_ED25519_PUBLIC_KEY);
    }

    #[test]
    fn test_extract_ed25519_public_key_from_der_invalid_data() {
        let invalid_der = vec![0u8; 10];
        let result = extract_ed25519_public_key_from_der(&invalid_der);
        assert!(result.is_err());
        assert!(matches!(result, Err(Ed25519Error::ParseError(_))));
    }

    #[test]
    fn test_extract_ed25519_public_key_preserves_key_bytes() {
        // Verify the exact bytes are preserved
        let spki_der = create_ed25519_spki_der(&TEST_ED25519_PUBLIC_KEY);
        let extracted = extract_ed25519_public_key_from_der(&spki_der).unwrap();

        for (i, (a, b)) in extracted
            .iter()
            .zip(TEST_ED25519_PUBLIC_KEY.iter())
            .enumerate()
        {
            assert_eq!(a, b, "Byte mismatch at position {}", i);
        }
    }
}
