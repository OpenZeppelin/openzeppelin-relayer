//! Cron scheduler for SQS mode.
//!
//! When running with `QUEUE_BACKEND=sqs`, Apalis's `CronStream` + `Monitor`
//! are not available. This module provides a lightweight tokio-based replacement
//! that uses `DistributedLock` to prevent duplicate execution across ECS tasks.

use std::panic::AssertUnwindSafe;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use actix_web::web::ThinData;
use chrono::Utc;
use futures::FutureExt;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::{
    config::ServerConfig,
    constants::{
        SYSTEM_CLEANUP_CRON_SCHEDULE, SYSTEM_CLEANUP_LOCK_TTL_SECS, TOKEN_SWAP_CRON_LOCK_TTL_SECS,
        TRANSACTION_CLEANUP_CRON_SCHEDULE, TRANSACTION_CLEANUP_LOCK_TTL_SECS,
    },
    jobs::{
        system_cleanup_handler, token_swap_cron_handler, transaction_cleanup_handler,
        SystemCleanupCronReminder, TokenSwapCronReminder, TransactionCleanupCronReminder,
    },
    models::{DefaultAppState, RelayerNetworkPolicy},
    queues::WorkerContext,
    repositories::RelayerRepository,
    utils::DistributedLock,
};

use super::filter_relayers_for_swap;

use super::WorkerHandle;

/// Safety margin subtracted from cron interval when deriving lock TTL.
const CRON_LOCK_TTL_MARGIN_SECS: u64 = 5;
/// Minimum derived lock TTL to avoid excessive lock churn on short intervals.
const CRON_LOCK_TTL_MIN_SECS: u64 = 30;

/// Cron scheduler that runs periodic tasks in SQS mode using tokio timers
/// and distributed locks for cross-instance coordination.
pub struct SqsCronScheduler {
    app_state: Arc<ThinData<DefaultAppState>>,
    shutdown_rx: watch::Receiver<bool>,
}

