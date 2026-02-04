//! Soroban Authorization Entry Signing Tool
//!
//! Signs a `user_auth_entry` from the `/build` endpoint response for gas abstraction.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example sign_soroban_auth_entry -- \
//!   --secret-key "S..." \
//!   --auth-entry "<base64 XDR>" \
//!   --network testnet
//! ```
//!
//! # Output
//!
//! - Default: Signed auth entry as base64 XDR
//! - With `--transaction-xdr`: JSON payload for `/transactions` endpoint

use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};
use eyre::{eyre, Result, WrapErr};
use sha2::{Digest, Sha256};
use soroban_rs::xdr::{
    AccountId, Hash, HashIdPreimage, HashIdPreimageSorobanAuthorization, Limits, ReadXdr,
    ScAddress, ScBytes, ScMap, ScMapEntry, ScSymbol, ScVal, ScVec, SorobanAddressCredentials,
    SorobanAuthorizationEntry, SorobanCredentials, WriteXdr,
};
use stellar_strkey::ed25519::{PrivateKey, PublicKey};

/// CLI arguments for signing Soroban authorization entries
#[derive(Parser, Debug)]
#[command(name = "sign-auth-entry")]
#[command(about = "Sign Soroban authorization entries for gas abstraction")]
#[command(version)]
struct Args {
    /// Stellar secret key (S... format)
    #[arg(short, long)]
    secret_key: String,

    /// Base64 XDR encoded SorobanAuthorizationEntry (from /build endpoint)
    #[arg(short, long)]
    auth_entry: String,

    /// Network: "testnet", "mainnet", or "custom:<passphrase>"
    #[arg(short, long, default_value = "testnet")]
    network: String,

    /// Transaction XDR from /build endpoint (optional, for JSON output)
    #[arg(short, long)]
    transaction_xdr: Option<String>,
}

/// Resolves network name to passphrase
fn resolve_network_passphrase(network: &str) -> Result<String> {
    let lower = network.to_lowercase();
    if lower == "testnet" {
        Ok("Test SDF Network ; September 2015".to_string())
    } else if lower == "mainnet" {
        Ok("Public Global Stellar Network ; September 2015".to_string())
    } else if lower.starts_with("custom:") {
        // Preserve original case for custom passphrase
        Ok(network
            .get(7..) // Skip "custom:" prefix
            .ok_or_else(|| eyre!("Invalid custom network format"))?
            .to_string())
    } else {
        Err(eyre!(
            "Invalid network '{}'. Use 'testnet', 'mainnet', or 'custom:<passphrase>'",
            network
        ))
    }
}

/// Extract the public key bytes from an ScAddress (Account type)
fn extract_account_public_key(address: &ScAddress) -> Result<[u8; 32]> {
    match address {
        ScAddress::Account(AccountId(public_key)) => match public_key {
            soroban_rs::xdr::PublicKey::PublicKeyTypeEd25519(uint256) => Ok(uint256.0),
        },
        _ => Err(eyre!(
            "Expected Account address (G...), got a different address type"
        )),
    }
}

