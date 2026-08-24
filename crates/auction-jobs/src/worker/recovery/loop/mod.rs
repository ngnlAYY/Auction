//! Settlement recovery 五阶段持久化循环的稳定装配。
//!
//! 调度、阶段实现和结果合并分别归属相邻 owner；本文件只保留循环 cadence 与阶段顺序。

use std::sync::Arc;

use auction_application::Clock;
use auction_observability::{WorkerLoop, WorkerResult};

use super::{PersistentWorker, merge_worker_result, poll_delay, process_recovery_action};

mod actions;
mod cases;
mod common;
mod health;
mod resolutions;
mod returns;

use actions::process_actions;
use cases::advance_cases;
use common::observe_phase;
use health::refresh_health;
use resolutions::{advance_relists, complete_refunds};
use returns::record_returns;

/// 立即依次推进 health、case、return、action、relist 与 refund，随后按旧版 cadence 重试。
pub(super) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        let started = std::time::Instant::now();
        let result = run_batch(worker.as_ref()).await;
        if result != WorkerResult::Success {
            tracing::warn!(
                "recovery worker batch was not fully processed; retrying after poll delay"
            );
        }
        worker.metrics.observe_iteration(WorkerLoop::AuctionRecovery, result, started.elapsed());

        let wait = poll_delay(worker.recovery.poll_interval, worker.recovery.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        if super::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
            return;
        }
        worker.metrics.observe_schedule_delay(
            WorkerLoop::AuctionRecovery,
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
    }
}

async fn run_batch<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let mut batch_result = WorkerResult::Success;
    refresh_health(worker, &mut batch_result).await;
    advance_cases(worker, &mut batch_result).await;
    record_returns(worker, &mut batch_result).await;
    process_actions(worker, &mut batch_result).await;
    advance_relists(worker, &mut batch_result).await;
    complete_refunds(worker, &mut batch_result).await;
    batch_result
}
