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
//! # Reactive rebuild (accelerator)
//!
//! Age-based recycling is the guaranteed backstop, but recovery is only as fast
//! as the configured max age. On top of it, connections are ALSO rebuilt
//! reactively when a command reply indicates the connection is pinned to a
//! read-only node — the tell-tale symptom of talking to a stale/demoted node
//! after an endpoint change. When such a reply is observed the wrapper rebuilds
//! the connection immediately (rate-limited by [`MIN_RECONNECT_GAP`]), so the
//! very next command re-resolves DNS onto the current node instead of waiting
//! out the age window. This detection intentionally covers BOTH shapes Redis
//! uses for a write-on-replica failure: a top-level `Err(ReadOnly)` (direct
//! write commands) AND an inline `Value::ServerError` embedded in an otherwise
//! `Ok(..)` reply (apalis runs queue ops as Lua `EVALSHA` scripts, and the
//! read-only failure is returned inside the script reply, not as a top-level
//! error). Reactive rebuild is an accelerator only; it does not replace the
//! age-based backstop, and it is not keyed on any specific error string beyond
//! the read-only marker.
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

/// Minimum gap between two reactive (read-only-triggered) rebuilds.
///
/// Rate-limits the reactive path so a burst of read-only replies (which all
/// arrive before a single rebuild can take effect) triggers at most one
/// rebuild per window instead of a reconnect storm.
const MIN_RECONNECT_GAP: Duration = Duration::from_secs(3);

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
    conn: C,
    created_at: Instant,
    /// Un-jittered base; `None` disables age-based recycling.
    base_max_age: Option<Duration>,
    /// `base_max_age` ± jitter, re-drawn on each rebuild.
    max_age: Option<Duration>,
    /// Timestamp of the last reactive rebuild; gates the rate limit.
    last_reconnect: Option<Instant>,
}

impl<C: ConnectionLike + Clone + Send> ConnState<C> {
    /// Whether the current connection has exceeded its (jittered) max age.
    fn is_expired(&self) -> bool {
        matches!(self.max_age, Some(age) if self.created_at.elapsed() >= age)
    }

    /// Whether a reactive rebuild is allowed right now under the rate limit:
    /// true if none has happened yet or at least `min_gap` has elapsed since
    /// the last one.
    fn reconnect_allowed(&self, min_gap: Duration) -> bool {
        match self.last_reconnect {
            None => true,
            Some(at) => at.elapsed() >= min_gap,
        }
    }

    /// Installs a freshly built connection: swaps it in, resets the age clock,
    /// and re-draws the jittered max age around the fixed base.
    fn install(&mut self, new_conn: C) {
        self.conn = new_conn;
        self.created_at = Instant::now();
        self.max_age = self.base_max_age.map(jittered_max_age);
    }

