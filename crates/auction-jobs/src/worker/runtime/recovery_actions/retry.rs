//! Recovery action 的持久化重试释放。

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind,
};

use super::*;
use crate::worker::diagnostics;

pub(in crate::worker::runtime) async fn release_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    claim: auction_application::recovery::RecoveryActionClaim,
    retry_delay: ChronoDuration,
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
