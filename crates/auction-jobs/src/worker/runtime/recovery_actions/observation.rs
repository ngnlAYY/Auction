//! Recovery 已入队 action 的外部结果观察。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::recovery::{RecoveryActionOutcome, RecoveryWorkStore};
use auction_observability::WorkerResult;
use chrono::Duration as ChronoDuration;

use super::release_recovery_action;
use crate::worker::diagnostics;

pub(in crate::worker::runtime) async fn observe_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::recovery::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    match store.observe_action(claim.clone(), clock.now()).await {
        Ok(RecoveryActionOutcome::Retry) => {
            release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_evidence_not_observed",
            )
            .await
        }
        Ok(RecoveryActionOutcome::Satisfied { proof_kind, proof_reference }) => {
            tracing::info!(
                recovery_id = %claim.recovery_id,
                action_id = %claim.action_id,
                proof_kind,
                proof_reference,
                "recovery action satisfied"
            );
            WorkerResult::Success
        }
        Ok(RecoveryActionOutcome::Failed { failure_code }) => {
            tracing::warn!(
                recovery_id = %claim.recovery_id,
                action_id = %claim.action_id,
                failure_code,
                "recovery action failed"
            );
            WorkerResult::Success
        }
        Ok(RecoveryActionOutcome::ManualReconcile { failure_code }) => {
            tracing::error!(
                recovery_id = %claim.recovery_id,
                action_id = %claim.action_id,
                failure_code,
                "recovery action requires manual reconcile"
            );
            WorkerResult::Success
        }
        Err(error) => {
            diagnostics::warn(
                "recovery_observation",
                "observe_action",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            release_recovery_action(store, clock, claim, retry_delay, "recovery_observation_failed")
                .await
        }
    }
}
