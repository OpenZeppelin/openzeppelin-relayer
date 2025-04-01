//! Test setup for solana rpc methods
use solana_sdk::{
    hash::Hash,
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use std::str::FromStr;

use crate::{
    jobs::MockJobProducerTrait,
    models::{
        EncodedSerializedTransaction, NetworkType, RelayerNetworkPolicy, RelayerRepoModel,
        RelayerSolanaPolicy, SolanaAllowedTokensPolicy, SolanaFeePaymentStrategy,
    },
    services::{MockJupiterServiceTrait, MockSolanaProviderTrait, MockSolanaSignTrait},
};

/// Creates a test context for Solana RPC methods
/// It includes a test transaction, relayer, and mock services
/// Used for testing methods with relayer fee strategy
pub fn setup_test_context() -> (
    RelayerRepoModel,
    MockSolanaSignTrait,
    MockSolanaProviderTrait,
    MockJupiterServiceTrait,
    EncodedSerializedTransaction,
    MockJobProducerTrait,
) {
    // Create test transaction
    let payer = Keypair::new();
    let source = Keypair::new();
    let recipient = Pubkey::new_unique();
    let ix = system_instruction::transfer(&source.pubkey(), &recipient, 1000);
    let message = Message::new(&[ix], Some(&payer.pubkey()));
    let transaction = Transaction::new_unsigned(message);
    // Create test relayer
    let relayer = RelayerRepoModel {
        id: "id".to_string(),
        name: "Relayer".to_string(),
        network: "testnet".to_string(),
        paused: false,
        network_type: NetworkType::Solana,
        policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: SolanaFeePaymentStrategy::Relayer,
            fee_margin_percentage: Some(0.5),
            allowed_accounts: None,
            allowed_tokens: None,
            min_balance: 10000,
            allowed_programs: None,
            max_signatures: Some(10),
            disallowed_accounts: None,
            max_allowed_fee_lamports: None,
            max_tx_data_size: 1000,
        }),
        signer_id: "test".to_string(),
        address: payer.pubkey().to_string(),
        notification_id: None,
        system_disabled: false,
    };

    // Setup mock signer
    let mock_signer = MockSolanaSignTrait::new();

    let encoded_tx =
        EncodedSerializedTransaction::try_from(&transaction).expect("Failed to encode transaction");

    let jupiter_service = MockJupiterServiceTrait::new();
    let provider = MockSolanaProviderTrait::new();
    let job_producer = MockJobProducerTrait::new();

    (
        relayer,
        mock_signer,
        provider,
        jupiter_service,
        encoded_tx,
        job_producer,
    )
}

pub struct UserFeeStrategyTestContext {
    pub relayer: RelayerRepoModel,
    pub signer: MockSolanaSignTrait,
    pub provider: MockSolanaProviderTrait,
    pub jupiter_service: MockJupiterServiceTrait,
    pub encoded_tx: EncodedSerializedTransaction,
    pub job_producer: MockJobProducerTrait,
    pub relayer_keypair: Keypair,
    pub user_keypair: Keypair,
    pub token: String,
    pub token_mint: Pubkey,
    pub user_token_account: Pubkey,
    pub relayer_token_account: Pubkey,
    pub main_transfer_amount: u64,
    pub fee_amount: u64,
}

