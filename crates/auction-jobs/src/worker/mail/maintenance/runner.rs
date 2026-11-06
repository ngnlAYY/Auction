//! 每世界独立维护循环；cron 只唤醒原持久恢复服务，低频观察不参与业务资格或完成判定。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::SafeErrorKind;
use auction_observability::WorkerLoop;

use super::super::poll::MailPollRuntime;
use super::{metrics::IterationGuard, run_maintenance, run_read_observation};
use crate::worker::diagnostics;
use crate::worker::runtime::{PersistentWorker, poll_delay};

/// 开关重读与 idle 等待的切片上限；正常退出视为致命，因此旧路径只能等待停止。
const SWITCH_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

enum WaitOutcome {
    Due,
    ModeChanged,
}

/// 先完成 Sent/compose 维护，再按内存间隔做完整 Read Mail 纯观察；手动收件不唤醒此循环。
pub(crate) async fn run_loop<C>(
    worker: Arc<PersistentWorker<C>>,
    mut runtime: MailPollRuntime,
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
    let mut isolated = runtime.switches.mooglemail_poll_isolation_enabled();
    loop {
        if *shutdown.borrow() || worker.control.is_draining() {
            return;
        }
        // W1.9：维护与只读观察都是外部读取；暂停期间不开始新一轮。
        if worker.external_access_paused() {
            if crate::worker::runtime::wait_while_paused(&mut shutdown).await {
                return;
            }
            continue;
        }
        if !runtime.config.apply_hot(&worker.hot.snapshot().mail) {
            diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
            tracing::warn!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "热读取的 MoogleMail 预算与当前租约不一致，维护继续使用上一次有效配置"
            );
        }
        let current = runtime.switches.mooglemail_poll_isolation_enabled();
        if current != isolated {
            isolated = current;
            // 从旧路径进入隔离时重新起算观察间隔，避免沿用旧路径的每轮观察节奏。
            if isolated {
                last_observation = tokio::time::Instant::now();
            }
        }
        if !isolated {
            // 旧路径由收件循环执行维护；本任务保持存活并周期重读，正常退出会被 runner 视为致命。
            if wait_for_switch(&mut shutdown, SWITCH_POLL_INTERVAL).await {
                return;
            }
            continue;
        }

        let Some(scheduled_wake) = next_maintenance_wake(&runtime, worker.clock.now()) else {
            diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
            tracing::error!(
                world_id = %runtime.world_id,
                mailbox_key = %runtime.mailbox_key,
                "MoogleMail 维护 cron 无法计算未来触发时刻，停止该维护循环"
            );
            return;
        };
        match wait_for_maintenance(&worker, &runtime, &mut shutdown, scheduled_wake).await {
            Some(WaitOutcome::Due) => {}
            Some(WaitOutcome::ModeChanged) => {
                isolated = false;
                continue;
            }
            None => return,
        }
        worker.metrics.observe_schedule_delay(
            maintenance_loop,
            tokio::time::Instant::now().saturating_duration_since(scheduled_wake),
        );
        let Some(guard) = acquire_maintenance_lock(&worker, &runtime, &mut shutdown).await else {
            return;
        };
        // 等锁期间可能已切回旧路径；锁内复读后才允许执行，避免与收件循环的 join 重叠。
        if !runtime.switches.mooglemail_poll_isolation_enabled() {
            drop(guard);
            isolated = false;
            continue;
        }
        let iteration = IterationGuard::new(Arc::clone(&worker.metrics), maintenance_loop);
        // 在途 sweep 沿用原预算排空，避免在 discard POST 中途因停止信号另建重试授权。
        // W12.5：维护轮次受共享连接配额限制，许可只覆盖本轮 sweep，不在 idle 等待期间占用。
        let bulk_permit = worker.bulk_permit().await;
        iteration.finish(
            diagnostics::scope(
                worker.metrics.clone(),
                maintenance_loop,
                run_maintenance(&runtime, &worker.write_lifecycle),
            )
            .await,
        );
        drop(bulk_permit);

        if last_observation.elapsed() >= runtime.config.read_mail_observation_interval {
            // 仅在 sweep 之后检查，实际观察间隔向上取整到维护节奏；失败也消耗这次间隔。
            last_observation = tokio::time::Instant::now();
            let iteration = IterationGuard::new(Arc::clone(&worker.metrics), observation_loop);
            iteration.finish(
                worker
                    .write_lifecycle
                    .background(diagnostics::scope(
                        worker.metrics.clone(),
                        observation_loop,
                        run_read_observation(&runtime),
                    ))
                    .await
                    .unwrap_or(auction_observability::WorkerResult::Canceled),
            );
        }
        drop(guard);
    }
}

async fn wait_for_switch(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    interval: std::time::Duration,
) -> bool {
    tokio::select! {
        biased;
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        () = tokio::time::sleep(interval) => false,
    }
}

fn next_maintenance_wake(
    runtime: &MailPollRuntime,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<tokio::time::Instant> {
    let delay = runtime
        .maintenance_schedule
        .next_after(now)?
        .signed_duration_since(now)
        .to_std()
        .ok()
        .filter(|delay| !delay.is_zero())?;
    Some(tokio::time::Instant::now() + poll_delay(delay, runtime.config.jitter))
}

async fn wait_for_maintenance<C>(
    worker: &PersistentWorker<C>,
    runtime: &MailPollRuntime,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    scheduled_wake: tokio::time::Instant,
) -> Option<WaitOutcome> {
    loop {
        let now = tokio::time::Instant::now();
        let slice = scheduled_wake.saturating_duration_since(now).min(SWITCH_POLL_INTERVAL);
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                return (!(changed.is_err() || *shutdown.borrow())).then_some(WaitOutcome::Due);
            }
            () = tokio::time::sleep_until(now + slice) => {
                if !runtime.switches.mooglemail_poll_isolation_enabled() {
                    return Some(WaitOutcome::ModeChanged);
                }
                if tokio::time::Instant::now() >= scheduled_wake {
                    return Some(WaitOutcome::Due);
                }
            }
        }
        if worker.control.is_draining() {
            return None;
        }
    }
}

async fn acquire_maintenance_lock<'a, C>(
    worker: &PersistentWorker<C>,
    runtime: &'a MailPollRuntime,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Option<tokio::sync::MutexGuard<'a, ()>> {
    if *shutdown.borrow() || worker.control.is_draining() {
        return None;
    }
    tokio::select! {
        biased;
        changed = shutdown.changed() => {
            drop(changed);
            None
        }
        guard = runtime.maintenance_lock.lock() => {
            (!worker.control.is_draining() && !*shutdown.borrow()).then_some(guard)
        }
    }
}
