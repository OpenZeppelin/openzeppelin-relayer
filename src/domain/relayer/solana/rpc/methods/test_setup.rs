/// Test setup for solana rpc methods
use solana_sdk::{
    message::Message, pubkey::Pubkey, signature::Keypair, signer::Signer, system_instruction,
    transaction::Transaction,
};

use crate::{
    models::{
        EncodedSerializedTransaction, NetworkType, RelayerNetworkPolicy, RelayerRepoModel,
        RelayerSolanaPolicy,
    },
    services::{MockJupiterServiceTrait, MockSolanaProviderTrait, MockSolanaSignTrait},
};

pub fn setup_test_context() -> (
    RelayerRepoModel,
    MockSolanaSignTrait,
    MockSolanaProviderTrait,
    MockJupiterServiceTrait,
    EncodedSerializedTransaction,
) {
    // Create test transaction
    let payer = Keypair::new();
    let recipient = Pubkey::new_unique();
    let ix = system_instruction::transfer(&payer.pubkey(), &recipient, 1000);
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
            allowed_accounts: None,
            allowed_tokens: None,
            min_balance: 10000,
            allowed_programs: None,
            max_signatures: Some(10),
            disallowed_accounts: None,
            max_allowed_transfer_amount_lamports: None,
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

    (relayer, mock_signer, provider, jupiter_service, encoded_tx)
}
