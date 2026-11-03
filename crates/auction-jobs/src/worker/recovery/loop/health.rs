//! Recovery health projection 阶段。

use auction_application::Clock;
use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::WorkerResult;

use super::{PersistentWorker, merge_worker_result};
use crate::worker::diagnostics;

pub(super) async fn refresh_health<C>(worker: &PersistentWorker<C>, batch_result: &mut WorkerResult)
where
    C: Clock + 'static,
{
    let Some(health) = &worker.recovery_health else {
        return;
    };
    match tokio::time::timeout(worker.recovery.item_timeout, health.run_once(worker.clock.now()))
        .await
    {
        Ok(Ok(true)) => tracing::info!("recovery health projection refreshed"),
        Ok(Ok(false)) => {}
        Ok(Err(error)) => {
            diagnostics::unpersisted_error(
                "recovery_health",
                "refresh_projection",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            merge_worker_result(batch_result, WorkerResult::Error);
        }
        Err(error) => {
            diagnostics::unpersisted_error(
                "recovery_health",
                "refresh_projection_timeout",
                diagnostics::timeout_kind(&error),
                DiagnosticContext::default(),
            );

            merge_worker_result(batch_result, WorkerResult::Error);
        }
    }
}
