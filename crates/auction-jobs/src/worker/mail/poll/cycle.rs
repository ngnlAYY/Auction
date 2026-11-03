//! MoogleMail Worker 的调度、领取和批次上下文。

use std::sync::Arc;

use auction_application::{
    Clock, MailDiagnosticRecorder, MailPollStore, MailProcessor, RecurringSchedule,
};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult};
use chrono::Duration as ChronoDuration;

use super::super::{PersistentWorker, WorkerError, poll_delay};
use super::maintenance::{run_maintenance, run_read_observation};
use super::processing::process_claim;
use super::{MailPollGateway, MailPollRuntime, MailPollRuntimeConfig};

/// 等待已装配日程与 `[0, jitter]` 后轮询；手动触发跳过等待和 due 时间。
pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    runtime: MailPollRuntime,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    loop {
        if *shutdown.borrow() || worker.control.is_draining() {
            return;
        }
        let now = worker.clock.now();
        let Some(next_wake_at) = runtime.schedule.next_after(now) else {
            tracing::error!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail cron 无法计算下一次触发时刻，停止该邮箱循环"
            );
            return;
        };
        let Some(schedule_delay) =
            next_wake_at.signed_duration_since(now).to_std().ok().filter(|delay| !delay.is_zero())
        else {
            tracing::error!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail cron 返回了非未来触发时刻，停止该邮箱循环"
            );
            return;
        };
        let wait = poll_delay(schedule_delay, runtime.config.jitter);
        let scheduled_wake = tokio::time::Instant::now() + wait;
        let scheduled_sleep = tokio::time::sleep_until(scheduled_wake);
        tokio::pin!(scheduled_sleep);
        let force = tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
                continue;
            }
            () = &mut scheduled_sleep => {
                worker.metrics.observe_schedule_delay(
                    runtime.loop_kind(),
                    tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
                );
                false
            }
            () = runtime.wakeup.notified() => true,
        };

        if let Err(error) = run_cycle(worker.as_ref(), &runtime, force).await {
            tracing::warn!(
                error = %error,
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail 邮箱轮询失败；该邮箱将在自身下一轮重试"
            );
        }
    }
}

async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
    force: bool,
) -> Result<(), WorkerError>
where
    C: Clock + 'static,
{
    // 启动时已校验完整配置；下一次触发时间在每轮调度时重新计算。
    if worker.control.is_draining() {
        return Ok(());
    }
    let loop_kind = runtime.loop_kind();
    let iteration_started = std::time::Instant::now();
    let result = if runtime.config.poll_isolation_enabled {
        // 新路径只等待收件；维护由 runner 的独立 JoinSet 监督，不改变 claim/证明/提交边界。
        worker
            .write_lifecycle
            .background(run_cycle_inner(worker, runtime, force))
            .await
            .unwrap_or(Ok(WorkerResult::Canceled))
    } else {
        // 旧路径保留每轮三分支 join 与原观察频率；下一轮仍等待最慢分支完成。
        let (_, _, result) = tokio::join!(
            worker.write_lifecycle.background(run_read_observation(runtime)),
            run_maintenance(runtime, &worker.write_lifecycle),
            worker.write_lifecycle.background(run_cycle_inner(worker, runtime, force)),
        );
        result.unwrap_or(Ok(WorkerResult::Canceled))
    };
    let metric_result = match &result {
        Ok(result) => *result,
        Err(_) => WorkerResult::Error,
    };
    worker.metrics.observe_iteration(loop_kind, metric_result, iteration_started.elapsed());
    result.map(|_| ())
}

async fn run_cycle_inner<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
    force: bool,
) -> Result<WorkerResult, WorkerError>
where
    C: Clock + 'static,
{
    if worker.control.is_draining() {
        return Ok(WorkerResult::Success);
    }
    let claim_started = tokio::time::Instant::now();
    let now = worker.clock.now();
    let lease_duration = ChronoDuration::from_std(runtime.config.lease_duration)
        .map_err(|_| WorkerError::InvalidMailPollConfiguration)?;
    let lease_until =
        now.checked_add_signed(lease_duration).ok_or(WorkerError::InvalidMailPollConfiguration)?;
    let lease_deadline = claim_started
        .checked_add(runtime.config.lease_duration)
        .ok_or(WorkerError::InvalidMailPollConfiguration)?;
    // 一个 runtime 只拥有一个 mailbox 状态；领取上限固定为一，避免让未来异常数据形成预领取队列。
    // 领取超时或回执迟到时不读取远端；可能已提交的 claim 按原租约自然恢复。
    let claims = match tokio::time::timeout_at(
        lease_deadline,
        runtime.store.claim_due(
            runtime.mailbox_key.clone(),
            worker.worker_id.clone(),
            now,
            lease_until,
            1,
            force,
        ),
    )
    .await
    {
        Ok(Ok(claims)) if tokio::time::Instant::now() < lease_deadline => claims,
        Ok(Ok(_)) | Err(_) => {
            tracing::warn!(
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail 邮箱领取未在租约截止前确认；保留可能已提交的 claim"
            );
            return Ok(WorkerResult::Error);
        }
        Ok(Err(error)) => return Err(error.into()),
    };
    if claims.is_empty() {
        return Ok(WorkerResult::Success);
    }

    let mut iteration_result = WorkerResult::Success;
    for claim in claims {
        // 领取可能跨过 drain；不伪造失败或释放，原邮箱租约到期后恢复。
        if worker.control.is_draining() {
            break;
        }
        let context = MailPollProcessContext {
            store: Arc::clone(&runtime.store),
            gateway: Arc::clone(&runtime.gateway),
            processor: runtime.processor.clone(),
            diagnostics: runtime.diagnostics.clone(),
            progress_observer: runtime.progress_observer.clone(),
            clock: Arc::clone(&worker.clock),
            world_id: runtime.world_id.clone(),
            mailbox_key: runtime.mailbox_key.clone(),
            config: runtime.config,
            schedule: Arc::clone(&runtime.schedule),
            metrics: Arc::clone(&worker.metrics),
            loop_kind: runtime.loop_kind(),
        };
        let item_result = process_claim(context, claim, lease_deadline).await;
        super::super::merge_worker_result(&mut iteration_result, item_result);
    }
    Ok(iteration_result)
}

pub(super) struct MailPollProcessContext<C> {
    pub(super) store: Arc<dyn MailPollStore>,
    pub(super) gateway: Arc<dyn MailPollGateway>,
    pub(super) processor: Option<Arc<dyn MailProcessor>>,
    pub(super) diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    pub(super) progress_observer: Option<Arc<dyn auction_application::MailPollProgressObserver>>,
    pub(super) clock: Arc<C>,
    pub(super) world_id: String,
    pub(super) mailbox_key: String,
    pub(super) config: MailPollRuntimeConfig,
    pub(super) schedule: Arc<dyn RecurringSchedule>,
    pub(super) metrics: Arc<dyn WorkerRecorder>,
    pub(super) loop_kind: WorkerLoop,
}
