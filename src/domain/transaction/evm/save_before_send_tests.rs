//! Tests use the in-memory version check and inject faults at write boundaries.
use super::tests::{
    create_test_network_model, create_test_no_mempool_network_model, make_mock_receipt,
};
use crate::{
    domain::{
        transaction::{
            evm::{
                is_noop, is_unsent, too_many_attempts, EvmRelayerTransaction,
                MockPriceCalculatorTrait, PriceParams,
            },
            Transaction,
        },
        SignTransactionResponse, SignTransactionResponseEvm,
    },
    jobs::{MockJobProducerTrait, TransactionCommand},
    models::{
        evm::Speed, EvmTransactionData, EvmTransactionDataSignature, EvmTransactionRequest,
        NetworkTransactionData, NetworkTransactionRequest, NetworkType, RelayerRepoModel,
        RepositoryError, TransactionRepoModel, TransactionStatus, TransactionUpdateRequest, U256,
    },
    repositories::{
        InMemoryTransactionCounter, InMemoryTransactionRepository, MockNetworkRepository,
        MockRelayerRepository, MockTransactionRepository, Repository, TransactionRepository,
    },
    services::{
        provider::{MockEvmProviderTrait, ProviderError},
        signer::MockSigner,
    },
};
use chrono::{Duration, Utc};
use futures::executor::block_on;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

type Worker = EvmRelayerTransaction<
    MockEvmProviderTrait,
    MockRelayerRepository,
    MockNetworkRepository,
    MockTransactionRepository,
    MockJobProducerTrait,
    MockSigner,
    InMemoryTransactionCounter,
    MockPriceCalculatorTrait,
>;

