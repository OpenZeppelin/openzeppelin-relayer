//! Derivation of blockchain addresses.

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

/// Derive EVM address from the DER payload.
pub fn derive_ethereum_address_from_der(der: &[u8]) -> Result<[u8; 20], DerError> {
    let pub_key = extract_public_key_from_der(der)?;

    let hash = alloy::primitives::keccak256(pub_key);

    // Take the last 20 bytes of the hash
    let address_bytes = &hash[hash.len() - 20..];

    let mut array = [0u8; 20];
    array.copy_from_slice(address_bytes);

    Ok(array)
}

/// Derive EVM address from the PEM string.
pub fn derive_ethereum_address_from_pem(pem_str: &str) -> Result<[u8; 20], DerError> {
    let pkey = pem::parse(pem_str).map_err(|e| DerError::ParseError(e.to_string()))?;
    let der = pkey.contents();
    derive_ethereum_address_from_der(der)
}

#[cfg(test)]
mod tests {
    use super::*;

    static VALID_SECP256K1_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMFYwEAYHKoZIzj0CAQYFK4EEAAoDQgAEjJaJh5wfZwvj8b3bQ4GYikqDTLXWUjMh
    kFs9lGj2N9B17zo37p4PSy99rDio0QHLadpso0rtTJDSISRW9MdOqA==\n-----END PUBLIC KEY-----\n";

    #[test]
    fn test_derive_ethereum_address_from_pem() {
        let pem = "not-a-valid-pem";
        let result = derive_ethereum_address_from_pem(pem);
        assert!(result.is_err());

        let result = derive_ethereum_address_from_pem(VALID_SECP256K1_PEM);
        assert!(result.is_ok());
        assert_eq!(
            format!("0x{}", hex::encode(result.unwrap())),
            "0xeeb8861f51b3f3f2204d64bbf7a7eb25e1b4d6cd"
        );
    }

    #[test]
    fn test_extract_public_key_from_der() {
        let der = &[1, 2, 3];
        let result = extract_public_key_from_der(der);
        assert!(result.is_err());

        let pem = pem::parse(VALID_SECP256K1_PEM).unwrap();
        let der = pem.contents();
        let result = extract_public_key_from_der(der);

        assert!(result.is_ok());
        assert_eq!(
            format!("0x{}", hex::encode(result.unwrap())),
            "0x8c9689879c1f670be3f1bddb4381988a4a834cb5d6523321905b3d9468f637d075ef3a37ee9e0f4b2f7dac38a8d101cb69da6ca34aed4c90d2212456f4c74ea8"
        );
    }

    #[test]
    fn test_derive_ethereum_address_from_der() {
        let der = &[1, 2, 3];
        let result = derive_ethereum_address_from_der(der);
        assert!(result.is_err());

        let pem = pem::parse(VALID_SECP256K1_PEM).unwrap();
        let der = pem.contents();
        let result = derive_ethereum_address_from_der(der);

        assert!(result.is_ok());
        assert_eq!(
            format!("0x{}", hex::encode(result.unwrap())),
            "0xeeb8861f51b3f3f2204d64bbf7a7eb25e1b4d6cd"
        );
    }
}
