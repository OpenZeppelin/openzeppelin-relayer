//! DER (Distinguished Encoding Rules) operations for cryptographic keys.
//!
//! This module provides utilities for parsing and extracting information from
//! DER-encoded cryptographic keys, particularly for ECDSA operations.

use k256::pkcs8::DecodePublicKey;

#[derive(Debug, thiserror::Error)]
pub enum DerError {
    #[error("Parse Error: {0}")]
    ParseError(String),
}

/// Extract raw 64-byte key from DER encoded key.
pub fn extract_public_key_from_der(der: &[u8]) -> Result<[u8; 64], DerError> {
    let pk = k256::ecdsa::VerifyingKey::from_public_key_der(der)
        .map_err(|e| DerError::ParseError(format!("ASN.1 parse error: {e}")))?
        .to_encoded_point(false)
        .as_bytes()
        .to_vec();
    let pub_key_no_prefix = &pk[1..];

    let mut array = [0u8; 64];
    array.copy_from_slice(pub_key_no_prefix);

    Ok(array)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SECP256K1_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEjJaJh5wfZwvj8b3bQ4GYikqDTLXWUjMh\nkFs9lGj2N9B17zo37p4PSy99rDio0QHLadpso0rtTJDSISRW9MdOqA==\n-----END PUBLIC KEY-----\n"; // noboost

    #[test]
    fn test_extract_public_key_from_der_with_invalid_data() {
        let invalid_der = &[1, 2, 3];
        let result = extract_public_key_from_der(invalid_der);
        assert!(result.is_err());

        assert!(matches!(result, Err(DerError::ParseError(_))));
    }

    #[test]
    fn test_extract_public_key_from_der_with_valid_secp256k1() {
        let pem = pem::parse(VALID_SECP256K1_PEM).unwrap();
        let der = pem.contents();
        let result = extract_public_key_from_der(der);

        assert!(result.is_ok());
        let public_key = result.unwrap();

        // Verify the public key is 64 bytes
        assert_eq!(public_key.len(), 64);

        // Verify the expected public key value
        assert_eq!(
            format!("0x{}", hex::encode(public_key)),
            "0x8c9689879c1f670be3f1bddb4381988a4a834cb5d6523321905b3d9468f637d075ef3a37ee9e0f4b2f7dac38a8d101cb69da6ca34aed4c90d2212456f4c74ea8"
        );
    }
}
