/// This is a simple example of how to create a transaction in Solana using the Rust SDK.
/// It demonstrates how to create a transaction with different types of instructions and encode
/// it as a base64 string.
/// Can be used for testing transaction encoding and decoding.
/// Run with  cargo run --example test_tx
use base64::{engine::general_purpose::STANDARD, Engine};
use eyre::Result;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{hash::Hash, message::Message, pubkey::Pubkey, transaction::Transaction};
use solana_system_interface::instruction;
use spl_token::instruction as token_instruction;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<()> {
    // Connect to Solana
    let client = RpcClient::new("https://api.mainnet-beta.solana.com");

    // Create test transaction
    // let payer = Keypair::new().pubkey();
    // from example file
    let payer = Pubkey::from_str("C6VBV1EK2Jx7kFgCkCD5wuDeQtEH8ct2hHGUPzEhUSc8").unwrap();
    let recipient = Pubkey::new_unique();

    // Get recent blockhash
    let recent_blockhash = client.get_latest_blockhash()?;
    let token_account = Pubkey::new_unique(); // In real scenario, this would be your token account
    let recipient_token_account = Pubkey::new_unique(); // Recipient's token account

    // Create different types of transactions for testing
    let transactions = vec![
        create_sol_transfer(&payer, &recipient, 1_000_000, recent_blockhash)?, // 0.001 SOL
        create_large_sol_transfer(&payer, &recipient, 1_000_000_000, recent_blockhash)?, // 1 SOL
        create_multi_instruction_tx(&payer, &recipient, recent_blockhash)?,
        create_token_transfer(
            &payer,
            &token_account,
            &recipient_token_account,
            1_000_000,
            recent_blockhash,
        )?,
    ];

    for (i, tx) in transactions.iter().enumerate() {
        let serialized = bincode::serialize(tx)?;
        let encoded = STANDARD.encode(serialized);
        println!("Transaction {}: {}", i, encoded);
    }

    Ok(())
}

fn create_sol_transfer(
    payer: &Pubkey,
    recipient: &Pubkey,
    amount: u64,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction> {
    let ix = instruction::transfer(payer, recipient, amount);
    let mut message = Message::new(&[ix], Some(payer));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

fn create_large_sol_transfer(
    payer: &Pubkey,
    recipient: &Pubkey,
    amount: u64,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction> {
    let ix = instruction::transfer(payer, recipient, amount);
    let mut message = Message::new(&[ix], Some(payer));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

fn create_multi_instruction_tx(
    payer: &Pubkey,
    recipient: &Pubkey,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Result<Transaction> {
    let instructions = vec![
        instruction::transfer(payer, recipient, 1_000_000),
        instruction::transfer(payer, recipient, 2_000_000),
        instruction::transfer(payer, recipient, 3_000_000),
    ];
    let mut message = Message::new(&instructions, Some(payer));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}

fn create_token_transfer(
    payer: &Pubkey,
    token_account: &Pubkey,
    recipient_token_account: &Pubkey,
    amount: u64,
    recent_blockhash: Hash,
) -> Result<Transaction> {
    let ix = token_instruction::transfer(
        &spl_token::id(),
        token_account,
        recipient_token_account,
        payer,
        &[payer],
        amount,
    )?;

    let mut message = Message::new(&[ix], Some(payer));
    message.recent_blockhash = recent_blockhash;
    Ok(Transaction::new_unsigned(message))
}
