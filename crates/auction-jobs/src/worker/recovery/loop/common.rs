//! Recovery 阶段结果合并与批次指标。

use auction_application::Clock;
use auction_observability::{WorkerLoop, WorkerResult};

use super::PersistentWorker;

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
