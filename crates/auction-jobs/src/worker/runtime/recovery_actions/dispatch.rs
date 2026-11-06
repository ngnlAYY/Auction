//! Recovery action 的状态分派与 prepared delivery 构造。

use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind, diagnostic_id,
};

use super::*;
use crate::worker::diagnostics;

pub(crate) async fn process_recovery_action<C: Clock + 'static>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::recovery::RecoveryActionClaim,
) -> WorkerResult {
    match diagnostics::catch_item_panic(
        "recovery_dispatch",
        process_recovery_action_inner(
            Arc::clone(&store),
            delivery,
            blacklister,
            clock,
            retry_delay,
            claim.clone(),
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(()) => {
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                store.mark_action_manual_reconcile(claim, "task_panic".to_owned()),
            )
            .await
            {
                Ok(Ok(())) => {}
                _ => diagnostics::unpersisted(SafeErrorKind::TASK_PANIC),
            }
            WorkerResult::Error
        }
    }
}

#[tracing::instrument(

    skip_all,

    fields(

        action_id = %claim.action_id,

    )

)]
async fn process_recovery_action_inner<C>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    clock: Arc<C>,
    retry_delay: ChronoDuration,
    claim: auction_application::recovery::RecoveryActionClaim,
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
                diagnostics::error(
                    "recovery_dispatch",
                    "persist_unsupported_action",
                    error.diagnostic_kind(),
                    DiagnosticContext {
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
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
        diagnostics::info(
            "recovery_dispatch",
            "require_delivery_owner",
            SafeErrorKind::CAPABILITY_UNAVAILABLE,
            DiagnosticContext {
                action_id: diagnostic_id(&claim.action_id),
                defer_reason: Some("recovery_delivery_unavailable"),
                ..Default::default()
            },
        );
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
                        diagnostics::error(
                            "recovery_dispatch",
                            "persist_missing_recipient",
                            error.diagnostic_kind(),
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
                        diagnostics::error(
                            "recovery_dispatch",
                            "persist_missing_source_mail",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                disposition: Some(DiagnosticDisposition::Unpersisted),
                                ..Default::default()
                            },
                        );
                        WorkerResult::Error
                    }
                };
            };
            source_external_mail_id
        };
        let legacy_command_id = CommandId::from_uuid(deterministic_uuid(
            CommandId::from_uuid(action_id),
            if is_payment_reminder {
                "recovery-payment-reminder-prepare"
            } else {
                "recovery-take-prepare"
            },
            1,
        ));
        // action 可跨多次退件复用；MID 固定本轮，领取版本/租约变化不能改变命令身份。
        let mut command_id = if is_payment_reminder {
            legacy_command_id
        } else {
            CommandId::from_uuid(deterministic_uuid(
                CommandId::from_uuid(action_id),
                &format!("recovery-take-prepare:{external_mail_id}"),
                2,
            ))
        };
        let request_hash = if is_payment_reminder {
            let request = RecoveryPaymentReminderRequest {
                recovery_id: claim.recovery_id.as_uuid(),
                action_id,
                recipient: &recipient,
                payment_reference: &claim.payment_reference,
            };
            RequestHash::from_request("recovery_payment_reminder", action_id, &request)
        } else {
            let request = RecoveryTakeRequest {
                recovery_id: claim.recovery_id.as_uuid(),
                action_id,
                recipient: &recipient,
                source_external_mail_id: &external_mail_id,
                expected_credits: claim.expected_credits,
                expected_cod: claim.expected_cod,
                expected_attachment_ids: &claim.expected_attachment_ids,
            };
            RequestHash::from_request("recovery_take", action_id, &request)
        };
        let request_hash = match request_hash {
            Ok(hash) => hash,
            Err(error) => {
                diagnostics::error(
                    "recovery_dispatch",
                    "hash_command",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
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
        let mut command = MailDeliveryCommand {
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
        if !is_payment_reminder {
            // 升级前可能已 prepare 提交却未 link；先精确恢复原 ID，避免同 MID 再建一个 take。
            let mut legacy_command = command.clone();
            legacy_command.command_id = legacy_command_id;
            match delivery.find_recovery_preparation(legacy_command).await {
                Ok(Some(_)) => {
                    command_id = legacy_command_id;
                    command.command_id = legacy_command_id;
                }
                Ok(None) => {}
                Err(error) => {
                    diagnostics::warn(
                        "recovery_dispatch",
                        "read_legacy_delivery",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                    return release_recovery_action(
                        store,
                        clock,
                        claim,
                        retry_delay,
                        "recovery_prepare_identity_unproven",
                    )
                    .await;
                }
            }
        }
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
                diagnostics::warn(
                    "recovery_dispatch",
                    "prepare_delivery",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
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
                diagnostics::error(
                    "recovery_dispatch",
                    "link_delivery",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
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

#[derive(Debug, Serialize)]
struct RecoveryPaymentReminderRequest<'a> {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    recipient: &'a str,
    payment_reference: &'a str,
}

#[derive(Debug, Serialize)]
struct RecoveryTakeRequest<'a> {
    recovery_id: uuid::Uuid,
    action_id: uuid::Uuid,
    recipient: &'a str,
    source_external_mail_id: &'a str,
    expected_credits: i64,
    expected_cod: i64,
    expected_attachment_ids: &'a [String],
}
