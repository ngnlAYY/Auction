//! Recovery prepared delivery 的激活与 action fence。

use super::*;

pub(in crate::worker::runtime) async fn activate_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Arc<dyn MailDeliveryEnqueuer>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    let (Some(delivery_id), Some(prepare_command_id), Some(prepare_request_hash)) =
        (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
    else {
        return match store
            .mark_action_manual_reconcile(claim, "recovery_prepare_identity_missing".to_owned())
            .await
        {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::error!(error = %error, "recovery prepare identity fence failed");
                WorkerResult::Error
            }
        };
    };
    let legacy_prepare_command_id = CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(claim.action_id.as_uuid()),
        if claim.kind == "payment_reminder" {
            "recovery-payment-reminder-prepare"
        } else {
            "recovery-take-prepare"
        },
        1,
    ));
    // 原 prepared 或已激活未回写的旧任务继续原 activation；新轮次跟随持久 prepare 身份。
    let legacy = prepare_command_id == legacy_prepare_command_id;
    let activate_command_id = CommandId::from_uuid(deterministic_uuid(
        if legacy { CommandId::from_uuid(claim.action_id.as_uuid()) } else { prepare_command_id },
        "recovery-delivery-activate",
        if legacy { 1 } else { 2 },
    ));
    let request = RecoveryActivationRequest {
        action_id: claim.action_id.as_uuid(),
        delivery_id,
        prepare_command_id: prepare_command_id.as_uuid(),
        prepare_request_hash: prepare_request_hash.as_bytes(),
    };
    let activate_request_hash = match RequestHash::from_request(
        "recovery_delivery_activation",
        claim.action_id.as_uuid(),
        &request,
    ) {
        Ok(hash) => hash,
        Err(error) => {
            tracing::error!(error = %error, "recovery delivery activation hash failed");
            return match store
                .mark_action_manual_reconcile(claim, "recovery_activation_hash_failed".to_owned())
                .await
            {
                Ok(()) => WorkerResult::Success,
                Err(fence_error) => {
                    tracing::error!(error = %fence_error, "recovery activation hash fence failed");
                    WorkerResult::Error
                }
            };
        }
    };
    let activated_at = clock.now();
    let activation = MailDeliveryActivation {
        delivery_id,
        prepare_command_id,
        prepare_request_hash,
        activate_command_id,
        activate_request_hash,
        activated_at,
    };
    let result = match delivery.activate_recovery(activation).await {
        Ok(result) if result.delivery_id == delivery_id => result,
        Ok(_) => {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_activation_result_invalid",
            )
            .await;
        }
        Err(error) => {
            tracing::warn!(error = %error, "recovery delivery activation failed");
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_activation_unavailable",
            )
            .await;
        }
    };
    let _ = result;
    let enqueued = match store
        .mark_action_enqueued(
            claim.clone(),
            activate_command_id,
            activate_request_hash,
            activated_at,
        )
        .await
    {
        Ok(claim) => claim,
        Err(error) => {
            tracing::warn!(error = %error, "recovery action activation link failed");
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_activation_link_failed",
            )
            .await;
        }
    };
    observe_recovery_action(store, clock, retry_delay, enqueued).await
}

#[derive(Debug, Serialize)]
struct RecoveryActivationRequest {
    action_id: uuid::Uuid,
    delivery_id: uuid::Uuid,
    prepare_command_id: uuid::Uuid,
    prepare_request_hash: [u8; 32],
}
