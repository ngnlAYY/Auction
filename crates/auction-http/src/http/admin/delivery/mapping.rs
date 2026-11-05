use auction_application::DeliveryAdminSnapshot;
use auction_contracts::AdminDeliveryResponse;

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
        lease_expires_at: snapshot.lease_expires_at.map(super::timestamp),
        fencing_version: snapshot.fencing_version,
        attempt_count: snapshot.attempt_count,
        claimed_at: snapshot.claimed_at.map(super::timestamp),
        sending_at: snapshot.sending_at.map(super::timestamp),
        completed_at: snapshot.completed_at.map(super::timestamp),
        external_reference: snapshot.external_reference,
        failure_code: snapshot.failure_code,
        reason_code: snapshot.reason_code,
        take_required: snapshot.take_required,
        returned_mail_id: snapshot.returned_mail_id,
        created_at: super::timestamp(snapshot.created_at),
        updated_at: super::timestamp(snapshot.updated_at),
    }
}

/// 将投递详情投影转换为管理端 DTO；摘要转小写 hex，绝不返回正文或凭据秘密。
pub(crate) fn map_detail(
    detail: auction_application::DeliveryAdminDetail,
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
                sending_at: super::timestamp(attempt.sending_at),
                recipient: attempt.recipient,
                subject: attempt.subject,
                body_digest: attempt
                    .body_digest
                    .as_ref()
                    .map(|digest| auction_application::lower_hex(digest)),
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
                observed_at: item.observed_at.map(super::timestamp),
                sent_at: item.sent_at.map(super::timestamp),
                recipient: item.recipient,
                subject: item.subject,
                body_digest: item
                    .body_digest
                    .as_ref()
                    .map(|digest| auction_application::lower_hex(digest)),
                collected: item.collected,
                cod_pending: item.cod_pending,
                remaining_credits: item.remaining_credits,
                remaining_equipment_count: item.remaining_equipment_count,
                receipt: item.receipt,
            })
            .collect(),
        custody: auction_contracts::AdminDeliveryCustodyResponse {
            collected: detail.custody.collected,
            cod_pending: detail.custody.cod_pending,
            remaining_credits: detail.custody.remaining_credits,
            receipt: detail.custody.receipt,
            observed_at: detail.custody.observed_at.map(super::timestamp),
            operator_confirmed_collected: detail.custody.operator_confirmed_collected,
        },
        dispositions: detail
            .dispositions
            .into_iter()
            .map(|item| auction_contracts::AdminDeliveryDispositionResponse {
                command_id: item.command_id.to_string(),
                kind: item.kind,
                occurred_at: super::timestamp(item.occurred_at),
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
            payment_deadline_at: super::timestamp(item.payment_deadline_at),
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
    bundle: auction_application::PlayerSupportBundle,
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
                created_at: super::timestamp(bid.created_at),
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
                effective_end_at: deal.effective_end_at.map(super::timestamp),
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
                created_at: super::timestamp(notification.created_at),
                updated_at: super::timestamp(notification.updated_at),
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
    preview: auction_application::SentConfirmationPreview,
) -> auction_contracts::AdminDeliverySentProofPreviewResponse {
    auction_contracts::AdminDeliverySentProofPreviewResponse {
        eligible: preview.eligible,
        blockers: preview.blockers,
        delivery_id: preview.delivery_id.to_string(),
        expected_fencing_version: preview.expected_fencing_version,
        external_reference: preview.external_reference,
        requires_collected_asset_acceptance: preview.requires_collected_asset_acceptance,
        requires_attempt_history_acceptance: preview.requires_attempt_history_acceptance,
        corrects_admin_failure: preview.corrects_admin_failure,
    }
}

/// 将人工证明结果映射为管理端 DTO；重放标志原样保留。
pub(crate) fn map_sent_proof_result(
    result: auction_application::SentConfirmationResult,
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
    use chrono::{TimeZone, Utc};

    use self::parent::*;
    use super as parent;
    use auction_application::{
        DeliveryAdminAttempt, DeliveryAdminCustody, DeliveryAdminDetail, DeliveryAdminDisposition,
        DeliveryAdminObligation, DeliveryAdminRecovery, DeliveryAdminSentEvidence,
        DeliveryAdminSnapshot, DeliverySendCredentialBinding,
    };

    fn time() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 2, 1, 14, 0).single().expect("timestamp")
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
                recipient: "Restia".to_owned(),
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
                recipient: Some("Restia".to_owned()),
                subject: Some("Payment".to_owned()),
                body_digest: Some([0xab; 32]),
                credits: Some(5_252_000),
                cod: Some(0),
                equipment_ids: vec!["300753957".to_owned()],
                external_mail_id: None,
                credential: Some(credential),
                bound_sent_mail_id: Some("3125345".to_owned()),
            }],
            evidence: vec![DeliveryAdminSentEvidence {
                reference: "mailbox_sweep:3125345".to_owned(),
                kind: "mailbox_sweep".to_owned(),
                external_mail_id: Some("3125345".to_owned()),
                observed_at: Some(time()),
                sent_at: None,
                recipient: Some("Restia".to_owned()),
                subject: Some("Payment".to_owned()),
                body_digest: Some([0xab; 32]),
                collected: Some(false),
                cod_pending: Some(0),
                remaining_credits: Some(0),
                remaining_equipment_count: Some(0),
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
        let value = serde_json::to_value(map_detail(detail())).expect("serialize");
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
        assert_eq!(value["evidence"][0]["reference"], "mailbox_sweep:3125345");
        let mut collected = detail();
        collected.custody.operator_confirmed_collected = true;
        assert_eq!(
            serde_json::to_value(map_detail(collected)).expect("serialize")["custody"]["operator_confirmed_collected"],
            true
        );
    }

    #[test]
    fn detail_dto_omits_absent_obligation_and_recovery() {
        let mut input = detail();
        input.obligation = None;
        input.recovery = None;
        let value = serde_json::to_value(map_detail(input)).expect("serialize");
        assert!(value.get("obligation").is_none());
        assert!(value.get("recovery").is_none());
    }
}