#[derive(Default)]
struct Events {
    signs: Vec<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    jobs: Vec<TransactionCommand>,
    notifications: usize,
    health: usize,
    error: Option<&'static str>,
    fault: Option<(&'static str, bool)>,
    fail_enqueue: bool,
    same_signature: bool,
    receipt: bool,
    cancel_in_send: bool,
}

struct Harness {
    repo: Arc<InMemoryTransactionRepository>,
    events: Arc<Mutex<Events>>,
    worker: Worker,
}

fn tx(status: TransactionStatus) -> TransactionRepoModel {
    TransactionRepoModel {
        id: "tx".into(),
        relayer_id: "relayer".into(),
        status,
        created_at: (Utc::now() - Duration::hours(2)).to_rfc3339(),
        priced_at: Some((Utc::now() - Duration::hours(1)).to_rfc3339()),
        network_type: NetworkType::Evm,
        network_data: NetworkTransactionData::Evm(EvmTransactionData {
            chain_id: 1,
            from: "0x0000000000000000000000000000000000000001".into(),
            to: Some("0x0000000000000000000000000000000000000002".into()),
            value: U256::from(1),
            data: Some("0x".into()),
            gas_limit: Some(21000),
            gas_price: Some(20_000_000_000),
            nonce: Some(7),
            hash: Some("A".into()),
            raw: Some(vec![0]),
            speed: Some(Speed::Fast),
            ..Default::default()
        }),
        hashes: vec!["A".into()],
        ..Default::default()
    }
}

fn price() -> PriceParams {
    PriceParams {
        gas_price: Some(30_000_000_000),
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        is_min_bumped: Some(true),
        extra_fee: None,
        total_cost: U256::from(1),
    }
}

impl Harness {
    async fn new(
        initial: TransactionRepoModel,
        rollup: bool,
        pause_sign: Option<(Arc<Notify>, Arc<Notify>)>,
    ) -> Self {
        let repo = Arc::new(InMemoryTransactionRepository::new());
        repo.create(initial).await.unwrap();
        let events = Arc::new(Mutex::new(Events::default()));
        let mut mock_repo = MockTransactionRepository::new();
        let r = repo.clone();
        let e = events.clone();
        mock_repo
            .expect_partial_update_if_version()
            .returning(move |id, version, update| {
                let kind = if update.is_canceled == Some(true) {
                    "cancel"
                } else if update.hashes.is_some() {
                    "save"
                } else if update.sent_at.is_some() {
                    "post"
                } else if update.status == Some(TransactionStatus::Submitted) {
                    "release"
                } else {
                    "other"
                };
                let fault = {
                    let mut events = e.lock().unwrap();
                    if events.fault.is_some_and(|(action, _)| action == kind) {
                        events.fault.take()
                    } else {
                        None
                    }
                };
                if let Some((_, lost_reply)) = fault {
                    if lost_reply {
                        block_on(r.partial_update_if_version(id.clone(), version, update))?;
                    }
                    return Err(RepositoryError::ConcurrentUpdateConflict(id));
                }
                block_on(r.partial_update_if_version(id, version, update))
            });
        let r = repo.clone();
        mock_repo
            .expect_partial_update()
            .returning(move |id, update| block_on(r.partial_update(id, update)));
        let r = repo.clone();
        mock_repo
            .expect_get_by_id_on_primary()
            .returning(move |id| block_on(r.get_by_id_on_primary(id)));
        let mut provider = MockEvmProviderTrait::new();
        provider
            .expect_get_balance()
            .returning(|_| Box::pin(async { Ok(U256::from(10u128.pow(19))) }));
        provider.expect_get_block_by_number().returning(|| {
            Box::pin(async {
                let mut b: alloy::rpc::types::Block = alloy::rpc::types::Block::default();
                b.header.gas_limit = 30_000_000;
                Ok(alloy::network::AnyRpcBlock::from(b))
            })
        });
        provider
            .expect_get_transaction_count()
            .returning(|_| Box::pin(async { Ok(7) }));
        let e = events.clone();
        provider
            .expect_get_transaction_receipt()
            .returning(move |_| {
                let receipt = e.lock().unwrap().receipt;
                Box::pin(async move { Ok(receipt.then(|| make_mock_receipt(true, Some(100)))) })
            });
        provider
            .expect_get_block_number()
            .returning(|| Box::pin(async { Ok(120) }));
        let r = repo.clone();
        let e = events.clone();
        provider
            .expect_send_raw_transaction()
            .returning(move |raw| {
                let r = r.clone();
                let e = e.clone();
                let raw = raw.to_vec();
                Box::pin(async move {
                    let stored = r.get_by_id("tx".into()).await.unwrap();
                    // These checks fail if either guarded write moves after the send.
                    assert_eq!(
                        stored.status,
                        TransactionStatus::Submitted,
                        "release must precede send"
                    );
                    let data = stored.network_data.get_evm_transaction_data().unwrap();
                    assert_eq!(data.raw.as_ref(), Some(&raw), "save must precede send");
                    assert!(stored.hashes.contains(data.hash.as_ref().unwrap()));
                    assert!(stored.priced_at.is_some());
                    let (error, cancel) = {
                        let mut events = e.lock().unwrap();
                        events.sent.push(raw);
                        (events.error, events.cancel_in_send)
                    };
                    if cancel {
                        let mut data = data;
                        data.raw = None;
                        data.signature = None;
                        data.to = Some(data.from.clone());
                        data.value = U256::from(0);
                        data.data = Some("0x".into());
                        r.partial_update_if_version(
                            stored.id,
                            stored.version,
                            TransactionUpdateRequest {
                                network_data: Some(NetworkTransactionData::Evm(data)),
                                is_canceled: Some(true),
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap();
                        e.lock().unwrap().cancel_in_send = false;
                    }
                    match error {
                        Some(message) => Err(ProviderError::Other(message.into())),
                        None => Ok("hash".into()),
                    }
                })
            });
        let mut signer = MockSigner::new();
        let e = events.clone();
        signer.expect_sign_transaction().returning(move |_| {
            let raw = {
                let mut events = e.lock().unwrap();
                let raw = vec![if events.same_signature {
                    0
                } else {
                    (events.signs.len() + 1) as u8
                }];
                events.signs.push(raw.clone());
                raw
            };
            let pause = if raw == vec![1] {
                pause_sign.clone()
            } else {
                None
            };
            Box::pin(async move {
                if let Some((reached, release)) = pause {
                    reached.notify_one();
                    release.notified().await;
                }
                Ok(SignTransactionResponse::Evm(SignTransactionResponseEvm {
                    hash: if raw == vec![0] {
                        "A".into()
                    } else {
                        format!("H{}", raw[0])
                    },
                    raw,
                    signature: EvmTransactionDataSignature {
                        r: "r".into(),
                        s: "s".into(),
                        v: 1,
                        sig: "sig".into(),
                    },
                }))
            })
        });
        let mut jobs = MockJobProducerTrait::new();
        let e = events.clone();
        jobs.expect_produce_submit_transaction_job()
            .returning(move |job, _| {
                let fail = {
                    let mut events = e.lock().unwrap();
                    if !events.fail_enqueue {
                        events.jobs.push(job.command);
                    }
                    events.fail_enqueue
                };
                Box::pin(async move {
                    if fail {
                        Err(crate::jobs::JobProducerError::QueueError(
                            "enqueue failed".into(),
                        ))
                    } else {
                        Ok(())
                    }
                })
            });
        let e = events.clone();
        jobs.expect_produce_send_notification_job()
            .returning(move |_, _| {
                e.lock().unwrap().notifications += 1;
                Box::pin(async { Ok(()) })
            });
        let e = events.clone();
        jobs.expect_produce_relayer_health_check_job()
            .returning(move |_, _| {
                e.lock().unwrap().health += 1;
                Box::pin(async { Ok(()) })
            });
        jobs.expect_produce_check_transaction_status_job()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        let mut network = MockNetworkRepository::new();
        let model = move || {
            let mut model = if rollup {
                create_test_no_mempool_network_model()
            } else {
                create_test_network_model()
            };
            if rollup {
                if let crate::models::NetworkConfigData::Evm(ref mut config) = model.config {
                    config
                        .common
                        .tags
                        .as_mut()
                        .unwrap()
                        .push(crate::constants::ARBITRUM_BASED_TAG.into());
                }
            }
            model
        };
        network
            .expect_get_by_chain_id()
            .returning(move |_, _| Ok(Some(model())));
        network
            .expect_get_by_name()
            .returning(move |_, _| Ok(Some(model())));
        let mut calc = MockPriceCalculatorTrait::new();
        calc.expect_calculate_bumped_gas_price()
            .returning(|_, _, _| Box::pin(async { Ok(price()) }));
        calc.expect_get_transaction_price_params()
            .returning(|_, _| Box::pin(async { Ok(price()) }));
        let worker = Worker::new(
            RelayerRepoModel {
                id: "relayer".into(),
                address: "0x0000000000000000000000000000000000000001".into(),
                network: "mainnet".into(),
                notification_id: Some("notification".into()),
                ..Default::default()
            },
            provider,
            Arc::new(MockRelayerRepository::new()),
            Arc::new(network),
            Arc::new(mock_repo),
            Arc::new(InMemoryTransactionCounter::new()),
            Arc::new(jobs),
            calc,
            signer,
        )
        .unwrap();
        Self {
            repo,
            events,
            worker,
        }
    }
    async fn stored(&self) -> TransactionRepoModel {
        self.repo.get_by_id("tx".into()).await.unwrap()
    }
    async fn patch(&self, update: TransactionUpdateRequest) -> TransactionRepoModel {
        let t = self.stored().await;
        self.repo
            .partial_update_if_version(t.id, t.version, update)
            .await
            .unwrap()
    }
    async fn age_payload(&self, seconds: i64) {
        let before = self.stored().await;
        let old_price =
            chrono::DateTime::parse_from_rfc3339(before.priced_at.as_ref().unwrap()).unwrap();
        let new_price = Utc::now() - Duration::seconds(seconds);
        let shift = new_price.signed_duration_since(old_price);
        let sent_at = before.sent_at.map(|time| {
            (chrono::DateTime::parse_from_rfc3339(&time).unwrap() + shift).to_rfc3339()
        });
        self.patch(TransactionUpdateRequest {
            priced_at: Some(new_price.to_rfc3339()),
            sent_at,
            ..Default::default()
        })
        .await;
    }
    async fn cycle(&self) {
        self.events.lock().unwrap().jobs.clear();
        self.worker
            .handle_transaction_status(self.stored().await, None)
            .await
            .unwrap();
    }
    async fn run_job(&self, job: TransactionCommand) {
        let tx = self.stored().await;
        match job {
            TransactionCommand::Submit | TransactionCommand::Resend => {
                self.worker.submit_transaction(tx).await.unwrap();
            }
            TransactionCommand::Resubmit => {
                self.worker.resubmit_transaction(tx).await.unwrap();
            }
            _ => panic!("unexpected job"),
        }
    }
    // Item 14: cross both boundaries again, then check all persistent payload fields.
    async fn recover(&self, needs_sign: bool) {
        let before = self.stored().await;
        self.events.lock().unwrap().error = None;
        self.age_payload(30).await;
        self.cycle().await;
        let jobs = self.events.lock().unwrap().jobs.clone();
        assert_eq!(jobs.len(), 1);
        assert_eq!(matches!(jobs[0], TransactionCommand::Resubmit), needs_sign);
        self.run_job(jobs[0].clone()).await;
        let after = self.stored().await;
        let data = after.network_data.get_evm_transaction_data().unwrap();
        assert!(after.sent_at.is_some());
        assert!(after.priced_at.is_some());
        if !needs_sign {
            assert_eq!(after.hashes, before.hashes);
            assert_eq!(
                data.raw,
                before.network_data.get_evm_transaction_data().unwrap().raw
            );
        }
        assert!(after.hashes.contains(data.hash.as_ref().unwrap()));
        assert_eq!(self.events.lock().unwrap().sent.last(), data.raw.as_ref());
    }
}

#[tokio::test]
async fn resubmit_saves_before_send_and_uses_returned_version() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    let result = h
        .worker
        .resubmit_transaction(h.stored().await)
        .await
        .unwrap();
    assert_eq!(result.hashes, ["A", "H1"]);
    assert!(result.sent_at.is_some());
    assert_eq!(result.version, 2);
    assert_eq!(result.metadata.unwrap().accepted_sends, 1);
}

#[tokio::test]
async fn submit_releases_sent_and_submitted_before_send() {
    for status in [TransactionStatus::Sent, TransactionStatus::Submitted] {
        let h = Harness::new(tx(status), false, None).await;
        let result = h.worker.submit_transaction(h.stored().await).await.unwrap();
        assert_eq!(result.version, 2);
        assert!(result.sent_at.is_some());
        assert!(h.events.lock().unwrap().signs.is_empty());
    }
}

#[tokio::test]
async fn save_conflict_sends_and_notifies_nothing_then_recovers() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.events.lock().unwrap().fault = Some(("save", false));
    assert!(h
        .worker
        .resubmit_transaction(h.stored().await)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert!(h.events.lock().unwrap().sent.is_empty());
    assert_eq!(h.events.lock().unwrap().notifications, 0);
    // The old bytes exist. The next cycle sends them without a new signature.
    h.recover(false).await;
}

#[tokio::test]
async fn failed_send_keeps_saved_hash_and_resends_without_signing() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.events.lock().unwrap().error = Some("transport error");
    assert!(h
        .worker
        .resubmit_transaction(h.stored().await)
        .await
        .is_err());
    let stored = h.stored().await;
    assert_eq!(stored.hashes, ["A", "H1"]);
    assert!(stored.sent_at.is_none());
    assert!(stored.priced_at.is_some());
    let same = h.worker.resubmit_transaction(stored).await.unwrap();
    assert_eq!(same.hashes, ["A", "H1"]);
    assert_eq!(h.events.lock().unwrap().signs.len(), 1);
    h.recover(false).await;
    assert_eq!(h.events.lock().unwrap().signs.len(), 1);
}

#[tokio::test]
async fn duplicate_resubmit_jobs_sign_once_per_timer() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.worker
        .resubmit_transaction(h.stored().await)
        .await
        .unwrap();
    h.worker
        .resubmit_transaction(h.stored().await)
        .await
        .unwrap();
    assert_eq!(h.events.lock().unwrap().signs.len(), 1);
    assert_eq!(h.stored().await.hashes, ["A", "H1"]);
}

