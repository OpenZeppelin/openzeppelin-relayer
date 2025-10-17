use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::Serialize;
use sha3::{Digest, Keccak256};

#[derive(Debug, Clone, thiserror::Error, Serialize)]
pub enum Secp256k1Error {
    #[error("Secp256k1 recovery error: {0}")]
    RecoveryError(String),
}

/// Internal helper for public key recovery that handles the common logic.
///
/// This function tries both recovery IDs (0 and 1) and returns the one that matches
/// the provided public key.
fn recover_v_internal<F>(pk: &[u8], _sig: &Signature, recover_fn: F) -> Result<u8, Secp256k1Error>
where
    F: Fn(u8, RecoveryId) -> Option<Vec<u8>>,
{
    for v in 0..2 {
        let rec_id = match RecoveryId::try_from(v) {
            Ok(id) => id,
            Err(_) => continue,
        };

        if let Some(recovered_key) = recover_fn(v, rec_id) {
            // Validate recovered key has expected format (65 bytes: 0x04 + 64 bytes)
            if recovered_key.len() != 65 || recovered_key[0] != 0x04 {
                continue;
            }

            // Skip first byte (0x04 uncompressed point marker) and compare 64-byte public key
            if recovered_key[1..] == pk[..] {
                return Ok(v);
            }
        }
    }

    Err(Secp256k1Error::RecoveryError(format!(
        "Failed to recover v value: no valid recovery ID found. \
         This usually indicates a signature/public key mismatch. \
         Public key length: {} bytes, tried recovery IDs: 0, 1",
        pk.len()
    )))
}

/// Recover `v` point from a signature and from the message contents
pub fn recover_public_key(pk: &[u8], sig: &Signature, bytes: &[u8]) -> Result<u8, Secp256k1Error> {
    // Validate public key length (64 bytes for uncompressed key without 0x04 prefix)
    if pk.len() != 64 {
        return Err(Secp256k1Error::RecoveryError(format!(
            "Invalid public key length: expected 64 bytes, got {}",
            pk.len()
        )));
    }

    let mut hasher = Keccak256::new();
    hasher.update(bytes);

    recover_v_internal(pk, sig, |_, rec_id| {
        VerifyingKey::recover_from_digest(hasher.clone(), sig, rec_id)
            .ok()
            .map(|key| key.to_encoded_point(false).as_bytes().to_vec())
    })
}

/// Recovers the v value from a signature for a pre-hashed message.
/// The hash parameter is already a 32-byte digest, not raw bytes.
pub fn recover_public_key_from_hash(
    pk: &[u8],
    sig: &Signature,
    hash: &[u8; 32],
) -> Result<u8, Secp256k1Error> {
    // Validate public key length (64 bytes for uncompressed key without 0x04 prefix)
    if pk.len() != 64 {
        return Err(Secp256k1Error::RecoveryError(format!(
            "Invalid public key length: expected 64 bytes, got {}",
            pk.len()
        )));
    }

    recover_v_internal(pk, sig, |_, rec_id| {
        VerifyingKey::recover_from_prehash(hash, sig, rec_id)
            .ok()
            .map(|key| key.to_encoded_point(false).as_bytes().to_vec())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloy::primitives::utils::eip191_message;
    use k256::{ecdsa::SigningKey, elliptic_curve::rand_core::OsRng};

    #[test]
    fn test_recover_public_key() {
        // Generate keypair
        let signing_key = SigningKey::random(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let public_key_vec = verifying_key.to_encoded_point(false).as_bytes().to_vec();
        let public_key_bytes = &public_key_vec[1..];
        println!("Pub key length: {}", public_key_bytes.len());

        // EIP-191 style of a message
        let eip_message = eip191_message(b"Hello World");

        // Ethereum-style hash: keccak256 of message
        let mut hasher = Keccak256::new();
        hasher.update(eip_message.clone());

        // Sign the message pre-hash
        let (signature, rec_id) = signing_key.sign_digest_recoverable(hasher).unwrap();

        // Try to recover the public key
        let recovery_result = recover_public_key(public_key_bytes, &signature, &eip_message);

        // Check that a valid recovery ID (0 or 1) is returned
        match recovery_result {
            Ok(v) => {
                assert!(v == 0 || v == 1, "Recovery ID should be 0 or 1, got {}", v);
                assert_eq!(rec_id.to_byte(), v, "Recovery ID should match")
            }
            Err(e) => panic!("Failed to recover public key: {:?}", e),
        }
    }

    #[test]
    fn test_recover_public_key_from_hash() {
        // Generate keypair
        let signing_key = SigningKey::random(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let public_key_vec = verifying_key.to_encoded_point(false).as_bytes().to_vec();
        let public_key_bytes = &public_key_vec[1..];

        // Create a pre-computed hash (simulating EIP-712)
        let message = b"Test message for EIP-712";
        let mut hasher = Keccak256::new();
        hasher.update(message);
        let hash: [u8; 32] = hasher.finalize().into();

        // Sign the hash directly (as KMS would do)
        let (signature, rec_id) = signing_key.sign_prehash_recoverable(&hash).unwrap();

        // Try to recover the public key from the hash
        let recovery_result = recover_public_key_from_hash(public_key_bytes, &signature, &hash);

        // Check that a valid recovery ID (0 or 1) is returned
        match recovery_result {
            Ok(v) => {
                assert!(v == 0 || v == 1, "Recovery ID should be 0 or 1, got {}", v);
                assert_eq!(rec_id.to_byte(), v, "Recovery ID should match")
            }
            Err(e) => panic!("Failed to recover public key from hash: {:?}", e),
        }
    }

    #[test]
    fn test_recover_public_key_from_hash_deterministic() {
        // Test that signing the same hash produces consistent v values
        let signing_key = SigningKey::random(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let public_key_vec = verifying_key.to_encoded_point(false).as_bytes().to_vec();
        let public_key_bytes = &public_key_vec[1..];

        // Create a fixed hash
        let hash: [u8; 32] = [0x42; 32];

        // Sign multiple times
        let (sig1, _) = signing_key.sign_prehash_recoverable(&hash).unwrap();
        let (sig2, _) = signing_key.sign_prehash_recoverable(&hash).unwrap();

        // Recover v values
        let v1 = recover_public_key_from_hash(public_key_bytes, &sig1, &hash).unwrap();
        let v2 = recover_public_key_from_hash(public_key_bytes, &sig2, &hash).unwrap();

        // Both should produce the same v value (deterministic signing with same key and hash)
        assert_eq!(v1, v2, "V values should be consistent for the same hash");
    }
}
