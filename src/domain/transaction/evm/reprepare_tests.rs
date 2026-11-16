use super::tests::{
    create_test_network_model, default_test_mocks, make_test_evm_relayer_transaction,
};
use crate::{
    constants::HEALTH_CHECK_NONCE_HINT_KEY,
    domain::{
        relayer::{SignTransactionResponse, SignTransactionResponseEvm},
        transaction::{
            evm::{EvmRelayerTransaction, MockPriceCalculatorTrait, PriceParams},
            Transaction,
        },
    },
    jobs::MockJobProducerTrait,
    models::{
        evm::Speed, EvmTransactionData, EvmTransactionDataSignature, NetworkTransactionData,
        NetworkType, RelayerEvmPolicy, RelayerNetworkPolicy, RelayerRepoModel, RepositoryError,
        TransactionRepoModel, TransactionStatus, U256,
    },
    repositories::{
        InMemoryTransactionCounter, InMemoryTransactionRepository, MockNetworkRepository,
        MockRelayerRepository, Repository, TransactionCounterTrait, TransactionRepository,
    },
    services::{provider::MockEvmProviderTrait, signer::MockSigner},
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::Notify;

const RELAYER_ID: &str = "test-relayer-id";
const ADDR: &str = "0xSender";

fn relayer() -> RelayerRepoModel {
    RelayerRepoModel {
        id: RELAYER_ID.to_string(),
        name: "r".to_string(),
        network: "mainnet".to_string(),
        address: ADDR.to_string(),
        paused: false,
        system_disabled: false,
        signer_id: "s".to_string(),
        notification_id: Some("notification".into()),
        policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy::default()),
        network_type: NetworkType::Evm,
        custom_rpc_urls: None,
        ..Default::default()
    }
}

fn pending_tx(nonce: Option<u64>, age: Duration) -> TransactionRepoModel {
    TransactionRepoModel {
        version: 0,
        id: "tx-1".to_string(),
        relayer_id: RELAYER_ID.to_string(),
        status: TransactionStatus::Pending,
        status_reason: None,
        created_at: (Utc::now() - age).to_rfc3339(),
        sent_at: None,
        confirmed_at: None,
        valid_until: None,
        delete_at: None,
        network_type: NetworkType::Evm,
        network_data: NetworkTransactionData::Evm(EvmTransactionData {
            chain_id: 1,
            from: ADDR.to_string(),
            to: Some("0xRecipient".to_string()),
            value: U256::from(1u64),
            data: Some("0xData".to_string()),
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce,
            signature: None,
            hash: None,
            speed: Some(Speed::Fast),
            raw: None,
        }),
        priced_at: None,
        hashes: vec![],
        noop_count: None,
        is_canceled: Some(false),
        metadata: None,
    }
}

fn block_ok() -> MockEvmProviderTrait {
    let mut p = MockEvmProviderTrait::new();
    p.expect_get_block_by_number().returning(|| {
        Box::pin(async {
            use alloy::{network::AnyRpcBlock, rpc::types::Block};
            let mut block: Block = Block::default();
            block.header.gas_limit = 30_000_000u64;
            Ok(AnyRpcBlock::from(block))
        })
    });
    p
}

fn provider(pause: Option<(Arc<Notify>, Arc<Notify>)>) -> MockEvmProviderTrait {
    let mut p = block_ok();
    p.expect_get_balance().returning(move |_| {
        let pause = pause.clone();
        Box::pin(async move {
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
            Ok(U256::from(10u128.pow(19)))
        })
    });
    p
}

fn price_calc() -> MockPriceCalculatorTrait {
    let mut pc = MockPriceCalculatorTrait::new();
    pc.expect_get_transaction_price_params().returning(|_, _| {
        Box::pin(std::future::ready(Ok(PriceParams {
            gas_price: Some(30_000_000_000),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            is_min_bumped: None,
            extra_fee: None,
            total_cost: U256::from(630_000_000_000_000u64),
        })))
    });
    pc.expect_calculate_bumped_gas_price().returning(|_, _, _| {
        Box::pin(async {
            Ok(PriceParams {
                gas_price: Some(30_000_000_000),
                is_min_bumped: Some(true),
                total_cost: U256::from(1),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                extra_fee: None,
            })
        })
    });
    pc
}

