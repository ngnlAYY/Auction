//! Settlement recovery 持久化循环的稳定装配；循环 cadence 与阶段顺序在此。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_observability::{WorkerLoop, WorkerResult};

use crate::worker::diagnostics;
use crate::worker::runtime::{
    PersistentWorker, merge_worker_result, poll_delay, process_recovery_action,
};

mod actions;
mod assets;
mod cases;
mod common;
pub(crate) mod health;
pub(crate) mod health_runtime;
mod resolutions;
mod returns;

use actions::process_actions;
use assets::reconcile_assets;
use cases::advance_cases;
use common::{observe_phase, run_phase};
use health::refresh_health;
use resolutions::{advance_cod_returns, advance_relists, complete_refunds};
use returns::record_returns;

/// 立即依次推进 health、case、return、action、COD return、relist 与 refund，随后按旧版 cadence 重试。
pub(super) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = auction_observability::WorkerLoop::AuctionRecovery;
    let body = async move {
        loop {
            if worker.control.is_draining() {
                return;
            }
            // 恢复流程会重新投递与核对远端邮件；暂停期间不领取新批次。
            if worker.external_access_paused() {
                if crate::worker::runtime::wait_while_paused(&mut shutdown).await {
                    return;
                }
                continue;
            }
            let started = std::time::Instant::now();
            // 恢复推进属于批量轮次，受共享连接配额限制；许可在本轮结束时释放。
            let _bulk_permit = worker.bulk_permit().await;
            let result = run_batch(worker.as_ref()).await;
            if result != WorkerResult::Success {
                tracing::warn!(
                    "recovery worker batch was not fully processed; retrying after poll delay"
                );
            }
            worker.metrics.observe_iteration(
                WorkerLoop::AuctionRecovery,
                result,
                started.elapsed(),
            );

            let wait = poll_delay(worker.recovery.poll_interval, worker.recovery.jitter);
            let scheduled_wake = tokio::time::Instant::now() + wait;
            if crate::worker::runtime::wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
                return;
            }
            worker.metrics.observe_schedule_delay(
                WorkerLoop::AuctionRecovery,
                tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
            );
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await
}

async fn run_batch<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let mut batch_result = WorkerResult::Success;
    refresh_health(worker, &mut batch_result).await;
    reconcile_assets(worker, &mut batch_result).await;
    advance_cases(worker, &mut batch_result).await;
    record_returns(worker, &mut batch_result).await;
    process_actions(worker, &mut batch_result).await;
    advance_cod_returns(worker, &mut batch_result).await;
    advance_relists(worker, &mut batch_result).await;
    complete_refunds(worker, &mut batch_result).await;
    batch_result
}
