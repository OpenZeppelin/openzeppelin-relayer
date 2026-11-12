//! Run against disposable Redis: cargo test --release nonce_atomic -- --ignored.
use openzeppelin_relayer::repositories::{RedisTransactionCounter, TransactionCounterTrait};
use openzeppelin_relayer::{
    models::{NetworkTransactionData, TransactionRepoModel, TransactionStatus},
    utils::RedisConnections,
};
use redis::AsyncCommands;
use std::sync::Arc;

async fn fixture() -> (RedisTransactionCounter, TransactionRepoModel) {
    let cfg = deadpool_redis::Config::from_url(
        std::env::var("REDIS_URL").expect("task-owned REDIS_URL required"),
    );
    let pool = Arc::new(
        cfg.builder()
            .unwrap()
            .max_size(8)
            .runtime(deadpool_redis::Runtime::Tokio1)
            .build()
            .unwrap(),
    );
    let repo = RedisTransactionCounter::new(
        Arc::new(RedisConnections::new_single_pool(pool)),
        format!("nonce-atomic-{}", uuid::Uuid::new_v4()),
    )
    .unwrap();
    let mut tx: TransactionRepoModel = serde_json::from_value(serde_json::json!({
      "id": uuid::Uuid::new_v4().to_string(), "relayer_id": "nonce-atomic-test-relayer", "status":"pending",
      "created_at":"2026-11-12T00:00:00Z", "network_type": openzeppelin_relayer::models::NetworkType::Evm,
      "network_data":{"network_data":"Evm","data":{
        "from":"0x1111111111111111111111111111111111111111", "to":"0x2222222222222222222222222222222222222222",
        "gas_limit":21000, "nonce":null, "value":"0x123456789abcdef123456789abcdef", "data":"0x", "chain_id":1
      }}, "hashes":[], "is_canceled":false
    })).unwrap();
    let NetworkTransactionData::Evm(ref mut evm) = tx.network_data else {
        unreachable!()
    };
    evm.nonce = None;
    seed(&repo, &tx).await;
    (repo, tx)
}
fn payer(tx: &TransactionRepoModel) -> String {
    tx.network_data.get_evm_transaction_data().unwrap().from
}
fn body(repo: &RedisTransactionCounter, tx: &TransactionRepoModel) -> String {
    format!("{}:relayer:{}:tx:{}", repo.key_prefix, tx.relayer_id, tx.id)
}
fn lookup(repo: &RedisTransactionCounter, tx: &TransactionRepoModel) -> String {
    format!("{}:relayer:tx_to_relayer:{}", repo.key_prefix, tx.id)
}
fn count(repo: &RedisTransactionCounter, tx: &TransactionRepoModel) -> String {
    format!(
        "{}:transaction_counter:{}:{}",
        repo.key_prefix,
        tx.relayer_id,
        payer(tx)
    )
}
fn index(repo: &RedisTransactionCounter, tx: &TransactionRepoModel, n: u64) -> String {
    format!("{}:relayer:{}:nonce:{}", repo.key_prefix, tx.relayer_id, n)
}
async fn seed(repo: &RedisTransactionCounter, tx: &TransactionRepoModel) {
    let mut conn = repo.connections.primary().get().await.unwrap();
    let _: () = redis::pipe()
        .atomic()
        .set(lookup(repo, tx), &tx.relayer_id)
        .set(body(repo, tx), serde_json::to_string(tx).unwrap())
        .query_async(&mut *conn)
        .await
        .unwrap();
}
async fn reserve(
    repo: &RedisTransactionCounter,
    tx: &TransactionRepoModel,
) -> TransactionRepoModel {
    repo.reserve_evm_nonce(&tx.relayer_id, &payer(tx), tx)
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_same_transaction_retries_and_next_request() {
    let (repo, tx) = fixture().await;
    let (a, b) = tokio::join!(reserve(&repo, &tx), reserve(&repo, &tx));
    for v in [a, b, reserve(&repo, &tx).await] {
        assert_eq!(
            v.network_data.get_evm_transaction_data().unwrap().nonce,
            Some(0)
        );
    }
    let mut next = tx.clone();
    next.id = uuid::Uuid::new_v4().to_string();
    seed(&repo, &next).await;
    assert_eq!(
        reserve(&repo, &next)
            .await
            .network_data
            .get_evm_transaction_data()
            .unwrap()
            .nonce,
        Some(1)
    );
    let mut c = repo.connections.primary().get().await.unwrap();
    assert_eq!(c.get::<_, String>(count(&repo, &tx)).await.unwrap(), "2");
    assert_eq!(
        c.get::<_, String>(index(&repo, &tx, 0)).await.unwrap(),
        tx.id
    );
    assert_eq!(
        c.get::<_, String>(index(&repo, &tx, 1)).await.unwrap(),
        next.id
    );
}
#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_distinct_transactions_get_distinct_nonces() {
    let (repo, tx) = fixture().await;
    let mut next = tx.clone();
    next.id = uuid::Uuid::new_v4().to_string();
    seed(&repo, &next).await;
    let (a, b) = tokio::join!(reserve(&repo, &tx), reserve(&repo, &next));
    let mut nonces = vec![
        a.network_data
            .get_evm_transaction_data()
            .unwrap()
            .nonce
            .unwrap(),
        b.network_data
            .get_evm_transaction_data()
            .unwrap()
            .nonce
            .unwrap(),
    ];
    nonces.sort();
    assert_eq!(nonces, vec![0, 1]);
}
#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_preserves_large_exact_integers_and_arrays() {
    let (repo, mut tx) = fixture().await;
    let NetworkTransactionData::Evm(ref mut evm) = tx.network_data else {
        unreachable!()
    };
    evm.gas_limit = Some(9007199254740993);
    seed(&repo, &tx).await;
    repo.set(&tx.relayer_id, &payer(&tx), 9007199254740993)
        .await
        .unwrap();
    let assigned = reserve(&repo, &tx).await;
    let data = assigned.network_data.get_evm_transaction_data().unwrap();
    assert_eq!(data.nonce, Some(9007199254740993));
    assert_eq!(data.gas_limit, Some(9007199254740993));
    assert!(assigned.hashes.is_empty());
}
#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_refuses_bad_counter_without_writes() {
    for value in ["garbage", "-1", "1.5", "+1", "01", "18446744073709551615"] {
        let (repo, tx) = fixture().await;
        let mut conn = repo.connections.primary().get().await.unwrap();
        let raw: String = conn.get(body(&repo, &tx)).await.unwrap();
        let _: () = conn.set(count(&repo, &tx), value).await.unwrap();
        assert!(repo
            .reserve_evm_nonce(&tx.relayer_id, &payer(&tx), &tx)
            .await
            .is_err());
        assert_eq!(
            conn.get::<_, String>(count(&repo, &tx)).await.unwrap(),
            value
        );
        assert_eq!(conn.get::<_, String>(body(&repo, &tx)).await.unwrap(), raw);
        assert!(!conn.exists::<_, bool>(index(&repo, &tx, 0)).await.unwrap());
    }
}
#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_refuses_scope_status_and_missing_data() {
    for fault in [
        "relayer",
        "payer",
        "canceled",
        "sent",
        "final",
        "missing",
        "malformed",
        "lookup",
        "occupied",
    ] {
        let (repo, mut tx) = fixture().await;
        let mut conn = repo.connections.primary().get().await.unwrap();
        let address = payer(&tx);
        match fault {
            "relayer" => tx.relayer_id = "other".into(),
            "canceled" => tx.is_canceled = Some(true),
            "sent" => tx.status = TransactionStatus::Sent,
            "final" => tx.status = TransactionStatus::Confirmed,
            _ => {}
        }
        if matches!(fault, "relayer" | "canceled" | "sent" | "final") {
            seed(&repo, &tx).await;
        }
        match fault {
            "missing" => {
                let _: () = conn.del(body(&repo, &tx)).await.unwrap();
            }
            "malformed" => {
                let _: () = conn.set(body(&repo, &tx), "{").await.unwrap();
            }
            "lookup" => {
                let _: () = conn.set(lookup(&repo, &tx), "different").await.unwrap();
            }
            "occupied" => {
                let _: () = conn
                    .set(index(&repo, &tx, 0), "different-transaction")
                    .await
                    .unwrap();
            }
            _ => {}
        }
        let relayer = if fault == "relayer" {
            "incorrect"
        } else {
            &tx.relayer_id
        };
        let from = if fault == "payer" {
            "0xwrong"
        } else {
            &address
        };
        let raw: Option<String> = conn.get(body(&repo, &tx)).await.unwrap();
        assert!(
            repo.reserve_evm_nonce(relayer, from, &tx).await.is_err(),
            "{fault}"
        );
        assert_eq!(
            conn.get::<_, Option<String>>(body(&repo, &tx))
                .await
                .unwrap(),
            raw,
            "{fault}"
        );
        assert!(
            !conn.exists::<_, bool>(count(&repo, &tx)).await.unwrap(),
            "{fault}"
        );
    }
}
#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_cas_rejects_intervening_body_counter_or_lookup_changes() {
    for fault in ["body", "counter", "lookup"] {
        let (repo, tx) = fixture().await;
        let mut conn = repo.connections.primary().get().await.unwrap();
        let raw: String = conn.get(body(&repo, &tx)).await.unwrap();
        match fault {
            "body" => {
                let mut updated = tx.clone();
                updated.status = TransactionStatus::Canceled;
                seed(&repo, &updated).await;
            }
            "counter" => {
                let _: () = conn.set(count(&repo, &tx), "5").await.unwrap();
            }
            "lookup" => {
                let _: () = conn.set(lookup(&repo, &tx), "other").await.unwrap();
            }
            _ => unreachable!(),
        }
        let actual: Option<String> = conn.get(body(&repo, &tx)).await.unwrap();
        let counter: Option<String> = conn.get(count(&repo, &tx)).await.unwrap();
        let applied: bool = redis::Script::new(include_str!(
            "../src/repositories/transaction_counter/reserve_evm_nonce.lua"
        ))
        .key(count(&repo, &tx))
        .key(body(&repo, &tx))
        .key(lookup(&repo, &tx))
        .key(index(&repo, &tx, 0))
        .arg(&tx.relayer_id)
        .arg(raw)
        .arg("")
        .arg("1")
        .arg("must-not-write")
        .arg(&tx.id)
        .invoke_async(&mut *conn)
        .await
        .unwrap();
        assert!(!applied);
        assert_eq!(
            conn.get::<_, Option<String>>(body(&repo, &tx))
                .await
                .unwrap(),
            actual
        );
        assert_eq!(
            conn.get::<_, Option<String>>(count(&repo, &tx))
                .await
                .unwrap(),
            counter
        );
        assert!(!conn.exists::<_, bool>(index(&repo, &tx, 0)).await.unwrap());
    }
}

#[tokio::test]
#[ignore = "Requires task-owned Redis"]
async fn nonce_atomic_rejects_payload_change_during_preparation() {
    let (repo, tx) = fixture().await;
    let mut changed = tx.clone();
    let NetworkTransactionData::Evm(ref mut data) = changed.network_data else {
        unreachable!()
    };
    data.to = Some("0x3333333333333333333333333333333333333333".into());
    seed(&repo, &changed).await;
    assert!(repo
        .reserve_evm_nonce(&tx.relayer_id, &payer(&tx), &tx)
        .await
        .is_err());
    let mut c = repo.connections.primary().get().await.unwrap();
    assert!(!c.exists::<_, bool>(count(&repo, &tx)).await.unwrap());
    assert_eq!(
        serde_json::from_str::<TransactionRepoModel>(
            &c.get::<_, String>(body(&repo, &tx)).await.unwrap()
        )
        .unwrap()
        .network_data
        .get_evm_transaction_data()
        .unwrap()
        .to,
        changed.network_data.get_evm_transaction_data().unwrap().to
    );
}
