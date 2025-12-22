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
    services::{
        provider::MockStellarProviderTrait, signer::MockSigner,
        stellar_dex::MockStellarDexServiceTrait,
    },
};
use chrono::Utc;
use soroban_rs::xdr::{
    AccountId, Asset, BytesM, Limits, Memo, MuxedAccount, Operation, OperationBody, PaymentOp,
    Preconditions, PublicKey as XdrPublicKey, SequenceNumber, Signature, SignatureHint,
    Transaction, TransactionEnvelope, TransactionExt, TransactionV0, TransactionV0Envelope,
    TransactionV1Envelope, Uint256, VecM, WriteXdr,
};
use stellar_strkey::ed25519::PublicKey;

// Common test addresses
pub const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
pub const TEST_PK_2: &str = "GCEZWKCA5VLDNRLN3RPRJMRZOX3Z6G5CHCGSNFHEYVXM3XOJMDS674JZ";
pub const TEST_CONTRACT: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
pub const INVALID_XDR: &str = "INVALID_BASE64_XDR_DATA";

/// Create a dummy signature for testing
pub fn dummy_signature() -> DecoratedSignature {
    let hint = SignatureHint([0; 4]);
    let bytes: Vec<u8> = vec![0u8; 64];
    let bytes_m: BytesM<64> = bytes.try_into().expect("BytesM conversion");
    DecoratedSignature {
        hint,
        signature: Signature(bytes_m),
    }
}

/// Create a dummy signature with custom hint
pub fn dummy_signature_with_hint(hint: [u8; 4]) -> DecoratedSignature {
    let bytes: Vec<u8> = vec![0u8; 64];
    let bytes_m: BytesM<64> = bytes.try_into().expect("BytesM conversion");
    DecoratedSignature {
        hint: SignatureHint(hint),
        signature: Signature(bytes_m),
    }
}

/// Parse a Stellar public key string into an XDR PublicKey
pub fn parse_public_key(address: &str) -> PublicKey {
    PublicKey::from_string(address).expect("Valid public key")
}

/// Create an XDR AccountId from a Stellar address string
pub fn create_account_id(address: &str) -> AccountId {
    let pk = parse_public_key(address);
    AccountId(XdrPublicKey::PublicKeyTypeEd25519(Uint256(pk.0)))
}

/// Create a MuxedAccount from a Stellar address string
pub fn create_muxed_account(address: &str) -> MuxedAccount {
    let pk = parse_public_key(address);
    MuxedAccount::Ed25519(Uint256(pk.0))
}

/// Create a simple payment operation
pub fn create_payment_operation(destination: &str, amount: i64, asset: Asset) -> Operation {
    let dest_pk = parse_public_key(destination);
    Operation {
        source_account: None,
        body: OperationBody::Payment(PaymentOp {
            destination: MuxedAccount::Ed25519(Uint256(dest_pk.0)),
            asset,
            amount,
        }),
    }
}

/// Create a native XLM payment operation
pub fn create_native_payment_operation(destination: &str, amount: i64) -> Operation {
    create_payment_operation(destination, amount, Asset::Native)
}

/// Create a V1 transaction envelope (unsigned)
pub fn create_v1_envelope(
    source: &str,
    operations: Vec<Operation>,
    fee: u32,
    seq_num: i64,
) -> TransactionEnvelope {
    let source_pk = parse_public_key(source);
    let ops: VecM<Operation, 100> = operations.try_into().expect("Valid operations");

    let tx = Transaction {
        source_account: MuxedAccount::Ed25519(Uint256(source_pk.0)),
        fee,
        seq_num: SequenceNumber(seq_num),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: ops,
        ext: TransactionExt::V0,
    };

    TransactionEnvelope::Tx(TransactionV1Envelope {
        tx,
        signatures: vec![].try_into().unwrap(),
    })
}