fn signer(
    label: &'static str,
    signed: Arc<Mutex<Vec<(&'static str, u64)>>>,
    pause: Option<(Arc<Notify>, Arc<Notify>)>,
    times: Option<usize>,
) -> MockSigner {
    let mut s = MockSigner::new();
    let exp = s.expect_sign_transaction();
    if let Some(n) = times {
        exp.times(n);
    }
    exp.returning(move |data| {
        let nonce = data.get_evm_transaction_data().unwrap().nonce.unwrap();
        signed.lock().unwrap().push((label, nonce));
        let pause = pause.clone();
        Box::pin(async move {
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
            Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                hash: format!("0x{label}{nonce}"),
                signature: EvmTransactionDataSignature {
                    r: "r".into(),
                    s: "s".into(),
                    v: 1,
                    sig: "0xsig".into(),
                },
                raw: vec![nonce as u8],
            }))
        })
    });
    s
}

#[derive(Default, Clone)]
struct Jobs {
    notifications: Arc<AtomicUsize>,
    submit: Arc<AtomicUsize>,
    request: Arc<AtomicUsize>,
    health_hints: Arc<Mutex<Vec<Option<String>>>>,
}

fn job_producer(jobs: &Jobs) -> MockJobProducerTrait {
    let mut j = MockJobProducerTrait::new();
    let submit = jobs.submit.clone();
    j.expect_produce_submit_transaction_job()
        .returning(move |_, _| {
            submit.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
    let request = jobs.request.clone();
    j.expect_produce_transaction_request_job()
        .returning(move |_, _| {
            request.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
    let hints = jobs.health_hints.clone();
    j.expect_produce_relayer_health_check_job()
        .returning(move |job, _| {
            hints.lock().unwrap().push(
                job.metadata
                    .as_ref()
                    .and_then(|m| m.get(HEALTH_CHECK_NONCE_HINT_KEY).cloned()),
            );
            Box::pin(async { Ok(()) })
        });
    let notifications = jobs.notifications.clone();
    j.expect_produce_send_notification_job()
        .returning(move |_, _| {
            notifications.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        });
    j
}

struct PausingCounter {
    inner: Arc<InMemoryTransactionCounter>,
    after_increment: bool,
    reached: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl TransactionCounterTrait for PausingCounter {
    async fn get(&self, r: &str, a: &str) -> Result<Option<u64>, RepositoryError> {
        self.inner.get(r, a).await
    }
    async fn get_and_increment(&self, r: &str, a: &str) -> Result<u64, RepositoryError> {
        if !self.after_increment {
            self.reached.notify_one();
            self.release.notified().await;
        }
        let n = self.inner.get_and_increment(r, a).await?;
        if self.after_increment {
            self.reached.notify_one();
            self.release.notified().await;
        }
        Ok(n)
    }
    async fn decrement(&self, r: &str, a: &str) -> Result<u64, RepositoryError> {
        self.inner.decrement(r, a).await
    }
    async fn set(&self, r: &str, a: &str, v: u64) -> Result<(), RepositoryError> {
        self.inner.set(r, a, v).await
    }
    async fn sync_floor(&self, r: &str, a: &str, f: u64) -> Result<u64, RepositoryError> {
        self.inner.sync_floor(r, a, f).await
    }
    async fn set_if_equals(
        &self,
        r: &str,
        a: &str,
        e: u64,
        v: u64,
    ) -> Result<bool, RepositoryError> {
        self.inner.set_if_equals(r, a, e, v).await
    }
    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        self.inner.drop_all_entries().await
    }
}

type Evm<TCR> = EvmRelayerTransaction<
    MockEvmProviderTrait,
    MockRelayerRepository,
    MockNetworkRepository,
    InMemoryTransactionRepository,
    MockJobProducerTrait,
    MockSigner,
    TCR,
    MockPriceCalculatorTrait,
>;

fn evm<TCR: TransactionCounterTrait + Send + Sync + 'static>(
    repo: Arc<InMemoryTransactionRepository>,
    counter: Arc<TCR>,
    provider: MockEvmProviderTrait,
    signer: MockSigner,
    jobs: &Jobs,
) -> Evm<TCR> {
    let mut network = MockNetworkRepository::new();
    network
        .expect_get_by_chain_id()
        .returning(|_, _| Ok(Some(create_test_network_model())));
    EvmRelayerTransaction::new(
        relayer(),
        provider,
        Arc::new(MockRelayerRepository::new()),
        Arc::new(network),
        repo,
        counter,
        Arc::new(job_producer(jobs)),
        price_calc(),
        signer,
    )
    .unwrap()
}

fn pair() -> (Arc<Notify>, Arc<Notify>) {
    (Arc::new(Notify::new()), Arc::new(Notify::new()))
}

async fn counter_value(c: &InMemoryTransactionCounter) -> u64 {
    c.get(RELAYER_ID, ADDR).await.unwrap().unwrap_or(0)
}

async fn stored(repo: &InMemoryTransactionRepository) -> (TransactionRepoModel, Option<u64>) {
    let tx = repo.get_by_id("tx-1".into()).await.unwrap();
    let nonce = tx.network_data.get_evm_transaction_data().unwrap().nonce;
    (tx, nonce)
}

async fn race_before_presign(
    repo: Arc<InMemoryTransactionRepository>,
    a_signs: Option<usize>,
) -> (
    Arc<InMemoryTransactionCounter>,
    Jobs,
    Vec<(&'static str, u64)>,
) {
    let counter = Arc::new(InMemoryTransactionCounter::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (a_reached, a_release) = pair();

    repo.create(pending_tx(None, Duration::seconds(30)))
        .await
        .unwrap();
    let a = evm(
        repo.clone(),
        counter.clone(),
        provider(Some((a_reached.clone(), a_release.clone()))),
        signer("A", signed.clone(), None, a_signs),
        &jobs,
    );
    let b = evm(
        repo.clone(),
        counter.clone(),
        provider(None),
        signer("B", signed.clone(), None, Some(1)),
        &jobs,
    );

    let snap_a = repo.get_by_id("tx-1".into()).await.unwrap();
    let (ra, rb) = tokio::join!(a.prepare_transaction(snap_a), async {
        a_reached.notified().await;
        let snap_b = repo.get_by_id("tx-1".into()).await.unwrap();
        let r = b.prepare_transaction(snap_b).await;
        a_release.notify_one();
        r
    });
    ra.unwrap();
    rb.unwrap();
    let s = signed.lock().unwrap().clone();
    (counter, jobs, s)
}

async fn race_after_presign(
    repo: Arc<InMemoryTransactionRepository>,
) -> (
    Arc<InMemoryTransactionCounter>,
    Jobs,
    Vec<(&'static str, u64)>,
) {
    let counter = Arc::new(InMemoryTransactionCounter::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (a_reached, a_release) = pair();

    repo.create(pending_tx(None, Duration::seconds(30)))
        .await
        .unwrap();
    let a = evm(
        repo.clone(),
        counter.clone(),
        provider(None),
        signer(
            "A",
            signed.clone(),
            Some((a_reached.clone(), a_release.clone())),
            Some(1),
        ),
        &jobs,
    );
    let b = evm(
        repo.clone(),
        counter.clone(),
        provider(None),
        signer("B", signed.clone(), None, Some(1)),
        &jobs,
    );

    let snap_a = repo.get_by_id("tx-1".into()).await.unwrap();
    let (ra, rb) = tokio::join!(a.prepare_transaction(snap_a), async {
        a_reached.notified().await;
        let snap_b = repo.get_by_id("tx-1".into()).await.unwrap();
        let r = b.prepare_transaction(snap_b).await;
        a_release.notify_one();
        r
    });
    assert!(ra.unwrap_err().is_concurrent_update_conflict());
    rb.unwrap();
    let s = signed.lock().unwrap().clone();
    (counter, jobs, s)
}

#[tokio::test]
async fn char_pending_requeues_prepare_every_cycle_between_20s_and_2min() {
    let jobs = Jobs::default();
    let mut mocks = default_test_mocks();
    mocks.job_producer = job_producer(&jobs);
    mocks
        .network_repo
        .expect_get_by_chain_id()
        .returning(|_, _| Ok(Some(create_test_network_model())));
    mocks.provider = block_ok();
    let tx_for_update = pending_tx(None, Duration::seconds(0));
    mocks
        .tx_repo
        .expect_partial_update_if_version()
        .returning(move |_, _, u| {
            let mut t = tx_for_update.clone();
            t.apply_partial_update(u);
            Ok(t)
        });
    let evm = make_test_evm_relayer_transaction(relayer(), mocks);

    // Before 20 s: no re-queue.
    let r = evm
        .handle_status_impl(pending_tx(None, Duration::seconds(12)), None)
        .await
        .unwrap();
    assert_eq!(r.status, TransactionStatus::Pending);
    assert_eq!(jobs.request.load(Ordering::SeqCst), 0);

    // Fastest cadence (8 s) from the first check after 20 s up to 2 min.
    let ages: Vec<i64> = (21..120).step_by(8).collect();
    for age in &ages {
        let r = evm
            .handle_status_impl(pending_tx(None, Duration::seconds(*age)), None)
            .await
            .unwrap();
        assert_eq!(r.status, TransactionStatus::Pending);
    }
    assert_eq!(ages.len(), 13);
    assert_eq!(jobs.request.load(Ordering::SeqCst), ages.len());

    // After 2 min: Failed, no more re-queues.
    let r = evm
        .handle_status_impl(pending_tx(None, Duration::seconds(125)), None)
        .await
        .unwrap();
    assert_eq!(r.status, TransactionStatus::Failed);
    assert_eq!(jobs.request.load(Ordering::SeqCst), ages.len());
}

#[tokio::test]
async fn duplicate_after_presign_reuses_nonce_with_one_winner() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let (counter, jobs, signed) = race_after_presign(repo.clone()).await;
    let (tx, nonce) = stored(&repo).await;

    assert_eq!(counter_value(&counter).await, 1, "one nonce allocated");
    assert_eq!(signed, vec![("A", 0), ("B", 0)], "both sign nonce 0");
    assert_eq!(
        jobs.submit.load(Ordering::SeqCst),
        1,
        "only winner queues submit"
    );
    assert_eq!(nonce, Some(0));
    assert_eq!(tx.status, TransactionStatus::Sent);
    assert_eq!(tx.hashes, vec!["0xB0".to_string()], "winner hash kept");
}

#[tokio::test]
async fn duplicate_before_presign_uses_fresh_record() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let (counter, jobs, signed) = race_before_presign(repo.clone(), Some(0)).await;
    let (tx, nonce) = stored(&repo).await;

    assert_eq!(counter_value(&counter).await, 1, "one nonce allocated");
    assert_eq!(signed, vec![("B", 0)]);
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 1);
    assert_eq!(nonce, Some(0), "record keeps nonce 0");
    assert_eq!(
        tx.hashes,
        vec!["0xB0".to_string()],
        "nonce 0 hash kept in history"
    );
}

#[tokio::test]
async fn pending_timeout_with_nonce_heals() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = pending_tx(Some(7), Duration::seconds(125));
    repo.create(tx.clone()).await.unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );

    let r = e.handle_pending_state(tx).await.unwrap();

    assert_eq!(r.status, TransactionStatus::Failed);
    assert_eq!(
        stored(&repo).await.1,
        Some(7),
        "Failed record keeps nonce 7"
    );
    assert_eq!(
        *jobs.health_hints.lock().unwrap(),
        vec![Some("7".to_string())]
    );
    assert_eq!(jobs.request.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn char_pending_gap_fill_noop_never_times_out() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let mut tx = pending_tx(Some(7), Duration::minutes(30));
    if let NetworkTransactionData::Evm(ref mut d) = tx.network_data {
        d.value = U256::from(0u64);
        d.data = Some("0x".into());
        d.to = Some(ADDR.into());
    }
    repo.create(tx.clone()).await.unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );

    let r = e.handle_pending_state(tx).await.unwrap();

    assert_eq!(
        r.status,
        TransactionStatus::Pending,
        "30 min old, still Pending"
    );
    assert_eq!(
        jobs.request.load(Ordering::SeqCst),
        1,
        "prepare re-queued again"
    );
}

#[tokio::test]
async fn prepare_failure_with_nonce_heals() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = pending_tx(Some(3), Duration::seconds(30));
    repo.create(tx.clone()).await.unwrap();
    let mut p = block_ok();
    p.expect_get_balance()
        .returning(|_| Box::pin(async { Ok(U256::from(0u64)) }));
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        p,
        MockSigner::new(),
        &jobs,
    );

    let r = e.prepare_transaction(tx).await.unwrap();

    assert_eq!(r.status, TransactionStatus::Failed);
    assert_eq!(stored(&repo).await.1, Some(3));
    assert_eq!(
        *jobs.health_hints.lock().unwrap(),
        vec![Some("3".to_string())]
    );
}

