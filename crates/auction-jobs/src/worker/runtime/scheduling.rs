//! Worker 各循环共享的调度、排空、重试和指标辅助；不决定任何业务状态迁移。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticDisposition};
use auction_observability::{WorkerLoop, WorkerRecorder, WorkerResult, WorkerRetryKind};
use auction_time::TimeDelta;
use tokio::task::JoinSet;

use super::error::WorkerError;
use crate::worker::diagnostics;

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

pub(super) fn retry_delay(attempt_count: u32) -> TimeDelta {
    let exponent = attempt_count.saturating_sub(1).min(8);
    let seconds = 5_u64.saturating_mul(1_u64 << exponent).min(15 * 60);
    TimeDelta::seconds(i64::try_from(seconds).unwrap_or(15 * 60))
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

/// 波次子任务 panic 时本轮的处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskPanicPolicy {
    /// 合并为本轮取消；工作项有持久失败端口与租约，原租约到期后由持久状态恢复。
    Cancel,
    /// 记录后把 panic 交给常驻监管失败关闭：没有持久失败端口的到期读取不能无限回扫同一故障项。
    Resume,
}

/// 等待本波全部子任务并合并结果；join 失败记 `item_join` 诊断（abort 或运行时关闭造成的取消
/// 不计未持久化），panic 另按 `kind` 计入 owner 观测，再按 `panic_policy` 合并为本轮取消或交给
/// 监管。`kind` 是诊断类别与 panic owner 映射的固定文本，调用方不得改名。只合并进程内结果，
/// 不写持久状态。
pub(crate) async fn merge_tasks(
    tasks: &mut JoinSet<WorkerResult>,
    phase_result: &mut WorkerResult,
    kind: &'static str,
    panic_policy: TaskPanicPolicy,
) {
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(item_result) => merge_worker_result(phase_result, item_result),
            Err(error) => {
                diagnostics::observe_task_panic(kind, &error);
                diagnostics::error(
                    kind,
                    "item_join",
                    diagnostics::join_kind(&error),
                    DiagnosticContext {
                        disposition: if error.is_cancelled() {
                            None
                        } else {
                            Some(DiagnosticDisposition::Unpersisted)
                        },
                        ..Default::default()
                    },
                );
                if panic_policy == TaskPanicPolicy::Resume && error.is_panic() {
                    std::panic::resume_unwind(error.into_panic());
                }
                *phase_result = WorkerResult::Canceled;
            }
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_observability::{WorkerLoop, WorkerPanicOwner, WorkerResult};
    use tokio::task::JoinSet;

    use super::{TaskPanicPolicy, merge_tasks};
    use crate::worker::diagnostics;
    use crate::worker::runtime::test_fixture::RecordingMetrics;

    #[tokio::test]
    async fn panicked_item_cancels_round_and_counts_its_owner_once() {
        let metrics = Arc::new(RecordingMetrics::default());
        let mut tasks = JoinSet::new();
        tasks.spawn(async { WorkerResult::Success });
        tasks.spawn(async { WorkerResult::Error });
        tasks.spawn(async { panic!("测试 panic 负载不得进入诊断") });
        let mut phase_result = WorkerResult::Success;

        diagnostics::scope(
            metrics.clone(),
            WorkerLoop::AuctionRecovery,
            merge_tasks(
                &mut tasks,
                &mut phase_result,
                "recovery_dispatch",
                TaskPanicPolicy::Cancel,
            ),
        )
        .await;

        // 取消优先于错误；其余子项照常等待收口，不因一个 panic 提前放弃。
        assert_eq!(phase_result, WorkerResult::Canceled);
        assert!(tasks.is_empty());
        assert_eq!(
            metrics.panics(),
            vec![(WorkerLoop::AuctionRecovery, WorkerPanicOwner::RecoveryAction)]
        );
    }

    #[tokio::test]
    async fn aborted_items_cancel_round_without_counting_a_panic() {
        let metrics = Arc::new(RecordingMetrics::default());
        let mut tasks = JoinSet::new();
        tasks.spawn(std::future::pending::<WorkerResult>());
        tasks.abort_all();
        let mut phase_result = WorkerResult::Success;

        // 与生产配对一致：只有慢速代理使用 Resume，其 panic 才会交给监管。
        diagnostics::scope(
            metrics.clone(),
            WorkerLoop::Auction,
            merge_tasks(&mut tasks, &mut phase_result, "slow_proxy", TaskPanicPolicy::Resume),
        )
        .await;

        // abort 或运行时关闭造成的取消不是 panic：不交给监管，也不计入 owner 的 panic 观测。
        assert_eq!(phase_result, WorkerResult::Canceled);
        assert!(metrics.panics().is_empty());
    }

    #[tokio::test]
    async fn resume_policy_hands_item_panic_to_supervisor_after_counting_owner() {
        let metrics = Arc::new(RecordingMetrics::default());
        let merged =
            tokio::spawn(diagnostics::scope(metrics.clone(), WorkerLoop::Auction, async {
                let mut tasks = JoinSet::new();
                tasks.spawn(async { panic!("测试 panic 负载不得进入诊断") });
                let mut phase_result = WorkerResult::Success;
                merge_tasks(&mut tasks, &mut phase_result, "slow_proxy", TaskPanicPolicy::Resume)
                    .await;
            }))
            .await;

        assert!(merged.is_err_and(|error| error.is_panic()), "panic 必须交给常驻监管失败关闭");
        assert_eq!(metrics.panics(), vec![(WorkerLoop::Auction, WorkerPanicOwner::SlowProxy)]);
    }
}
