//! Recovery 买家黑名单 action 的幂等执行与 proof 收口。

use super::*;

pub(in crate::worker::runtime) async fn process_recovery_buyer_blacklist<C>(
    store: Arc<dyn RecoveryWorkStore>,
    blacklister: Arc<dyn RecoveryBuyerBlacklister>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if claim.execution_state != "pending" {
        tracing::error!(
            state = %claim.execution_state,
            "recovery buyer blacklist action state is not executable"
        );
        return WorkerResult::Error;
    }
    let action_id = claim.action_id.as_uuid();
    let command_id = CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(action_id),
        "recovery-buyer-blacklist-command",
        1,
    ));
    let request = RecoveryBuyerBlacklistRequest {
        recovery_id: claim.recovery_id.as_uuid(),
        action_id,
        buyer_user_id: claim.buyer_user_id,
        public_reason: RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON,
        actor_id: RECOVERY_BUYER_BLACKLIST_ACTOR,
        occurred_at: claim.created_at,
    };
    let request_hash =
        match RequestHash::from_request("users_recovery_buyer_blacklist", action_id, &request) {
            Ok(hash) => hash,
            Err(error) => {
                tracing::error!(error = %error, "recovery buyer blacklist request hash failed");
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_request_hash_failed".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(fence_error) => {
                        tracing::error!(
                            error = %fence_error,
                            "recovery buyer blacklist manual fence failed"
                        );
                        WorkerResult::Error
                    }
                };
            }
        };
    let command = RecoveryBuyerBlacklistCommand {
        command_id: command_id.as_uuid(),
        request_hash,
        recovery_id: claim.recovery_id,
        action_id: claim.action_id,
        buyer_user_id: claim.buyer_user_id,
        public_reason: RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON.to_owned(),
        actor_id: RECOVERY_BUYER_BLACKLIST_ACTOR.to_owned(),
        occurred_at: claim.created_at,
    };
    match blacklister.ensure_buyer_blacklisted(command).await {
        Ok(result)
            if result.validate()
                && result.command_id == command_id.as_uuid()
                && result.recovery_id == claim.recovery_id
                && result.action_id == claim.action_id
                && result.buyer_user_id == claim.buyer_user_id
                && result.occurred_at == claim.created_at =>
        {
            match store
                .mark_action_satisfied(
                    claim,
                    RECOVERY_BUYER_BLACKLIST_PROOF_KIND.to_owned(),
                    result.command_id.to_string(),
                    clock.now(),
                )
                .await
            {
                Ok(()) => WorkerResult::Success,
                Err(error) => {
                    tracing::error!(error = %error, "recovery buyer blacklist proof fence failed");
                    WorkerResult::Error
                }
            }
        }
        Ok(_) => match store
            .mark_action_manual_reconcile(
                claim,
                "recovery_buyer_blacklist_result_invalid".to_owned(),
            )
            .await
        {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::error!(error = %error, "recovery buyer blacklist result fence failed");
                WorkerResult::Error
            }
        },
        Err(error) => {
            tracing::warn!(error = %error, "recovery buyer blacklist owner failed");
            release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_buyer_blacklist_unavailable",
            )
            .await
        }
    }
}

#[derive(Debug, Serialize)]
struct RecoveryBuyerBlacklistRequest {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    buyer_user_id: uuid::Uuid,
    public_reason: &'static str,
    actor_id: &'static str,
    occurred_at: chrono::DateTime<chrono::Utc>,
}