/// Signs a Soroban authorization entry
fn sign_auth_entry(
    secret_key: &str,
    auth_entry_xdr: &str,
    network_passphrase: &str,
) -> Result<String> {
    // 1. Parse the secret key
    let private_key =
        PrivateKey::from_string(secret_key).map_err(|e| eyre!("Invalid secret key: {}", e))?;
    let signing_key = SigningKey::from_bytes(&private_key.0);
    let verifying_key = signing_key.verifying_key();
    let public_key_bytes = verifying_key.to_bytes();

    // 2. Decode the auth entry
    let auth_entry = SorobanAuthorizationEntry::from_xdr_base64(auth_entry_xdr, Limits::none())
        .wrap_err("Failed to decode auth entry XDR")?;

    // 3. Extract credentials
    let addr_creds = match &auth_entry.credentials {
        SorobanCredentials::Address(creds) => creds,
        _ => return Err(eyre!("Expected Address credentials in auth entry")),
    };

    // 4. Verify the public key matches the address in the auth entry
    let expected_public_key = extract_account_public_key(&addr_creds.address)?;
    let signer_public_key = PublicKey(public_key_bytes);
    let expected_g_address = PublicKey(expected_public_key);

    eprintln!(
        "Auth entry address: G{}",
        stellar_strkey::Strkey::PublicKeyEd25519(expected_g_address)
            .to_string()
            .strip_prefix("G")
            .unwrap_or("")
    );
    eprintln!(
        "Signer public key:  G{}",
        stellar_strkey::Strkey::PublicKeyEd25519(signer_public_key)
            .to_string()
            .strip_prefix("G")
            .unwrap_or("")
    );

    if public_key_bytes != expected_public_key {
        return Err(eyre!(
            "Secret key does not match auth entry address!\n\
             Expected: {}\n\
             Got: {}",
            stellar_strkey::Strkey::PublicKeyEd25519(expected_g_address),
            stellar_strkey::Strkey::PublicKeyEd25519(signer_public_key)
        ));
    }

    eprintln!("Public key matches auth entry address");

    // 5. Create the network ID (hash of passphrase)
    let network_id_bytes: [u8; 32] = Sha256::digest(network_passphrase.as_bytes()).into();
    let network_id = Hash(network_id_bytes);

    eprintln!("Network: {}", network_passphrase);
    eprintln!("Nonce: {}", addr_creds.nonce);
    eprintln!(
        "Signature expiration ledger: {}",
        addr_creds.signature_expiration_ledger
    );

    // 6. Build the preimage for signing
    let preimage = HashIdPreimage::SorobanAuthorization(HashIdPreimageSorobanAuthorization {
        network_id,
        nonce: addr_creds.nonce,
        signature_expiration_ledger: addr_creds.signature_expiration_ledger,
        invocation: auth_entry.root_invocation.clone(),
    });

    // 7. Serialize preimage to XDR and hash
    let preimage_xdr = preimage
        .to_xdr(Limits::none())
        .wrap_err("Failed to serialize preimage")?;
    let preimage_hash = Sha256::digest(&preimage_xdr);

    eprintln!("Preimage hash: {}", hex::encode(&preimage_hash));

    // 8. Sign the hash
    let signature = signing_key.sign(&preimage_hash);

    eprintln!(
        "Signature (64 bytes): {}",
        hex::encode(signature.to_bytes())
    );

    // 10. Create the signature ScVal (Vec containing a Map with public_key and signature)
    // Format: Vec<AccountEd25519Signature> where each entry is a Map with:
    //   - "public_key": BytesN<32>
    //   - "signature": BytesN<64>
    let signature_map = ScMap::try_from(vec![
        ScMapEntry {
            key: ScVal::Symbol(
                ScSymbol::try_from("public_key".as_bytes().to_vec())
                    .map_err(|_| eyre!("Failed to create public_key symbol"))?,
            ),
            val: ScVal::Bytes(
                ScBytes::try_from(public_key_bytes.to_vec())
                    .map_err(|_| eyre!("Failed to create public_key bytes"))?,
            ),
        },
        ScMapEntry {
            key: ScVal::Symbol(
                ScSymbol::try_from("signature".as_bytes().to_vec())
                    .map_err(|_| eyre!("Failed to create signature symbol"))?,
            ),
            val: ScVal::Bytes(
                ScBytes::try_from(signature.to_bytes().to_vec())
                    .map_err(|_| eyre!("Failed to create signature bytes"))?,
            ),
        },
    ])
    .map_err(|_| eyre!("Failed to create signature map"))?;

    let signature_scval = ScVal::Vec(Some(
        ScVec::try_from(vec![ScVal::Map(Some(signature_map))])
            .map_err(|_| eyre!("Failed to create signature vec"))?,
    ));

    eprintln!("Created signature ScVal structure");

    // 11. Create the signed auth entry
    let signed_auth_entry = SorobanAuthorizationEntry {
        credentials: SorobanCredentials::Address(SorobanAddressCredentials {
            address: addr_creds.address.clone(),
            nonce: addr_creds.nonce,
            signature_expiration_ledger: addr_creds.signature_expiration_ledger,
            signature: signature_scval,
        }),
        root_invocation: auth_entry.root_invocation,
    };

    // 12. Serialize to base64 XDR
    let signed_xdr = signed_auth_entry
        .to_xdr_base64(Limits::none())
        .wrap_err("Failed to serialize signed auth entry")?;

    Ok(signed_xdr)
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Resolve network passphrase
    let network_passphrase = resolve_network_passphrase(&args.network)?;

    // Sign the auth entry
    let signed_auth_entry =
        sign_auth_entry(&args.secret_key, &args.auth_entry, &network_passphrase)?;

    // Output based on whether transaction_xdr was provided
    match args.transaction_xdr {
        Some(tx_xdr) => {
            // Output JSON payload for /transactions endpoint
            let payload = serde_json::json!({
                "network": args.network,
                "transaction_xdr": tx_xdr,
                "signed_auth_entry": signed_auth_entry
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        None => {
            // Output just the signed auth entry
            println!("{}", signed_auth_entry);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_network_passphrase_testnet() {
        let passphrase = resolve_network_passphrase("testnet").unwrap();
        assert_eq!(passphrase, "Test SDF Network ; September 2015");
    }

    #[test]
    fn test_resolve_network_passphrase_mainnet() {
        let passphrase = resolve_network_passphrase("mainnet").unwrap();
        assert_eq!(passphrase, "Public Global Stellar Network ; September 2015");
    }

    #[test]
    fn test_resolve_network_passphrase_custom() {
        let passphrase = resolve_network_passphrase("custom:My Custom Network").unwrap();
        assert_eq!(passphrase, "My Custom Network");
    }

    #[test]
    fn test_resolve_network_passphrase_invalid() {
        let result = resolve_network_passphrase("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_network_passphrase_case_insensitive() {
        let passphrase = resolve_network_passphrase("TESTNET").unwrap();
        assert_eq!(passphrase, "Test SDF Network ; September 2015");
    }
}
