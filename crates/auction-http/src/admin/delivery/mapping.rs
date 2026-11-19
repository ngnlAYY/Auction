//! Admin 交付 DTO 映射。

use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminGuardBlocker, DeliveryAdminGuardSnapshot,
    DeliveryAdminSnapshot,
};
use auction_contracts::{
    AdminDeliveryActionAvailability, AdminDeliveryActionBlocker, AdminDeliveryActionsResponse,
    AdminDeliveryResponse,
};

use crate::mapping::timestamp;

pub(super) fn map_actions(
    guard: &DeliveryAdminGuardSnapshot,
    acknowledge: Option<DeliveryAdminGuardBlocker>,
) -> AdminDeliveryActionsResponse {
    let decision = |action| map_availability(guard.projected_blocker(action));
    AdminDeliveryActionsResponse {
        release: decision(DeliveryAdminAction::Release),
        retry_failed: decision(DeliveryAdminAction::RetryFailed),
        reconcile_retry: decision(DeliveryAdminAction::ReconcileRetry),
        reconcile_succeeded: decision(DeliveryAdminAction::ReconcileSucceeded),
        reconcile_failed: decision(DeliveryAdminAction::ReconcileFailed),
        acknowledge: map_availability(acknowledge),
    }
}

fn map_availability(blocker: Option<DeliveryAdminGuardBlocker>) -> AdminDeliveryActionAvailability {
    use DeliveryAdminGuardBlocker as Blocker;

    let reason = match blocker {
        None => return AdminDeliveryActionAvailability::Eligible {},
        Some(Blocker::SettlementReturnActionOwned) => {
            AdminDeliveryActionBlocker::SettlementReturnActionOwned
        }
        Some(Blocker::SettlementIneligible) => AdminDeliveryActionBlocker::SettlementIneligible,
        Some(Blocker::FundedDelivery) => AdminDeliveryActionBlocker::FundedDelivery,
        Some(Blocker::RetiredDelivery) => AdminDeliveryActionBlocker::RetiredDelivery,
        Some(Blocker::StaleFencingVersion) => AdminDeliveryActionBlocker::StaleFencingVersion,
        Some(Blocker::SentSighted) => AdminDeliveryActionBlocker::SentSighted,
        Some(Blocker::ReconcileSucceededForSend) => {
            AdminDeliveryActionBlocker::ReconcileSucceededForSend
        }
        Some(Blocker::DeliveryNotEligible) => AdminDeliveryActionBlocker::DeliveryNotEligible,
        Some(Blocker::StatusMismatch) => AdminDeliveryActionBlocker::StatusMismatch,
        Some(Blocker::QuarantinedMailActionNotAllowed) => {
            AdminDeliveryActionBlocker::QuarantinedMailActionNotAllowed
        }
        Some(Blocker::MissingReconcileReference) => {
            AdminDeliveryActionBlocker::MissingReconcileReference
        }
        Some(Blocker::RecipientRenamed) => AdminDeliveryActionBlocker::RecipientRenamed,
        Some(Blocker::AlreadyDisposed) => AdminDeliveryActionBlocker::AlreadyDisposed,
        Some(Blocker::NotAcknowledgeable) => AdminDeliveryActionBlocker::NotAcknowledgeable,
        Some(Blocker::RecoveryRetryIneligible) => {
            AdminDeliveryActionBlocker::RecoveryRetryIneligible
        }
        Some(Blocker::SettlementFulfillmentState) => {
            AdminDeliveryActionBlocker::SettlementFulfillmentState
        }
        Some(Blocker::MissingSendingFence) => AdminDeliveryActionBlocker::MissingSendingFence,
        Some(Blocker::CommandSnapshotUnreadable) => {
            AdminDeliveryActionBlocker::DeliveryCommandSnapshotUnreadable
        }
    };
    AdminDeliveryActionAvailability::Blocked { reason }
}

/// 将 delivery 应用快照转换为管理端安全 DTO。
pub(crate) fn map_snapshot(snapshot: DeliveryAdminSnapshot) -> AdminDeliveryResponse {
    AdminDeliveryResponse {
        id: snapshot.id.to_string(),
        origin_kind: snapshot.origin_kind,
        origin_id: snapshot.origin_id,
        recipient: snapshot.recipient,
        cod: snapshot.cod,
        status: snapshot.status,
        template: snapshot.template,
        category: snapshot.category,
        world: snapshot.world,
        retirement_reason: snapshot.retirement_reason,
        suppression_reason: snapshot.suppression_reason,
        lease_expires_at: snapshot.lease_expires_at.map(timestamp),
        fencing_version: snapshot.fencing_version,
        attempt_count: snapshot.attempt_count,
        claimed_at: snapshot.claimed_at.map(timestamp),
        sending_at: snapshot.sending_at.map(timestamp),
        completed_at: snapshot.completed_at.map(timestamp),
        external_reference: snapshot.external_reference,
        failure_code: snapshot.failure_code,
        reason_code: snapshot.reason_code,
        take_required: snapshot.take_required,
        returned_mail_id: snapshot.returned_mail_id,
        created_at: timestamp(snapshot.created_at),
        updated_at: timestamp(snapshot.updated_at),
    }
}

