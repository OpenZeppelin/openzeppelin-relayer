use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::Serialize;
use sha3::{Digest, Keccak256};

#[derive(Debug, Clone, thiserror::Error, Serialize)]
pub enum Secp256k1Error {
    #[error("Secp256k1 recovery error: {0}")]
    RecoveryError(String),
}

/// Recover `v` point from a signature and from the message contents
pub fn recover_public_key(pk: &[u8], sig: &Signature, bytes: &[u8]) -> Result<u8, Secp256k1Error> {
    let mut hasher = Keccak256::new();
    hasher.update(bytes);
    for v in 0..2 {
        let rec_id = match RecoveryId::try_from(v) {
            Ok(id) => id,
            Err(_) => continue,
        };

        let recovered_key = match VerifyingKey::recover_from_digest(hasher.clone(), sig, rec_id) {
            Ok(key) => key.to_encoded_point(false).as_bytes().to_vec(),
            Err(_) => {
                continue;
            }
        };
        if recovered_key[1..] == pk[..] {
            return Ok(v);
        }
    }

    Err(Secp256k1Error::RecoveryError(
        "No valid v point was found".to_string(),
    ))
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
        let public_key_bytes = &verifying_key.to_encoded_point(false).as_bytes().to_vec()[1..];
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
}
