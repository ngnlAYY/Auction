//! Recovery 买家黑名单 action 的幂等执行与 proof 收口。

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError,
};

use super::*;
use crate::worker::diagnostics;

pub(in crate::worker::runtime) async fn process_recovery_buyer_blacklist<C>(
    store: Arc<dyn RecoveryWorkStore>,
    blacklister: Arc<dyn RecoveryBuyerBlacklister>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::recovery::RecoveryActionClaim,
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
    // 理由由退回记录在创建 action 时冻结；历史 action 没有理由时回退默认三次文案。
    let public_reason =
        claim.reason.clone().unwrap_or_else(|| RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON.to_owned());
    if !is_recovery_buyer_blacklist_reason(&public_reason) {
        tracing::error!(action_id = %action_id, "recovery buyer blacklist reason is invalid");
        return match store
            .mark_action_manual_reconcile(claim, "recovery_request_hash_failed".to_owned())
            .await
        {
            Ok(()) => WorkerResult::Success,
            Err(fence_error) => {
                diagnostics::error(
                    "recovery_blacklist",
                    "persist_invalid_reason",
                    fence_error.diagnostic_kind(),
                    DiagnosticContext {
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
                WorkerResult::Error
            }
        };
    }
    let request = RecoveryBuyerBlacklistRequest {
        recovery_id: claim.recovery_id.as_uuid(),
        action_id,
        buyer_user_id: claim.buyer_user_id,
        public_reason: &public_reason,
        actor_id: RECOVERY_BUYER_BLACKLIST_ACTOR,
        occurred_at: claim.created_at,
    };
    let request_hash =
        match RequestHash::from_request("users_recovery_buyer_blacklist", action_id, &request) {
            Ok(hash) => hash,
            Err(error) => {
                diagnostics::error(
                    "recovery_blacklist",
                    "hash_command",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_request_hash_failed".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(fence_error) => {
                        diagnostics::error(
                            "recovery_blacklist",
                            "persist_hash_failure",
                            fence_error.diagnostic_kind(),
                            DiagnosticContext {
                                disposition: Some(DiagnosticDisposition::Unpersisted),
                                ..Default::default()
                            },
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
        public_reason,
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
            let proof_digest = match result.legacy_proof_digest() {
                Ok(proof_digest) => proof_digest,
                Err(error) => {
                    diagnostics::error(
                        "recovery_blacklist",
                        "build_result_proof",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                    return match store
                        .mark_action_manual_reconcile(
                            claim,
                            "recovery_buyer_blacklist_result_invalid".to_owned(),
                        )
                        .await
                    {
                        Ok(()) => WorkerResult::Success,
                        Err(fence_error) => {
                            diagnostics::error(
                                "recovery_blacklist",
                                "persist_invalid_proof",
                                fence_error.diagnostic_kind(),
                                DiagnosticContext {
                                    disposition: Some(DiagnosticDisposition::Unpersisted),
                                    ..Default::default()
                                },
                            );
                            WorkerResult::Error
                        }
                    };
                }
            };
            match store
                .mark_action_satisfied(
                    claim,
                    RECOVERY_BUYER_BLACKLIST_PROOF_KIND.to_owned(),
                    result.command_id.to_string(),
                    proof_digest,
                    clock.now(),
                )
                .await
            {
                Ok(()) => WorkerResult::Success,
                Err(error) => {
                    diagnostics::error(
                        "recovery_blacklist",
                        "persist_completion",
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
        Ok(_) => match store
            .mark_action_manual_reconcile(
                claim,
                "recovery_buyer_blacklist_result_invalid".to_owned(),
            )
            .await
        {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                diagnostics::error(
                    "recovery_blacklist",
                    "persist_invalid_result",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
                WorkerResult::Error
            }
        },
        Err(error) => {
            diagnostics::warn(
                "recovery_blacklist",
                "execute_blacklist",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
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
struct RecoveryBuyerBlacklistRequest<'a> {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    buyer_user_id: uuid::Uuid,
    public_reason: &'a str,
    actor_id: &'static str,
    occurred_at: chrono::DateTime<chrono::Utc>,
}