#[tokio::test]
async fn guarded_duplicate_before_presign_exits_on_fresh_read() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let (counter, jobs, signed) = race_before_presign(repo.clone(), Some(0)).await;
    let (tx, nonce) = stored(&repo).await;

    assert_eq!(counter_value(&counter).await, 1, "one nonce");
    assert_eq!(signed, vec![("B", 0)], "one signature");
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 1, "one submit job");
    assert_eq!(nonce, Some(0));
    assert_eq!(tx.hashes, vec!["0xB0".to_string()]);
    assert!(jobs.health_hints.lock().unwrap().is_empty());
}

#[tokio::test]
async fn guarded_duplicate_after_presign_one_payload() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let (counter, jobs, signed) = race_after_presign(repo.clone()).await;
    let (tx, nonce) = stored(&repo).await;

    assert_eq!(counter_value(&counter).await, 1);
    assert_eq!(signed, vec![("A", 0), ("B", 0)], "A's signature is wasted");
    assert_eq!(
        jobs.submit.load(Ordering::SeqCst),
        1,
        "only the winner submits"
    );
    assert_eq!(nonce, Some(0));
    assert_eq!(
        tx.hashes,
        vec!["0xB0".to_string()],
        "record matches the one submitted payload"
    );
}

#[tokio::test]
async fn guarded_race_in_alloc_window_loser_allocates_last() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let inner = Arc::new(InMemoryTransactionCounter::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (b_reached, b_release) = pair();
    repo.create(pending_tx(None, Duration::seconds(30)))
        .await
        .unwrap();

    let b_counter = Arc::new(PausingCounter {
        inner: inner.clone(),
        after_increment: false,
        reached: b_reached.clone(),
        release: b_release.clone(),
    });
    let a = evm(
        repo.clone(),
        inner.clone(),
        provider(None),
        signer("A", signed.clone(), None, Some(1)),
        &jobs,
    );
    let b = evm(
        repo.clone(),
        b_counter,
        provider(None),
        signer("B", signed.clone(), None, Some(0)),
        &jobs,
    );

    let snap = repo.get_by_id("tx-1".into()).await.unwrap();
    let snap2 = snap.clone();
    let (rb, ra) = tokio::join!(b.prepare_transaction(snap), async {
        b_reached.notified().await;
        let r = a.prepare_transaction(snap2).await;
        b_release.notify_one();
        r
    });
    ra.unwrap();
    assert!(rb.unwrap_err().is_concurrent_update_conflict());
    let (tx, nonce) = stored(&repo).await;

    assert_eq!(
        counter_value(&inner).await,
        2,
        "nonce 1 is healed asynchronously"
    );
    assert_eq!(*signed.lock().unwrap(), vec![("A", 0)]);
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 1);
    assert_eq!(nonce, Some(0));
    assert_eq!(tx.hashes, vec!["0xA0".to_string()]);
    assert_eq!(
        *jobs.health_hints.lock().unwrap(),
        vec![Some("1".to_string())]
    );
}