    /// Claims the age-rebuild window: pushes the age clock back so the
    /// connection only re-expires after `gap`. Concurrent clones then see a
    /// non-expired connection (one build in flight, no thundering herd), and a
    /// failed build cannot retry faster than the gap. A successful build
    /// resets the clock fully via `install`.
    fn defer_expiry(&mut self, gap: Duration) {
        if let Some(age) = self.max_age {
            self.created_at = Instant::now() - age.saturating_sub(gap);
        }
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
    min_reconnect_gap: Duration,
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
            last_reconnect: None,
        };
        Self {
            builder,
            state: Arc::new(Mutex::new(state)),
            min_reconnect_gap: MIN_RECONNECT_GAP,
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
    /// The rebuild window is claimed under the first lock BEFORE building
    /// (via `defer_expiry`) — success or failure — so only one clone builds at
    /// a time and a failed build retries no faster than `min_reconnect_gap`.
    /// The `builder().await` runs with NO lock held.
    async fn conn_for_command(&self) -> C {
        let stale_conn = {
            let mut guard = self.lock();
            if !guard.is_expired() {
                return guard.conn.clone();
            }
            guard.defer_expiry(self.min_reconnect_gap);
            guard.conn.clone()
        };

        match (self.builder)().await {
            Ok(new_conn) => {
                let mut guard = self.lock();
                guard.install(new_conn);
                guard.conn.clone()
            }
            Err(e) => {
                warn!(error = %e, "failed to rebuild Redis connection (age); keeping existing connection");
                stale_conn
            }
        }
    }

    /// Reactively rebuilds the inner connection in response to a read-only
    /// reply, rate-limited by `min_reconnect_gap`.
    ///
    /// The rate-limit window is claimed under the first lock BEFORE building —
    /// success or failure — so concurrent clones short-circuit at the gate
    /// (one build in flight, no thundering herd) and a failed rebuild retries
    /// no faster than the gap. The `builder().await` runs with NO lock held.
    async fn handle_readonly(&self) {
        {
            let mut guard = self.lock();
            if !guard.reconnect_allowed(self.min_reconnect_gap) {
                return;
            }
            guard.last_reconnect = Some(Instant::now());
        }

        match (self.builder)().await {
            Ok(new_conn) => self.lock().install(new_conn),
            Err(e) => {
                warn!(error = %e, "failed to rebuild Redis connection (read-only); keeping existing connection");
            }
        }
    }
}

/// Returns true if a reply indicates the connection is pinned to a read-only
/// node, recursing into aggregate replies so nested and pipeline replies
/// (e.g. Lua `EVALSHA` script results) are covered.
///
/// Redis surfaces a write-on-replica failure as an inline `Value::ServerError`
/// carrying the `READONLY` code, which can appear at the top level or nested
/// inside an aggregate — redis-rs only lifts it to a top-level `Err` one layer
/// above this wrapper, so we must inspect the `Value` shape directly.
fn is_readonly_value(v: &Value) -> bool {
    match v {
        Value::ServerError(e) => e.code() == "READONLY",
        Value::Array(items) | Value::Set(items) => items.iter().any(is_readonly_value),
        Value::Map(pairs) => pairs
            .iter()
            .any(|(k, val)| is_readonly_value(k) || is_readonly_value(val)),
        Value::Push { data, .. } => data.iter().any(is_readonly_value),
        Value::Attribute { data, .. } => is_readonly_value(data),
        _ => false,
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
            let result = conn.req_packed_command(cmd).await;
            // Reactive rebuild on a read-only reply, in either shape: a
            // top-level `Err(ReadOnly)` (direct writes) or an inline
            // `Value::ServerError` embedded in an `Ok(..)` reply (Lua scripts).
            if matches!(&result, Err(e) if e.kind() == redis::ErrorKind::ReadOnly)
                || matches!(&result, Ok(v) if is_readonly_value(v))
            {
                self.handle_readonly().await;
            }
            result
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
            let result = conn.req_packed_commands(cmd, offset, count).await;
            // Reactive rebuild on a read-only reply, in either shape: a
            // top-level `Err(ReadOnly)` or an inline `Value::ServerError`
            // embedded in any element of the `Ok(..)` pipeline reply.
            if matches!(&result, Err(e) if e.kind() == redis::ErrorKind::ReadOnly)
                || matches!(&result, Ok(vs) if vs.iter().any(is_readonly_value))
            {
                self.handle_readonly().await;
            }
            result
        })
    }

    fn get_db(&self) -> i64 {
        self.lock().conn.get_db()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// How a `FakeConn` should reply to commands, so tests can reproduce the
    /// exact production shapes of a read-only failure.
    #[derive(Clone, Copy)]
    enum Reply {
        /// Normal success (`Value::Okay` / `[Value::Okay]`).
        Ok,
        /// Inline read-only `ServerError` inside an otherwise-`Ok(..)` reply —
        /// the apalis Lua-script shape that a naive top-level `Err` guard
        /// missed. THE regression case.
        InlineReadonly,
        /// Inline read-only nested one aggregate level deeper
        /// (`Ok(Array[Array[ServerError]])`) — exercises the recursion arm.
        NestedInlineReadonly,
        /// Top-level `Err(ReadOnly)` — the shape direct write commands take.
        ErrReadonly,
    }

    /// Parses a RESP `-READONLY ...` error line into the `Value::ServerError`
    /// that redis-rs produces in production, WITHOUT naming redis's private
    /// `ServerError`/`ServerErrorKind` types. This is exactly the inline error
    /// value that appears embedded in an otherwise-`Ok(..)` script reply.
    fn readonly_value() -> Value {
        redis::parse_redis_value(b"-READONLY You can't write against a read only replica.\r\n")
            .expect("parse READONLY server error")
    }

    /// A top-level `Err` whose kind is `ReadOnly`, as a direct write would
    /// surface it one layer above this wrapper.
    fn readonly_err() -> redis::RedisError {
        redis::RedisError::from((redis::ErrorKind::ReadOnly, "read only replica"))
    }

    /// A fake `ConnectionLike` we fully control: it replies according to its
    /// `reply` mode and records how many commands this instance served.
    #[derive(Clone)]
    struct FakeConn {
        /// Unique id so tests can tell rebuilt connections apart.
        id: u64,
        /// Count of commands served by THIS connection instance.
        calls: Arc<AtomicUsize>,
        /// How this connection replies to commands.
        reply: Reply,
    }

    impl FakeConn {
        fn new(id: u64) -> Self {
            Self::with_reply(id, Reply::Ok)
        }

        fn with_reply(id: u64, reply: Reply) -> Self {
            Self {
                id,
                calls: Arc::new(AtomicUsize::new(0)),
                reply,
            }
        }
    }

    impl ConnectionLike for FakeConn {
        fn req_packed_command<'a>(&'a mut self, _cmd: &'a Cmd) -> RedisFuture<'a, Value> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                match self.reply {
                    Reply::Ok => Ok(Value::Okay),
                    Reply::InlineReadonly => Ok(readonly_value()),
                    Reply::NestedInlineReadonly => {
                        Ok(Value::Array(vec![Value::Array(vec![readonly_value()])]))
                    }
                    Reply::ErrReadonly => Err(readonly_err()),
                }
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
                match self.reply {
                    Reply::Ok => Ok(vec![Value::Okay]),
                    Reply::InlineReadonly => Ok(vec![readonly_value()]),
                    Reply::NestedInlineReadonly => {
                        Ok(vec![Value::Array(vec![Value::Array(vec![
                            readonly_value(),
                        ])])])
                    }
                    Reply::ErrReadonly => Err(readonly_err()),
                }
            })
        }

        fn get_db(&self) -> i64 {
            self.id as i64
        }
    }

    /// Builds a `RefreshingConnection<FakeConn>` whose builder hands out a new
    /// (healthy, `Reply::Ok`) `FakeConn` with incrementing id on each rebuild,
    /// and counts rebuilds. The initial connection is also healthy.
    fn make_conn(max_age_ms: u64) -> (RefreshingConnection<FakeConn>, Arc<AtomicU64>) {
        make_conn_with_initial_reply(max_age_ms, Reply::Ok)
    }

    /// Like `make_conn`, but the INITIAL connection replies with `initial_reply`
    /// (e.g. a read-only shape). Rebuilt connections are always healthy so that
    /// a reactive rebuild resolves the condition after exactly one rebuild.
    fn make_conn_with_initial_reply(
        max_age_ms: u64,
        initial_reply: Reply,
    ) -> (RefreshingConnection<FakeConn>, Arc<AtomicU64>) {
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

        let initial = FakeConn::with_reply(next_id.fetch_add(1, Ordering::SeqCst), initial_reply);
        let conn = RefreshingConnection::from_parts(builder, initial, max_age_ms);
        (conn, rebuild_count)
    }

    /// Force the shared state to look expired regardless of jitter.
    fn force_expire(conn: &RefreshingConnection<FakeConn>) {
        let mut guard = conn.lock();
        guard.created_at = Instant::now() - Duration::from_secs(3600);
    }

    /// Push `last_reconnect` far enough into the past that the reactive rate
    /// limit no longer blocks another rebuild.
    fn elapse_reconnect_gap(conn: &RefreshingConnection<FakeConn>) {
        let mut guard = conn.lock();
        guard.last_reconnect = Some(Instant::now() - Duration::from_secs(3600));
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
        // max_age_ms == 0 => AGE-based rebuild disabled. The reactive path is
        // independent and stays active (see test_reactive_rebuild_*).
        let (mut conn, rebuilds) = make_conn(0);
        assert!(
            conn.lock().max_age.is_none(),
            "max_age should be None when disabled"
        );
        // Even a very old created_at must not trigger a rebuild.
        force_expire(&conn);
        assert!(!conn.lock().is_expired());

        // A healthy reply must not trigger the reactive path either.
        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            0,
            "age-based rebuild must be disabled when max_age_ms == 0"
        );
    }

    /// THE regression guard: an inline read-only `ServerError` embedded in an
    /// otherwise-`Ok(..)` single-command reply (the apalis Lua-script shape a
    /// naive top-level `Err` guard missed) must trigger exactly one reactive
    /// rebuild. Age is disabled here so only the reactive path can fire.
    #[tokio::test]
    async fn test_reactive_rebuild_on_inline_readonly_command() {
        let (mut conn, rebuilds) = make_conn_with_initial_reply(0, Reply::InlineReadonly);

        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            1,
            "inline Ok(Value::ServerError(READONLY)) must trigger exactly one reactive rebuild"
        );
    }

    /// Same regression guard for the pipeline path: an inline read-only
    /// `ServerError` nested in an `Ok(vec![..])` reply must trigger exactly one
    /// reactive rebuild.
    #[tokio::test]
    async fn test_reactive_rebuild_on_inline_readonly_commands() {
        let (mut conn, rebuilds) = make_conn_with_initial_reply(0, Reply::InlineReadonly);

        let _ = conn.req_packed_commands(&Pipeline::new(), 0, 1).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            1,
            "inline Ok(vec![Value::ServerError(READONLY)]) must trigger exactly one reactive rebuild"
        );
    }

    /// Direct-write shape: a top-level `Err(ReadOnly)` must also trigger a
    /// reactive rebuild.
    #[tokio::test]
    async fn test_reactive_rebuild_on_err_readonly() {
        let (mut conn, rebuilds) = make_conn_with_initial_reply(0, Reply::ErrReadonly);

        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            1,
            "top-level Err(ReadOnly) must trigger exactly one reactive rebuild"
        );
    }

    /// The rate limit: repeated read-only replies within `min_reconnect_gap`
    /// cause a single rebuild; once the gap has elapsed, another is allowed.
    ///
    /// Here the builder produces conns that STILL reply read-only, so the
    /// condition never clears and every command re-enters the reactive path —
    /// isolating the rate limit as the only thing that gates rebuild count.
    #[tokio::test]
    async fn test_reactive_rebuild_rate_limited() {
        let rebuild_count = Arc::new(AtomicU64::new(0));
        let next_id = Arc::new(AtomicU64::new(1));
        let rc = rebuild_count.clone();
        let ni = next_id.clone();

        // Every built connection also replies read-only, so the reactive path
        // fires on every command; only the rate limit caps the rebuild count.
        let builder: Builder<FakeConn> = Arc::new(move || {
            let rc = rc.clone();
            let ni = ni.clone();
            Box::pin(async move {
                rc.fetch_add(1, Ordering::SeqCst);
                let id = ni.fetch_add(1, Ordering::SeqCst);
                Ok(FakeConn::with_reply(id, Reply::InlineReadonly))
            })
        });
        let initial = FakeConn::with_reply(
            next_id.fetch_add(1, Ordering::SeqCst),
            Reply::InlineReadonly,
        );
        // Age disabled so only the reactive path can rebuild.
        let mut conn = RefreshingConnection::from_parts(builder, initial, 0);

        // First read-only reply: allowed (last_reconnect is None) -> 1 rebuild.
        let _ = conn.req_packed_command(&Cmd::new()).await;
        // Second read-only reply immediately after: within the gap -> no rebuild.
        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuild_count.load(Ordering::SeqCst),
            1,
            "repeated read-only within the gap must rebuild at most once"
        );

        // Simulate the gap elapsing: the next read-only reply is allowed again.
        elapse_reconnect_gap(&conn);
        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuild_count.load(Ordering::SeqCst),
            2,
            "after the gap elapses, another reactive rebuild is allowed"
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

    /// Builds a wrapper whose builder ALWAYS fails, counting attempts — for
    /// asserting that failed rebuilds are rate-limited instead of storming.
    fn make_conn_with_failing_builder(
        max_age_ms: u64,
        initial_reply: Reply,
    ) -> (RefreshingConnection<FakeConn>, Arc<AtomicU64>) {
        let attempts = Arc::new(AtomicU64::new(0));
        let at = attempts.clone();
        let builder: Builder<FakeConn> = Arc::new(move || {
            let at = at.clone();
            Box::pin(async move {
                at.fetch_add(1, Ordering::SeqCst);
                Err(redis::RedisError::from((
                    redis::ErrorKind::IoError,
                    "connect failed",
                )))
            })
        });
        let initial = FakeConn::with_reply(1, initial_reply);
        let conn = RefreshingConnection::from_parts(builder, initial, max_age_ms);
        (conn, attempts)
    }

    /// A FAILED reactive rebuild must still consume the rate-limit window;
    /// otherwise every read-only reply during an outage would hammer the
    /// builder (reconnect storm).
    #[tokio::test]
    async fn test_reactive_failed_rebuild_is_rate_limited() {
        let (mut conn, attempts) = make_conn_with_failing_builder(0, Reply::InlineReadonly);

        // First read-only reply: one (failed) build attempt claims the window.
        // Subsequent read-only replies within the gap must not retry.
        let _ = conn.req_packed_command(&Cmd::new()).await;
        let _ = conn.req_packed_command(&Cmd::new()).await;
        let _ = conn.req_packed_command(&Cmd::new()).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "failed reactive rebuilds within the gap must not retry the builder"
        );
        assert_eq!(conn.get_db(), 1, "existing connection must be preserved");

        // Once the gap elapses, one more attempt is allowed.
        elapse_reconnect_gap(&conn);
        let _ = conn.req_packed_command(&Cmd::new()).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "after the gap elapses, another rebuild attempt is allowed"
        );
    }

    /// A FAILED age rebuild must defer the next attempt (via the claimed
    /// window) instead of retrying the builder on every command.
    #[tokio::test]
    async fn test_age_failed_rebuild_is_rate_limited() {
        let (mut conn, attempts) = make_conn_with_failing_builder(60_000, Reply::Ok);
        force_expire(&conn);

        // First command: one (failed) build attempt claims the window.
        // Further commands within the gap reuse the stale connection.
        let _ = conn.req_packed_command(&Cmd::new()).await;
        let _ = conn.req_packed_command(&Cmd::new()).await;
        let _ = conn.req_packed_command(&Cmd::new()).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a failed age rebuild must not retry the builder on every command"
        );
        assert_eq!(conn.get_db(), 1, "existing connection must be preserved");

        // Once the claim expires, a retry is allowed.
        force_expire(&conn);
        let _ = conn.req_packed_command(&Cmd::new()).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "after the claim expires, another rebuild attempt is allowed"
        );
    }

    /// The aggregate recursion: a read-only `ServerError` nested one level
    /// deeper (array-in-array, as a script reply can be shaped) must still
    /// trigger the reactive rebuild.
    #[tokio::test]
    async fn test_reactive_rebuild_on_nested_readonly() {
        let (mut conn, rebuilds) = make_conn_with_initial_reply(0, Reply::NestedInlineReadonly);

        let _ = conn.req_packed_command(&Cmd::new()).await;

        assert_eq!(
            rebuilds.load(Ordering::SeqCst),
            1,
            "a READONLY nested inside aggregates must trigger a reactive rebuild"
        );
    }
}
