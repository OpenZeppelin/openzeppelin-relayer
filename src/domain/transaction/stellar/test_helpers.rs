#[cfg(test)]
use crate::domain::transaction::stellar::StellarRelayerTransaction;
use crate::{
    jobs::MockJobProducerTrait,
    models::{
        AssetSpec, DecoratedSignature, NetworkTransactionData, NetworkType, OperationSpec,
        RelayerNetworkPolicy, RelayerRepoModel, RelayerStellarPolicy, StellarTransactionData,
        TransactionRepoModel, TransactionStatus,
    },
    repositories::{MockRepository, MockTransactionCounterTrait, MockTransactionRepository},
    services::{MockSigner, MockStellarProviderTrait},
};
use chrono::Utc;
use soroban_rs::xdr::{Signature, SignatureHint};

pub const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

pub fn dummy_signature() -> DecoratedSignature {
    use soroban_rs::xdr::BytesM;
    let hint = SignatureHint([0; 4]);
    let bytes: Vec<u8> = vec![0u8; 64];
    let bytes_m: BytesM<64> = bytes.try_into().expect("BytesM conversion");
    DecoratedSignature {
        hint,
        signature: Signature(bytes_m),
    }
}

pub fn create_test_relayer() -> RelayerRepoModel {
    RelayerRepoModel {
        id: "relayer-1".to_string(),
        name: "Test Relayer".to_string(),
        network: "testnet".to_string(),
        paused: false,
        network_type: NetworkType::Stellar,
        signer_id: "signer-1".to_string(),
        policies: RelayerNetworkPolicy::Stellar(RelayerStellarPolicy::default()),
        address: TEST_PK.to_string(),
        notification_id: Some("test-notification-id".to_string()),
        system_disabled: false,
        custom_rpc_urls: None,
    }
}

pub fn payment_op(destination: &str) -> OperationSpec {
    OperationSpec::Payment {
        destination: destination.to_string(),
        amount: 100,
        asset: AssetSpec::Native,
    }
}

pub fn create_test_transaction(relayer_id: &str) -> TransactionRepoModel {
    let stellar_tx_data = StellarTransactionData {
        source_account: TEST_PK.to_string(),
        fee: Some(100),
        sequence_number: Some(1),
        memo: None,
        valid_until: None,
        network_passphrase: "Test SDF Network ; September 2015".to_string(),
        signatures: Vec::new(),
        hash: None,
        simulation_transaction_data: None,
        transaction_input: crate::models::TransactionInput::Operations(vec![payment_op(TEST_PK)]),
        signed_envelope_xdr: None,
    };
    TransactionRepoModel {
        id: "tx-1".to_string(),
        relayer_id: relayer_id.to_string(),
        status: TransactionStatus::Pending,
        created_at: Utc::now().to_rfc3339(),
        sent_at: None,
        confirmed_at: None,
        valid_until: None,
        network_data: NetworkTransactionData::Stellar(stellar_tx_data),
        priced_at: None,
        hashes: Vec::new(),
        network_type: NetworkType::Stellar,
        noop_count: None,
        is_canceled: Some(false),
        status_reason: None,
        delete_at: None,
    }
}

pub struct TestMocks {
    pub provider: MockStellarProviderTrait,
    pub relayer_repo: MockRepository<RelayerRepoModel, String>,
    pub tx_repo: MockTransactionRepository,
    pub job_producer: MockJobProducerTrait,
    pub signer: MockSigner,
    pub counter: MockTransactionCounterTrait,
}

pub fn default_test_mocks() -> TestMocks {
    TestMocks {
        provider: MockStellarProviderTrait::new(),
        relayer_repo: MockRepository::new(),
        tx_repo: MockTransactionRepository::new(),
        job_producer: MockJobProducerTrait::new(),
        signer: MockSigner::new(),
        counter: MockTransactionCounterTrait::new(),
    }
}

#[allow(clippy::type_complexity)]
pub fn make_stellar_tx_handler(
    relayer: RelayerRepoModel,
    mocks: TestMocks,
) -> StellarRelayerTransaction<
    MockRepository<RelayerRepoModel, String>,
    MockTransactionRepository,
    MockJobProducerTrait,
    MockSigner,
    MockStellarProviderTrait,
    MockTransactionCounterTrait,
> {
    use std::sync::Arc;
    StellarRelayerTransaction::new(
        relayer,
        Arc::new(mocks.relayer_repo),
        Arc::new(mocks.tx_repo),
        Arc::new(mocks.job_producer),
        Arc::new(mocks.signer),
        mocks.provider,
        Arc::new(mocks.counter),
    )
    .expect("handler construction should succeed")
}
