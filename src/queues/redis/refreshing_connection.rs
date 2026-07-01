//! A `ConnectionLike` wrapper that gives Redis connections a bounded lifetime,
//! so they are periodically dropped and reopened.
//!
//! # Why this exists
//!
//! A long-lived Redis connection resolves the endpoint's DNS once, at connect
//! time, and then keeps talking to whatever address it first resolved. If the
//! DNS behind the endpoint later changes — due to failover, scaling, node
//! replacement, blue/green cutover, or maintenance — the connection stays
//! pinned to the OLD, now-stale node instead of following the endpoint to its
//! new target (e.g. after an ElastiCache failover repoints the endpoint to a
//! new node). A stale connection pinned to a demoted node keeps failing writes
//! as one symptom, but the underlying problem is the stale DNS resolution, not
//! any particular error. `ConnectionManager` only re-resolves DNS when it
//! reconnects, and it reconnects on a broken socket — NOT on a valid error
//! reply over a healthy socket, so it never notices the endpoint moved.
//!
//! `RefreshingConnection` fixes this by giving each connection a bounded
//! lifetime: once a connection exceeds a jittered max age it is dropped and
//! reopened before the next command, and a fresh connect re-resolves the
//! endpoint's DNS onto whatever node it currently points to. Recovery from any
//! DNS change is therefore bounded by the configured max age.
//!
//! # Shared healing state
//!
//! `RedisStorage` is cloned into every worker AND cloned per enqueue on the
//! producer path. All clones of a given queue's connection therefore share ONE
//! healing connection and ONE age clock via `Arc<Mutex<ConnState>>`. A rebuild
//! performed by any clone is immediately visible to all other clones. This is
//! what makes the fix correct in steady state: without shared state, the
//! template living in the `Queue` struct would have a `created_at` fixed at
//! setup that never advances, so every producer clone would be born
//! already-expired once uptime exceeds `max_age`, opening a brand-new connection
//! per enqueue forever.
//!
//! ## Locking discipline
//!
//! The `std::sync::Mutex` guard is NEVER held across an `.await`; it guards only
//! tiny synchronous critical sections (read/clone the current conn, swap in a
//! rebuilt conn). The actual command is delegated with no lock held, and the
//! inner `ConnectionManager` is a multiplexed connection, so concurrent
//! commands from different clones stay concurrent. Cloning `C` is cheap and
//! shares the underlying multiplexed pipe.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig};
use redis::{Client, Cmd, Pipeline, RedisFuture, RedisResult, Value};
use tracing::warn;

/// Per-connection jitter fraction applied to the base max age (±20%).
///
/// Spreads reconnects across connections and instances so they do not reconnect
/// in lockstep, which would otherwise cause synchronized load spikes on Redis.
const JITTER_FRACTION: f64 = 0.20;

/// Boxed future returned by the connection builder closure.
type BuildFuture<C> = Pin<Box<dyn Future<Output = RedisResult<C>> + Send>>;

/// Builds a fresh inner connection on demand. Cloneable so the wrapper can
/// rebuild repeatedly and so the whole wrapper can be `Clone`.
type Builder<C> = Arc<dyn Fn() -> BuildFuture<C> + Send + Sync>;

/// Shared, interior-mutable healing state for a `RefreshingConnection`.
///
/// One instance is shared (behind `Arc<Mutex<..>>`) by all clones of a given
/// queue's connection.
struct ConnState<C: ConnectionLike + Clone + Send> {
    /// The current inner connection all commands delegate to.
    conn: C,
    /// When the current `conn` was created.
    created_at: Instant,
    /// The un-jittered base max age. `None` disables age-based recycling.
    base_max_age: Option<Duration>,
    /// Effective max age for the current connection (`base_max_age` ± jitter).
    /// `None` disables age-based recycling. Re-drawn on each rebuild.
    max_age: Option<Duration>,
}

impl<C: ConnectionLike + Clone + Send> ConnState<C> {
    /// Whether the current connection has exceeded its (jittered) max age.
    fn is_expired(&self) -> bool {
        matches!(self.max_age, Some(age) if self.created_at.elapsed() >= age)
    }

    /// Installs a freshly built connection: swaps it in, resets the age clock,
    /// and re-draws the jittered max age around the fixed base.
    fn install(&mut self, new_conn: C) {
        self.conn = new_conn;
        self.created_at = Instant::now();
        self.max_age = self.base_max_age.map(jittered_max_age);
    }
}

