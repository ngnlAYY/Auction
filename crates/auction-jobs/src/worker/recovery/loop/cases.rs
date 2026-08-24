//! Recovery case 阶段。

use auction_application::Clock;
use auction_observability::WorkerResult;

use super::{PersistentWorker, observe_phase};

pub(super) async fn advance_cases<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    let mut count = 0_usize;
    let mut phase_result = WorkerResult::Success;
    for _ in 0..worker.recovery.batch_limit {
        match tokio::time::timeout(
            worker.recovery.item_timeout,
            store.advance_due_case(worker.clock.now()),
        )
        .await
        {
            Ok(Ok(Some(advance))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %advance.recovery_id,
                    stage = %advance.stage,
                    reminder_count = advance.reminder_count,
                    state_version = advance.state_version,
                    completed = advance.completed,
                    "recovery case advanced"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery case phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery case item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}
