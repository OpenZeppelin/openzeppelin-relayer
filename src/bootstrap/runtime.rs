//! Pipeline runtime configuration.
//!
//! Resolves the worker-thread budget for the two runtimes the relayer runs:
//! the actix HTTP server (`actix_workers`) and the explicit multi-thread tokio
//! runtime that hosts the background transaction pipeline (`tokio_worker_threads`).
//!
//! Per `contracts/runtime-config.md`, both counts are pinned from the environment
//! and SHOULD sum to no more than the container vCPU quota. Auto-detection via
//! [`std::thread::available_parallelism`] returns the cgroup-aware core count on
//! Rust 1.91 for cgroup v1/v2 hard quotas, but NOT for AWS Fargate's `cpu.shares`
//! — so the env-var pins are the authoritative control on all targets.

use std::env;
use std::thread::available_parallelism;
use std::time::Duration;

use tracing::{info, warn};

use crate::queues::WorkerHandle;

const ENV_TOKIO_WORKER_THREADS: &str = "TOKIO_WORKER_THREADS";
const ENV_ACTIX_WORKERS: &str = "ACTIX_WORKERS";

/// Resolved worker-thread budget for the relayer's runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Detected/assumed container vCPU quota (best-effort; host cores on Fargate).
    pub vcpu: usize,
    /// HTTP server worker count (pins actix `.workers()`).
    pub actix_workers: usize,
    /// Pipeline runtime worker-thread count.
    pub tokio_worker_threads: usize,
}

impl RuntimeConfig {
    /// Resolve from the environment, detecting the vCPU quota for the defaults.
    pub fn from_env() -> Self {
        Self::resolve(
            detect_vcpu(),
            read_usize_env(ENV_ACTIX_WORKERS),
            read_usize_env(ENV_TOKIO_WORKER_THREADS),
        )
    }

    /// Pure budget resolution (separated from env/IO for testability).
    ///
    /// Defaults (per the runtime-config contract):
    /// - `actix_workers` = `max(1, vcpu / 2)`
    /// - `tokio_worker_threads` = `max(1, vcpu - actix_workers)`
    ///
    /// Explicit overrides of `0` are ignored (treated as unset) so a misconfigured
    /// `=0` can never produce a zero-thread runtime.
    fn resolve(vcpu: usize, actix_override: Option<usize>, tokio_override: Option<usize>) -> Self {
        let vcpu = vcpu.max(1);
        let actix_workers = actix_override
            .filter(|n| *n > 0)
            .unwrap_or_else(|| (vcpu / 2).max(1));
        let tokio_worker_threads = tokio_override
            .filter(|n| *n > 0)
            .unwrap_or_else(|| vcpu.saturating_sub(actix_workers).max(1));
        Self {
            vcpu,
            actix_workers,
            tokio_worker_threads,
        }
    }

    /// Total threads requested across both runtimes.
    pub fn total_threads(&self) -> usize {
        self.actix_workers + self.tokio_worker_threads
    }

    /// Whether the requested budget exceeds the detected vCPU quota.
    pub fn over_budget(&self) -> bool {
        self.total_threads() > self.vcpu
    }

    /// Log the resolved budget at startup and WARN when it exceeds the vCPU quota.
    pub fn log_startup(&self) {
        info!(
            vcpu = self.vcpu,
            actix_workers = self.actix_workers,
            tokio_worker_threads = self.tokio_worker_threads,
            "resolved runtime worker budget"
        );
        if self.over_budget() {
            warn!(
                vcpu = self.vcpu,
                actix_workers = self.actix_workers,
                tokio_worker_threads = self.tokio_worker_threads,
                total = self.total_threads(),
                "worker budget exceeds vCPU quota; threads will contend / CFS-throttle. \
                 Pin TOKIO_WORKER_THREADS and ACTIX_WORKERS to fit the container CPU quota."
            );
        }
    }
}