#[tokio::test]
async fn accepted_a_failed_b_uses_unsent_stamp_and_reprice_has_priority() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.worker.submit_transaction(h.stored().await).await.unwrap();
    h.age_payload(10000).await;
    h.events.lock().unwrap().error = Some("transport");
    assert!(h
        .worker
        .resubmit_transaction(h.stored().await)
        .await
        .is_err());
    assert!(is_unsent(&h.stored().await).unwrap());
    h.recover(false).await;
    // Both the resend age and price timer are due. Only a reprice is queued.
    h.age_payload(10000).await;
    h.cycle().await;
    assert!(matches!(
        h.events.lock().unwrap().jobs.as_slice(),
        [TransactionCommand::Resubmit]
    ));
    h.run_job(TransactionCommand::Resubmit).await;
}

#[tokio::test]
async fn no_raw_submit_is_a_noop_and_resubmit_signs_at_once() {
    let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
    let mut data = h
        .stored()
        .await
        .network_data
        .get_evm_transaction_data()
        .unwrap();
    data.raw = None;
    h.patch(TransactionUpdateRequest {
        network_data: Some(NetworkTransactionData::Evm(data)),
        priced_at: Some(Utc::now().to_rfc3339()),
        ..Default::default()
    })
    .await;
    h.worker.submit_transaction(h.stored().await).await.unwrap();
    assert!(h.events.lock().unwrap().sent.is_empty());
    h.cycle().await;
    assert!(matches!(
        h.events.lock().unwrap().jobs.as_slice(),
        [TransactionCommand::Resubmit]
    ));
    h.run_job(TransactionCommand::Resubmit).await;
    assert_eq!(h.events.lock().unwrap().signs.len(), 1);
}

