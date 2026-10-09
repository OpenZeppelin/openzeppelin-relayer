//! Idempotency-Key handling for transaction creation.
//!
//! A client may send an `Idempotency-Key` header with a create request. The
//! transaction id is chosen before the key is reserved and stored in the key
//! record, so a repeat request can always find the transaction it created.
use std::{
    future::Future,
    sync::{Arc, Once},
};

use actix_web::http::header::HeaderMap;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    config::ServerConfig,
    models::{ApiError, RelayerError, RepositoryError, TransactionRepoModel},
    repositories::{IdempotencyRecord, TransactionRepository},
};

/// Maximum accepted length of an idempotency key after trimming.
const IDEMPOTENCY_KEY_MAX_LEN: usize = 255;
/// Error message for an invalid `Idempotency-Key` header.
const INVALID_IDEMPOTENCY_KEY_MESSAGE: &str = "Invalid Idempotency-Key header";
/// Header name clients use to send the key.
const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";
/// Minimum time after reservation during which a missing transaction means
/// the original request is still being processed.
const MIN_IN_FLIGHT_WINDOW_SECONDS: i64 = 60;
/// Margin added to the request timeout so a request that is still running
/// is never mistaken for an abandoned one.
const IN_FLIGHT_WINDOW_MARGIN_SECONDS: i64 = 10;

const IN_PROGRESS_MESSAGE: &str = "A request with this Idempotency-Key is already in progress";
const PAYLOAD_MISMATCH_MESSAGE: &str = "Idempotency-Key reused with a different request payload";
const TRANSACTION_GONE_MESSAGE: &str =
    "No transaction found for this Idempotency-Key. It may have \
     completed and been removed from storage. Confirm whether the original request was \
     processed before you send it again with a new key";

/// Reads the optional `Idempotency-Key` header.
///
/// Returns `Ok(None)` when the header is absent, and `Bad Request` when it is
/// not valid visible ASCII.
pub fn idempotency_key_from_headers(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    match headers.get(IDEMPOTENCY_KEY_HEADER) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .map(|v| Some(v.to_string()))
            .map_err(|_| ApiError::BadRequest(INVALID_IDEMPOTENCY_KEY_MESSAGE.to_string())),
    }
}

