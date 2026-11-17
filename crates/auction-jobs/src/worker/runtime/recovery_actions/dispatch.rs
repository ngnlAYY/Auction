//! Recovery action 的状态分派与 prepared delivery 构造。

use std::sync::Arc;

use auction_application::admin::RecoveryBuyerBlacklister;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, SafeErrorKind, diagnostic_id,
};
use auction_application::core::{Clock, RequestHash};
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailNotificationTemplate,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_domain::{CommandId, deterministic_uuid};
use auction_observability::WorkerResult;
use auction_time::TimeDelta;
use serde::Serialize;

use super::{
    activate_recovery_action, mark_manual_reconcile, observe_recovery_action,
    process_recovery_buyer_blacklist, release_recovery_action,
};
use crate::worker::diagnostics;

pub(crate) async fn process_recovery_action<C: Clock + 'static>(
    store: Arc<dyn RecoveryWorkStore>,
    delivery: Option<Arc<dyn MailDeliveryEnqueuer>>,
    blacklister: Option<Arc<dyn RecoveryBuyerBlacklister>>,
    clock: Arc<C>,
    retry_delay: TimeDelta,
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
    retry_delay: TimeDelta,
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
        return mark_manual_reconcile(
            store.as_ref(),
            claim,
            "unsupported_recovery_action",
            "recovery_dispatch",
            "persist_unsupported_action",
        )
        .await;
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
                return mark_manual_reconcile(
                    store.as_ref(),
                    claim,
                    "recovery_recipient_missing",
                    "recovery_dispatch",
                    "persist_missing_recipient",
                )
                .await;
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
                return mark_manual_reconcile(
                    store.as_ref(),
                    claim,
                    "recovery_source_mail_missing",
                    "recovery_dispatch",
                    "persist_missing_source_mail",
                )
                .await;
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
        let mut command = recovery_delivery_command(
            &claim,
            is_payment_reminder,
            command_id,
            request_hash,
            recipient,
            external_mail_id,
        );
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
            .link_action_preparation(claim.clone(), delivery_id, command_id, request_hash)
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

/// recovery action 的 prepared delivery 命令：付款提醒固定寄往 main，领取退件沿用 action 的目标
/// 世界与冻结的预期资产；两者都须落在统一投递判定内。
fn recovery_delivery_command(
    claim: &auction_application::recovery::RecoveryActionClaim,
    is_payment_reminder: bool,
    command_id: CommandId,
    request_hash: RequestHash,
    recipient: String,
    external_mail_id: String,
) -> MailDeliveryCommand {
    MailDeliveryCommand {
        command_id,
        request_hash,
        origin: MailDeliveryOrigin::SettlementRecoveryAction,
        origin_id: format!("recovery:{}", claim.recovery_id),
        operation: if is_payment_reminder {
            MailDeliveryOperation::Send
        } else {
            MailDeliveryOperation::Take
        },
        world_id: if is_payment_reminder { "main".to_owned() } else { claim.world_id.clone() },
        external_mail_id,
        recipient,
        template: is_payment_reminder.then_some(MailNotificationTemplate::RecoveryPaymentReminder),
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
    }
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

#[cfg(test)]
mod tests {
    use auction_application::core::RequestHash;
    use auction_application::recovery::RecoveryActionClaim;
    use auction_domain::{CommandId, SettlementId};
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::recovery_delivery_command;

    fn claim(
        kind: &str,
        world_id: &str,
        credits: i64,
        attachments: &[&str],
    ) -> RecoveryActionClaim {
        RecoveryActionClaim {
            action_id: SettlementId::from_uuid(Uuid::new_v4()),
            recovery_id: SettlementId::from_uuid(Uuid::new_v4()),
            kind: kind.to_owned(),
            world_id: world_id.to_owned(),
            buyer_user_id: Uuid::new_v4(),
            recipient: Some("Buyer".to_owned()),
            recipient_eligible: true,
            recipient_state_version: 1,
            payment_reference: Uuid::new_v4().to_string(),
            source_external_mail_id: Some("3125357".to_owned()),
            expected_credits: credits,
            expected_cod: 0,
            expected_attachment_ids: attachments.iter().map(|value| (*value).to_owned()).collect(),
            reason: None,
            execution_state: "pending".to_owned(),
            delivery_id: None,
            prepare_command_id: None,
            prepare_request_hash: None,
            activate_command_id: None,
            activate_request_hash: None,
            state_version: 1,
            claim_token: Uuid::new_v4(),
            lease_until: Timestamp::now(),
            fencing_version: 1,
            retry_count: 0,
            worker_id: "fixture-worker".to_owned(),
            claimed_at: Timestamp::now(),
            created_at: Timestamp::now(),
        }
    }

    /// 付款提醒与三种领取退件的命令都落在统一投递判定内：规范 recovery UUID、main 提醒与
    /// 按冻结预期携带的资产。
    #[test]
    fn recovery_delivery_commands_pass_the_unified_delivery_judgement() {
        let reminder = claim("payment_reminder", "main", 0, &[]);
        let command = recovery_delivery_command(
            &reminder,
            true,
            CommandId::from_uuid(Uuid::new_v4()),
            RequestHash::new([5; 32]),
            "Buyer".to_owned(),
            reminder.payment_reference.clone(),
        );
        assert_eq!(command.shape().check(), Ok(()));
        for (kind, world_id, credits, attachments) in [
            ("reclaim_equipment_take", "main", 0, &["900000001"][..]),
            ("cancel_carrier_take", "main", 1, &[]),
            ("recover_equipment_take", "isekai", 0, &["900000002"]),
        ] {
            let take = claim(kind, world_id, credits, attachments);
            let command = recovery_delivery_command(
                &take,
                false,
                CommandId::from_uuid(Uuid::new_v4()),
                RequestHash::new([6; 32]),
                "Buyer".to_owned(),
                "3125357".to_owned(),
            );
            assert_eq!(command.shape().check(), Ok(()), "{kind}");
        }
    }
}
