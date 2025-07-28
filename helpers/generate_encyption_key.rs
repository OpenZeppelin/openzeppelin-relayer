//! Encryption Key Generation Tool
//!
//! This tool generates a random 32-byte encryption key using OpenSSL and prints it to the console.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example generate_encryption_key
//! ```
//!
//! # Requirements
//!
//! This tool requires OpenSSL to be installed and available in the system PATH.
use eyre::{eyre, Result};
use std::process::Command;

/// Main entry point for encryption key generation tool
fn main() -> Result<()> {
    let encryption_key = generate_encryption_key()?;
    println!("Generated new encryption key: {}", encryption_key);
    Ok(())
}

/// Generates a 32-byte base64-encoded encryption key using OpenSSL
fn generate_encryption_key() -> Result<String> {
    let output = Command::new("openssl")
        .args(["rand", "-base64", "32"])
        .output()
        .map_err(|e| eyre!("Failed to execute openssl command: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("OpenSSL command failed: {}", stderr));
    }

    let key = String::from_utf8(output.stdout)
        .map_err(|e| eyre!("Failed to parse openssl output as UTF-8: {}", e))?
        .trim()
        .to_string();

    if key.is_empty() {
        return Err(eyre!("OpenSSL returned empty key"));
    }

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose, Engine as _};

    #[test]
    fn test_encryption_key_generation() {
        let key = generate_encryption_key();
        assert!(key.is_ok(), "Failed to generate encryption key");

        let key_string = key.unwrap();
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

        assert!(
            key1.is_ok() && key2.is_ok(),
            "Both key generations should succeed"
        );
        assert_ne!(
            key1.unwrap(),
            key2.unwrap(),
            "Two generated keys should be different"
        );
    }
}
