//! Recovery relist 与 refund resolution 阶段。

use auction_application::Clock;
use auction_observability::WorkerResult;
use chrono::Duration as ChronoDuration;

use super::{PersistentWorker, merge_worker_result, observe_phase};

pub(super) async fn advance_relists<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
) where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_resolution else {
        return;
    };
    let Ok(retry_delay) = ChronoDuration::from_std(worker.recovery.retry_delay) else {
        tracing::error!("recovery relist retry delay is invalid");
        merge_worker_result(batch_result, WorkerResult::Error);
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        if worker.control.is_draining() {
            break;
        }
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.advance_due_relist(worker.clock.now(), retry_delay),
        )
        .await
        {
            Ok(Ok(Some(advance))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %advance.recovery_id,
                    stage = %advance.stage,
                    resolution = ?advance.resolution,
                    relisted_auction_id = ?advance.relisted_auction_id,
                    relisted_lot_id = ?advance.relisted_lot_id,
                    state_version = advance.state_version,
                    completed = advance.completed,
                    "recovery relist resolution advanced"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery relist phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery relist item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}

pub(super) async fn complete_refunds<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
) where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_resolution else {
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        if worker.control.is_draining() {
            break;
        }
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.complete_due_refund(worker.clock.now()),
        )
        .await
        {
            Ok(Ok(Some(advance))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %advance.recovery_id,
                    stage = %advance.stage,
                    resolution = ?advance.resolution,
                    relisted_auction_id = ?advance.relisted_auction_id,
                    relisted_lot_id = ?advance.relisted_lot_id,
                    state_version = advance.state_version,
                    completed = advance.completed,
                    "recovery refund resolution advanced"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery refund phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery refund item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}