/// 将投递详情投影转换为管理端 DTO；摘要转小写 hex，绝不返回正文或凭据秘密。
pub(crate) fn map_detail(
    detail: auction_application::delivery::DeliveryAdminDetail,
    include_sent_proof: bool,
) -> auction_contracts::AdminDeliveryDetailResponse {
    auction_contracts::AdminDeliveryDetailResponse {
        delivery: map_snapshot(detail.delivery),
        attempts: detail
            .attempts
            .into_iter()
            .map(|attempt| auction_contracts::AdminDeliveryAttemptResponse {
                operation: attempt.operation,
                fencing_version: attempt.fencing_version,
                world_id: attempt.world_id,
                sending_at: timestamp(attempt.sending_at),
                recipient: attempt.recipient,
                subject: attempt.subject,
                body_digest: attempt
                    .body_digest
                    .as_ref()
                    .map(|digest| auction_application::core::lower_hex(digest)),
                credits: attempt.credits,
                cod: attempt.cod,
                equipment_ids: attempt.equipment_ids,
                external_mail_id: attempt.external_mail_id,
                credential: attempt.credential.map(|credential| {
                    auction_contracts::AdminDeliveryCredentialResponse {
                        credential_id: credential.credential_id.to_string(),
                        credential_version: credential.credential_version,
                        runtime_generation: credential.runtime_generation,
                    }
                }),
                bound_sent_mail_id: attempt.bound_sent_mail_id,
            })
            .collect(),
        evidence: detail
            .evidence
            .into_iter()
            .map(|item| auction_contracts::AdminDeliveryEvidenceResponse {
                reference: item.reference,
                kind: item.kind,
                external_mail_id: item.external_mail_id,
                observed_at: item.observed_at.map(timestamp),
                sent_at: item.sent_at.map(timestamp),
                recipient: item.recipient,
                subject: item.subject,
                body_digest: item
                    .body_digest
                    .as_ref()
                    .map(|digest| auction_application::core::lower_hex(digest)),
                collected: item.collected,
                cod_pending: item.cod_pending,
                remaining_credits: item.remaining_credits,
                remaining_equipment_count: item.remaining_equipment_count,
                remaining_equipment_ids: include_sent_proof.then_some(item.remaining_equipment_ids),
                pre_collection_equipment_ids: include_sent_proof
                    .then_some(item.pre_collection_equipment_ids),
                receipt: item.receipt,
            })
            .collect(),
        custody: auction_contracts::AdminDeliveryCustodyResponse {
            collected: detail.custody.collected,
            cod_pending: detail.custody.cod_pending,
            remaining_credits: detail.custody.remaining_credits,
            receipt: detail.custody.receipt,
            observed_at: detail.custody.observed_at.map(timestamp),
            operator_confirmed_collected: detail.custody.operator_confirmed_collected,
        },
        dispositions: detail
            .dispositions
            .into_iter()
            .map(|item| auction_contracts::AdminDeliveryDispositionResponse {
                command_id: item.command_id.to_string(),
                kind: item.kind,
                occurred_at: timestamp(item.occurred_at),
                status: item.status,
                fencing_version: item.fencing_version,
                failure_code: item.failure_code,
                external_reference: item.external_reference,
                actor: item.actor,
                reason: item.reason,
            })
            .collect(),
        obligation: detail.obligation.map(|item| {
            auction_contracts::AdminDeliveryObligationResponse {
                obligation_id: item.obligation_id.to_string(),
                auction_id: item.auction_id.to_string(),
                lot_id: item.lot_id.to_string(),
                world_id: item.world_id,
                obligation_kind: item.obligation_kind,
                business_state: item.business_state,
                amount: item.amount,
                fulfillment_state: item.fulfillment_state,
                fulfillment_error_code: item.fulfillment_error_code,
            }
        }),
        recovery: detail.recovery.map(|item| auction_contracts::AdminDeliveryRecoveryResponse {
            recovery_id: item.recovery_id.to_string(),
            stage: item.stage,
            resolution: item.resolution,
            payment_deadline_at: timestamp(item.payment_deadline_at),
            payment_obligation_id: item.payment_obligation_id.to_string(),
            blocking_code: item.blocking_code,
            manual_reconcile_delivery_id: item
                .manual_reconcile_delivery_id
                .map(|id| id.to_string()),
        }),
    }
}

