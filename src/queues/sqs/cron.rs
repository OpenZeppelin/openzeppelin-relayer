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
    fn test_derive_cron_lock_ttl_fallback_on_invalid_cron() {
        let fallback = Duration::from_secs(240);
        let ttl = derive_cron_lock_ttl("not-a-cron", fallback);
        assert_eq!(ttl, fallback);
    }
}
