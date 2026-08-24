//! Recovery action 的持久化重试释放。

use super::*;

pub(in crate::worker::runtime) async fn release_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    claim: auction_application::RecoveryActionClaim,
    retry_delay: ChronoDuration,
    failure_code: &'static str,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let now = clock.now();
    let retry_at = now.checked_add_signed(retry_delay).unwrap_or(now);
    if let Err(error) = store.release_action(claim, retry_at, failure_code.to_owned()).await {
        tracing::error!(error = %error, failure_code, "recovery action retry fence failed");
        return WorkerResult::Error;
    }
    WorkerResult::Success
}