/// This test context is for user fee strategy
/// It creates a transaction with two instructions:
/// 1. Main transfer from user to recipient
/// 2. Fee transfer from user to relayer
pub fn setup_test_context_user_fee_strategy() -> UserFeeStrategyTestContext {
    let token_owner = Keypair::new();
    let relayer_keypair = Keypair::new();

    let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC token mint
    let token_mint = Pubkey::from_str(test_token).unwrap();

    let source_token_account = get_associated_token_address(&token_owner.pubkey(), &token_mint);
    let destination_token_account =
        get_associated_token_address(&Pubkey::new_unique(), &token_mint);
    let relayer_token_account =
        get_associated_token_address(&relayer_keypair.pubkey(), &token_mint);

    let main_transfer_amount = 5_000_000u64; // Main transfer amount (5 USDC)
    let fee_amount = 1_000_000u64; // Fee amount (1 USDC)

    // Accounts for main transfer
    let main_transfer_accounts = vec![
        AccountMeta::new(source_token_account, false), // Source token account (writable)
        AccountMeta::new(destination_token_account, false), // Destination token account (writable)
        AccountMeta::new_readonly(token_owner.pubkey(), true), // Owner of source account (signer)
        AccountMeta::new_readonly(token_mint, false),  // Token mint (readonly)
    ];

    // Create main transfer instruction
    let main_transfer_data = spl_token::instruction::TokenInstruction::TransferChecked {
        amount: main_transfer_amount,
        decimals: 6,
    }
    .pack();

    let main_transfer_ix = Instruction {
        program_id: spl_token::id(),
        accounts: main_transfer_accounts,
        data: main_transfer_data,
    };

    // Accounts for fee transfer
    let fee_transfer_accounts = vec![
        AccountMeta::new(source_token_account, false), // Source token account (writable)
        AccountMeta::new(relayer_token_account, false), // Relayer token account (writable)
        AccountMeta::new_readonly(token_owner.pubkey(), true), // Owner of source account (signer)
        AccountMeta::new_readonly(token_mint, false),  // Token mint (readonly)
    ];

    // Create fee transfer instruction
    let fee_transfer_data = spl_token::instruction::TokenInstruction::TransferChecked {
        amount: fee_amount,
        decimals: 6,
    }
    .pack();

    let fee_transfer_ix = Instruction {
        program_id: spl_token::id(),
        accounts: fee_transfer_accounts,
        data: fee_transfer_data,
    };

    // Create the message with both instructions, making sure all accounts are properly marked
    let message = Message::new_with_blockhash(
        &[fee_transfer_ix, main_transfer_ix],
        Some(&relayer_keypair.pubkey()), // Fee payer
        &Hash::default(),                // We'll use a default blockhash for testing
    );

    let transaction = Transaction::new_unsigned(message);
    let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

    let relayer = RelayerRepoModel {
        id: "id".to_string(),
        name: "Relayer".to_string(),
        network: "testnet".to_string(),
        paused: false,
        network_type: NetworkType::Solana,
        policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: SolanaFeePaymentStrategy::User,
            fee_margin_percentage: Some(0.5),
            allowed_accounts: None,
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            min_balance: 10000,
            allowed_programs: None,
            max_signatures: Some(10),
            disallowed_accounts: None,
            max_allowed_fee_lamports: None,
            max_tx_data_size: 1000,
        }),
        signer_id: "test".to_string(),
        address: relayer_keypair.pubkey().to_string(),
        notification_id: None,
        system_disabled: false,
    };

    let mock_signer = MockSolanaSignTrait::new();
    let provider = MockSolanaProviderTrait::new();
    let jupiter_service = MockJupiterServiceTrait::new();
    let job_producer = MockJobProducerTrait::new();

    UserFeeStrategyTestContext {
        relayer,
        signer: mock_signer,
        provider,
        jupiter_service,
        encoded_tx,
        job_producer,
        relayer_keypair,
        user_keypair: token_owner,
        token_mint,
        token: test_token.to_string(),
        user_token_account: source_token_account,
        relayer_token_account,
        main_transfer_amount,
        fee_amount,
    }
}

pub struct UserFeeStrategySingleTxTestContext {
    pub relayer: RelayerRepoModel,
    pub signer: MockSolanaSignTrait,
    pub provider: MockSolanaProviderTrait,
    pub jupiter_service: MockJupiterServiceTrait,
    pub encoded_tx: EncodedSerializedTransaction,
    pub job_producer: MockJobProducerTrait,
    pub relayer_keypair: Keypair,
    pub user_keypair: Keypair,
    pub payer_keypair: Keypair,
    pub token: String,
    pub token_mint: Pubkey,
    pub user_token_account: Pubkey,
    pub relayer_token_account: Pubkey,
    pub payer_token_account: Pubkey,
    pub main_transfer_amount: u64,
    pub fee_amount: u64,
}

