//! 每世界独立维护循环；cron 只唤醒原持久恢复服务，低频观察不参与业务资格或完成判定。

use std::sync::Arc;

use auction_application::Clock;
use auction_observability::WorkerLoop;

use super::super::super::{PersistentWorker, poll_delay, wait_until_or_shutdown};
use super::super::MailPollRuntime;
use super::{metrics::IterationGuard, run_maintenance, run_read_observation};

/// 先完成 Sent/compose 维护，再按内存间隔做完整 Read Mail 纯观察；手动收件不唤醒此循环。
pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    runtime: MailPollRuntime,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let (maintenance_loop, observation_loop) = if runtime.world_id == "isekai" {
        (WorkerLoop::MailMaintenanceIsekai, WorkerLoop::MailReadObservationIsekai)
    } else {
        (WorkerLoop::MailMaintenanceMain, WorkerLoop::MailReadObservationMain)
    };
    // 首次观察延后一个完整间隔；此时刻只减少纯观察读取，不充当持久任务的 due 或完成证据。
    let mut last_observation = tokio::time::Instant::now();
    loop {
        if *shutdown.borrow() || worker.control.is_draining() {
            return;
        }
        let now = worker.clock.now();
        let Some(delay) = runtime
            .maintenance_schedule
            .next_after(now)
            .and_then(|next| next.signed_duration_since(now).to_std().ok())
            .filter(|delay| !delay.is_zero())
        else {
            tracing::error!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail 维护 cron 无法计算未来触发时刻，停止该维护循环"
            );
            return;
        };
        let scheduled_wake = tokio::time::Instant::now() + poll_delay(delay, runtime.config.jitter);
        if wait_until_or_shutdown(&mut shutdown, scheduled_wake).await {
            return;
        }
        if *shutdown.borrow() || worker.control.is_draining() {
            return;
        }
        worker.metrics.observe_schedule_delay(
            maintenance_loop,
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
        let iteration = IterationGuard::new(Arc::clone(&worker.metrics), maintenance_loop);
        // 在途 sweep 沿用原预算排空，避免在 discard POST 中途因停止信号另建重试授权。
        iteration.finish(run_maintenance(&runtime, &worker.write_lifecycle).await);

        if *shutdown.borrow() || worker.control.is_draining() {
            return;
        }
        if last_observation.elapsed() >= runtime.config.read_mail_observation_interval {
            // 仅在 sweep 之后检查，实际观察间隔向上取整到维护节奏；失败也消耗这次间隔。
            last_observation = tokio::time::Instant::now();
            let iteration = IterationGuard::new(Arc::clone(&worker.metrics), observation_loop);
            iteration.finish(
                worker
                    .write_lifecycle
                    .background(run_read_observation(&runtime))
                    .await
                    .unwrap_or(auction_observability::WorkerResult::Canceled),
            );
        }
    }
}