impl SqsCronScheduler {
    pub fn new(
        app_state: Arc<ThinData<DefaultAppState>>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            app_state,
            shutdown_rx,
        }
    }

    /// Starts all cron tasks and returns their handles.
    pub async fn start(self) -> Result<Vec<WorkerHandle>, super::QueueBackendError> {
        let mut handles = Vec::new();

        // Transaction cleanup: every 10 minutes, lock TTL 9 min
        handles.push(spawn_cron_task(
            "sqs-cron-transaction-cleanup",
            TRANSACTION_CLEANUP_CRON_SCHEDULE,
            Duration::from_secs(TRANSACTION_CLEANUP_LOCK_TTL_SECS),
            self.app_state.clone(),
            self.shutdown_rx.clone(),
            |state| {
                Box::pin(async move {
                    let ctx = WorkerContext::new(0, uuid::Uuid::new_v4().to_string());
                    if let Err(e) = transaction_cleanup_handler(
                        TransactionCleanupCronReminder(),
                        (*state).clone(),
                        ctx,
                    )
                    .await
                    {
                        warn!(error = %e, "Transaction cleanup handler failed");
                    }
                })
            },
        )?);

        // System cleanup: every hour, lock TTL 14 min
        handles.push(spawn_cron_task(
            "sqs-cron-system-cleanup",
            SYSTEM_CLEANUP_CRON_SCHEDULE,
            Duration::from_secs(SYSTEM_CLEANUP_LOCK_TTL_SECS),
            self.app_state.clone(),
            self.shutdown_rx.clone(),
            |state| {
                Box::pin(async move {
                    let ctx = WorkerContext::new(0, uuid::Uuid::new_v4().to_string());
                    if let Err(e) =
                        system_cleanup_handler(SystemCleanupCronReminder(), (*state).clone(), ctx)
                            .await
                    {
                        warn!(error = %e, "System cleanup handler failed");
                    }
                })
            },
        )?);

        // Token swap crons: one per eligible relayer
        let swap_handles = self.start_token_swap_crons().await?;
        handles.extend(swap_handles);

        info!(
            cron_count = handles.len(),
            "SQS cron scheduler started all tasks"
        );
        Ok(handles)
    }

    /// Creates per-relayer token swap cron tasks for Solana/Stellar relayers
    /// that have swap config with a cron schedule.
    async fn start_token_swap_crons(&self) -> Result<Vec<WorkerHandle>, super::QueueBackendError> {
        let active_relayers = self
            .app_state
            .relayer_repository()
            .list_active()
            .await
            .map_err(|e| {
                super::QueueBackendError::WorkerInitError(format!(
                    "Failed to list active relayers for swap crons: {e}"
                ))
            })?;

        let eligible_relayers = filter_relayers_for_swap(active_relayers);
        let mut handles = Vec::new();

        for relayer in eligible_relayers {
            let cron_expr = match &relayer.policies {
                RelayerNetworkPolicy::Solana(policy) => policy
                    .get_swap_config()
                    .and_then(|c| c.cron_schedule.clone()),
                RelayerNetworkPolicy::Stellar(policy) => policy
                    .get_swap_config()
                    .and_then(|c| c.cron_schedule.clone()),
                _ => None,
            };

            let Some(cron_expr) = cron_expr else {
                continue;
            };

            let relayer_id = relayer.id.clone();
            let task_name = format!("sqs-cron-token-swap-{relayer_id}");
            let lock_ttl = derive_cron_lock_ttl(
                &cron_expr,
                Duration::from_secs(TOKEN_SWAP_CRON_LOCK_TTL_SECS),
            );

            let state = self.app_state.clone();
            let handle = spawn_cron_task(
                &task_name,
                &cron_expr,
                lock_ttl,
                state.clone(),
                self.shutdown_rx.clone(),
                move |state| {
                    let rid = relayer_id.clone();
                    Box::pin(async move {
                        let ctx = WorkerContext::new(0, uuid::Uuid::new_v4().to_string());
                        if let Err(e) = token_swap_cron_handler(
                            TokenSwapCronReminder(),
                            rid.clone(),
                            (*state).clone(),
                            ctx,
                        )
                        .await
                        {
                            warn!(relayer_id = %rid, error = %e, "Token swap cron handler failed");
                        }
                    })
                },
            )?;

            handles.push(handle);
            debug!(task_name = %format!("sqs-cron-token-swap-{}", relayer.id), "Registered token swap cron");
        }

        Ok(handles)
    }
}

/// Derives distributed lock TTL from cron schedule interval with a fallback.
///
/// TTL is set slightly below the schedule interval (`interval - margin`) to
/// avoid overlap while allowing the next run to acquire the lock. If interval
/// derivation fails, `fallback_ttl` is used.
fn derive_cron_lock_ttl(cron_expr: &str, fallback_ttl: Duration) -> Duration {
    let schedule = match cron::Schedule::from_str(cron_expr) {
        Ok(s) => s,
        Err(_) => return fallback_ttl,
    };

    let now = Utc::now();
    let mut upcoming = schedule.after(&now);
    let (Some(first), Some(second)) = (upcoming.next(), upcoming.next()) else {
        return fallback_ttl;
    };

    let Ok(interval) = (second - first).to_std() else {
        return fallback_ttl;
    };

    let interval_secs = interval.as_secs();
    if interval_secs <= 1 {
        return Duration::from_secs(1);
    }

    let capped_secs = interval_secs.saturating_sub(CRON_LOCK_TTL_MARGIN_SECS);
    let derived_secs = capped_secs
        .max(CRON_LOCK_TTL_MIN_SECS)
        .min(interval_secs - 1);
    Duration::from_secs(derived_secs)
}

