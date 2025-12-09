//! # EIP-712 Signature Verification Example
//!
//! This example demonstrates how to verify EIP-712 signatures returned by the relayer API.
//!
//! ## Usage
//!
//! ```bash
//! # Basic usage with default test values
//! cargo run --example verify_eip712
//!
//! # With custom values from API response (hex values can be provided with or without 0x prefix)
//! cargo run --example verify_eip712 -- \
//!   --domain-separator "f2cee375fa42b42143804025fc449deafd50cc031ca257e0b194a650a912090f" \
//!   --hash-struct "c52c0ee5d84264471806290a3f2c4cecfc5490626bf912d01f240d7a274b371e" \
//!   --r "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef" \
//!   --s "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321" \
//!   --v 27 \
//!   --expected-address "0x7E5F4552091a69125d5DFcb7b8c2659029395Bdf"
//!
//! # Using complete signature (130 hex characters for r+s+v)
//! cargo run --example verify_eip712 -- \
//!   --signature "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdeffedcba0987654321fedcba0987654321fedcba0987654321fedcba09876543211b" \
//!   --expected-address "0x7E5F4552091a69125d5DFcb7b8c2659029395Bdf"
//! ```
//!
//! ## Where to Get Test Values
//!
//! The domain separator, hash struct, and signature values can be obtained from the relayer API's
//! `signTypedData` endpoint response. Use these values to verify that the signature was created
//! by the expected signer address.

use alloy::primitives::{keccak256, Address, Signature};
use clap::Parser;
use color_eyre::eyre::{eyre, Result, WrapErr};
use std::str::FromStr;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Domain separator (32 bytes as hex, with or without 0x prefix).
    /// Example: "f2cee375fa42b42143804025fc449deafd50cc031ca257e0b194a650a912090f"
    /// or "0xf2cee375fa42b42143804025fc449deafd50cc031ca257e0b194a650a912090f"
    #[arg(
        long,
        default_value = "f2cee375fa42b42143804025fc449deafd50cc031ca257e0b194a650a912090f"
    )]
    domain_separator: String,

    /// Hash struct message (32 bytes as hex, with or without 0x prefix).
    /// Example: "c52c0ee5d84264471806290a3f2c4cecfc5490626bf912d01f240d7a274b371e"
    #[arg(
        long,
        default_value = "c52c0ee5d84264471806290a3f2c4cecfc5490626bf912d01f240d7a274b371e"
    )]
    hash_struct: String,

    /// Signature r component (32 bytes as hex, with or without 0x prefix).
    /// Example: "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
    #[arg(long)]
    r: Option<String>,

    /// Signature s component (32 bytes as hex, with or without 0x prefix).
    /// Example: "fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321"
    #[arg(long)]
    s: Option<String>,

    /// Signature v component (27, 28, 0, or 1)
    #[arg(long)]
    v: Option<u8>,

    /// Expected signer address (to verify against, with or without 0x prefix).
    /// Example: "0x7E5F4552091a69125d5DFcb7b8c2659029395Bdf"
    #[arg(long)]
    expected_address: Option<String>,

    /// Complete signature as single hex string (130 hex chars for r+s+v, with or without 0x prefix).
    /// Alternative to providing r, s, v separately.
    /// Example: "1234...cdef1b"
    #[arg(long)]
    signature: Option<String>,
}