#[tokio::test]
async fn guarded_race_in_alloc_window_loser_allocates_first() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let inner = Arc::new(InMemoryTransactionCounter::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (b_reached, b_release) = pair();
    repo.create(pending_tx(None, Duration::seconds(30)))
        .await
        .unwrap();

    let b_counter = Arc::new(PausingCounter {
        inner: inner.clone(),
        after_increment: true,
        reached: b_reached.clone(),
        release: b_release.clone(),
    });
    let a = evm(
        repo.clone(),
        inner.clone(),
        provider(None),
        signer("A", signed.clone(), None, Some(1)),
        &jobs,
    );
    let b = evm(
        repo.clone(),
        b_counter,
        provider(None),
        signer("B", signed.clone(), None, Some(0)),
        &jobs,
    );

    let snap = repo.get_by_id("tx-1".into()).await.unwrap();
    let snap2 = snap.clone();
    let (rb, ra) = tokio::join!(b.prepare_transaction(snap), async {
        b_reached.notified().await;
        let r = a.prepare_transaction(snap2).await;
        b_release.notify_one();
        r
    });
    ra.unwrap();
    assert!(rb.unwrap_err().is_concurrent_update_conflict());
    let (_, nonce) = stored(&repo).await;

    assert_eq!(counter_value(&inner).await, 2, "nonce 0 leaked");
    assert_eq!(*signed.lock().unwrap(), vec![("A", 1)]);
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 1);
    assert_eq!(nonce, Some(1));
    assert_eq!(
        *jobs.health_hints.lock().unwrap(),
        vec![Some("0".to_string())],
        "gap fill for nonce 0 scheduled immediately"
    );
}