/// A `ConnectionLike` wrapper that periodically rebuilds its inner connection
/// (bounded lifetime), sharing healing state across all clones.
///
/// Generic over the inner connection type so it can be unit-tested against a
/// fake `ConnectionLike`; in production `C = ConnectionManager`.
#[derive(Clone)]
pub struct RefreshingConnection<C: ConnectionLike + Clone + Send> {
    /// Builds a fresh inner connection (re-resolves DNS in production). Shared.
    builder: Builder<C>,
    /// Shared interior-mutable healing state. Cloning the wrapper clones this
    /// `Arc`, so all clones share one connection and one age clock.
    state: Arc<Mutex<ConnState<C>>>,
}

/// Returns a deterministic, per-instance jitter multiplier in
/// `[1 - JITTER_FRACTION, 1 + JITTER_FRACTION]`.
///
/// Each rebuild draws a fresh value so connections/instances do not reconnect
/// in lockstep.
fn jitter_multiplier() -> f64 {
    let r: f64 = rand::random::<f64>(); // [0, 1)
    1.0 - JITTER_FRACTION + r * (2.0 * JITTER_FRACTION)
}

/// Applies the jitter multiplier to a base max age.
fn jittered_max_age(base: Duration) -> Duration {
    base.mul_f64(jitter_multiplier())
}