fn strip_0x_prefix(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    println!("EIP-712 Signature Verification Tool\n");

    // Parse domain separator
    let domain_separator = hex::decode(strip_0x_prefix(&args.domain_separator))
        .wrap_err("Failed to decode domain separator from hex")?;

    if domain_separator.len() != 32 {
        return Err(eyre!(
            "Domain separator must be exactly 32 bytes, got {}",
            domain_separator.len()
        ));
    }

    // Parse hash struct
    let hash_struct = hex::decode(strip_0x_prefix(&args.hash_struct))
        .wrap_err("Failed to decode hash struct from hex")?;

    if hash_struct.len() != 32 {
        return Err(eyre!(
            "Hash struct must be exactly 32 bytes, got {}",
            hash_struct.len()
        ));
    }

    // Construct EIP-712 message: "\x19\x01" ++ domainSeparator ++ hashStruct
    let mut eip712_message = Vec::with_capacity(66);
    eip712_message.extend_from_slice(&[0x19, 0x01]);
    eip712_message.extend_from_slice(&domain_separator);
    eip712_message.extend_from_slice(&hash_struct);

    // Hash the EIP-712 message
    let message_hash = keccak256(&eip712_message);

    println!("EIP-712 Message Components:");
    println!("  Domain Separator: 0x{}", hex::encode(&domain_separator));
    println!("  Hash Struct:      0x{}", hex::encode(&hash_struct));
    println!("  Message Hash:     0x{}\n", hex::encode(message_hash));

    // Parse signature
    let (r_bytes, s_bytes, v_value) = if let Some(sig_str) = args.signature {
        // Parse complete signature
        let sig_hex = strip_0x_prefix(&sig_str);
        if sig_hex.len() != 130 {
            return Err(eyre!(
                "Complete signature must be 130 hex characters (65 bytes), got {}",
                sig_hex.len()
            ));
        }

        let r = hex::decode(&sig_hex[0..64])
            .wrap_err("Failed to decode r component from complete signature")?;
        let s = hex::decode(&sig_hex[64..128])
            .wrap_err("Failed to decode s component from complete signature")?;
        let v = u8::from_str_radix(&sig_hex[128..130], 16)
            .wrap_err("Failed to decode v component from complete signature")?;

        (r, s, v)
    } else {
        // Parse individual components
        let r_str = args
            .r
            .ok_or_else(|| eyre!("Either --signature or --r/--s/--v must be provided"))?;
        let s_str = args
            .s
            .ok_or_else(|| eyre!("Either --signature or --r/--s/--v must be provided"))?;
        let v = args
            .v
            .ok_or_else(|| eyre!("Either --signature or --r/--s/--v must be provided"))?;

        let r = hex::decode(strip_0x_prefix(&r_str))
            .wrap_err("Failed to decode r component from hex")?;
        let s = hex::decode(strip_0x_prefix(&s_str))
            .wrap_err("Failed to decode s component from hex")?;

        if r.len() != 32 {
            return Err(eyre!(
                "r component must be exactly 32 bytes, got {}",
                r.len()
            ));
        }
        if s.len() != 32 {
            return Err(eyre!(
                "s component must be exactly 32 bytes, got {}",
                s.len()
            ));
        }

        (r, s, v)
    };

    // Validate v value
    if ![0, 1, 27, 28].contains(&v_value) {
        return Err(eyre!(
            "v component must be 0, 1, 27, or 28, got {}",
            v_value
        ));
    }

    // Construct full signature
    let mut sig_bytes = Vec::with_capacity(65);
    sig_bytes.extend_from_slice(&r_bytes);
    sig_bytes.extend_from_slice(&s_bytes);
    sig_bytes.push(v_value);

    println!("Signature Components:");
    println!("  r: 0x{}", hex::encode(&r_bytes));
    println!("  s: 0x{}", hex::encode(&s_bytes));
    println!("  v: {}", v_value);
    println!("  Full: 0x{}\n", hex::encode(&sig_bytes));

    // Parse signature using alloy
    let signature = Signature::try_from(&sig_bytes[..])
        .wrap_err("Failed to parse signature bytes into Alloy Signature type")?;

    // Recover the address from the signature
    let recovered_address = signature
        .recover_address_from_prehash(&message_hash)
        .wrap_err("Failed to recover signer address from signature")?;

    println!("Verification Result:");
    println!("  Recovered Address: {}", recovered_address);

    // Compare with expected address if provided
    if let Some(expected_str) = args.expected_address {
        let expected_address =
            Address::from_str(&expected_str).wrap_err("Failed to parse expected address")?;

        println!("  Expected Address:  {}", expected_address);

        if recovered_address == expected_address {
            println!("\nSUCCESS: Signature is valid!");
            println!("The signature was created by the expected signer.");
            Ok(())
        } else {
            println!("\nFAILURE: Signature verification failed!");
            println!("The recovered address does not match the expected address.");
            println!("This signature was NOT created by the expected signer.");
            Err(eyre!("Signature verification failed"))
        }
    } else {
        println!("\nWARNING: No expected address provided for verification.");
        println!("Use --expected-address to verify the signer.");
        println!("\nThe signature is structurally valid and was created by:");
        println!("{}", recovered_address);
        Ok(())
    }
}