#[tokio::test]
async fn cancel_failed_enqueue_and_lost_cancel_reply_recover_without_wait() {
    for lost_reply in [false, true] {
        let mut initial = tx(TransactionStatus::Sent);
        initial.created_at = Utc::now().to_rfc3339();
        let h = Harness::new(initial, false, None).await;
        h.events.lock().unwrap().fail_enqueue = !lost_reply;
        if lost_reply {
            h.events.lock().unwrap().fault = Some(("cancel", true));
        }
        let result = h.worker.cancel_transaction(h.stored().await).await;
        if lost_reply {
            assert!(result.unwrap_err().is_concurrent_update_conflict());
        }
        let t = h.stored().await;
        let d = t.network_data.get_evm_transaction_data().unwrap();
        assert!(is_noop(&d));
        assert!(d.raw.is_none());
        assert!(d.signature.is_none());
        assert_eq!(d.hash.as_deref(), Some("A"));
        h.events.lock().unwrap().fail_enqueue = false;
        h.cycle().await;
        assert!(matches!(
            h.events.lock().unwrap().jobs.as_slice(),
            [TransactionCommand::Resubmit]
        ));
        h.run_job(TransactionCommand::Resubmit).await;
        assert_eq!(h.events.lock().unwrap().sent, [vec![1]]);
        assert_eq!(h.stored().await.hashes, ["A", "H1"]);
        assert!(h.stored().await.sent_at.is_some());
        assert!(h.stored().await.priced_at.is_some());
    }
}

