//! Recovery return evidence 阶段。

use auction_application::Clock;
use auction_observability::WorkerResult;

use super::{PersistentWorker, run_phase};

pub(super) async fn record_returns<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    run_phase(
        worker,
        batch_result,
        "return",
        |now| store.record_due_return(now),
        |recorded| {
            tracing::info!(
                recovery_id = %recorded.recovery_id,
                return_mail_id = %recorded.return_mail_id,
                stage = %recorded.stage,
                blocking_code = ?recorded.blocking_code,
                action_kinds = ?recorded.action_kinds,
                state_version = recorded.state_version,
                "recovery return evidence recorded"
            );
        },
    )
    .await;
}
