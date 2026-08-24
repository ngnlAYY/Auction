//! Recovery action 的准备、激活、观察、重试和买家黑名单处理。
//!
//! 每个外部写入都使用稳定命令/请求摘要与 action fence；结果不确定时释放到持久化重试或人工
//! 对账，不把本地调用返回直接当作外部事实。

use super::*;

pub(super) async fn process_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if !matches!(
        claim.kind.as_str(),
        "payment_reminder"
            | "reclaim_equipment_take"
            | "cancel_carrier_take"
            | "recover_equipment_take"
            | "buyer_blacklist"
    ) {
        let failure_code = "unsupported_recovery_action";
        return match store.mark_action_manual_reconcile(claim, failure_code.to_owned()).await {
            Ok(()) => WorkerResult::Success,
            Err(error) => {
                tracing::error!(error = %error, "recovery action unsupported-kind fence failed");
                WorkerResult::Error
            }
        };
    }

    if claim.kind == "buyer_blacklist" {
        let Some(blacklister) = blacklister else {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_buyer_blacklist_owner_unavailable",
            )
            .await;
        };
        return process_recovery_buyer_blacklist(store, blacklister, clock, retry_delay, claim)
            .await;
    }

    let Some(delivery) = delivery else {
        return release_recovery_action(
            store,
            clock,
            claim,
            retry_delay,
            "recovery_delivery_unavailable",
        )
        .await;
    };

    if claim.execution_state == "pending" {
        let recipient = match claim.recipient.clone() {
            Some(recipient) => recipient,
            None => {
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_recipient_missing".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(error) => {
                        tracing::error!(error = %error, "recovery action recipient fence failed");
                        WorkerResult::Error
                    }
                };
            }
        };
        if !claim.recipient_eligible {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_recipient_ineligible",
            )
            .await;
        }
        let action_id = claim.action_id.as_uuid();
        let is_payment_reminder = claim.kind == "payment_reminder";
        let external_mail_id = if is_payment_reminder {
            claim.payment_reference.clone()
        } else {
            let Some(source_external_mail_id) = claim.source_external_mail_id.clone() else {
                return match store
                    .mark_action_manual_reconcile(claim, "recovery_source_mail_missing".to_owned())
                    .await
                {
                    Ok(()) => WorkerResult::Success,
                    Err(error) => {
                        tracing::error!(error = %error, "recovery action source mail fence failed");
                        WorkerResult::Error
                    }
                };
            };
            source_external_mail_id
        };
        let command_id = CommandId::from_uuid(deterministic_uuid(
            CommandId::from_uuid(action_id),
            if is_payment_reminder {
                "recovery-payment-reminder-prepare"
            } else {
                "recovery-take-prepare"
            },
            1,
        ));
        let request_hash = if is_payment_reminder {
            let request = RecoveryPaymentReminderRequest {
                recovery_id: claim.recovery_id.as_uuid(),
                action_id,
                recipient: recipient.clone(),
                payment_reference: claim.payment_reference.clone(),
            };
            RequestHash::from_request("recovery_payment_reminder", action_id, &request)
        } else {
            let request = RecoveryTakeRequest {
                recovery_id: claim.recovery_id.as_uuid(),
                action_id,
                recipient: recipient.clone(),
                source_external_mail_id: external_mail_id.clone(),
                expected_credits: claim.expected_credits,
                expected_cod: claim.expected_cod,
                expected_attachment_ids: claim.expected_attachment_ids.clone(),
            };
            RequestHash::from_request("recovery_take", action_id, &request)
        };
        let request_hash = match request_hash {
            Ok(hash) => hash,
            Err(error) => {
                tracing::error!(error = %error, "recovery payment reminder request hash failed");
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_request_hash_failed",
                )
                .await;
            }
        };
        let command = MailDeliveryCommand {
            command_id,
            request_hash,
            origin: MailDeliveryOrigin::SettlementRecoveryAction,
            origin_id: format!("recovery:{}", claim.recovery_id),
            operation: if is_payment_reminder {
                MailDeliveryOperation::Send
            } else {
                MailDeliveryOperation::Take
            },
            // 付款提醒固定发往 main；退回领取动作使用 recovery action 对应的目标世界。
            world_id: if is_payment_reminder { "main".to_owned() } else { claim.world_id.clone() },
            external_mail_id,
            recipient,
            template: is_payment_reminder
                .then_some(MailNotificationTemplate::RecoveryPaymentReminder),
            result_code: is_payment_reminder.then_some("payment_overdue".to_owned()),
            user_key_user_id: None,
            credits: if is_payment_reminder { 0 } else { claim.expected_credits },
            cod: if is_payment_reminder { 0 } else { claim.expected_cod },
            attachment_ids: if is_payment_reminder {
                Vec::new()
            } else {
                claim.expected_attachment_ids.clone()
            },
            notification_body: None,
        };
        let delivery_id = match delivery.prepare_recovery(command).await {
            Ok(result) if !result.delivery_id.is_nil() => result.delivery_id,
            Ok(_) => {
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_delivery_result_invalid",
                )
                .await;
            }
            Err(error) => {
                tracing::warn!(error = %error, "recovery delivery prepare failed");
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_delivery_unavailable",
                )
                .await;
            }
        };
        let linked = match store
            .link_action_preparation(
                claim.clone(),
                delivery_id,
                command_id,
                request_hash,
                clock.now(),
            )
            .await
        {
            Ok(claim) => claim,
            Err(error) => {
                tracing::error!(error = %error, "recovery action prepared link failed");
                return release_recovery_action(
                    store,
                    clock,
                    claim,
                    retry_delay,
                    "recovery_delivery_link_failed",
                )
                .await;
            }
        };
        return activate_recovery_action(store, delivery, clock, retry_delay, linked).await;
    }

    if claim.execution_state == "prepared" {
        if !claim.recipient_eligible {
            return release_recovery_action(
                store,
                clock,
                claim,
                retry_delay,
                "recovery_recipient_ineligible",
            )
            .await;
        }
        return activate_recovery_action(store, delivery, clock, retry_delay, claim).await;
    }

    if claim.execution_state == "enqueued" {
        return observe_recovery_action(store, clock, retry_delay, claim).await;
    }

    tracing::error!(state = %claim.execution_state, "recovery action state is not executable");
    WorkerResult::Error
}