#[tokio::test]
async fn stale_submit_after_clean_cancel_sends_only_noop() {
    let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
    let stale = h.stored().await;
    h.worker.cancel_transaction(stale.clone()).await.unwrap();
    assert!(h
        .worker
        .submit_transaction(stale)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert!(h.events.lock().unwrap().sent.is_empty());
    h.recover(true).await;
    for command in [
        TransactionCommand::Submit,
        TransactionCommand::Resend,
        TransactionCommand::Resubmit,
    ] {
        h.run_job(command).await;
    }
    assert!(h
        .events
        .lock()
        .unwrap()
        .sent
        .iter()
        .all(|raw| raw == &vec![1]));
    assert!(is_noop(
        &h.stored()
            .await
            .network_data
            .get_evm_transaction_data()
            .unwrap()
    ));
}

#[tokio::test]
async fn release_conflict_and_lost_release_reply_recover_stored_bytes() {
    for lost_reply in [false, true] {
        let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
        h.events.lock().unwrap().fault = Some(("release", lost_reply));
        assert!(h
            .worker
            .submit_transaction(h.stored().await)
            .await
            .unwrap_err()
            .is_concurrent_update_conflict());
        assert!(h.events.lock().unwrap().sent.is_empty());
        h.recover(false).await;
        assert!(h.events.lock().unwrap().signs.is_empty());
    }
}

#[tokio::test]
async fn lost_save_reply_recovers_without_new_signature() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.events.lock().unwrap().fault = Some(("save", true));
    assert!(h
        .worker
        .resubmit_transaction(h.stored().await)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert!(h.events.lock().unwrap().sent.is_empty());
    assert_eq!(h.stored().await.hashes, ["A", "H1"]);
    h.recover(false).await;
    assert_eq!(h.events.lock().unwrap().signs.len(), 1);
}

#[tokio::test]
async fn cancel_during_send_makes_post_conflict_and_status_repairs_it() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.events.lock().unwrap().cancel_in_send = true;
    assert!(h
        .worker
        .resubmit_transaction(h.stored().await)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert_eq!(h.stored().await.hashes, ["A", "H1"]);
    assert!(h.stored().await.sent_at.is_none());
    h.recover(true).await;
    assert!(is_noop(
        &h.stored()
            .await
            .network_data
            .get_evm_transaction_data()
            .unwrap()
    ));
}

#[tokio::test]
async fn nonce_too_high_counts_after_release_and_save_and_queues_health() {
    for reprice in [false, true] {
        let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
        h.events.lock().unwrap().error = Some("nonce too high");
        for _ in 0..3 {
            h.age_payload(10000).await;
            if reprice {
                h.worker
                    .resubmit_transaction(h.stored().await)
                    .await
                    .unwrap();
            } else {
                h.worker.submit_transaction(h.stored().await).await.unwrap();
            }
        }
        assert_eq!(h.stored().await.metadata.unwrap().nonce_too_high_retries, 3);
        assert_eq!(h.events.lock().unwrap().health, 1);
        h.recover(false).await;
        assert_eq!(h.stored().await.metadata.unwrap().nonce_too_high_retries, 0);
    }
}

#[tokio::test]
async fn nonce_too_low_leaves_submitted_and_recovers_on_next_cycle() {
    for reprice in [false, true] {
        let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
        h.events.lock().unwrap().error = Some("nonce too low");
        if reprice {
            h.worker
                .resubmit_transaction(h.stored().await)
                .await
                .unwrap();
        } else {
            h.worker.submit_transaction(h.stored().await).await.unwrap();
        }
        assert_eq!(h.stored().await.status, TransactionStatus::Submitted);
        assert!(h.stored().await.sent_at.is_none());
        h.recover(false).await;
    }
}

#[tokio::test]
async fn timer_readers_accept_missing_sent_at_and_sent_status() {
    let mut t = tx(TransactionStatus::Submitted);
    t.hashes.push("B".into());
    let h = Harness::new(t, false, None).await;
    h.age_payload(30).await;
    h.cycle().await;
    h.run_job(TransactionCommand::Submit).await;
    h.patch(TransactionUpdateRequest {
        status: Some(TransactionStatus::Sent),
        ..Default::default()
    })
    .await;
    assert!(!h.worker.should_resubmit(&h.stored().await).await.unwrap());
    h.age_payload(10000).await;
    assert!(h.worker.should_resubmit(&h.stored().await).await.unwrap());
}

#[tokio::test]
async fn sixty_failed_saves_and_one_accepted_send_do_not_trip_valve() {
    let h = Harness::new(tx(TransactionStatus::Submitted), true, None).await;
    h.worker.submit_transaction(h.stored().await).await.unwrap();
    for _ in 0..60 {
        h.age_payload(30).await;
        h.events.lock().unwrap().error = Some("transport");
        assert!(h
            .worker
            .resubmit_transaction(h.stored().await)
            .await
            .is_err());
        // Cross both boundaries after every failure. Arbitrum's timer is 20 s,
        // so a fresh status cycle has no job; the duplicate worker must do nothing.
        h.cycle().await;
        h.worker
            .resubmit_transaction(h.stored().await)
            .await
            .unwrap();
        let t = h.stored().await;
        assert!(t.sent_at.is_some());
        assert!(t.priced_at.is_some());
        assert_eq!(t.hashes.len(), h.events.lock().unwrap().signs.len() + 1);
        assert!(!too_many_attempts(&t));
    }
    assert_eq!(h.stored().await.metadata.unwrap().accepted_sends, 1);
    h.events.lock().unwrap().error = None;
    h.age_payload(30).await;
    h.cycle().await;
    h.run_job(TransactionCommand::Resubmit).await;
    assert_eq!(h.stored().await.metadata.unwrap().accepted_sends, 2);
}

#[tokio::test]
async fn accepted_count_grows_only_on_ok_and_hashes_stay_unique() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.events.lock().unwrap().same_signature = true;
    for message in [
        None,
        Some("already known"),
        Some("replacement transaction underpriced"),
    ] {
        h.age_payload(10000).await;
        h.events.lock().unwrap().error = message;
        h.worker
            .resubmit_transaction(h.stored().await)
            .await
            .unwrap();
        assert_eq!(h.stored().await.hashes, ["A"]);
        assert!(h.stored().await.sent_at.is_some());
        assert_eq!(h.stored().await.metadata.unwrap().accepted_sends, 1);
        h.cycle().await;
        h.run_job(TransactionCommand::Resubmit).await;
        assert_eq!(h.stored().await.hashes, ["A"]);
        assert!(h.stored().await.sent_at.is_some());
        assert!(h.stored().await.priced_at.is_some());
    }
}

