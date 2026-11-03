//! Recovery case 阶段。

use auction_application::Clock;
use auction_observability::WorkerResult;

use super::{PersistentWorker, run_phase};

pub(super) async fn advance_cases<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(store) = &worker.recovery_store else {
        return;
    };
    run_phase(
        worker,
        batch_result,
        ("advance_case", "advance_case_timeout"),
        |now| store.advance_due_case(now),
        |advance| {
            tracing::info!(
                recovery_id = %advance.recovery_id,
                stage = %advance.stage,
                reminder_count = advance.reminder_count,
                state_version = advance.state_version,
                completed = advance.completed,
                "recovery case advanced"
            );
        },
    )
    .await;
}