/// 将玩家支持汇总映射为管理端只读 DTO；分区上限与参考编号由应用层唯一校验。
pub(crate) fn map_player_support(
    bundle: auction_application::delivery::PlayerSupportBundle,
) -> auction_contracts::AdminPlayerSupportBundleResponse {
    use auction_contracts::{
        AdminPlayerSupportBidResponse, AdminPlayerSupportBundleResponse,
        AdminPlayerSupportDealResponse, AdminPlayerSupportNotificationResponse,
        AdminPlayerSupportObligationResponse, AdminPlayerSupportReferenceResponse,
        AdminPlayerSupportUserResponse,
    };
    AdminPlayerSupportBundleResponse {
        user: bundle.user.map(|user| AdminPlayerSupportUserResponse {
            id: user.id.to_string(),
            username: user.username,
            status: user.status,
            game_player_id: user.game_player_id,
        }),
        bids: bundle
            .bids
            .into_iter()
            .map(|bid| AdminPlayerSupportBidResponse {
                bid_id: bid.bid_id.to_string(),
                lot_id: bid.lot_id.to_string(),
                auction_id: bid.auction_id.to_string(),
                lot_number: bid.lot_number,
                world_id: bid.world_id,
                amount: bid.amount,
                mechanism: bid.mechanism,
                lot_status: bid.lot_status,
                created_at: timestamp(bid.created_at),
            })
            .collect(),
        deals: bundle
            .deals
            .into_iter()
            .map(|deal| AdminPlayerSupportDealResponse {
                lot_id: deal.lot_id.to_string(),
                auction_id: deal.auction_id.to_string(),
                lot_number: deal.lot_number,
                world_id: deal.world_id,
                equipment_name: deal.equipment_name,
                final_price: deal.final_price,
                effective_end_at: deal.effective_end_at.map(timestamp),
            })
            .collect(),
        obligations: bundle
            .obligations
            .into_iter()
            .map(|obligation| AdminPlayerSupportObligationResponse {
                obligation_id: obligation.obligation_id.to_string(),
                auction_id: obligation.auction_id.to_string(),
                lot_id: obligation.lot_id.to_string(),
                world_id: obligation.world_id,
                obligation_kind: obligation.obligation_kind,
                business_state: obligation.business_state,
                amount: obligation.amount,
                fulfillment_state: obligation.fulfillment_state,
                fulfillment_error_code: obligation.fulfillment_error_code,
                delivery_id: obligation.delivery_id.map(|id| id.to_string()),
            })
            .collect(),
        deliveries: bundle.deliveries.into_iter().map(map_snapshot).collect(),
        notifications: bundle
            .notifications
            .into_iter()
            .map(|notification| AdminPlayerSupportNotificationResponse {
                notification_id: notification.notification_id.to_string(),
                channel: notification.channel,
                kind: notification.kind,
                lot_id: notification.lot_id.map(|id| id.to_string()),
                auction_id: notification.auction_id.map(|id| id.to_string()),
                state: notification.state,
                state_reason: notification.state_reason,
                delivery_id: notification.delivery_id.map(|id| id.to_string()),
                created_at: timestamp(notification.created_at),
                updated_at: timestamp(notification.updated_at),
            })
            .collect(),
        references: bundle
            .references
            .into_iter()
            .map(|reference| AdminPlayerSupportReferenceResponse {
                kind: reference.kind,
                id: reference.id.to_string(),
            })
            .collect(),
        truncated: bundle.truncated,
    }
}

/// 将人工证明预览映射为管理端 DTO；`blockers` 保持闭集字符串，浏览器不推导新结论。
pub(crate) fn map_sent_proof_preview(
    preview: auction_application::delivery::SentConfirmationPreview,
    include_responsibility_reasons: bool,
) -> auction_contracts::AdminDeliverySentProofPreviewResponse {
    auction_contracts::AdminDeliverySentProofPreviewResponse {
        eligible: preview.eligible,
        blockers: preview.blockers,
        delivery_id: preview.delivery_id.to_string(),
        expected_fencing_version: preview.expected_fencing_version,
        external_reference: preview.external_reference,
        requires_collected_asset_acceptance: preview.requires_collected_asset_acceptance,
        requires_attempt_history_acceptance: preview.requires_attempt_history_acceptance,
        prior_unbound_attempts: include_responsibility_reasons
            .then_some(preview.prior_unbound_attempts),
        sent_before_current_attempt: include_responsibility_reasons
            .then_some(preview.sent_before_current_attempt),
        corrects_admin_failure: preview.corrects_admin_failure,
    }
}