fn replacement() -> NetworkTransactionRequest {
    NetworkTransactionRequest::Evm(EvmTransactionRequest {
        to: Some("0x0000000000000000000000000000000000000003".into()),
        value: U256::from(2),
        gas_limit: Some(21000),
        gas_price: Some(40_000_000_000),
        ..Default::default()
    })
}

#[tokio::test]
async fn replace_saves_hash_keeps_status_and_queues_submit() {
    for status in [
        TransactionStatus::Pending,
        TransactionStatus::Sent,
        TransactionStatus::Submitted,
    ] {
        let h = Harness::new(tx(status.clone()), false, None).await;
        let replaced = h
            .worker
            .replace_transaction(h.stored().await, replacement())
            .await
            .unwrap();
        assert_eq!(replaced.status, status);
        assert_eq!(replaced.hashes, ["A", "H1"]);
        assert!(h.events.lock().unwrap().sent.is_empty());
        assert!(matches!(
            h.events.lock().unwrap().jobs.as_slice(),
            [TransactionCommand::Submit]
        ));
        if status != TransactionStatus::Pending {
            h.run_job(TransactionCommand::Submit).await;
            assert_eq!(h.events.lock().unwrap().signs.len(), 1);
            assert_eq!(h.events.lock().unwrap().sent, [vec![1]]);
        }
    }
}