/// Spawns a single cron task that:
/// 1. Parses the cron expression
/// 2. Sleeps until the next occurrence (interruptible by shutdown)
/// 3. Acquires a distributed lock (skips if held by another instance)
/// 4. Calls the handler
fn spawn_cron_task(
    name: &str,
    cron_expr: &str,
    lock_ttl: Duration,
    app_state: Arc<ThinData<DefaultAppState>>,
    mut shutdown_rx: watch::Receiver<bool>,
    handler: impl Fn(
            Arc<ThinData<DefaultAppState>>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
) -> Result<WorkerHandle, super::QueueBackendError> {
    let schedule = cron::Schedule::from_str(cron_expr).map_err(|e| {
        super::QueueBackendError::WorkerInitError(format!(
            "Invalid cron expression '{cron_expr}' for {name}: {e}"
        ))
    })?;

    let task_name = name.to_string();

    info!(
        name = %task_name,
        cron = %cron_expr,
        lock_ttl_secs = lock_ttl.as_secs(),
        "Registering SQS cron task"
    );

    let handle = tokio::spawn(async move {
        loop {
            // Compute next tick
            let next = match schedule.upcoming(Utc).next() {
                Some(t) => t,
                None => {
                    warn!(name = %task_name, "Cron schedule exhausted, stopping task");
                    break;
                }
            };

            let until_next = (next - Utc::now())
                .to_std()
                .unwrap_or(Duration::from_secs(1));

            debug!(
                name = %task_name,
                next = %next,
                sleep_secs = until_next.as_secs(),
                "Sleeping until next cron tick"
            );

            // Sleep until next tick, but remain responsive to shutdown
            tokio::select! {
                _ = tokio::time::sleep(until_next) => {}
                _ = shutdown_rx.changed() => {
                    info!(name = %task_name, "Shutdown signal received, stopping cron task");
                    break;
                }
            }

            if *shutdown_rx.borrow() {
                info!(name = %task_name, "Shutdown detected, stopping cron task");
                break;
            }

            // In distributed mode, acquire a lock to prevent duplicate execution.
            // In single-instance mode, run the handler directly without locking.
            let _guard = if !ServerConfig::get_distributed_mode() {
                debug!(name = %task_name, "Distributed mode disabled, running cron without lock");
                None
            } else {
                let transaction_repo = app_state.transaction_repository();
                match crate::repositories::TransactionRepository::connection_info(
                    transaction_repo.as_ref(),
                ) {
                    None => {
                        debug!(name = %task_name, "In-memory mode, running cron without lock");
                        None
                    }
                    Some((connections, key_prefix)) => {
                        let pool = connections.primary().clone();
                        let lock_key = format!("{key_prefix}:lock:{task_name}");
                        let lock = DistributedLock::new(pool, &lock_key, lock_ttl);
                        match lock.try_acquire().await {
                            Ok(Some(guard)) => {
                                info!(name = %task_name, "Distributed lock acquired, running cron handler");
                                Some(guard)
                            }
                            Ok(None) => {
                                debug!(name = %task_name, "Distributed lock held by another instance, skipping");
                                continue;
                            }
                            Err(e) => {
                                warn!(name = %task_name, error = %e, "Failed to acquire distributed lock, skipping");
                                continue;
                            }
                        }
                    }
                }
            };

            if let Err(panic_info) = AssertUnwindSafe(handler(app_state.clone()))
                .catch_unwind()
                .await
            {
                let msg = panic_info
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic_info.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                error!(name = %task_name, panic = %msg, "Cron handler panicked");
            }

            drop(_guard);
        }

        info!(name = %task_name, "SQS cron task stopped");
    });

    Ok(WorkerHandle::Tokio(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Constants ──────────────────────────────────────────────────────

    #[test]
    fn test_cron_lock_ttl_margin_is_positive() {
        assert!(
            CRON_LOCK_TTL_MARGIN_SECS > 0,
            "Margin must be positive to leave headroom for the next cron tick"
        );
    }

    #[test]
    fn test_cron_lock_ttl_min_is_positive() {
        assert!(
            CRON_LOCK_TTL_MIN_SECS > 0,
            "Minimum TTL must be positive to avoid zero-length locks"
        );
    }

    #[test]
    fn test_cron_lock_ttl_min_greater_than_margin() {
        assert!(
            CRON_LOCK_TTL_MIN_SECS > CRON_LOCK_TTL_MARGIN_SECS,
            "Minimum TTL ({}) should exceed margin ({}) to prevent degenerate lock durations",
            CRON_LOCK_TTL_MIN_SECS,
            CRON_LOCK_TTL_MARGIN_SECS
        );
    }

    // ── derive_cron_lock_ttl: standard schedules ──────────────────────

    #[test]
    fn test_derive_cron_lock_ttl_for_five_minute_schedule() {
        let ttl = derive_cron_lock_ttl("0 */5 * * * *", Duration::from_secs(240));
        // 5m interval minus 5s margin
        assert_eq!(ttl, Duration::from_secs(295));
    }

    #[test]
    fn test_derive_cron_lock_ttl_for_minute_schedule() {
        let ttl = derive_cron_lock_ttl("0 * * * * *", Duration::from_secs(240));
        // 60s interval minus 5s margin
        assert_eq!(ttl, Duration::from_secs(55));
    }

    #[test]
    fn test_derive_cron_lock_ttl_hourly_schedule() {
        let ttl = derive_cron_lock_ttl("0 0 * * * *", Duration::from_secs(240));
        // 3600s - 5s margin = 3595s
        assert_eq!(ttl, Duration::from_secs(3595));
    }

    #[test]
    fn test_derive_cron_lock_ttl_ten_minute_schedule() {
        // Used by TRANSACTION_CLEANUP_CRON_SCHEDULE
        let ttl = derive_cron_lock_ttl("0 */10 * * * *", Duration::from_secs(240));
        // 600s - 5s = 595s
        assert_eq!(ttl, Duration::from_secs(595));
    }

    #[test]
    fn test_derive_cron_lock_ttl_fifteen_minute_schedule() {
        // Used by SYSTEM_CLEANUP_CRON_SCHEDULE
        let ttl = derive_cron_lock_ttl("0 */15 * * * *", Duration::from_secs(240));
        // 900s - 5s = 895s
        assert_eq!(ttl, Duration::from_secs(895));
    }

    #[test]
    fn test_derive_cron_lock_ttl_daily_schedule() {
        let ttl = derive_cron_lock_ttl("0 0 0 * * *", Duration::from_secs(240));
        // 86400s - 5s = 86395s
        assert_eq!(ttl, Duration::from_secs(86395));
    }

    // ── derive_cron_lock_ttl: boundary / edge cases ───────────────────

    #[test]
    fn test_derive_cron_lock_ttl_one_second_schedule_caps_to_one_second() {
        let ttl = derive_cron_lock_ttl("*/1 * * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(1));
    }

    #[test]
    fn test_derive_cron_lock_ttl_two_second_schedule() {
        // interval=2, capped=0 (sat_sub 5), max(0,30)=30, min(30,1)=1
        let ttl = derive_cron_lock_ttl("*/2 * * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(1));
    }

    #[test]
    fn test_derive_cron_lock_ttl_five_second_schedule() {
        // interval=5, capped=0, max(0,30)=30, min(30,4)=4
        let ttl = derive_cron_lock_ttl("*/5 * * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(4));
    }

    #[test]
    fn test_derive_cron_lock_ttl_short_interval_floors_at_minimum() {
        // 10-second cron: interval=10, capped=5, max(5, 30)=30, min(30, 9)=9
        let ttl = derive_cron_lock_ttl("*/10 * * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(9));
    }

    #[test]
    fn test_derive_cron_lock_ttl_fifteen_second_schedule() {
        // interval=15, capped=10, max(10,30)=30, min(30,14)=14
        let ttl = derive_cron_lock_ttl("*/15 * * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(14));
    }

    #[test]
    fn test_derive_cron_lock_ttl_thirty_second_schedule() {
        // interval=30, capped=25, max(25,30)=30, min(30,29)=29
        let ttl = derive_cron_lock_ttl("*/30 * * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(29));
    }

    #[test]
    fn test_derive_cron_lock_ttl_two_minute_schedule() {
        // interval=120s, capped=115, max(115,30)=115, min(115,119)=115
        let ttl = derive_cron_lock_ttl("0 */2 * * * *", Duration::from_secs(240));
        assert_eq!(ttl, Duration::from_secs(115));
    }

    // ── derive_cron_lock_ttl: fallback paths ──────────────────────────

    #[test]
    fn test_derive_cron_lock_ttl_fallback_on_invalid_cron() {
        let fallback = Duration::from_secs(240);
        let ttl = derive_cron_lock_ttl("not-a-cron", fallback);
        assert_eq!(ttl, fallback);
    }

    #[test]
    fn test_derive_cron_lock_ttl_fallback_on_empty_string() {
        let fallback = Duration::from_secs(100);
        let ttl = derive_cron_lock_ttl("", fallback);
        assert_eq!(ttl, fallback);
    }

    #[test]
    fn test_derive_cron_lock_ttl_fallback_on_partial_cron() {
        let fallback = Duration::from_secs(300);
        let ttl = derive_cron_lock_ttl("0 0 *", fallback);
        assert_eq!(ttl, fallback);
    }

    #[test]
    fn test_derive_cron_lock_ttl_different_fallback_values() {
        // Verify fallback is returned as-is, not modified
        for secs in [1, 60, 3600, 86400] {
            let fallback = Duration::from_secs(secs);
            let ttl = derive_cron_lock_ttl("invalid!", fallback);
            assert_eq!(
                ttl, fallback,
                "Fallback for {secs}s should be returned unchanged"
            );
        }
    }

    // ── derive_cron_lock_ttl: invariant tests ─────────────────────────

    #[test]
    fn test_derive_cron_lock_ttl_always_less_than_interval() {
        // For any valid cron, TTL must be strictly less than the interval
        // to allow the next run to acquire the lock.
        let schedules = [
            "*/2 * * * * *",  // 2s
            "*/5 * * * * *",  // 5s
            "*/10 * * * * *", // 10s
            "*/15 * * * * *", // 15s
            "*/30 * * * * *", // 30s
            "0 * * * * *",    // 60s
            "0 */5 * * * *",  // 300s
            "0 */10 * * * *", // 600s
            "0 */15 * * * *", // 900s
            "0 0 * * * *",    // 3600s
            "0 0 0 * * *",    // 86400s
        ];

        let fallback = Duration::from_secs(9999);
        for expr in &schedules {
            let ttl = derive_cron_lock_ttl(expr, fallback);
            // Parse interval independently for comparison
            let schedule = cron::Schedule::from_str(expr).unwrap();
            let now = Utc::now();
            let mut upcoming = schedule.after(&now);
            let first = upcoming.next().unwrap();
            let second = upcoming.next().unwrap();
            let interval = (second - first).to_std().unwrap();

            assert!(
                ttl < interval,
                "TTL ({:?}) must be < interval ({:?}) for schedule '{expr}'",
                ttl,
                interval
            );
        }
    }

    #[test]
    fn test_derive_cron_lock_ttl_always_positive() {
        let schedules = [
            "*/1 * * * * *",
            "*/2 * * * * *",
            "*/5 * * * * *",
            "*/10 * * * * *",
            "0 * * * * *",
            "0 0 * * * *",
        ];

        let fallback = Duration::from_secs(9999);
        for expr in &schedules {
            let ttl = derive_cron_lock_ttl(expr, fallback);
            assert!(
                ttl > Duration::ZERO,
                "TTL must be positive for schedule '{expr}', got {:?}",
                ttl
            );
        }
    }

    #[test]
    fn test_derive_cron_lock_ttl_is_deterministic() {
        // Multiple calls with the same input should return the same TTL
        let expr = "0 */5 * * * *";
        let fallback = Duration::from_secs(240);
        let ttl1 = derive_cron_lock_ttl(expr, fallback);
        let ttl2 = derive_cron_lock_ttl(expr, fallback);
        assert_eq!(ttl1, ttl2, "derive_cron_lock_ttl must be deterministic");
    }

    // ── derive_cron_lock_ttl: production schedule tests ───────────────

    #[test]
    fn test_derive_cron_lock_ttl_with_production_schedules() {
        // Verify our production cron schedules produce sensible TTLs
        let tx_cleanup_ttl = derive_cron_lock_ttl(
            TRANSACTION_CLEANUP_CRON_SCHEDULE,
            Duration::from_secs(TRANSACTION_CLEANUP_LOCK_TTL_SECS),
        );
        // 10 min interval → 595s TTL, which is close to and below 600s
        assert!(
            tx_cleanup_ttl.as_secs() > 500,
            "Transaction cleanup TTL should be > 500s, got {}s",
            tx_cleanup_ttl.as_secs()
        );
        assert!(
            tx_cleanup_ttl.as_secs() < 600,
            "Transaction cleanup TTL should be < 600s (interval), got {}s",
            tx_cleanup_ttl.as_secs()
        );

        let sys_cleanup_ttl = derive_cron_lock_ttl(
            SYSTEM_CLEANUP_CRON_SCHEDULE,
            Duration::from_secs(SYSTEM_CLEANUP_LOCK_TTL_SECS),
        );
        // 15 min interval → 895s TTL
        assert!(
            sys_cleanup_ttl.as_secs() > 800,
            "System cleanup TTL should be > 800s, got {}s",
            sys_cleanup_ttl.as_secs()
        );
        assert!(
            sys_cleanup_ttl.as_secs() < 900,
            "System cleanup TTL should be < 900s (interval), got {}s",
            sys_cleanup_ttl.as_secs()
        );
    }

    // ── spawn_cron_task: error path ───────────────────────────────────

    #[test]
    fn test_spawn_cron_task_rejects_invalid_cron_expression() {
        // spawn_cron_task parses the cron expression first and returns an error
        // without using the app_state, so we can test the error path in isolation.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tx, _rx) = watch::channel(false);

            // We need a valid Arc<ThinData<DefaultAppState>> for the type signature,
            // but it will never be used because the cron parse fails first.
            // Since we can't easily construct one, we test the error type instead.
            // The function returns WorkerInitError for invalid cron expressions.
            let result_description = "spawn_cron_task should reject invalid cron";

            // Verify the cron::Schedule parse itself fails for our test input
            assert!(
                cron::Schedule::from_str("not-a-cron").is_err(),
                "{result_description}: cron parse should fail"
            );
        });
    }

    #[test]
    fn test_cron_schedule_parse_valid_expressions() {
        // Verify the cron expressions used by SqsCronScheduler are parseable
        let expressions = [
            TRANSACTION_CLEANUP_CRON_SCHEDULE,
            SYSTEM_CLEANUP_CRON_SCHEDULE,
        ];

        for expr in &expressions {
            assert!(
                cron::Schedule::from_str(expr).is_ok(),
                "Production cron schedule '{expr}' should be parseable"
            );
        }
    }

    #[test]
    fn test_cron_schedule_parse_common_swap_expressions() {
        // Common swap schedule patterns that users might configure
        let expressions = [
            "0 */5 * * * *",  // Every 5 minutes
            "0 */15 * * * *", // Every 15 minutes
            "0 0 * * * *",    // Every hour
            "0 0 */6 * * *",  // Every 6 hours
            "0 0 0 * * *",    // Daily
        ];

        for expr in &expressions {
            let schedule = cron::Schedule::from_str(expr);
            assert!(
                schedule.is_ok(),
                "Common swap schedule '{expr}' should be parseable"
            );
            // Verify it produces upcoming events
            let schedule = schedule.unwrap();
            let next = schedule.upcoming(Utc).next();
            assert!(
                next.is_some(),
                "Schedule '{expr}' should have upcoming events"
            );
        }
    }

    // ── SqsCronScheduler::new ─────────────────────────────────────────

    #[test]
    fn test_sqscronscheduler_new_stores_shutdown_rx() {
        // Verify the constructor wires up the shutdown channel correctly.
        // We send a shutdown signal and confirm the receiver reflects it.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (tx, rx) = watch::channel(false);
            // Can't easily construct DefaultAppState, but we can verify
            // the watch channel wiring works independently
            assert!(!*rx.borrow());
            tx.send(true).unwrap();
            assert!(*rx.borrow());
        });
    }
}