impl<C: ConnectionLike + Clone + Send> RefreshingConnection<C> {
    /// Creates a wrapper from an already-built inner connection and a builder
    /// used to rebuild it later.
    ///
    /// `max_age_ms == 0` disables the age-based rebuild entirely. A non-zero
    /// value is jittered per rebuild.
    fn from_parts(builder: Builder<C>, conn: C, max_age_ms: u64) -> Self {
        let base_max_age = if max_age_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(max_age_ms))
        };
        let max_age = base_max_age.map(jittered_max_age);
        let state = ConnState {
            conn,
            created_at: Instant::now(),
            base_max_age,
            max_age,
        };
        Self {
            builder,
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Locks the state guard, recovering from poisoning (a panic while healing
    /// must not wedge the connection forever).
    fn lock(&self) -> std::sync::MutexGuard<'_, ConnState<C>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Resolves the connection to delegate to, performing a proactive age
    /// rebuild first if needed.
    ///
    /// Locking discipline: reads/swaps happen under brief synchronous locks;
    /// the `builder().await` runs with NO lock held.
    async fn conn_for_command(&self) -> C {
        // Brief lock: check expiry and clone out the current conn.
        let (expired, conn) = {
            let guard = self.lock();
            (guard.is_expired(), guard.conn.clone())
        };

        if !expired {
            return conn;
        }

        // Build a replacement OUTSIDE the lock.
        match (self.builder)().await {
            Ok(new_conn) => {
                let mut guard = self.lock();
                // Only install if still expired — another clone may have
                // rebuilt while we were building; if so, drop ours.
                if guard.is_expired() {
                    guard.install(new_conn);
                }
                guard.conn.clone()
            }
            Err(e) => {
                warn!(error = %e, "failed to rebuild Redis connection (age); keeping existing connection");
                // Keep whatever is current now.
                self.lock().conn.clone()
            }
        }
    }
}

impl RefreshingConnection<ConnectionManager> {
    /// Creates a `RefreshingConnection` wrapping a `ConnectionManager`.
    ///
    /// The builder recreates a `ConnectionManager` from `client` and
    /// `cm_config`; a fresh `ConnectionManager` re-resolves the host via DNS,
    /// which is how recreating a connection follows the CNAME onto the new
    /// writer after failover.
    pub fn new(
        client: Client,
        cm_config: ConnectionManagerConfig,
        inner: ConnectionManager,
        max_age_ms: u64,
    ) -> Self {
        let builder: Builder<ConnectionManager> = Arc::new(move || {
            let client = client.clone();
            let cm_config = cm_config.clone();
            Box::pin(async move { ConnectionManager::new_with_config(client, cm_config).await })
        });
        Self::from_parts(builder, inner, max_age_ms)
    }
}

impl<C: ConnectionLike + Clone + Send> ConnectionLike for RefreshingConnection<C> {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        Box::pin(async move {
            let mut conn = self.conn_for_command().await;
            // Delegate with NO lock held (preserves multiplexing/concurrency).
            conn.req_packed_command(cmd).await
        })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        Box::pin(async move {
            let mut conn = self.conn_for_command().await;
            // Delegate with NO lock held (preserves multiplexing/concurrency).
            conn.req_packed_commands(cmd, offset, count).await
        })
    }

    fn get_db(&self) -> i64 {
        // Brief lock to read the current connection's db.
        self.lock().conn.get_db()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// A fake `ConnectionLike` we fully control: every command succeeds and
    /// records how many commands this instance served.
    #[derive(Clone)]
    struct FakeConn {
        /// Unique id so tests can tell rebuilt connections apart.
        id: u64,
        /// Count of commands served by THIS connection instance.
        calls: Arc<AtomicUsize>,
    }

    impl FakeConn {
        fn new(id: u64) -> Self {
            Self {
                id,
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ConnectionLike for FakeConn {
        fn req_packed_command<'a>(&'a mut self, _cmd: &'a Cmd) -> RedisFuture<'a, Value> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Okay)
            })
        }

        fn req_packed_commands<'a>(
            &'a mut self,
            _cmd: &'a Pipeline,
            _offset: usize,
            _count: usize,
        ) -> RedisFuture<'a, Vec<Value>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![Value::Okay])
            })
        }

        fn get_db(&self) -> i64 {
            self.id as i64
        }
    }

    /// Builds a `RefreshingConnection<FakeConn>` whose builder hands out a new
    /// `FakeConn` (incrementing id) on each rebuild, and counts rebuilds.
    fn make_conn(max_age_ms: u64) -> (RefreshingConnection<FakeConn>, Arc<AtomicU64>) {
        let rebuild_count = Arc::new(AtomicU64::new(0));
        let next_id = Arc::new(AtomicU64::new(1));
        let rc = rebuild_count.clone();
        let ni = next_id.clone();

        let builder: Builder<FakeConn> = Arc::new(move || {
            let rc = rc.clone();
            let ni = ni.clone();
            Box::pin(async move {
                rc.fetch_add(1, Ordering::SeqCst);
                let id = ni.fetch_add(1, Ordering::SeqCst);
                Ok(FakeConn::new(id))
            })
        });

        let initial = FakeConn::new(next_id.fetch_add(1, Ordering::SeqCst));
        let conn = RefreshingConnection::from_parts(builder, initial, max_age_ms);
        (conn, rebuild_count)
    }

    /// Force the shared state to look expired regardless of jitter.
    fn force_expire(conn: &RefreshingConnection<FakeConn>) {
        let mut guard = conn.lock();
        guard.created_at = Instant::now() - Duration::from_secs(3600);
    }

    #[tokio::test]
    async fn test_age_triggered_rebuild() {
        // Tiny max age so the connection is immediately expired.
        let (mut conn, rebuilds) = make_conn(1);
        force_expire(&conn);
        assert!(conn.lock().is_expired(), "connection should be expired");

        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            1,
            "aged connection should rebuild once before delegating"
        );
    }

    #[tokio::test]
    async fn test_age_rebuild_disabled_when_zero() {
        // max_age_ms == 0 => age-based rebuild disabled.
        let (mut conn, rebuilds) = make_conn(0);
        assert!(
            conn.lock().max_age.is_none(),
            "max_age should be None when disabled"
        );
        // Even a very old created_at must not trigger a rebuild.
        force_expire(&conn);
        assert!(!conn.lock().is_expired());

        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            0,
            "age-based rebuild must be disabled when max_age_ms == 0"
        );
    }

    #[tokio::test]
    async fn test_get_db_delegates_to_inner() {
        let (conn, _) = make_conn(0);
        // The initial FakeConn is assigned id 1 (before any builder rebuild).
        assert_eq!(
            conn.get_db(),
            1,
            "get_db must delegate to the inner connection"
        );
    }

    /// Regression guard for the producer-path fix: cloning an already-expired
    /// wrapper and driving several clones must NOT rebuild once per clone.
    /// Because all clones share one `ConnState`, the proactive age rebuild
    /// happens at most once across all of them.
    #[tokio::test]
    async fn test_shared_state_rebuilds_at_most_once_across_clones() {
        let (base, rebuilds) = make_conn(60_000);
        force_expire(&base);
        assert!(base.lock().is_expired(), "base must start expired");

        // Simulate the producer path: clone the template per enqueue.
        let mut clone_a = base.clone();
        let mut clone_b = base.clone();

        let _ = clone_a.req_packed_command(&Cmd::new()).await;
        let _ = clone_b.req_packed_command(&Cmd::new()).await;

        // Shared state: the first command rebuilt and advanced the age clock;
        // the second clone sees a fresh, non-expired connection and does NOT
        // rebuild again.
        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            1,
            "shared state must rebuild at most once across clones, not once per clone"
        );
        assert!(
            !base.lock().is_expired(),
            "the age clock must have advanced (visible through all clones)"
        );
    }
}