// This test context is for a single transaction used in rpc methods like prepare transactions and fee estimate
pub fn setup_test_context_single_tx_user_fee_strategy() -> UserFeeStrategySingleTxTestContext {
    let payer_keypair = Keypair::new(); // This will pay transaction fees only
    let token_owner = Keypair::new(); // This will own the token account
    let relayer_keypair = Keypair::new();

    let test_token = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"; // USDC token mint
    let token_mint = Pubkey::from_str(test_token).unwrap();

    let source_token_account = get_associated_token_address(&token_owner.pubkey(), &token_mint);
    let destination_token_account =
        get_associated_token_address(&Pubkey::new_unique(), &token_mint);
    let relayer_token_account =
        get_associated_token_address(&relayer_keypair.pubkey(), &token_mint);

    let payer_token_account = get_associated_token_address(&payer_keypair.pubkey(), &token_mint);

    let main_transfer_amount = 5_000_000u64; // Main transfer amount (5 USDC)
    let fee_amount = 1_000_000u64; // Fee amount (1 USDC)

    // Create manual token transfer instruction
    let transfer_accounts = vec![
        AccountMeta::new(source_token_account, false), // Source token account (writable)
        AccountMeta::new(destination_token_account, false), // Destination token account (writable)
        AccountMeta::new_readonly(token_owner.pubkey(), true), // Owner of source account (signer, NOT writable)
        AccountMeta::new_readonly(token_mint, false),          // Token mint (readonly)
    ];

    // Create transfer instruction data
    let transfer_data = spl_token::instruction::TokenInstruction::TransferChecked {
        amount: main_transfer_amount,
        decimals: 6, // USDC has 6 decimals
    }
    .pack();

    let transfer_ix = Instruction {
        program_id: spl_token::id(),
        accounts: transfer_accounts,
        data: transfer_data,
    };

    let message = Message::new_with_blockhash(
        &[transfer_ix],
        Some(&payer_keypair.pubkey()), // Separate fee payer
        &Hash::default(),
    );

    let transaction = Transaction::new_unsigned(message);
    let encoded_tx = EncodedSerializedTransaction::try_from(&transaction).unwrap();

    let relayer = RelayerRepoModel {
        id: "id".to_string(),
        name: "Relayer".to_string(),
        network: "testnet".to_string(),
        paused: false,
        network_type: NetworkType::Solana,
        policies: RelayerNetworkPolicy::Solana(RelayerSolanaPolicy {
            fee_payment_strategy: SolanaFeePaymentStrategy::User,
            fee_margin_percentage: Some(0.5),
            allowed_accounts: None,
            allowed_tokens: Some(vec![SolanaAllowedTokensPolicy {
                mint: test_token.to_string(),
                symbol: Some("USDC".to_string()),
                decimals: Some(6),
                max_allowed_fee: Some(10_000_000),
                conversion_slippage_percentage: Some(1.0),
            }]),
            min_balance: 10000,
            allowed_programs: None,
            max_signatures: Some(10),
            disallowed_accounts: None,
            max_allowed_fee_lamports: None,
            max_tx_data_size: 1000,
        }),
        signer_id: "test".to_string(),
        address: relayer_keypair.pubkey().to_string(),
        notification_id: None,
        system_disabled: false,
    };

    let mock_signer = MockSolanaSignTrait::new();
    let provider = MockSolanaProviderTrait::new();
    let jupiter_service = MockJupiterServiceTrait::new();
    let job_producer = MockJobProducerTrait::new();

    UserFeeStrategySingleTxTestContext {
        relayer,
        signer: mock_signer,
        provider,
        jupiter_service,
        encoded_tx,
        job_producer,
        relayer_keypair,
        user_keypair: token_owner,
        payer_keypair,
        token_mint,
        token: test_token.to_string(),
        user_token_account: source_token_account,
        payer_token_account,
        relayer_token_account,
        main_transfer_amount,
        fee_amount,
    }
}
