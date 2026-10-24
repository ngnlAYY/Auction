//! MoogleMail Worker 的调度、领取和批次上下文。

use std::sync::Arc;

use auction_application::{
    Clock, MailDiagnosticRecorder, MailPollStore, MailProcessor, RecurringSchedule,
};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult};
use chrono::Duration as ChronoDuration;

use super::super::{PersistentWorker, WorkerError, poll_delay};
use super::processing::process_claim;
use super::{MailPollGateway, MailPollRuntime, MailPollRuntimeConfig};

/// 按旧版语义独立运行一个邮箱：等待下一 cron 时刻与 `[0, jitter]` 后轮询，手动触发跳过等待和 due 时间。
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
        let Ok(schedule_delay) = next_wake_at.signed_duration_since(now).to_std() else {
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
    if !runtime.validate() {
        return Err(WorkerError::InvalidMailPollConfiguration);
    }
    if worker.control.is_draining() {
        return Ok(());
    }
    let loop_kind = runtime.loop_kind();
    let iteration_started = std::time::Instant::now();
    // 三条 future 各自有界；慢 Read Mail 或 Sent 不延迟入站领取和另一类观察。
    // 本轮统一持有、取消和排空，不派生无人持有的后台任务。
    let (_, _, result) = tokio::join!(
        run_read_observation(runtime),
        run_maintenance(runtime),
        run_cycle_inner(worker, runtime, force),
    );
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
    let claims = runtime
        .store
        .claim_due(
            runtime.mailbox_key.clone(),
            worker.worker_id.clone(),
            now,
            lease_until,
            1,
            force,
        )
        .await?;
    if claims.is_empty() {
        return Ok(WorkerResult::Success);
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
    let mut iteration_result = WorkerResult::Success;
    for claim in claims {
        let item_result = process_claim(context.clone(), claim, lease_deadline).await;
        super::super::merge_worker_result(&mut iteration_result, item_result);
    }
    Ok(iteration_result)
}

/// Read Mail 的周期观察独立于发送核对；只定位既有待读/动作目标，不创建历史入站命令。
async fn run_read_observation(runtime: &MailPollRuntime) {
    let Some(maintenance) = &runtime.maintenance else { return };
    let deadline = tokio::time::Instant::now() + runtime.config.fetch_timeout;
    match tokio::time::timeout_at(
        deadline,
        maintenance.observe_read_mail(&runtime.world_id, deadline.into_std()),
    )
    .await
    {
        Ok(Ok(observed)) => tracing::info!(
            world_id = %runtime.world_id,
            read_observed = observed,
            "MoogleMail Read Mail 共享观察完成"
        ),
        Ok(Err(error)) => tracing::warn!(
            world_id = %runtime.world_id,
            error = %error,
            "MoogleMail Read Mail 观察失败；不撤销其它页面处理"
        ),
        Err(_) => tracing::warn!(
            world_id = %runtime.world_id,
            "MoogleMail Read Mail 观察预算耗尽；不撤销其它页面处理"
        ),
    }
}

async fn run_maintenance(runtime: &MailPollRuntime) {
    let Some(maintenance) = &runtime.maintenance else { return };
    // 一次维护共用读取预算；等待数据库、凭据排队、列表/详情及 compose/discard 都不得重置截止点。
    let deadline = tokio::time::Instant::now() + runtime.config.fetch_timeout;
    let result = tokio::time::timeout_at(
        deadline,
        maintenance.sweep(&runtime.world_id, deadline.into_std()),
    )
    .await;
    match result {
        Ok(Ok(report)) => {
            tracing::info!(
                world_id = %runtime.world_id,
                compose_clean = report.compose_clean,
                compose_error = ?report.compose_error,
                compose_matches = report.compose_matches,
                discarded = report.discarded,
                matched_delivery = ?report.matched_delivery,
                sent_candidates = report.sent_candidates,
                sent_matched = report.sent_matched,
                sent_state_failed = report.sent_state_failed,
                sent_state_error = ?report.sent_state_error,
                sent_observed = report.sent_observed,
                sent_recovered = report.sent_recovered,
                sweep_requeued = report.sweep_requeued,
                "MoogleMail 共享读取维护完成"
            );
        }
        Err(_) => {
            tracing::warn!(
                world_id = %runtime.world_id,
                "MoogleMail 维护总预算耗尽；保留未知外写围栏，不撤销并行 Inbox 处理"
            );
        }
        Ok(Err(_)) if tokio::time::Instant::now() >= deadline => {
            tracing::warn!(
                world_id = %runtime.world_id,
                "MoogleMail 维护总预算耗尽；保留未知外写围栏，不撤销并行 Inbox 处理"
            );
        }
        Ok(Err(error)) => {
            // 维护失败不阻断收件轮询，也不把读到的部分观察当作成功。
            tracing::warn!(
                error = %error,
                world_id = %runtime.world_id,
                "MoogleMail 共享读取维护失败；保留未知外写围栏，不撤销并行 Inbox 处理"
            );
        }
    }
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

impl<C> Clone for MailPollProcessContext<C> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            gateway: Arc::clone(&self.gateway),
            processor: self.processor.clone(),
            diagnostics: self.diagnostics.clone(),
            progress_observer: self.progress_observer.clone(),
            clock: Arc::clone(&self.clock),
            world_id: self.world_id.clone(),
            mailbox_key: self.mailbox_key.clone(),
            config: self.config,
            schedule: Arc::clone(&self.schedule),
            metrics: Arc::clone(&self.metrics),
            loop_kind: self.loop_kind,
        }
    }
}
