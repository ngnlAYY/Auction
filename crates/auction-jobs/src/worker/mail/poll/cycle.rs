//! MoogleMail Worker 的调度、领取和批次上下文。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_world,
};
use auction_application::mail::{MailDiagnosticRecorder, MailPollStore, MailProcessor};
use auction_application::scheduling::RecurringSchedule;
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult};
use chrono::{DateTime, Duration as ChronoDuration, Utc};

use super::super::maintenance::{run_maintenance, run_read_observation};
use super::processing::process_claim;
use super::{MailPollGateway, MailPollRuntime, MailPollRuntimeConfig};
use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, WorkerError, poll_delay};

/// 收件等待期间重读开关的切片上限；不改变原触发点或重复叠加 jitter。
const SWITCH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 等待已装配日程与 `[0, jitter]` 后轮询；手动触发跳过等待和 due 时间。
pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut runtime: MailPollRuntime,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) where
    C: Clock + 'static,
{
    let diagnostic_metrics = worker.metrics.clone();
    let diagnostic_loop = runtime.loop_kind();
    let body = async move {
        loop {
            if *shutdown.borrow() || worker.control.is_draining() {
                return;
            }
            // 暂停只阻止开始新的领取轮；已领取项与已进入围栏的写入按原路径结束。
            if worker.external_access_paused() {
                if crate::worker::runtime::wait_while_paused(&mut shutdown).await {
                    return;
                }
                continue;
            }
            // 每个循环边界只替换预算类热参数；租约与日程仍沿用启动配置。
            if !runtime.config.apply_hot(&worker.hot.snapshot().mail) {
                diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
                tracing::warn!(
                    world_id = %runtime.world_id,
                    mailbox_key = %runtime.mailbox_key,
                    "热读取的 MoogleMail 预算与当前租约不一致，继续使用上一次有效配置"
                );
            }
            let mut isolated = runtime.switches.mooglemail_poll_isolation_enabled();
            let mut schedule = poll_schedule(&runtime, isolated);
            let Some(mut scheduled_wake) =
                next_poll_wake(&runtime, schedule.as_ref(), worker.clock.now())
            else {
                diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
                tracing::error!(
                    world_id = %runtime.world_id,
                    mailbox_key = %runtime.mailbox_key,
                    "MoogleMail cron 无法计算下一次触发时刻，停止该邮箱循环"
                );
                return;
            };
            let force = loop {
                let now = tokio::time::Instant::now();
                let slice = scheduled_wake.saturating_duration_since(now).min(SWITCH_POLL_INTERVAL);
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            return;
                        }
                        break None;
                    }
                    () = tokio::time::sleep_until(now + slice) => {
                        let current = runtime.switches.mooglemail_poll_isolation_enabled();
                        if current != isolated {
                            isolated = current;
                            schedule = poll_schedule(&runtime, isolated);
                            let Some(next_wake) =
                                next_poll_wake(&runtime, schedule.as_ref(), worker.clock.now())
                            else {
                                diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
                                tracing::error!(
                                    world_id = %runtime.world_id,
                                    mailbox_key = %runtime.mailbox_key,
                                    "MoogleMail cron 无法在切换后计算下一次触发时刻，停止该邮箱循环"
                                );
                                return;
                            };
                            scheduled_wake = next_wake;
                            continue;
                        }
                        if tokio::time::Instant::now() >= scheduled_wake {
                            worker.metrics.observe_schedule_delay(
                                runtime.loop_kind(),
                                tokio::time::Instant::now()
                                    .saturating_duration_since(scheduled_wake),
                            );
                            break Some(false);
                        }
                    }
                    () = runtime.wakeup.notified() => break Some(true),
                }
            };
            let Some(force) = force else { continue };
            // 手动触发或到期时再读一次，让本轮使用醒来的那一时刻的模式与日程。
            isolated = runtime.switches.mooglemail_poll_isolation_enabled();
            schedule = poll_schedule(&runtime, isolated);

            if let Err(error) =
                run_cycle(worker.as_ref(), &runtime, force, isolated, schedule, &mut shutdown).await
            {
                diagnostics::unpersisted_error(
                    "mail_poll",
                    "poll_mailbox",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        world: diagnostic_world(&runtime.world_id),
                        ..Default::default()
                    },
                );
            }
        }
    };
    diagnostics::scope(diagnostic_metrics, diagnostic_loop, body).await;
}