/// Create a V0 transaction envelope (unsigned)
pub fn create_v0_envelope(
    source: &str,
    operations: Vec<Operation>,
    fee: u32,
    seq_num: i64,
) -> TransactionEnvelope {
    let source_pk = parse_public_key(source);
    let ops: VecM<Operation, 100> = operations.try_into().expect("Valid operations");

    let tx = TransactionV0 {
        source_account_ed25519: Uint256(source_pk.0),
        fee,
        seq_num: SequenceNumber(seq_num),
        time_bounds: None,
        memo: Memo::None,
        operations: ops,
        ext: soroban_rs::xdr::TransactionV0Ext::V0,
    };

    TransactionEnvelope::TxV0(TransactionV0Envelope {
        tx,
        signatures: vec![].try_into().unwrap(),
    })
}

/// Create a simple V1 envelope with one payment operation (unsigned)
pub fn create_simple_v1_envelope(source: &str, destination: &str) -> TransactionEnvelope {
    let payment_op = create_native_payment_operation(destination, 1_000_000);
    create_v1_envelope(source, vec![payment_op], 100, 1)
}

/// Create a simple V0 envelope with one payment operation (unsigned)
pub fn create_simple_v0_envelope(source: &str, destination: &str) -> TransactionEnvelope {
    let payment_op = create_native_payment_operation(destination, 1_000_000);
    create_v0_envelope(source, vec![payment_op], 100, 1)
}

/// Add signatures to an envelope
pub fn add_signatures_to_envelope(
    envelope: &mut TransactionEnvelope,
    signatures: Vec<DecoratedSignature>,
) {
    let sigs: VecM<DecoratedSignature, 20> = signatures.try_into().expect("Valid signatures");
    match envelope {
        TransactionEnvelope::TxV0(ref mut e) => {
            e.signatures = sigs;
        }
        TransactionEnvelope::Tx(ref mut e) => {
            e.signatures = sigs;
        }
        TransactionEnvelope::TxFeeBump(ref mut e) => {
            e.signatures = sigs;
        }
    }
}

/// Create a signed V1 envelope
pub fn create_signed_v1_envelope(source: &str, destination: &str) -> TransactionEnvelope {
    let mut envelope = create_simple_v1_envelope(source, destination);
    add_signatures_to_envelope(&mut envelope, vec![dummy_signature()]);
    envelope
}

/// Create a signed V0 envelope
pub fn create_signed_v0_envelope(source: &str, destination: &str) -> TransactionEnvelope {
    let mut envelope = create_simple_v0_envelope(source, destination);
    add_signatures_to_envelope(&mut envelope, vec![dummy_signature()]);
    envelope
}

/// Convert an envelope to XDR base64 string
pub fn envelope_to_xdr(envelope: &TransactionEnvelope) -> String {
    envelope.to_xdr_base64(Limits::none()).expect("Valid XDR")
}

/// Create a simple unsigned transaction XDR
pub fn create_unsigned_xdr(source: &str, destination: &str) -> String {
    let envelope = create_simple_v1_envelope(source, destination);
    envelope_to_xdr(&envelope)
}

/// Create a simple signed transaction XDR
pub fn create_signed_xdr(source: &str, destination: &str) -> String {
    let envelope = create_signed_v1_envelope(source, destination);
    envelope_to_xdr(&envelope)
}

/// Create a test XDR with multiple operations
pub fn create_xdr_with_operations(
    source: &str,
    operations: Vec<Operation>,
    include_signature: bool,
) -> String {
    let mut envelope = create_v1_envelope(source, operations, 100, 1);
    if include_signature {
        add_signatures_to_envelope(&mut envelope, vec![dummy_signature()]);
    }
    envelope_to_xdr(&envelope)
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
        ..Default::default()
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
        transaction_result_xdr: None,
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
    pub dex_service: MockStellarDexServiceTrait,
}

pub fn default_test_mocks() -> TestMocks {
    TestMocks {
        provider: MockStellarProviderTrait::new(),
        relayer_repo: MockRepository::new(),
        tx_repo: MockTransactionRepository::new(),
        job_producer: MockJobProducerTrait::new(),
        signer: MockSigner::new(),
        counter: MockTransactionCounterTrait::new(),
        dex_service: MockStellarDexServiceTrait::new(),
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
    MockStellarDexServiceTrait,
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
        Arc::new(mocks.dex_service),
    )
    .expect("handler construction should succeed")
}
