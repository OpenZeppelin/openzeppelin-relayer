//! Encryption Key Generation Tool
//!
//! This tool generates a random 32-byte base64-encoded encryption key and prints it to the console.
//!
//! Other tools can be used to generate key like:
//!
//! ```bash
//! openssl rand -base64 32
//! ```
//!
//! # Usage
//!
//! ```bash
//! cargo run --example generate_encryption_key
//! ```
use eyre::Result;
use openzeppelin_relayer::utils::generate_encryption_key;

/// Main entry point for encryption key generation tool
fn main() -> Result<()> {
    let encryption_key = generate_encryption_key();
    println!("Generated new encryption key: {}", encryption_key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose, Engine as _};

    #[test]
    fn test_encryption_key_generation() {
        let key = generate_encryption_key();

        let key_string = key;
        assert!(!key_string.is_empty(), "Generated key should not be empty");

        // Verify it's valid base64
        let decoded = general_purpose::STANDARD.decode(&key_string);
        assert!(decoded.is_ok(), "Generated key is not valid base64");

        // Verify it's 32 bytes when decoded
        let decoded_bytes = decoded.unwrap();
        assert_eq!(decoded_bytes.len(), 32, "Decoded key should be 32 bytes");
    }

    #[test]
    fn test_multiple_keys_are_different() {
        let key1 = generate_encryption_key();
        let key2 = generate_encryption_key();

        assert_ne!(key1, key2, "Two generated keys should be different");
    }
}
