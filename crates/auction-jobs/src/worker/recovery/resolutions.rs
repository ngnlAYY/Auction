//! Recovery COD return、relist 与 refund resolution 阶段。

use auction_application::core::Clock;
use auction_application::recovery::RecoveryResolutionAdvance;
use auction_observability::WorkerResult;
use chrono::Duration as ChronoDuration;

use super::{PersistentWorker, merge_worker_result, run_phase};

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
    run_phase(
        worker,
        batch_result,
        ("advance_relist", "advance_relist_timeout"),
        |now| store.advance_due_relist(now, retry_delay),
        |advance| record(advance, "relist"),
    )
    .await;
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
    run_phase(
        worker,
        batch_result,
        ("complete_refund", "complete_refund_timeout"),
        |now| store.complete_due_refund(now),
        |advance| record(advance, "refund"),
    )
    .await;
}

pub(super) async fn advance_cod_returns<C>(
    worker: &PersistentWorker<C>,
    batch_result: &mut WorkerResult,
) where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_resolution else {
        return;
    };
    run_phase(
        worker,
        batch_result,
        ("advance_cod_return", "advance_cod_return_timeout"),
        |now| store.advance_due_cod_return(now),
        |advance| record(advance, "COD return"),
    )
    .await;
}

fn record(advance: &RecoveryResolutionAdvance, phase: &str) {
    tracing::info!(
        recovery_id = %advance.recovery_id,
        stage = %advance.stage,
        resolution = ?advance.resolution,
        relisted_auction_id = ?advance.relisted_auction_id,
        relisted_lot_id = ?advance.relisted_lot_id,
        state_version = advance.state_version,
        completed = advance.completed,
        "recovery {phase} resolution advanced"
    );
}