#[tokio::test]
async fn canceled_submitted_transaction_ends_by_original_receipt() {
    let h = Harness::new(tx(TransactionStatus::Submitted), false, None).await;
    h.worker.cancel_transaction(h.stored().await).await.unwrap();
    h.events.lock().unwrap().receipt = true;
    h.cycle().await;
    assert_eq!(h.stored().await.status, TransactionStatus::Confirmed);
}

#[tokio::test]
async fn sent_retries_stored_bytes_before_timer_and_noop_queues_reprice() {
    let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
    h.age_payload(30).await;
    h.cycle().await;
    assert!(matches!(
        h.events.lock().unwrap().jobs.as_slice(),
        [TransactionCommand::Submit]
    ));
    h.run_job(TransactionCommand::Submit).await;
    assert!(h.events.lock().unwrap().signs.is_empty());
    let mut initial = tx(TransactionStatus::Sent);
    initial.valid_until = Some((Utc::now() - Duration::seconds(1)).to_rfc3339());
    let h = Harness::new(initial, false, None).await;
    h.cycle().await;
    assert!(matches!(
        h.events.lock().unwrap().jobs.as_slice(),
        [TransactionCommand::Resubmit]
    ));
    h.run_job(TransactionCommand::Resubmit).await;
    assert!(is_noop(
        &h.stored()
            .await
            .network_data
            .get_evm_transaction_data()
            .unwrap()
    ));
}