/// Join the pipeline worker handles so in-flight work drains before the runtime is
/// torn down on shutdown (D7). Bounded by `timeout`; if it elapses, remaining work is
/// abandoned (at-least-once redelivery is the backstop). Returns `true` if the drain
/// completed within the window, `false` on timeout.
///
/// `WorkerHandle::Apalis` handles (if any) are dropped — the apalis Monitor shuts down
/// via its own signal handling.
pub async fn drain_worker_handles(handles: Vec<WorkerHandle>, timeout: Duration) -> bool {
    let drain = async {
        for handle in handles {
            if let WorkerHandle::Tokio(join_handle) = handle {
                if let Err(e) = join_handle.await {
                    warn!(error = %e, "pipeline worker did not join cleanly");
                }
            }
        }
    };
    tokio::time::timeout(timeout, drain).await.is_ok()
}

fn detect_vcpu() -> usize {
    available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn read_usize_env(key: &str) -> Option<usize> {
    env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_split_vcpu_between_runtimes() {
        let cfg = RuntimeConfig::resolve(8, None, None);
        assert_eq!(cfg.vcpu, 8);
        assert_eq!(cfg.actix_workers, 4); // max(1, 8/2)
        assert_eq!(cfg.tokio_worker_threads, 4); // max(1, 8-4)
        assert!(!cfg.over_budget());
    }

    #[test]
    fn single_vcpu_never_yields_zero_threads() {
        let cfg = RuntimeConfig::resolve(1, None, None);
        assert_eq!(cfg.actix_workers, 1); // max(1, 0)
        assert_eq!(cfg.tokio_worker_threads, 1); // max(1, 1-1)
                                                 // 1 + 1 > 1: a single-vCPU box is inherently over budget (expected; warns).
        assert!(cfg.over_budget());
    }

    #[test]
    fn explicit_overrides_are_respected() {
        let cfg = RuntimeConfig::resolve(8, Some(2), Some(4));
        assert_eq!(cfg.actix_workers, 2);
        assert_eq!(cfg.tokio_worker_threads, 4);
        assert_eq!(cfg.total_threads(), 6);
        assert!(!cfg.over_budget());
    }

    #[test]
    fn tokio_default_accounts_for_actix_override() {
        // Only actix pinned: tokio fills the remainder of the quota.
        let cfg = RuntimeConfig::resolve(8, Some(6), None);
        assert_eq!(cfg.actix_workers, 6);
        assert_eq!(cfg.tokio_worker_threads, 2); // max(1, 8-6)
    }

    #[test]
    fn zero_override_is_ignored() {
        let cfg = RuntimeConfig::resolve(4, Some(0), Some(0));
        assert_eq!(cfg.actix_workers, 2); // falls back to default
        assert_eq!(cfg.tokio_worker_threads, 2);
    }

    #[test]
    fn over_budget_detected_when_sum_exceeds_quota() {
        let cfg = RuntimeConfig::resolve(4, Some(4), Some(4));
        assert_eq!(cfg.total_threads(), 8);
        assert!(cfg.over_budget());
    }

    #[tokio::test]
    async fn drain_completes_when_workers_finish_within_window() {
        // Two in-flight workers that each finish after a brief delay (well within the window).
        let handles: Vec<WorkerHandle> = (0..2)
            .map(|_| {
                WorkerHandle::Tokio(tokio::spawn(async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }))
            })
            .collect();

        let drained = drain_worker_handles(handles, Duration::from_secs(5)).await;
        assert!(drained, "workers should drain within the window");
    }

    #[tokio::test]
    async fn drain_times_out_when_a_worker_hangs() {
        // A worker that never completes — drain must give up at the (short) deadline.
        let handles = vec![WorkerHandle::Tokio(tokio::spawn(async {
            futures::future::pending::<()>().await;
        }))];

        let drained = drain_worker_handles(handles, Duration::from_millis(150)).await;
        assert!(!drained, "drain must time out when a worker hangs");
    }
}