#[tokio::test]
async fn guarded_pending_timeout_with_nonce_schedules_gap_fill() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = pending_tx(Some(7), Duration::seconds(125));
    repo.create(tx.clone()).await.unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );

    let r = e.handle_pending_state(tx).await.unwrap();

    assert_eq!(r.status, TransactionStatus::Failed);
    assert_eq!(
        *jobs.health_hints.lock().unwrap(),
        vec![Some("7".to_string())]
    );
}

#[tokio::test]
async fn guarded_pending_timeout_without_nonce_schedules_nothing() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = pending_tx(None, Duration::seconds(125));
    repo.create(tx.clone()).await.unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );

    let r = e.handle_pending_state(tx).await.unwrap();

    assert_eq!(r.status, TransactionStatus::Failed);
    assert!(jobs.health_hints.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancel_pending_with_nonce_queues_heal() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = repo
        .create(pending_tx(Some(7), Duration::seconds(30)))
        .await
        .unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );
    let canceled = e.cancel_transaction(tx).await.unwrap();
    assert_eq!(canceled.status, TransactionStatus::Canceled);
    assert_eq!(*jobs.health_hints.lock().unwrap(), vec![Some("7".into())]);
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancel_during_prepare_stops_postsign() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let counter = Arc::new(InMemoryTransactionCounter::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (reached, release) = pair();
    let tx = repo
        .create(pending_tx(None, Duration::seconds(30)))
        .await
        .unwrap();
    let e = evm(
        repo.clone(),
        counter,
        provider(None),
        signer(
            "A",
            signed,
            Some((reached.clone(), release.clone())),
            Some(1),
        ),
        &jobs,
    );
    let (prepared, canceled) = tokio::join!(e.prepare_transaction(tx), async {
        reached.notified().await;
        let current = repo.get_by_id("tx-1".into()).await.unwrap();
        let canceled = e.cancel_transaction(current).await.unwrap();
        release.notify_one();
        canceled
    });
    assert!(prepared.unwrap_err().is_concurrent_update_conflict());
    assert_eq!(canceled.status, TransactionStatus::Canceled);
    assert_eq!(stored(&repo).await.0.version, 2);
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 0);
    assert_eq!(*jobs.health_hints.lock().unwrap(), vec![Some("0".into())]);
}