/// Trims an idempotency key and checks its length.
///
/// Returns the normalized key, or a `Bad Request` error when the key is empty
/// or longer than [`IDEMPOTENCY_KEY_MAX_LEN`] characters.
pub fn normalize_idempotency_key(key: &str) -> Result<String, ApiError> {
    let trimmed = key.trim();
    if trimmed.is_empty() || trimmed.chars().count() > IDEMPOTENCY_KEY_MAX_LEN {
        return Err(ApiError::BadRequest(
            INVALID_IDEMPOTENCY_KEY_MESSAGE.to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Computes the lowercase hex SHA-256 fingerprint of a request body.
///
/// `serde_json` stores map keys in a `BTreeMap` (the `preserve_order` feature
/// is off), so `to_string` already produces canonical JSON with keys sorted
/// recursively.
pub fn transaction_request_fingerprint(request: &serde_json::Value) -> Result<String, ApiError> {
    let canonical = serde_json::to_string(request)
        .map_err(|e| ApiError::InternalError(format!("Failed to fingerprint request: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

/// Runs [`create_idempotent`] in a detached task and waits for its result.
///
/// The timeout middleware drops the handler future when a request times out.
/// The detached task keeps running, so the transaction is still stored and
/// its jobs are still queued. A retry with the same key then returns it.
pub async fn create_idempotent_detached<TR, F, Fut>(
    repo: Arc<TR>,
    relayer_id: String,
    key: String,
    fingerprint: String,
    ttl_seconds: u64,
    create: F,
) -> Result<TransactionRepoModel, ApiError>
where
    TR: TransactionRepository + Send + Sync + 'static,
    F: FnOnce(String) -> Fut + Send + 'static,
    Fut: Future<Output = Result<TransactionRepoModel, RelayerError>> + Send + 'static,
{
    // Plugin socket tasks run on the multi-thread runtime, not an Actix
    // LocalSet, so spawn_local panics there. tokio::spawn is Send-safe on
    // both that runtime and Actix's current-thread runtime.
    tokio::spawn(async move {
        create_idempotent(
            repo.as_ref(),
            &relayer_id,
            &key,
            fingerprint,
            ttl_seconds,
            create,
        )
        .await
    })
    .await
    .map_err(|e| ApiError::InternalError(format!("Idempotent create task failed: {e}")))?
}

/// Returns the TTL to use for a reservation.
///
/// A key must outlive the in-flight window. Otherwise it can expire while the
/// first request is still running, and a retry would create a second
/// transaction. A configured value below the window is raised to the window.
fn effective_ttl_seconds(configured_seconds: u64, window_seconds: i64) -> u64 {
    let window = u64::try_from(window_seconds).unwrap_or(0);
    if configured_seconds < window {
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            warn!(
                configured_seconds,
                window_seconds = window,
                "IDEMPOTENCY_KEY_TTL_SECONDS is below the in-flight window; using the window"
            );
        });
        return window;
    }
    configured_seconds
}

/// Creates a transaction at most once per relayer and idempotency key.
///
/// `create` receives the transaction id to use. Outcomes:
/// - New key: reserve it, then call `create`.
/// - Known key, same fingerprint, transaction stored: return that transaction.
/// - Known key, different fingerprint: `422`.
/// - Known key, transaction not stored yet: `409` inside the in-flight window,
///   `404` after it (the original request never stored it, or it was pruned).
///
/// When `create` fails, the key is released only if the transaction was not
/// stored. Otherwise a retry would create a second transaction.
pub async fn create_idempotent<TR, F, Fut>(
    repo: &TR,
    relayer_id: &str,
    key: &str,
    fingerprint: String,
    ttl_seconds: u64,
    create: F,
) -> Result<TransactionRepoModel, ApiError>
where
    TR: TransactionRepository + Sync + ?Sized,
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<TransactionRepoModel, RelayerError>>,
{
    let window = in_flight_window_seconds(ServerConfig::get_request_timeout_seconds());
    let ttl_seconds = effective_ttl_seconds(ttl_seconds, window);
    let record = IdempotencyRecord {
        fingerprint,
        tx_id: Uuid::new_v4().to_string(),
        created_at: Utc::now().to_rfc3339(),
    };

    if !repo
        .reserve_idempotency_key(relayer_id, key, &record, ttl_seconds)
        .await?
    {
        return replay(repo, relayer_id, key, &record.fingerprint, window).await;
    }

    match create(record.tx_id.clone()).await {
        Ok(transaction) => Ok(transaction),
        Err(e) => {
            match repo
                .release_idempotency_key_if_unused(relayer_id, key, &record.tx_id)
                .await
            {
                Ok(true) => debug!(relayer_id, "released idempotency key after create failure"),
                Ok(false) => info!(
                    relayer_id,
                    tx_id = %record.tx_id,
                    "kept idempotency key: transaction was stored before the failure"
                ),
                Err(release_error) => warn!(
                    relayer_id,
                    tx_id = %record.tx_id,
                    error = %release_error,
                    "failed to release idempotency key after create failure"
                ),
            }
            Err(e.into())
        }
    }
}

/// Resolves a repeated request for a key that is already reserved.
async fn replay<TR>(
    repo: &TR,
    relayer_id: &str,
    key: &str,
    fingerprint: &str,
    window_seconds: i64,
) -> Result<TransactionRepoModel, ApiError>
where
    TR: TransactionRepository + Sync + ?Sized,
{
    // The key expired between the failed reserve and this read. Ask the client
    // to retry; the retry reserves the key normally.
    let Some(existing) = repo.get_idempotency_record(relayer_id, key).await? else {
        return Err(ApiError::Conflict(IN_PROGRESS_MESSAGE.to_string()));
    };

    if existing.fingerprint != fingerprint {
        return Err(ApiError::UnprocessableEntity(
            PAYLOAD_MISMATCH_MESSAGE.to_string(),
        ));
    }

    // Read the primary: a lagging replica could hide a stored transaction and
    // turn a valid replay into a 404.
    match repo.get_by_id_on_primary(existing.tx_id.clone()).await {
        Ok(transaction) => {
            info!(
                relayer_id,
                tx_id = %transaction.id,
                "serving idempotent transaction replay"
            );
            Ok(transaction)
        }
        Err(RepositoryError::NotFound(_)) if is_in_flight(&existing.created_at, window_seconds) => {
            Err(ApiError::Conflict(IN_PROGRESS_MESSAGE.to_string()))
        }
        Err(RepositoryError::NotFound(_)) => {
            Err(ApiError::NotFound(TRANSACTION_GONE_MESSAGE.to_string()))
        }
        Err(e) => Err(e.into()),
    }
}

/// Returns the in-flight window for a request timeout.
///
/// The window must outlast the longest a request can run. Otherwise a retry
/// could get `404` while the original request is still creating the
/// transaction, and the client would send a duplicate under a new key.
fn in_flight_window_seconds(request_timeout_seconds: u64) -> i64 {
    let timeout = i64::try_from(request_timeout_seconds).unwrap_or(i64::MAX / 2);
    MIN_IN_FLIGHT_WINDOW_SECONDS.max(timeout.saturating_add(IN_FLIGHT_WINDOW_MARGIN_SECONDS))
}

/// Returns true while the reservation is younger than `window_seconds`.
/// An unparsable timestamp counts as in flight, the safe side.
fn is_in_flight(created_at: &str, window_seconds: i64) -> bool {
    DateTime::parse_from_rfc3339(created_at)
        .map(|t| (Utc::now() - t.with_timezone(&Utc)).num_seconds() < window_seconds)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        repositories::{InMemoryTransactionRepository, Repository},
        utils::mocks::mockutils::create_mock_transaction,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const RELAYER: &str = "relayer-1";
    const KEY: &str = "key-1";
    const TTL: u64 = 3600;

    /// Builds a `create` closure that stores a transaction under the given id
    /// and counts how often it runs.
    fn storing_create<'a>(
        repo: &'a InMemoryTransactionRepository,
        calls: &'a AtomicUsize,
    ) -> impl FnOnce(
        String,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<TransactionRepoModel, RelayerError>> + 'a>,
    > {
        move |tx_id| {
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let mut tx = create_mock_transaction();
                tx.id = tx_id;
                tx.relayer_id = RELAYER.to_string();
                repo.create(tx.clone())
                    .await
                    .map_err(|e| RelayerError::Internal(e.to_string()))?;
                Ok(tx)
            })
        }
    }

    async fn reserve_stale(repo: &InMemoryTransactionRepository, fingerprint: &str, age_secs: i64) {
        let record = IdempotencyRecord {
            fingerprint: fingerprint.to_string(),
            tx_id: "never-stored".to_string(),
            created_at: (Utc::now() - chrono::Duration::seconds(age_secs)).to_rfc3339(),
        };
        assert!(repo
            .reserve_idempotency_key(RELAYER, KEY, &record, TTL)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_replay_returns_same_transaction_without_second_create() {
        let repo = InMemoryTransactionRepository::new();
        let calls = AtomicUsize::new(0);

        let first = create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await
        .unwrap();
        let second = create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await
        .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_different_payload_returns_unprocessable_entity() {
        let repo = InMemoryTransactionRepository::new();
        let calls = AtomicUsize::new(0);

        create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp-a".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await
        .unwrap();
        let result = create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp-b".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await;

        assert!(matches!(result, Err(ApiError::UnprocessableEntity(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_reserved_key_without_transaction_returns_conflict_in_window() {
        let repo = InMemoryTransactionRepository::new();
        let calls = AtomicUsize::new(0);
        reserve_stale(&repo, "fp", 0).await;

        let result = create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await;

        assert!(matches!(result, Err(ApiError::Conflict(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_reserved_key_without_transaction_returns_not_found_after_window() {
        let repo = InMemoryTransactionRepository::new();
        let calls = AtomicUsize::new(0);
        reserve_stale(
            &repo,
            "fp",
            in_flight_window_seconds(ServerConfig::get_request_timeout_seconds()) + 1,
        )
        .await;

        let result = create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await;

        assert!(matches!(result, Err(ApiError::NotFound(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_failure_before_store_releases_key() {
        let repo = InMemoryTransactionRepository::new();
        let calls = AtomicUsize::new(0);

        let result = create_idempotent(&repo, RELAYER, KEY, "fp".into(), TTL, |_| async {
            Err(RelayerError::Internal("network not found".into()))
        })
        .await;
        assert!(result.is_err());

        // The retry with the same key creates the transaction.
        create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_failure_after_store_keeps_key_and_replays() {
        let repo = InMemoryTransactionRepository::new();
        let calls = AtomicUsize::new(0);

        // Stores the transaction, then fails, like a failed job push.
        let result = create_idempotent(&repo, RELAYER, KEY, "fp".into(), TTL, |tx_id| {
            let repo = &repo;
            async move {
                let mut tx = create_mock_transaction();
                tx.id = tx_id;
                tx.relayer_id = RELAYER.to_string();
                repo.create(tx).await.unwrap();
                Err(RelayerError::Internal("queue unavailable".into()))
            }
        })
        .await;
        assert!(result.is_err());

        let replay = create_idempotent(
            &repo,
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(repo.get_by_id(replay.id).await.is_ok());
    }

    #[actix_web::test]
    async fn test_detached_create_completes_after_caller_is_dropped() {
        let repo = Arc::new(InMemoryTransactionRepository::new());
        let gate = Arc::new(tokio::sync::Notify::new());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

        let create = {
            let repo = repo.clone();
            let gate = gate.clone();
            move |tx_id: String| async move {
                let _ = started_tx.send(());
                gate.notified().await;
                let mut tx = create_mock_transaction();
                tx.id = tx_id;
                tx.relayer_id = RELAYER.to_string();
                repo.create(tx.clone())
                    .await
                    .map_err(|e| RelayerError::Internal(e.to_string()))?;
                let _ = done_tx.send(());
                Ok(tx)
            }
        };

        {
            // Drop the caller while `create` is running, like a request timeout.
            let caller = create_idempotent_detached(
                repo.clone(),
                RELAYER.to_string(),
                KEY.to_string(),
                "fp".into(),
                TTL,
                create,
            );
            tokio::pin!(caller);
            tokio::select! {
                _ = &mut caller => panic!("create must still be waiting on the gate"),
                _ = started_rx => {}
            }
        }
        gate.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), done_rx)
            .await
            .expect("detached create did not store the transaction after the caller was dropped")
            .expect("detached create task ended before storing the transaction");

        let record = repo
            .get_idempotency_record(RELAYER, KEY)
            .await
            .unwrap()
            .unwrap();

        let calls = AtomicUsize::new(0);
        let replay = create_idempotent(
            repo.as_ref(),
            RELAYER,
            KEY,
            "fp".into(),
            TTL,
            storing_create(&repo, &calls),
        )
        .await
        .unwrap();
        assert_eq!(replay.id, record.tx_id);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_effective_ttl_is_at_least_the_window() {
        assert_eq!(effective_ttl_seconds(86400, 60), 86400);
        assert_eq!(effective_ttl_seconds(30, 60), 60);
        assert_eq!(effective_ttl_seconds(0, 130), 130);
    }

    #[test]
    fn test_in_flight_window_outlasts_request_timeout() {
        assert_eq!(in_flight_window_seconds(30), MIN_IN_FLIGHT_WINDOW_SECONDS);
        assert_eq!(in_flight_window_seconds(0), MIN_IN_FLIGHT_WINDOW_SECONDS);
        assert_eq!(in_flight_window_seconds(120), 130);
        assert!(in_flight_window_seconds(u64::MAX) > 0);
    }

    #[test]
    fn test_is_in_flight_uses_window() {
        let created = (Utc::now() - chrono::Duration::seconds(90)).to_rfc3339();
        assert!(!is_in_flight(&created, 60));
        assert!(is_in_flight(&created, 130));
        assert!(is_in_flight("not-a-timestamp", 60));
    }

    #[test]
    fn test_idempotency_key_from_headers() {
        use actix_web::http::header::{HeaderName, HeaderValue};

        let mut headers = HeaderMap::new();
        assert_eq!(idempotency_key_from_headers(&headers).unwrap(), None);

        headers.insert(
            HeaderName::from_static("idempotency-key"),
            HeaderValue::from_static("abc-123"),
        );
        assert_eq!(
            idempotency_key_from_headers(&headers).unwrap(),
            Some("abc-123".to_string())
        );

        headers.insert(
            HeaderName::from_static("idempotency-key"),
            HeaderValue::from_bytes(b"key-\xff").unwrap(),
        );
        assert!(matches!(
            idempotency_key_from_headers(&headers),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn test_normalize_idempotency_key_valid() {
        assert_eq!(normalize_idempotency_key("  abc-123  ").unwrap(), "abc-123");
        assert_eq!(
            normalize_idempotency_key(&"a".repeat(255)).unwrap().len(),
            255
        );
    }

    #[test]
    fn test_normalize_idempotency_key_rejects_empty_and_too_long() {
        for key in ["", "   ", &"a".repeat(256)] {
            assert!(matches!(
                normalize_idempotency_key(key),
                Err(ApiError::BadRequest(_))
            ));
        }
    }

    #[test]
    fn test_fingerprint_is_stable_across_key_ordering() {
        let a = serde_json::json!({"a": 1, "b": {"c": 2, "d": 3}});
        let b = serde_json::json!({"b": {"d": 3, "c": 2}, "a": 1});
        assert_eq!(
            transaction_request_fingerprint(&a).unwrap(),
            transaction_request_fingerprint(&b).unwrap()
        );
    }

    #[test]
    fn test_fingerprint_differs_for_different_bodies() {
        let a = serde_json::json!({"a": 1});
        let b = serde_json::json!({"a": 2});
        assert_ne!(
            transaction_request_fingerprint(&a).unwrap(),
            transaction_request_fingerprint(&b).unwrap()
        );
    }
}