#[tokio::test]
async fn cancel_while_signing_prevents_save_and_broadcast() {
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let h = Harness::new(
        tx(TransactionStatus::Sent),
        false,
        Some((reached.clone(), release.clone())),
    )
    .await;
    let snapshot = h.stored().await;
    let (resubmit, ()) = tokio::join!(h.worker.resubmit_transaction(snapshot), async {
        reached.notified().await;
        h.worker.cancel_transaction(h.stored().await).await.unwrap();
        release.notify_one();
    });
    assert!(resubmit.unwrap_err().is_concurrent_update_conflict());
    assert!(h.events.lock().unwrap().sent.is_empty());
    h.recover(true).await;
    assert_eq!(h.events.lock().unwrap().sent, [vec![2]]);
}

#[test]
fn backoff_caps_at_sixteen_intervals_and_metadata_defaults_to_zero() {
    assert_eq!(
        crate::utils::get_resubmit_timeout_with_backoff(1000, usize::MAX),
        16000
    );
    let metadata: crate::models::TransactionMetadata = serde_json::from_str("{}").unwrap();
    assert_eq!(metadata.accepted_sends, 0);
}

#[tokio::test]
async fn prepare_uses_saved_payload_and_sent_status() {
    let mut initial = tx(TransactionStatus::Pending);
    initial.hashes.clear();
    let mut data = initial.network_data.get_evm_transaction_data().unwrap();
    data.hash = None;
    data.raw = None;
    initial.network_data = NetworkTransactionData::Evm(data);
    let h = Harness::new(initial, false, None).await;
    let prepared = h
        .worker
        .prepare_transaction(h.stored().await)
        .await
        .unwrap();
    assert_eq!(prepared.status, TransactionStatus::Sent);
    assert_eq!(prepared.hashes, ["H1"]);
    assert!(prepared.priced_at.is_some());
    assert!(h.events.lock().unwrap().sent.is_empty());
    h.run_job(TransactionCommand::Submit).await;
    assert_eq!(h.events.lock().unwrap().sent, [vec![1]]);
}

#[tokio::test]
async fn lost_post_reply_resends_bytes_and_preserves_saved_hashes() {
    let h = Harness::new(tx(TransactionStatus::Sent), false, None).await;
    h.events.lock().unwrap().fault = Some(("post", false));
    assert!(h
        .worker
        .submit_transaction(h.stored().await)
        .await
        .unwrap_err()
        .is_concurrent_update_conflict());
    assert_eq!(h.events.lock().unwrap().sent, [vec![0]]);
    assert!(h.stored().await.sent_at.is_none());
    h.recover(false).await;
    assert!(h.events.lock().unwrap().signs.is_empty());
}

#[tokio::test]
async fn pending_cancel_blocks_all_queued_workers() {
    let h = Harness::new(tx(TransactionStatus::Pending), false, None).await;
    let old = h.stored().await;
    h.worker.cancel_transaction(old.clone()).await.unwrap();
    h.cycle().await;
    h.run_job(TransactionCommand::Submit).await;
    h.run_job(TransactionCommand::Resubmit).await;
    assert_eq!(h.stored().await.status, TransactionStatus::Canceled);
    assert!(h.events.lock().unwrap().sent.is_empty());
    assert!(h.events.lock().unwrap().signs.is_empty());
}

#[test]
fn unsent_and_timer_fallbacks_cover_old_records() {
    let mut t = tx(TransactionStatus::Submitted);
    assert!(is_unsent(&t).unwrap());
    t.sent_at = Some(Utc::now().to_rfc3339());
    assert!(!is_unsent(&t).unwrap());
    t.priced_at = None;
    assert!(!is_unsent(&t).unwrap());
    assert!(
        crate::domain::transaction::evm::get_age_since_priced(&t).unwrap() < Duration::seconds(1)
    );
    t.sent_at = None;
    assert!(
        crate::domain::transaction::evm::get_age_since_priced(&t).unwrap() > Duration::hours(1)
    );
}
