//! Worker 各循环共享的调度、排空、重试和指标辅助；不决定任何业务状态迁移。

use super::error::WorkerError;
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use chrono::Duration as ChronoDuration;
use tokio::task::JoinSet;

pub(crate) fn poll_delay(
    poll_interval: std::time::Duration,
    jitter: std::time::Duration,
) -> std::time::Duration {
    if jitter.is_zero() {
        return poll_interval;
    }
    let jitter_nanos = jitter.as_nanos();
    let random_sample = uuid::Uuid::new_v4().as_u128();
    let offset_nanos = random_sample % (jitter_nanos + 1);
    let offset_nanos = u64::try_from(offset_nanos).unwrap_or(u64::MAX);
    poll_interval.saturating_add(std::time::Duration::from_nanos(offset_nanos))
}

pub(super) async fn drain_delivery_tasks(
    tasks: &mut JoinSet<Result<(), WorkerError>>,
) -> Result<(), WorkerError> {
    let mut first_error = None;
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result.map_err(WorkerError::DeliveryTaskJoin).and_then(|result| result)
        {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub(super) async fn drain_worker_tasks(
    tasks: &mut JoinSet<()>,
    group: &'static str,
) -> Result<(), WorkerError> {
    let mut first_error = None;
    while let Some(result) = tasks.join_next().await {
        if let Err(source) = result {
            first_error.get_or_insert(WorkerError::DrainTaskJoin { group, source });
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// 等待下一轮 deadline；收到 drain 后立即跳过休眠，且不再领取新工作。
pub(crate) async fn wait_until_or_shutdown(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        biased;
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        () = tokio::time::sleep_until(deadline) => false,
    }
}
/// 主动暂停期间重读开关的固定切片；所有循环共用同一节奏。
pub(in crate::worker) const PAUSE_REREAD_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(5);

/// 暂停期间的有界等待；返回 true 表示收到 drain。
pub(crate) async fn wait_while_paused(shutdown: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    wait_until_or_shutdown(shutdown, tokio::time::Instant::now() + PAUSE_REREAD_INTERVAL).await
}

pub(super) fn retry_delay(attempt_count: u32) -> ChronoDuration {
    let exponent = attempt_count.saturating_sub(1).min(8);
    let seconds = 5_u64.saturating_mul(1_u64 << exponent).min(15 * 60);
    ChronoDuration::seconds(i64::try_from(seconds).unwrap_or(15 * 60))
}

/// 合并批次内任务结果；取消优先于错误，错误优先于成功。
pub(crate) fn merge_worker_result(current: &mut WorkerResult, next: WorkerResult) {
    match next {
        WorkerResult::Canceled => *current = WorkerResult::Canceled,
        WorkerResult::Error if *current == WorkerResult::Success => {
            *current = WorkerResult::Error;
        }
        WorkerResult::Success | WorkerResult::Error => {}
    }
}

pub(super) fn bind_worker_metric_modes(recorder: &dyn WorkerRecorder) {
    for loop_kind in [WorkerLoop::MailMain, WorkerLoop::MailIsekai] {
        recorder.bind_retry_mode(loop_kind, WorkerRetryKind::Inline);
        recorder.bind_retry_mode(loop_kind, WorkerRetryKind::Persisted);
    }
    for loop_kind in
        [WorkerLoop::Delivery, WorkerLoop::AuctionFulfillment, WorkerLoop::AuctionRecovery]
    {
        recorder.bind_retry_mode(loop_kind, WorkerRetryKind::Persisted);
    }
}