/// 将人工证明结果映射为管理端 DTO；重放标志原样保留。
pub(crate) fn map_sent_proof_result(
    result: auction_application::delivery::SentConfirmationResult,
) -> auction_contracts::AdminDeliverySentProofResponse {
    auction_contracts::AdminDeliverySentProofResponse {
        command_id: result.command_id.to_string(),
        delivery_id: result.delivery_id.to_string(),
        external_reference: result.external_reference,
        replayed: result.replayed,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::DeliverySentProofService;
    use auction_application::core::ApplicationFuture;
    use auction_application::delivery::{
        DeliveryAdminAttempt, DeliveryAdminCustody, DeliveryAdminDetail, DeliveryAdminDisposition,
        DeliveryAdminObligation, DeliveryAdminRecovery, DeliveryAdminSentEvidence,
        DeliveryAdminSnapshot, DeliverySendCredentialBinding, DeliverySentConfirmationPort,
        ManualSentEvidence, SentConfirmationCommand, SentConfirmationPreview,
        SentConfirmationResult,
    };
    use auction_contracts::{
        AdminDeliverySentEvidenceRequest, AdminDeliverySentProofPreviewRequest,
    };
    use auction_time::Timestamp;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use uuid::Uuid;

    use super::*;
    use crate::admin::delivery::sent_proof_preview;
    use crate::server::contract_fixtures::{
        AdminFixturePorts, admin_address, admin_get, admin_headers, admin_state, assert_http,
        response_json, wire_fixture,
    };

    fn time() -> auction_time::Timestamp {
        Timestamp::from_ymd_hms(2026, 11, 2, 1, 14, 0).expect("timestamp")
    }

    fn detail() -> DeliveryAdminDetail {
        let credential = DeliverySendCredentialBinding {
            credential_id: uuid::Uuid::from_u128(11),
            credential_version: 2,
            runtime_generation: 3,
        };
        DeliveryAdminDetail {
            delivery: DeliveryAdminSnapshot {
                id: uuid::Uuid::from_u128(1),
                origin_kind: "settlement".to_owned(),
                origin_id: uuid::Uuid::from_u128(2).to_string(),
                recipient: "Sample".to_owned(),
                cod: Some(0),
                status: "manual_reconcile".to_owned(),
                template: Some("settlement_seller_payout".to_owned()),
                category: Some("settlement".to_owned()),
                world: Some("main".to_owned()),
                retirement_reason: None,
                suppression_reason: None,
                lease_expires_at: None,
                fencing_version: 4,
                attempt_count: 1,
                claimed_at: None,
                sending_at: Some(time()),
                completed_at: Some(time()),
                external_reference: None,
                failure_code: Some("write_unknown".to_owned()),
                reason_code: None,
                take_required: false,
                returned_mail_id: None,
                created_at: time(),
                updated_at: time(),
            },
            attempts: vec![DeliveryAdminAttempt {
                operation: "send".to_owned(),
                fencing_version: 4,
                world_id: "main".to_owned(),
                sending_at: time(),
                recipient: Some("Sample".to_owned()),
                subject: Some("Payment".to_owned()),
                body_digest: Some([0xab; 32]),
                credits: Some(5_252_000),
                cod: Some(0),
                equipment_ids: vec!["300753957".to_owned()],
                external_mail_id: None,
                credential: Some(credential),
                bound_sent_mail_id: Some("7000001".to_owned()),
            }],
            evidence: vec![DeliveryAdminSentEvidence {
                reference: "mailbox_sweep:7000001".to_owned(),
                kind: "mailbox_sweep".to_owned(),
                external_mail_id: Some("7000001".to_owned()),
                observed_at: Some(time()),
                sent_at: None,
                recipient: Some("Sample".to_owned()),
                subject: Some("Payment".to_owned()),
                body_digest: Some([0xab; 32]),
                collected: Some(false),
                cod_pending: Some(0),
                remaining_credits: Some(0),
                remaining_equipment_count: Some(0),
                remaining_equipment_ids: None,
                pre_collection_equipment_ids: None,
                receipt: None,
            }],
            custody: DeliveryAdminCustody {
                collected: Some(false),
                cod_pending: Some(0),
                remaining_credits: Some(0),
                receipt: None,
                observed_at: Some(time()),
                operator_confirmed_collected: false,
            },
            dispositions: vec![DeliveryAdminDisposition {
                command_id: uuid::Uuid::from_u128(3),
                kind: "reconcile".to_owned(),
                occurred_at: time(),
                status: Some("manual_reconcile".to_owned()),
                fencing_version: Some(4),
                failure_code: Some("write_unknown".to_owned()),
                external_reference: None,
                actor: None,
                reason: None,
            }],
            obligation: Some(DeliveryAdminObligation {
                obligation_id: uuid::Uuid::from_u128(4),
                auction_id: uuid::Uuid::from_u128(5),
                lot_id: uuid::Uuid::from_u128(6),
                world_id: "main".to_owned(),
                obligation_kind: "seller_payout".to_owned(),
                business_state: "ready".to_owned(),
                amount: 300_000,
                fulfillment_state: Some("enqueued".to_owned()),
                fulfillment_error_code: None,
            }),
            recovery: Some(DeliveryAdminRecovery {
                recovery_id: uuid::Uuid::from_u128(7),
                stage: "awaiting_payment".to_owned(),
                resolution: None,
                payment_deadline_at: time(),
                payment_obligation_id: uuid::Uuid::from_u128(4),
                blocking_code: Some("delivery_manual_reconcile".to_owned()),
                manual_reconcile_delivery_id: Some(uuid::Uuid::from_u128(1)),
            }),
        }
    }

    #[test]
    fn detail_dto_is_a_redacted_projection() {
        let value = serde_json::to_value(map_detail(detail(), false)).expect("serialize");
        let encoded = serde_json::to_string(&value).expect("encode");
        // 详情不得泄露 payload 原文、lease token、Cookie、登录 key 或命令摘要。
        for forbidden in [
            "payload",
            "lease_token",
            "request_digest",
            "command_hash",
            "cookie",
            "password",
            "login_key",
            "body\"",
        ] {
            assert!(!encoded.contains(forbidden), "detail leaked {forbidden}");
        }
        let credential = &value["attempts"][0]["credential"];
        assert_eq!(
            credential.as_object().expect("credential object").keys().cloned().collect::<Vec<_>>(),
            vec!["credential_id", "credential_version", "runtime_generation"]
        );
        assert_eq!(
            value["attempts"][0]["body_digest"],
            "abababababababababababababababababababababababababababababababab"
        );
        // 已发布 wire 省略 false，浏览器以同一默认值补齐。
        assert!(value["custody"].get("operator_confirmed_collected").is_none());
        assert_eq!(value["evidence"][0]["reference"], "mailbox_sweep:7000001");
        let mut collected = detail();
        collected.custody.operator_confirmed_collected = true;
        assert_eq!(
            serde_json::to_value(map_detail(collected, false)).expect("serialize")["custody"]["operator_confirmed_collected"],
            true
        );
    }

    #[test]
    fn detail_dto_omits_absent_obligation_and_recovery() {
        let mut input = detail();
        input.obligation = None;
        input.recovery = None;
        let value = serde_json::to_value(map_detail(input, false)).expect("serialize");
        assert!(value.get("obligation").is_none());
        assert!(value.get("recovery").is_none());
    }

    #[tokio::test]
    async fn delivery_serialized_blocked_registration_reply_matches_browser_fixture() {
        let mut input = detail();
        let delivery = &mut input.delivery;
        delivery.id = uuid::Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").expect("uuid");
        delivery.origin_kind = "mail_intake".to_owned();
        delivery.origin_id = "main:4242:send".to_owned();
        delivery.status = "failed".to_owned();
        delivery.template = Some("register_key".to_owned());
        delivery.category = Some("account".to_owned());
        delivery.claimed_at = Some(time());
        delivery.failure_code = Some("preflight_blocked".to_owned());
        let guard = DeliveryAdminGuardSnapshot {
            action: DeliveryAdminAction::Release,
            expected_fencing_version: delivery.fencing_version,
            fencing_version: delivery.fencing_version,
            status: delivery.status.clone(),
            reason_code: delivery.reason_code.clone(),
            has_sending_fence: true,
            origin_kind: delivery.origin_kind.clone(),
            payload_operation: Some("send".to_owned()),
            sent_reference_sighted: false,
            funded_delivery: false,
            command_snapshot_readable: true,
            retired: false,
            lot_eligible: Some(true),
            settlement_eligible: None,
            settlement_fulfillment_state: None,
            has_reconcile_reference: false,
            mail_handed_over: false,
            recipient_renamed: true,
            recovery_retry_eligible: true,
        };
        let attempt = &mut input.attempts[0];
        attempt.subject = Some("Fixture".to_owned());
        attempt.body_digest = Some([0x44; 32]);
        attempt.credits = Some(0);
        attempt.equipment_ids.clear();
        attempt.credential = None;
        attempt.bound_sent_mail_id = None;
        input.evidence.clear();
        input.dispositions.clear();
        input.obligation = None;
        input.recovery = None;
        input.custody.collected = None;
        input.custody.cod_pending = None;
        input.custody.remaining_credits = None;
        input.custody.observed_at = None;
        let blocked = map_actions(&guard, Some(DeliveryAdminGuardBlocker::AlreadyDisposed));
        assert_eq!(
            blocked.retry_failed.blocker(),
            Some(AdminDeliveryActionBlocker::RecipientRenamed)
        );
        let mut before_fence = input.clone();
        before_fence.delivery.sending_at = None;
        before_fence.delivery.failure_code = Some("invalid_payload".to_owned());
        before_fence.delivery.suppression_reason = before_fence.delivery.failure_code.clone();
        before_fence.attempts.clear();
        for (detail, fixture) in [
            (
                input,
                include_str!(
                    "../../../../auction-web-admin/src/api/delivery/blocked_registration_reply.json"
                ),
            ),
            (
                before_fence,
                include_str!(
                    "../../../../auction-web-admin/src/api/delivery/blocked_registration_reply_before_fence.json"
                ),
            ),
        ] {
            let guard = DeliveryAdminGuardSnapshot {
                has_sending_fence: detail.delivery.sending_at.is_some(),
                ..guard.clone()
            };
            let path = format!(
                "/api/v1/admin/deliveries/{}?include=disposition_actions",
                detail.delivery.id
            );
            let state =
                fixture_delivery_state(FixtureDeliveryDetail { detail, guard, acknowledge: None });
            let response = admin_get(state, &path).await;
            let actual = response_json(StatusCode::OK, response).await;
            let expected: serde_json::Value =
                serde_json::from_str(fixture).expect("浏览器回归样本");
            assert_eq!(actual, expected);
        }
    }

    fn sent_proof_detail() -> FixtureDeliveryDetail {
        let mut input = detail();
        input.delivery.id = SENT_PROOF_DELIVERY_ID;
        input.delivery.template = Some("settlement_equipment_handoff".to_owned());
        input.delivery.claimed_at = Some(time());
        input.attempts[0].credits = Some(0);
        input.attempts[0].credential = None;
        input.attempts[0].bound_sent_mail_id = None;
        input.dispositions.clear();
        input.obligation = None;
        input.recovery = None;
        let guard = DeliveryAdminGuardSnapshot {
            action: DeliveryAdminAction::Release,
            expected_fencing_version: 4,
            fencing_version: 4,
            status: "manual_reconcile".to_owned(),
            reason_code: None,
            has_sending_fence: true,
            origin_kind: "settlement".to_owned(),
            payload_operation: Some("send".to_owned()),
            sent_reference_sighted: true,
            funded_delivery: false,
            command_snapshot_readable: true,
            retired: false,
            lot_eligible: None,
            settlement_eligible: Some(true),
            settlement_fulfillment_state: Some("manual_reconcile".to_owned()),
            has_reconcile_reference: false,
            mail_handed_over: false,
            recipient_renamed: false,
            recovery_retry_eligible: false,
        };
        FixtureDeliveryDetail {
            detail: input,
            guard,
            acknowledge: Some(DeliveryAdminGuardBlocker::StatusMismatch),
        }
    }

    #[tokio::test]
    async fn sent_proof_detail_serialization_matches_browser_fixture() {
        let mut input = sent_proof_detail();
        let ids = || vec!["300753957".to_owned()];
        let expected: [serde_json::Value; 3] =
            serde_json::from_value(wire_fixture("admin/delivery-sent-proof-detail"))
                .expect("三种 Sent 证据样本");
        let path = format!("/api/v1/admin/deliveries/{SENT_PROOF_DELIVERY_ID}");
        for ((collected, remaining, before_collection), extended) in
            [(false, Some(ids()), None), (false, None, None), (true, Some(Vec::new()), Some(ids()))]
                .into_iter()
                .zip(expected)
        {
            let evidence = &mut input.detail.evidence[0];
            evidence.collected = Some(collected);
            evidence.remaining_equipment_count = Some(i64::from(!collected));
            evidence.remaining_equipment_ids = remaining;
            evidence.pre_collection_equipment_ids = before_collection;
            input.detail.custody.collected = Some(collected);
            let state = fixture_delivery_state(input.clone());
            let response = admin_get(state.clone(), &format!("{path}?include=sent_proof")).await;
            assert_eq!(response_json(StatusCode::OK, response).await, extended);

            let mut baseline = extended;
            for evidence in
                baseline["data"]["detail"]["evidence"].as_array_mut().expect("Sent 证据")
            {
                let evidence = evidence.as_object_mut().expect("证据对象");
                evidence.remove("remaining_equipment_ids");
                evidence.remove("pre_collection_equipment_ids");
            }
            let response = admin_get(state, &format!("{path}?include=disposition_actions")).await;
            assert_eq!(response_json(StatusCode::OK, response).await, baseline);
        }
    }

    struct FixtureSentPreview;

    impl DeliverySentConfirmationPort for FixtureSentPreview {
        fn preview_sent_confirmation<'a>(
            &'a self,
            delivery_id: Uuid,
            _evidence: ManualSentEvidence,
            reference: Option<&'a str>,
        ) -> ApplicationFuture<'a, SentConfirmationPreview> {
            assert_eq!(reference, Some("mailbox_sweep:7000001"));
            Box::pin(async move {
                Ok(SentConfirmationPreview {
                    eligible: true,
                    blockers: Vec::new(),
                    delivery_id,
                    expected_fencing_version: 4,
                    external_reference: "mooglemail:main:send:7000001".to_owned(),
                    requires_collected_asset_acceptance: false,
                    requires_attempt_history_acceptance: true,
                    prior_unbound_attempts: true,
                    sent_before_current_attempt: true,
                    corrects_admin_failure: true,
                })
            })
        }

        fn confirm_sent<'a>(
            &'a self,
            _command: SentConfirmationCommand,
            _reference: Option<&'a str>,
        ) -> ApplicationFuture<'a, SentConfirmationResult> {
            panic!("预览样本不提交 Sent 证明")
        }
    }

    #[tokio::test]
    async fn sent_proof_preview_serialization_adds_reasons_only_on_request() {
        let state =
            admin_state().with_delivery_sent_proof(Arc::new(DeliverySentProofService::new(
                Arc::new(FixtureDeliveries { detail: Some(sent_proof_detail()) }),
                Arc::new(FixtureSentPreview),
            )));
        let input = AdminDeliverySentProofPreviewRequest {
            evidence_reference: "mailbox_sweep:7000001".to_owned(),
            evidence: AdminDeliverySentEvidenceRequest {
                schema_version: 1,
                world_id: "main".to_owned(),
                external_mail_id: "7000001".to_owned(),
                sender_uid: "1".to_owned(),
                sender_name: "Auction".to_owned(),
                recipient: "Sample".to_owned(),
                subject: "Payment".to_owned(),
                sent_minute: crate::mapping::timestamp(time()),
                observed_at: crate::mapping::timestamp(time()),
                original_body_digest: [0xab; 32],
                observed_body_digest: [0xab; 32],
                remaining_credits: 0,
                cod_pending: 0,
                equipment_ids: Vec::new(),
                collected: false,
                receipt: String::new(),
            },
        };
        let bytes = axum::body::Bytes::from(serde_json::to_vec(&input).expect("预览请求"));
        let path = format!("/api/v1/admin/deliveries/{SENT_PROOF_DELIVERY_ID}/sent-proof/preview");
        let request = |query| {
            sent_proof_preview(
                State(state.clone()),
                admin_address(),
                Path(SENT_PROOF_DELIVERY_ID.to_string()),
                admin_headers(),
                format!("{path}{query}").parse().expect("预览 URI"),
                bytes.clone(),
            )
        };
        let name = "admin/delivery-sent-proof-preview";
        assert_http(name, StatusCode::OK, request("?include=responsibility_reasons").await).await;
        let mut baseline = wire_fixture(name);
        let fields = baseline["data"].as_object_mut().expect("预览字段");
        assert_eq!(fields.remove("prior_unbound_attempts"), Some(true.into()));
        assert_eq!(fields.remove("sent_before_current_attempt"), Some(true.into()));
        assert_eq!(response_json(StatusCode::OK, request("").await).await, baseline);
    }

    #[test]
    fn delivery_serialized_new_action_blockers_match_browser_fixture() {
        let response = [
            map_availability(Some(DeliveryAdminGuardBlocker::SettlementFulfillmentState)),
            map_availability(Some(DeliveryAdminGuardBlocker::MissingSendingFence)),
            map_availability(Some(DeliveryAdminGuardBlocker::CommandSnapshotUnreadable)),
        ];
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../auction-web-admin/src/api/delivery/disposition_action_blockers.json"
        ))
        .expect("浏览器回归样本");
        assert_eq!(serde_json::to_value(response).expect("序列化阻断原因"), fixture);
    }

    fn preflight_reply(failure: &str) -> DeliveryAdminDetail {
        use uuid::Uuid;

        let mut input = detail();
        let before_fence = failure == "invalid_payload";
        let delivery = &mut input.delivery;
        delivery.id = Uuid::from_u128(if before_fence {
            0x4000_8000_0000_0000_002b
        } else {
            0x4000_8000_0000_0000_002c
        });
        delivery.origin_kind = "mail_intake".to_owned();
        delivery.origin_id =
            if before_fence { "main:4243:send" } else { "main:4242:send" }.to_owned();
        delivery.status = "failed".to_owned();
        delivery.template = Some("register_key".to_owned());
        delivery.category = Some("account".to_owned());
        delivery.claimed_at = Some(time());
        delivery.failure_code = Some(failure.to_owned());
        if before_fence {
            delivery.sending_at = None;
            delivery.suppression_reason = delivery.failure_code.clone();
            input.attempts.clear();
        } else {
            let attempt = &mut input.attempts[0];
            attempt.subject = Some("Fixture".to_owned());
            attempt.credits = Some(0);
            attempt.equipment_ids.clear();
            attempt.credential = None;
            attempt.bound_sent_mail_id = None;
        }
        input.evidence.clear();
        input.dispositions.clear();
        input.obligation = None;
        input.recovery = None;
        input.custody.collected = None;
        input.custody.cod_pending = None;
        input.custody.remaining_credits = None;
        input.custody.observed_at = None;
        input
    }

    fn superseded_settlement() -> DeliveryAdminDetail {
        use uuid::Uuid;

        let mut input = preflight_reply("invalid_payload");
        let delivery = &mut input.delivery;
        delivery.id = Uuid::from_u128(0x4000_8000_0000_0000_002d);
        delivery.origin_kind = "settlement".to_owned();
        delivery.origin_id = Uuid::from_u128(0x4000_8000_0000_0000_002e).to_string();
        delivery.template = Some("settlement_seller_payout".to_owned());
        delivery.category = Some("settlement".to_owned());
        delivery.failure_code = Some("recovery_superseded".to_owned());
        delivery.suppression_reason = None;
        input
    }

    fn settlement_return_take() -> DeliveryAdminDetail {
        use auction_application::settlement::SYSTEM_RETURN_SENDER;
        use uuid::Uuid;

        let mut input = superseded_settlement();
        let delivery = &mut input.delivery;
        delivery.id = Uuid::from_u128(0x4000_8000_0000_0000_0031);
        delivery.origin_kind = "settlement_return_action".to_owned();
        delivery.origin_id =
            format!("settlement_return:{}", Uuid::from_u128(0x4000_8000_0000_0000_0032));
        delivery.recipient = SYSTEM_RETURN_SENDER.to_owned();
        delivery.status = "pending".to_owned();
        delivery.template = None;
        delivery.fencing_version = 0;
        delivery.attempt_count = 0;
        delivery.claimed_at = None;
        delivery.completed_at = None;
        delivery.failure_code = None;
        input
    }

    const SENT_PROOF_DELIVERY_ID: uuid::Uuid =
        uuid::Uuid::from_u128(0x0f8f_ad5b_d9cb_469f_a165_7086_7728_950e);

    #[derive(Clone)]
    struct FixtureDeliveryDetail {
        detail: DeliveryAdminDetail,
        guard: DeliveryAdminGuardSnapshot,
        acknowledge: Option<DeliveryAdminGuardBlocker>,
    }

    #[derive(Default)]
    struct FixtureDeliveries {
        detail: Option<FixtureDeliveryDetail>,
    }

    fn fixture_delivery_state(detail: FixtureDeliveryDetail) -> crate::state::AdminApiState {
        AdminFixturePorts {
            deliveries: Some(Arc::new(FixtureDeliveries { detail: Some(detail) })),
            ..Default::default()
        }
        .state()
    }

    impl auction_application::delivery::DeliveryAdminPort for FixtureDeliveries {
        fn list(
            &self,
            request: auction_application::delivery::DeliveryAdminListRequest,
        ) -> auction_application::core::ApplicationFuture<
            '_,
            auction_application::delivery::DeliveryAdminPage,
        > {
            Box::pin(async move {
                let mut entries = vec![
                    preflight_reply("preflight_blocked").delivery,
                    preflight_reply("invalid_payload").delivery,
                ];
                if request.include_settlement_return_actions {
                    entries.insert(0, superseded_settlement().delivery);
                    entries.insert(0, settlement_return_take().delivery);
                }
                Ok(auction_application::delivery::DeliveryAdminPage {
                    total: i64::try_from(entries.len()).expect("交付数量"),
                    entries,
                    limit: request.limit,
                    offset: request.offset,
                })
            })
        }

        fn detail(
            &self,
            id: uuid::Uuid,
        ) -> auction_application::core::ApplicationFuture<'_, DeliveryAdminDetail> {
            Box::pin(async move {
                if let Some(input) = &self.detail
                    && input.detail.delivery.id == id
                {
                    return Ok(input.detail.clone());
                }
                [
                    preflight_reply("invalid_payload"),
                    preflight_reply("preflight_blocked"),
                    superseded_settlement(),
                    settlement_return_take(),
                ]
                .into_iter()
                .find(|detail| detail.delivery.id == id)
                .ok_or_else(|| auction_application::delivery::DeliveryAdminError::NotFound.into())
            })
        }

        fn detail_with_guard(
            &self,
            id: uuid::Uuid,
        ) -> auction_application::core::ApplicationFuture<
            '_,
            (DeliveryAdminDetail, DeliveryAdminGuardSnapshot, Option<DeliveryAdminGuardBlocker>),
        > {
            Box::pin(async move {
                if let Some(input) = &self.detail
                    && input.detail.delivery.id == id
                {
                    return Ok((input.detail.clone(), input.guard.clone(), input.acknowledge));
                }
                let detail = self.detail(id).await?;
                let delivery = &detail.delivery;
                let guard = DeliveryAdminGuardSnapshot {
                    action: DeliveryAdminAction::Release,
                    expected_fencing_version: delivery.fencing_version,
                    fencing_version: delivery.fencing_version,
                    status: delivery.status.clone(),
                    reason_code: None,
                    has_sending_fence: delivery.sending_at.is_some(),
                    origin_kind: delivery.origin_kind.clone(),
                    payload_operation: Some("send".to_owned()),
                    sent_reference_sighted: false,
                    funded_delivery: false,
                    command_snapshot_readable: true,
                    retired: false,
                    lot_eligible: Some(true),
                    settlement_eligible: None,
                    settlement_fulfillment_state: None,
                    has_reconcile_reference: false,
                    mail_handed_over: false,
                    recipient_renamed: true,
                    recovery_retry_eligible: true,
                };
                Ok((detail, guard, None))
            })
        }

        fn execute(
            &self,
            _command: auction_application::delivery::DeliveryAdminCommand,
        ) -> auction_application::core::ApplicationFuture<
            '_,
            auction_application::delivery::DeliveryAdminResult,
        > {
            Box::pin(async {
                Err(auction_application::core::ApplicationError::CapabilityUnavailable(
                    "样本不写投递",
                ))
            })
        }

        fn player_support(
            &self,
            request: auction_application::delivery::PlayerSupportRequest,
        ) -> auction_application::core::ApplicationFuture<
            '_,
            auction_application::delivery::PlayerSupportBundle,
        > {
            use auction_application::delivery::{
                PlayerSupportBid, PlayerSupportBundle, PlayerSupportUser,
            };
            use uuid::Uuid;

            Box::pin(async move {
                let extended = request.include_settlement_return_actions;
                let mut deliveries = vec![preflight_reply("invalid_payload").delivery];
                if extended {
                    deliveries.push(superseded_settlement().delivery);
                }
                Ok(PlayerSupportBundle {
                    user: Some(PlayerSupportUser {
                        id: Uuid::from_u128(0x4000_8000_0000_0000_0024),
                        username: "Sample".to_owned(),
                        status: "active".to_owned(),
                        game_player_id: None,
                    }),
                    bids: vec![PlayerSupportBid {
                        bid_id: Uuid::from_u128(0x4000_8000_0000_0000_0011),
                        lot_id: Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c),
                        auction_id: crate::server::contract_fixtures::AUCTION_ID,
                        lot_number: 1,
                        world_id: "main".to_owned(),
                        amount: if extended { 3_000 } else { 2_000 },
                        mechanism: if extended { "proxy" } else { "ordinary" }.to_owned(),
                        lot_status: if extended { "sold" } else { "live" }.to_owned(),
                        created_at: time(),
                    }],
                    deliveries,
                    ..Default::default()
                })
            })
        }
    }

    #[tokio::test]
    async fn delivery_read_contract_fixtures() {
        use crate::admin::delivery::{detail, list, player_support};

        let state = AdminFixturePorts {
            deliveries: Some(Arc::new(FixtureDeliveries::default())),
            ..Default::default()
        }
        .state();
        for (name, uri) in [
            ("admin/deliveries", "/api/v1/admin/deliveries"),
            (
                "admin/deliveries-settlement-returns",
                "/api/v1/admin/deliveries?include=settlement_returns",
            ),
        ] {
            let response = list(
                State(state.clone()),
                admin_address(),
                admin_headers(),
                uri.parse().expect("列表 URI"),
            )
            .await;
            assert_http(name, StatusCode::OK, response).await;
        }

        for (name, uri) in [
            ("admin/player-support", "/api/v1/admin/player-support?user=Sample"),
            (
                "admin/player-support-settlement-returns",
                "/api/v1/admin/player-support?user=Sample&include=settlement_returns",
            ),
        ] {
            let response = player_support(
                State(state.clone()),
                admin_address(),
                admin_headers(),
                uri.parse().expect("玩家支持 URI"),
            )
            .await;
            assert_http(name, StatusCode::OK, response).await;
        }

        for (name, item, include) in [
            ("admin/delivery-invalid-payload", preflight_reply("invalid_payload"), ""),
            ("admin/delivery-preflight-blocked", preflight_reply("preflight_blocked"), ""),
            (
                "admin/delivery-disposition-actions",
                preflight_reply("invalid_payload"),
                "?include=disposition_actions",
            ),
            (
                "admin/delivery-settlement-returns",
                superseded_settlement(),
                "?include=settlement_returns",
            ),
            ("admin/delivery-return-take", settlement_return_take(), "?include=settlement_returns"),
        ] {
            let id = item.delivery.id.to_string();
            let uri = format!("/api/v1/admin/deliveries/{id}{include}");
            let response = detail(
                State(state.clone()),
                admin_address(),
                Path(id),
                admin_headers(),
                uri.parse().expect("详情 URI"),
            )
            .await;
            assert_http(name, StatusCode::OK, response).await;
        }
        let take_id = settlement_return_take().delivery.id.to_string();
        let uri = format!("/api/v1/admin/deliveries/{take_id}");
        let omitted = detail(
            State(state),
            admin_address(),
            Path(take_id),
            admin_headers(),
            uri.parse().expect("详情 URI"),
        )
        .await;
        assert_http("admin/error-delivery-projection-excluded", StatusCode::NOT_FOUND, omitted)
            .await;
    }
}
