//! Recovery action 的持久化收口：重试释放与人工对账。

use std::sync::Arc;

use auction_application::core::Clock;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_observability::WorkerResult;
use auction_time::TimeDelta;

use crate::worker::diagnostics;

pub(in crate::worker::runtime) async fn release_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    claim: auction_application::recovery::RecoveryActionClaim,
    retry_delay: TimeDelta,
    failure_code: &'static str,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let now = clock.now();
    let Some(retry_at) = now.checked_add_signed(retry_delay) else {
        // 不能把不可表示的重试时间折回 now，否则可能绕过退避并重复执行未知外部结果。
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
        tracing::error!(
            failure_code,
            "recovery action retry timestamp overflow; claim remains leased"
        );
        return WorkerResult::Error;
    };
    if let Err(error) = store.release_action(claim, retry_at, failure_code.to_owned()).await {
        diagnostics::error(
            "recovery_dispatch",
            "release_action",
            error.diagnostic_kind(),
            DiagnosticContext {
                disposition: Some(DiagnosticDisposition::Unpersisted),
                ..Default::default()
            },
        );
        return WorkerResult::Error;
    }
    WorkerResult::Success
}

/// 把 action 转人工对账；写入成功即已持久处置，失败记录未确认处置的 ERROR 并按失败计入。
/// `kind`/`phase` 是调用点的固定诊断类别，失败码由调用方逐字给出。
pub(in crate::worker::runtime) async fn mark_manual_reconcile(
    store: &dyn RecoveryWorkStore,
    claim: auction_application::recovery::RecoveryActionClaim,
    failure_code: &'static str,
    kind: &'static str,
    phase: &'static str,
) -> WorkerResult {
    match store.mark_action_manual_reconcile(claim, failure_code.to_owned()).await {
        Ok(()) => WorkerResult::Success,
        Err(error) => {
            diagnostics::error(
                kind,
                phase,
                error.diagnostic_kind(),
                DiagnosticContext {
                    disposition: Some(DiagnosticDisposition::Unpersisted),
                    ..Default::default()
                },
            );
            WorkerResult::Error
        }
    }
}
