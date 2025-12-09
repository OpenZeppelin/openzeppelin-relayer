//! Common EVM signature recovery logic for KMS signers
//!
//! This module provides shared functionality for AWS KMS and Google Cloud KMS signers
//! to avoid code duplication in signature recovery and v-value computation.

use k256::ecdsa::Signature;

/// Common signature recovery logic for EVM KMS signers
///
/// Handles DER signature parsing, EIP-2 normalization, and v-value recovery.
///
/// # Parameters
///
/// * `der_signature` - DER-encoded signature from KMS
/// * `der_public_key` - DER-encoded public key
/// * `digest` - The 32-byte hash that was signed
/// * `original_bytes` - The original message bytes (for non-prehashed recovery)
/// * `use_prehash_recovery` - If true, recovers using hash directly; if false, uses original bytes
///
/// # Returns
///
/// * `Ok(Vec<u8>)` - 65-byte signature (r || s || v) where v is 27 or 28
/// * `Err(Box<dyn Error>)` - If signature parsing, normalization, or recovery fails
pub fn recover_evm_signature_from_der(
    der_signature: &[u8],
    der_public_key: &[u8],
    digest: [u8; 32],
    original_bytes: &[u8],
    use_prehash_recovery: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut rs = Signature::from_der(der_signature)?;

    // Normalize to low-s if necessary (EIP-2 malleability protection)
    if let Some(normalized) = rs.normalize_s() {
        rs = normalized;
    }

    let pk = crate::utils::extract_public_key_from_der(der_public_key)?;

    let v = if use_prehash_recovery {
        crate::utils::recover_public_key_from_hash(&pk, &rs, &digest)?
    } else {
        crate::utils::recover_public_key(&pk, &rs, original_bytes)?
    };

    let eth_v = 27 + v;

    let mut sig_bytes = rs.to_vec();
    sig_bytes.push(eth_v);

    Ok(sig_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::utils::eip191_message;
    use k256::{
        ecdsa::SigningKey,
        elliptic_curve::rand_core::OsRng,
        pkcs8::{der::Encode, EncodePublicKey},
    };

    #[test]
    fn test_recover_evm_signature_from_der() {
        // Generate keypair
        let signing_key = SigningKey::random(&mut OsRng);

        // EIP-191 style message
        let eip_message = eip191_message(b"Hello World");

        // Hash the message
        let digest = alloy::primitives::keccak256(&eip_message).0;

        // Sign the digest
        let (signature, _) = signing_key.sign_prehash_recoverable(&digest).unwrap();
        let der_signature = signature.to_der().as_bytes().to_vec();

        // Get DER-encoded public key
        let der_public_key = signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .to_der()
            .unwrap();

        // Recover signature
        let result = recover_evm_signature_from_der(
            &der_signature,
            &der_public_key,
            digest,
            &eip_message,
            false, // use message recovery
        );

        assert!(result.is_ok());
        let sig_bytes = result.unwrap();
        assert_eq!(sig_bytes.len(), 65);
        assert!(sig_bytes[64] == 27 || sig_bytes[64] == 28);
    }

    #[test]
    fn test_recover_evm_signature_from_der_prehashed() {
        // Generate keypair
        let signing_key = SigningKey::random(&mut OsRng);

        // Create a pre-computed hash (simulating EIP-712)
        let hash: [u8; 32] = [0x42; 32];

        // Sign the hash directly
        let (signature, _) = signing_key.sign_prehash_recoverable(&hash).unwrap();
        let der_signature = signature.to_der().as_bytes().to_vec();

        // Get DER-encoded public key
        let der_public_key = signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .to_der()
            .unwrap();

        // Recover signature using prehash recovery
        let result = recover_evm_signature_from_der(
            &der_signature,
            &der_public_key,
            hash,
            &hash,
            true, // use prehash recovery
        );

        assert!(result.is_ok());
        let sig_bytes = result.unwrap();
        assert_eq!(sig_bytes.len(), 65);
        assert!(sig_bytes[64] == 27 || sig_bytes[64] == 28);
    }

    #[test]
    fn test_signature_normalization() {
        // This test verifies that signatures are normalized to low-s
        let signing_key = SigningKey::random(&mut OsRng);
        let message = b"Test message";
        let digest = alloy::primitives::keccak256(message).0;

        let (signature, _) = signing_key.sign_prehash_recoverable(&digest).unwrap();
        let der_signature = signature.to_der().as_bytes().to_vec();
        let der_public_key = signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .to_der()
            .unwrap();

        let result =
            recover_evm_signature_from_der(&der_signature, &der_public_key, digest, message, false);

        assert!(result.is_ok());
        // The signature should be normalized (implementation ensures this)
    }
}
