//! Recovery return evidence 阶段。

use auction_application::Clock;
use auction_observability::WorkerResult;

use super::{PersistentWorker, observe_phase};

pub(super) async fn record_returns<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
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
            store.record_due_return(worker.clock.now()),
        )
        .await
        {
            Ok(Ok(Some(recorded))) => {
                count = count.saturating_add(1);
                tracing::info!(
                    recovery_id = %recorded.recovery_id,
                    return_mail_id = %recorded.return_mail_id,
                    stage = %recorded.stage,
                    blocking_code = ?recorded.blocking_code,
                    action_kinds = ?recorded.action_kinds,
                    state_version = recorded.state_version,
                    "recovery return evidence recorded"
                );
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "recovery return phase failed");
                phase_result = WorkerResult::Error;
                break;
            }
            Err(_) => {
                tracing::warn!("recovery return item timed out");
                phase_result = WorkerResult::Error;
                break;
            }
        }
    }
    observe_phase(worker, batch_result, phase_result, count);
}