/// 激活 recovery 的 prepared delivery，并以 action fence 记录 activation identity。
///
/// delivery 状态和 recovery action 不在同一张表中，因此允许在两次本地提交之间崩溃；
/// 下一轮会用相同 identity 重放 activation，再由 action fence 决定是否能够继续观察结果。
async fn activate_recovery_action<C>(
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
    let activate_command_id = CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(claim.action_id.as_uuid()),
        "recovery-delivery-activate",
        1,
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

async fn process_recovery_buyer_blacklist<C>(
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

async fn observe_recovery_action<C>(
    store: Arc<dyn RecoveryWorkStore>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::RecoveryActionClaim,
) -> WorkerResult
where
    C: Clock + 'static,
{
    match store.observe_action(claim.clone(), clock.now()).await {
        Ok(RecoveryActionOutcome::Retry) => {
            release_recovery_action(store, clock, claim, retry_delay, "recovery_delivery_pending")
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
            tracing::warn!(error = %error, "recovery action observation failed");
            release_recovery_action(store, clock, claim, retry_delay, "recovery_observation_failed")
                .await
        }
    }
}

async fn release_recovery_action<C>(
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
#[derive(Debug, Serialize)]
struct RecoveryPaymentReminderRequest {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    recipient: String,
    payment_reference: String,
}

#[derive(Debug, Serialize)]
struct RecoveryTakeRequest {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    recipient: String,
    source_external_mail_id: String,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RecoveryActivationRequest {
    action_id: uuid::Uuid,
    delivery_id: uuid::Uuid,
    prepare_command_id: uuid::Uuid,
    prepare_request_hash: [u8; 32],
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
