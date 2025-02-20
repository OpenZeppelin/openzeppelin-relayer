

/// This is a simple example of how to create a transaction in Solana using the Rust SDK.
/// It demonstrates how to create a transaction with different types of instructions and encode it
/// as a base64 string.
/// Can be used for testing transaction encoding and decoding.
/// Run with  cargo run --example test_tx

use eyre::Result;
use std::str::FromStr;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use solana_sdk::hash::Hash;
use base64::{engine::general_purpose::STANDARD, Engine};
use spl_token::instruction as token_instruction;

#[tokio::main]
async fn main() -> Result<()> {
    // Connect to Solana
    let client = RpcClient::new("https://api.mainnet-beta.solana.com");

    // Create test transaction
    let payer = Keypair::new();
    let recipient = Pubkey::new_unique();

    // Get recent blockhash
    let recent_blockhash = client.get_latest_blockhash()?;
    let usdc_mint = Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")?;
    let token_account = Pubkey::new_unique(); // In real scenario, this would be your token account
    let recipient_token_account = Pubkey::new_unique(); // Recipient's token account


    // Create different types of transactions for testing
    let transactions = vec![
        create_sol_transfer(&payer, &recipient, 1_000_000, recent_blockhash)?, // 0.001 SOL
        create_large_sol_transfer(&payer, &recipient, 1_000_000_000, recent_blockhash)?, // 1 SOL
        create_multi_instruction_tx(&payer, &recipient, recent_blockhash)?,
        create_token_transfer(&payer, &token_account, &recipient_token_account, &usdc_mint, 1_000_000, recent_blockhash)?,
    ];

    for (i, tx) in transactions.iter().enumerate() {
        let serialized = bincode::serialize(tx)?;
        let encoded = STANDARD.encode(serialized);
        println!("Transaction {}: {}", i, encoded);
    }

    Ok(())
}

fn create_sol_transfer(
    payer: &Keypair,
    recipient: &Pubkey,
    amount: u64,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction> {
    let ix = system_instruction::transfer(&payer.pubkey(), recipient, amount);
    let mut message = Message::new(&[ix], Some(&payer.pubkey()));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

fn create_large_sol_transfer(
    payer: &Keypair,
    recipient: &Pubkey,
    amount: u64,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction> {
    let ix = system_instruction::transfer(&payer.pubkey(), recipient, amount);
    let mut message = Message::new(&[ix], Some(&payer.pubkey()));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

fn create_multi_instruction_tx(
    payer: &Keypair,
    recipient: &Pubkey,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction> {
    let instructions = vec![
        system_instruction::transfer(&payer.pubkey(), recipient, 1_000_000),
        system_instruction::transfer(&payer.pubkey(), recipient, 2_000_000),
        system_instruction::transfer(&payer.pubkey(), recipient, 3_000_000),
    ];
    let mut message = Message::new(&instructions, Some(&payer.pubkey()));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

fn create_token_transfer(
    payer: &Keypair,
    token_account: &Pubkey,
    recipient_token_account: &Pubkey,
    mint: &Pubkey,
    amount: u64,
    recent_blockhash: Hash,
) -> Result<Transaction> {
    let ix = token_instruction::transfer(
        &spl_token::id(),
        token_account,
        recipient_token_account,
        &payer.pubkey(),
        &[&payer.pubkey()],
        amount,
    )?;

    let mut message = Message::new(&[ix], Some(&payer.pubkey()));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