async fn run_cycle<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
    force: bool,
    isolated: bool,
    schedule: Arc<dyn RecurringSchedule>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
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
    // 邮件收件与维护属于批量轮次，受共享连接配额限制；许可在本轮结束时释放。
    let _bulk_permit = worker.bulk_permit().await;
    let result = if isolated {
        // 新路径只等待收件；维护由 runner 的独立 JoinSet 监督，不改变 claim/证明/提交边界。
        worker
            .write_lifecycle
            .background(run_cycle_inner(worker, runtime, force, schedule))
            .await
            .unwrap_or(Ok(WorkerResult::Canceled))
    } else {
        // 旧路径在 claim 前取得世界维护门，并持有到三分支 join 结束；等锁可被停止打断。
        let Some(_maintenance_guard) =
            super::super::acquire_maintenance_lock(worker, runtime, shutdown).await
        else {
            return Ok(());
        };
        // 保留每轮三分支 join 与原观察频率；下一轮仍等待最慢分支完成。
        let (_, _, result) = tokio::join!(
            worker.write_lifecycle.background(run_read_observation(runtime)),
            run_maintenance(runtime, &worker.write_lifecycle),
            worker.write_lifecycle.background(run_cycle_inner(worker, runtime, force, schedule)),
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
    schedule: Arc<dyn RecurringSchedule>,
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
        diagnostic_outcome @ (Ok(Ok(_)) | Err(..)) => {
            diagnostics::warn(
                "mail_poll",
                if diagnostic_outcome.is_err() {
                    "claim_mailbox_timeout"
                } else {
                    "claim_mailbox_expired"
                },
                if diagnostic_outcome.is_err() {
                    SafeErrorKind::TIMEOUT
                } else {
                    SafeErrorKind::LEASE_EXPIRED
                },
                DiagnosticContext::default(),
            );
            drop(diagnostic_outcome);

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
            schedule: Arc::clone(&schedule),
            metrics: Arc::clone(&worker.metrics),
            external_metrics: Arc::clone(&runtime.external_metrics),
            loop_kind: runtime.loop_kind(),
        };
        let item_result = process_claim(context, claim, lease_deadline).await;
        crate::worker::runtime::merge_worker_result(&mut iteration_result, item_result);
    }
    Ok(iteration_result)
}

fn poll_schedule(runtime: &MailPollRuntime, isolated: bool) -> Arc<dyn RecurringSchedule> {
    if isolated { Arc::clone(&runtime.phased_schedule) } else { Arc::clone(&runtime.schedule) }
}

fn next_poll_wake(
    runtime: &MailPollRuntime,
    schedule: &dyn RecurringSchedule,
    now: DateTime<Utc>,
) -> Option<tokio::time::Instant> {
    let next_wake_at = schedule.next_after(now)?;
    let schedule_delay =
        next_wake_at.signed_duration_since(now).to_std().ok().filter(|delay| !delay.is_zero())?;
    Some(tokio::time::Instant::now() + poll_delay(schedule_delay, runtime.config.jitter))
}

pub(super) struct MailPollProcessContext<C> {
    pub(super) store: Arc<dyn MailPollStore>,
    pub(super) gateway: Arc<dyn MailPollGateway>,
    pub(super) processor: Option<Arc<dyn MailProcessor>>,
    pub(super) diagnostics: Option<Arc<dyn MailDiagnosticRecorder>>,
    pub(super) progress_observer:
        Option<Arc<dyn auction_application::mail::MailPollProgressObserver>>,
    pub(super) clock: Arc<C>,
    pub(super) world_id: String,
    pub(super) mailbox_key: String,
    pub(super) config: MailPollRuntimeConfig,
    pub(super) schedule: Arc<dyn RecurringSchedule>,
    pub(super) metrics: Arc<dyn WorkerRecorder>,
    pub(super) external_metrics:
        Arc<dyn auction_application::external_metrics::ExternalMetricsObserver>,
    pub(super) loop_kind: WorkerLoop,
}
