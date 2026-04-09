//! Midnight dust registration preparation utility.
//!
//! Generates DUST registration payload plus wallet addresses from a seed using
//! the same byte-shape used by Midnight e2e tests (`new_dust_hex`).
//!
//! Usage:
//! 1) Seed directly:
//! `cargo run --example midnight_dust_prep --features midnight -- --seed-hex <64-hex-bytes> --network preview`
//! 2) From keystore:
//! `cargo run --example midnight_dust_prep --features midnight -- --keystore-path <path> --passphrase <pass> --network preview`

#[cfg(feature = "midnight")]
use clap::Parser;
#[cfg(feature = "midnight")]
use eyre::{bail, Result, WrapErr};
#[cfg(feature = "midnight")]
use hex::ToHex;
#[cfg(feature = "midnight")]
use midnight_node_ledger_helpers::{
    serialize_untagged, DefaultDB, DustWallet, IntoWalletAddress, ShieldedWallet, UnshieldedWallet,
    WalletSeed,
};

#[cfg(feature = "midnight")]
#[derive(Parser, Debug)]
#[command(author, version, about = "Prepare Midnight DUST registration payload")]
struct Args {
    /// 32-byte wallet seed in hex (with or without 0x prefix)
    #[arg(long)]
    seed_hex: Option<String>,

    /// Path to an encrypted local signer keystore (alternative to --seed-hex)
    #[arg(long)]
    keystore_path: Option<String>,

    /// Keystore passphrase (falls back to KEYSTORE_PASSPHRASE env var)
    #[arg(long)]
    passphrase: Option<String>,

    /// Midnight network id used for bech32 addresses (preview/preprod/mainnet/...)
    #[arg(long, default_value = "preview")]
    network: String,
}

#[cfg(feature = "midnight")]
fn parse_seed_hex(seed_hex: &str) -> Result<[u8; 32]> {
    let normalized = seed_hex.trim().trim_start_matches("0x");
    let bytes = hex::decode(normalized)
        .wrap_err("Invalid --seed-hex: expected hex string (with optional 0x prefix)")?;

    if bytes.len() != 32 {
        bail!(
            "Invalid --seed-hex length: expected 32 bytes (64 hex chars), got {} bytes",
            bytes.len()
        );
    }

    <[u8; 32]>::try_from(bytes.as_slice()).wrap_err("Failed to parse 32-byte seed")
}

#[cfg(feature = "midnight")]
fn load_seed_from_keystore(path: &str, passphrase: &str) -> Result<[u8; 32]> {
    let loaded = oz_keystore::LocalClient::load(path.into(), passphrase.to_string());
    <[u8; 32]>::try_from(loaded.as_slice()).wrap_err("Loaded key is not 32 bytes")
}

#[cfg(feature = "midnight")]
fn resolve_seed(args: &Args) -> Result<[u8; 32]> {
    match (&args.seed_hex, &args.keystore_path) {
        (Some(seed_hex), None) => parse_seed_hex(seed_hex),
        (None, Some(path)) => {
            let passphrase = args
                .passphrase
                .clone()
                .or_else(|| std::env::var("KEYSTORE_PASSPHRASE").ok())
                .ok_or_else(|| {
                    eyre::eyre!("Missing passphrase: pass --passphrase or set KEYSTORE_PASSPHRASE")
                })?;

            load_seed_from_keystore(path, &passphrase)
        }
        (Some(_), Some(_)) => bail!("Pass only one key source: --seed-hex or --keystore-path"),
        (None, None) => bail!("Missing key source: pass --seed-hex or --keystore-path"),
    }
}

#[cfg(feature = "midnight")]
fn main() -> Result<()> {
    let args = Args::parse();
    let seed = resolve_seed(&args)?;
    let wallet_seed = WalletSeed::Medium(seed);

    let shielded_wallet = ShieldedWallet::<DefaultDB>::default(wallet_seed);
    let unshielded_wallet = UnshieldedWallet::default(wallet_seed);
    let dust_wallet = DustWallet::<DefaultDB>::default(wallet_seed, None);

    let shielded_address = shielded_wallet.address(&args.network).to_bech32();
    let viewing_key = shielded_wallet.viewing_key(&args.network);
    let unshielded_address = unshielded_wallet.address(&args.network).to_bech32();
    let dust_address = dust_wallet.address(&args.network).to_bech32();

    let dust_public_raw = serialize_untagged(&dust_wallet.public_key)
        .wrap_err("Failed to serialize DUST public key")?;
    let dust_public_raw_hex = dust_public_raw.encode_hex::<String>();

    let mut dust_public_registration = dust_public_raw;
    if dust_public_registration.len() == 32 {
        dust_public_registration.push(0);
    }
    let dust_public_registration_hex = dust_public_registration.encode_hex::<String>();

    println!("Midnight wallet + DUST registration payload");
    println!("network={}", args.network);
    println!("shielded_address={shielded_address}");
    println!("viewing_key={viewing_key}");
    println!("unshielded_address={unshielded_address}");
    println!("dust_address={dust_address}");
    println!(
        "midnight_address_hex={}",
        unshielded_wallet.user_address.0 .0.encode_hex::<String>()
    );
    println!(
        "coin_public_key_hex={}",
        shielded_wallet.coin_public_key.0 .0.encode_hex::<String>()
    );
    println!("raw_dust_public_key_hex={dust_public_raw_hex}");
    println!("registration_dust_public_key_hex={dust_public_registration_hex}");
    println!(
        "registration_dust_public_key_bytes={}",
        dust_public_registration.len()
    );
    println!();
    println!(
        "Use registration_dust_public_key_hex when submitting the Cardano-side registration transaction."
    );

    Ok(())
}

#[cfg(not(feature = "midnight"))]
fn main() {
    eprintln!(
        "This example requires the `midnight` feature. \
Run: cargo run --example midnight_dust_prep --features midnight -- --seed-hex <64-hex-bytes>"
    );
    std::process::exit(1);
}
