//! Recovery 阶段的逐项推进模板、结果合并与批次指标。

use auction_application::auction::ApplicationFuture;
use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::{DateTime, Utc};

use super::PersistentWorker;
use crate::worker::diagnostics;

/// 逐项推进一个到期阶段：每项独立超时，排空、无到期项、首个失败或超时都结束本阶段；
/// 成功条目交给 `record` 记录，阶段结果并入批次并按成功批次计量。
pub(super) async fn run_phase<'a, C, T>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
    phase: (&'static str, &'static str),
    next_due: impl Fn(DateTime<Utc>) -> ApplicationFuture<'a, Option<T>>,
    record: impl Fn(&T),
) where
    C: Clock + 'static,
{
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        if worker.control.is_draining() {
            break;
        }
        match tokio::time::timeout(worker.recovery.item_timeout, next_due(worker.clock.now())).await
        {
            Ok(Ok(Some(item))) => {
                count = count.saturating_add(1);
                record(&item);
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                diagnostics::unpersisted_error(
                    "recovery_resolution",
                    phase.0,
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                phase_result = WorkerResult::Error;
                break;
            }
            Err(error) => {
                diagnostics::unpersisted_error(
                    "recovery_resolution",
                    phase.1,
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}

pub(super) fn observe_phase<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
    phase_result: WorkerResult,
    count: usize,
) where
    C: Clock + 'static,
{
    if phase_result == WorkerResult::Success {
        worker.metrics.observe_batch(
            WorkerLoop::AuctionRecovery,
            count,
            usize::from(worker.recovery.batch_limit),
        );
    }
    super::merge_worker_result(batch_result, phase_result);
}