#[tokio::test]
async fn stale_cancel_stops_without_jobs_or_notifications() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = repo
        .create(pending_tx(Some(7), Duration::seconds(30)))
        .await
        .unwrap();
    repo.partial_update(
        tx.id.clone(),
        crate::models::TransactionUpdateRequest::default(),
    )
    .await
    .unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );
    assert!(e
        .cancel_transaction(tx)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert_eq!(stored(&repo).await.0.status, TransactionStatus::Pending);
    assert!(jobs.health_hints.lock().unwrap().is_empty());
    assert_eq!(jobs.notifications.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn circuit_breaker_pending_nonce_queues_heal() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let tx = repo
        .create(pending_tx(Some(42), Duration::seconds(30)))
        .await
        .unwrap();
    let e = evm(
        repo,
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );
    let ctx = crate::jobs::StatusCheckContext::default();
    let failed = e.handle_circuit_breaker_safely(tx, &ctx).await.unwrap();
    assert_eq!(failed.status, TransactionStatus::Failed);
    assert_eq!(*jobs.health_hints.lock().unwrap(), vec![Some("42".into())]);
}

#[tokio::test]
async fn stale_nonce_retry_metadata_stops_before_health_job() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let mut tx = pending_tx(Some(7), Duration::seconds(30));
    tx.metadata = Some(crate::models::TransactionMetadata {
        nonce_too_high_retries: 2,
        ..Default::default()
    });
    let tx = repo.create(tx).await.unwrap();
    repo.partial_update(
        tx.id.clone(),
        crate::models::TransactionUpdateRequest {
            metadata: Some(crate::models::TransactionMetadata::default()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );
    assert!(e
        .handle_nonce_too_high(&tx, "test")
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert_eq!(
        stored(&repo)
            .await
            .0
            .metadata
            .unwrap()
            .nonce_too_high_retries,
        0
    );
    assert!(jobs.health_hints.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancel_during_resubmit_preserves_cancel_patch() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (reached, release) = pair();
    let mut tx = pending_tx(Some(7), Duration::seconds(30));
    tx.status = TransactionStatus::Submitted;
    tx.sent_at = Some(Utc::now().to_rfc3339());
    let tx = repo.create(tx).await.unwrap();
    let mut p = provider(None);
    // PR 4 will move the version check before this send.
    p.expect_send_raw_transaction()
        .times(1)
        .returning(|_| Box::pin(async { Ok("0xA7".into()) }));
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        p,
        signer(
            "A",
            signed,
            Some((reached.clone(), release.clone())),
            Some(1),
        ),
        &jobs,
    );
    let (resubmitted, canceled) = tokio::join!(e.resubmit_transaction(tx), async {
        reached.notified().await;
        let current = repo.get_by_id("tx-1".into()).await.unwrap();
        let canceled = e.cancel_transaction(current).await.unwrap();
        release.notify_one();
        canceled
    });
    assert!(resubmitted.unwrap_err().is_concurrent_update_conflict());
    let stored = stored(&repo).await.0;
    assert_eq!(stored.is_canceled, Some(true));
    assert_eq!(stored.version, canceled.version);
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 1);
    assert_eq!(jobs.notifications.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancel_during_replace_stops_jobs_and_notifications() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let signed = Arc::new(Mutex::new(vec![]));
    let (reached, release) = pair();
    let mut tx = pending_tx(Some(7), Duration::seconds(30));
    tx.status = TransactionStatus::Submitted;
    tx.sent_at = Some(Utc::now().to_rfc3339());
    let tx = repo.create(tx).await.unwrap();
    let e = evm(
        repo.clone(),
        Arc::new(InMemoryTransactionCounter::new()),
        provider(None),
        signer(
            "A",
            signed,
            Some((reached.clone(), release.clone())),
            Some(1),
        ),
        &jobs,
    );
    let request =
        crate::models::NetworkTransactionRequest::Evm(crate::models::EvmTransactionRequest {
            to: Some("0xReplacement".into()),
            gas_limit: Some(21000),
            ..Default::default()
        });
    let (replaced, canceled) = tokio::join!(e.replace_transaction(tx, request), async {
        reached.notified().await;
        let current = repo.get_by_id("tx-1".into()).await.unwrap();
        let canceled = e.cancel_transaction(current).await.unwrap();
        release.notify_one();
        canceled
    });
    assert!(replaced.unwrap_err().is_concurrent_update_conflict());
    assert_eq!(stored(&repo).await.0.version, canceled.version);
    assert_eq!(stored(&repo).await.0.is_canceled, Some(true));
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 1);
    assert_eq!(jobs.notifications.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_noop_stops_without_notification() {
    let repo = Arc::new(InMemoryTransactionRepository::new());
    let jobs = Jobs::default();
    let mut tx = pending_tx(Some(7), Duration::seconds(30));
    tx.status = TransactionStatus::Sent;
    let tx = repo.create(tx).await.unwrap();
    repo.partial_update(
        tx.id.clone(),
        crate::models::TransactionUpdateRequest::default(),
    )
    .await
    .unwrap();
    let e = evm(
        repo,
        Arc::new(InMemoryTransactionCounter::new()),
        block_ok(),
        MockSigner::new(),
        &jobs,
    );
    assert!(e
        .process_noop_transaction(&tx, None)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert_eq!(jobs.submit.load(Ordering::SeqCst), 0);
    assert_eq!(jobs.notifications.load(Ordering::SeqCst), 0);
}
