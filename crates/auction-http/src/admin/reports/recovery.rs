//! Admin recovery 安全报告的结果校验与契约映射。
//! 内部失败码只在这个防腐边界归并为旧 v1 公开码；未知值映射失败，不回显原文。

use auction_application::recovery::{
    RECOVERY_SEND_ATTRIBUTION_BLOCKERS, RecoveryReport, allowed_recovery_commands,
};
use auction_contracts::{
    AdminRecoveryActionResponse, AdminRecoveryReportResponse, AdminRecoveryResponse,
};
use auction_domain::SettlementId;
use uuid::Uuid;

use crate::mapping::timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryReportMappingError {
    UnknownErrorCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicRecoveryErrorCode {
    InvalidFact,
    RecipientUnavailable,
    DeliveryConflict,
    DeliveryUnavailable,
    DeliveryFailed,
    StaleClaim,
    EvidenceNotObserved,
    EvidenceUnavailable,
    EvidenceMismatch,
    OutcomeUnknown,
    RelistUnavailable,
}

impl PublicRecoveryErrorCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidFact => "invalid_recovery_fact",
            Self::RecipientUnavailable => "recovery_recipient_unavailable",
            Self::DeliveryConflict => "recovery_delivery_conflict",
            Self::DeliveryUnavailable => "recovery_delivery_unavailable",
            Self::DeliveryFailed => "recovery_delivery_failed",
            Self::StaleClaim => "stale_recovery_claim",
            Self::EvidenceNotObserved => "recovery_evidence_not_observed",
            Self::EvidenceUnavailable => "recovery_evidence_unavailable",
            Self::EvidenceMismatch => "recovery_evidence_mismatch",
            Self::OutcomeUnknown => "recovery_outcome_unknown",
            Self::RelistUnavailable => "recovery_relist_unavailable",
        }
    }
}

fn map_recovery_error_code(
    value: &str,
) -> Result<PublicRecoveryErrorCode, RecoveryReportMappingError> {
    match value {
        // 旧 v1 已持久化的公开码必须保持原值，内部实现码只在这个防腐边界归并。
        "invalid_recovery_fact" => Ok(PublicRecoveryErrorCode::InvalidFact),
        "recovery_recipient_unavailable" => Ok(PublicRecoveryErrorCode::RecipientUnavailable),
        "recovery_delivery_conflict" => Ok(PublicRecoveryErrorCode::DeliveryConflict),
        "recovery_delivery_unavailable" => Ok(PublicRecoveryErrorCode::DeliveryUnavailable),
        "recovery_delivery_failed" => Ok(PublicRecoveryErrorCode::DeliveryFailed),
        "stale_recovery_claim" => Ok(PublicRecoveryErrorCode::StaleClaim),
        "recovery_evidence_not_observed" => Ok(PublicRecoveryErrorCode::EvidenceNotObserved),
        "recovery_evidence_unavailable" => Ok(PublicRecoveryErrorCode::EvidenceUnavailable),
        "recovery_evidence_mismatch" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        "recovery_outcome_unknown" => Ok(PublicRecoveryErrorCode::OutcomeUnknown),
        "recovery_relist_unavailable" => Ok(PublicRecoveryErrorCode::RelistUnavailable),
        // 旧赛季装备已随 isekai 换季清空，属于永久不能重挂。
        "relist_season_closed" => Ok(PublicRecoveryErrorCode::RelistUnavailable),
        "evidence_mismatch" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        // 退件不能把原投递每一次可能外达的发送逐一归属：同属证据不符，不新增公开码。
        "send_unattributed" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        // 成功页但远端引用不能证明的旧语义是 outcome_unknown，不能伪装成已观察到的证据缺失。
        "delivery_reference_mismatch"
        | "delivery_reference_missing"
        | "delivery_reference_unproven"
        | "delivery_outcome_unknown" => Ok(PublicRecoveryErrorCode::OutcomeUnknown),
        "delivery_failed" | "send_rejected" | "invalid_payload" => {
            Ok(PublicRecoveryErrorCode::DeliveryFailed)
        }
        "send_rejected_after_stage"
        | "write_unknown"
        | "gateway_outcome_unknown"
        | "invalid_gateway_result"
        | "lease_expired_outcome_unknown"
        | "task_panic"
        | "preflight_unproven" => Ok(PublicRecoveryErrorCode::OutcomeUnknown),
        "delivery_unavailable"
        | "recovery_observation_failed"
        | "recovery_buyer_blacklist_unavailable"
        | "recovery_buyer_blacklist_owner_unavailable"
        | "recovery_activation_unavailable"
        | "recovery_activation_link_failed"
        | "credential_unavailable"
        | "secret_not_found"
        | "invalid_secret"
        | "login_key_unavailable"
        | "session_expired"
        | "rate_limited"
        | "remote_temporary"
        | "response_too_large" => Ok(PublicRecoveryErrorCode::DeliveryUnavailable),
        "recovery_delivery_pending" => Ok(PublicRecoveryErrorCode::EvidenceNotObserved),
        "recovery_recipient_missing" | "recovery_recipient_ineligible" | "recipient_not_found" => {
            Ok(PublicRecoveryErrorCode::RecipientUnavailable)
        }
        "recovery_buyer_blacklist_result_invalid"
        | "recovery_activation_result_invalid"
        | "recovery_delivery_result_invalid"
        | "recovery_delivery_link_failed" => Ok(PublicRecoveryErrorCode::DeliveryConflict),
        "recovery_request_hash_failed"
        | "recovery_prepare_identity_missing"
        | "recovery_activation_hash_failed"
        | "recovery_source_mail_missing"
        | "unsupported_recovery_action"
        | "configuration" => Ok(PublicRecoveryErrorCode::InvalidFact),
        "not_observed" => Ok(PublicRecoveryErrorCode::EvidenceNotObserved),
        "protocol_drift" | "identity_mismatch" => Ok(PublicRecoveryErrorCode::EvidenceMismatch),
        _ => Err(RecoveryReportMappingError::UnknownErrorCode),
    }
}

fn map_optional_recovery_error_code(
    value: Option<String>,
) -> Result<Option<String>, RecoveryReportMappingError> {
    value
        .as_deref()
        .map(map_recovery_error_code)
        .transpose()
        .map(|value| value.map(PublicRecoveryErrorCode::as_str).map(ToOwned::to_owned))
}

/// `include_send_attribution` 来自显式 `include=send_attribution`；缺省投影不输出发送归属原因。
pub(super) fn map_recovery_report(
    report: RecoveryReport,
    include_send_attribution: bool,
) -> Result<AdminRecoveryReportResponse, RecoveryReportMappingError> {
    let recoveries = report
        .recoveries
        .into_iter()
        .map(|recovery| -> Result<AdminRecoveryResponse, RecoveryReportMappingError> {
            let actions = recovery
                .actions
                .into_iter()
                .map(|action| -> Result<AdminRecoveryActionResponse, RecoveryReportMappingError> {
                    Ok(AdminRecoveryActionResponse {
                        id: action.id.to_string(),
                        kind: action.kind,
                        state: action.state,
                        delivery_id: action.delivery_id.map(|id| id.to_string()),
                        failure_code: map_optional_recovery_error_code(action.failure_code)?,
                        proof_kind: action.proof_kind,
                        proof_observed_at: action.proof_observed_at.map(timestamp),
                        updated_at: timestamp(action.updated_at),
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(AdminRecoveryResponse {
                id: recovery.id.to_string(),
                auction_id: recovery.auction_id.to_string(),
                lot_id: recovery.lot_id.to_string(),
                lot_number: recovery.lot_number,
                world_id: recovery.world_id,
                equipment_name: recovery.equipment_name,
                equipment_category: recovery.equipment_category,
                stage: recovery.stage,
                resolution: recovery.resolution,
                state_version: recovery.state_version,
                payment_deadline_at: timestamp(recovery.payment_deadline_at),
                reminder_count: recovery.reminder_count,
                reminder_limit: recovery.reminder_limit,
                custody_proof_kind: recovery.custody_proof_kind,
                custody_proof_observed_at: recovery.custody_proof_observed_at.map(timestamp),
                blocking_code: map_optional_recovery_error_code(recovery.blocking_code)?,
                send_attribution_blocker: recovery
                    .send_attribution_blocker
                    .filter(|_| include_send_attribution),
                manual_reconcile_delivery_id: recovery
                    .manual_reconcile_delivery_id
                    .map(|id| id.to_string()),
                relisted_auction_id: recovery.relisted_auction_id.map(|id| id.to_string()),
                relisted_lot_id: recovery.relisted_lot_id.map(|id| id.to_string()),
                actions,
                allowed_commands: recovery.allowed_commands,
                updated_at: timestamp(recovery.updated_at),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AdminRecoveryReportResponse { auction_id: report.auction_id.to_string(), recoveries })
}

pub(super) fn valid_recovery_report(report: &RecoveryReport, auction_id: Uuid) -> bool {
    if report.auction_id.as_uuid() != auction_id {
        return false;
    }
    let mut recovery_ids = std::collections::BTreeSet::new();
    let mut lot_ids = std::collections::BTreeSet::new();
    for recovery in &report.recoveries {
        if recovery.id.is_nil()
            || recovery.auction_id.as_uuid() != auction_id
            || recovery.lot_id.is_nil()
            || recovery.lot_number <= 0
            || !matches!(recovery.world_id.as_str(), "main" | "isekai")
            || recovery.equipment_name.is_empty()
            || recovery.equipment_category.is_empty()
            || !matches!(
                recovery.stage.as_str(),
                "awaiting_payment"
                    | "payment_overdue"
                    | "delivering_equipment"
                    | "reclaiming"
                    | "relist_pending"
                    | "refunding"
                    | "completed"
            )
            || recovery.resolution.as_ref().is_some_and(|resolution| {
                !matches!(
                    resolution.as_str(),
                    "reclaimed"
                        | "cancelled"
                        | "equipment_recovered"
                        | "returned_after_three_cod_returns"
                )
            })
            || recovery.state_version <= 0
            || recovery.payment_deadline_at.timestamp() <= 0
            || recovery.reminder_limit != 1
            || recovery.reminder_count < 0
            || recovery.reminder_count > recovery.reminder_limit
            || recovery.updated_at.timestamp() <= 0
            || recovery.blocking_code.as_ref().is_some_and(std::string::String::is_empty)
            || recovery.send_attribution_blocker.as_deref().is_some_and(|blocker| {
                recovery.stage != "reclaiming"
                    || !RECOVERY_SEND_ATTRIBUTION_BLOCKERS.contains(&blocker)
            })
            || recovery.manual_reconcile_delivery_id.is_some_and(SettlementId::is_nil)
            || recovery.relisted_auction_id.is_some_and(auction_domain::AuctionId::is_nil)
            || recovery.relisted_lot_id.is_some_and(auction_domain::LotId::is_nil)
            || recovery.relisted_auction_id.is_some() != recovery.relisted_lot_id.is_some()
            || !recovery_ids.insert(recovery.id.as_uuid())
            || !lot_ids.insert(recovery.lot_id.as_uuid())
        {
            return false;
        }
        let mut action_ids = std::collections::BTreeSet::new();
        for action in &recovery.actions {
            if action.id.is_nil()
                || !matches!(
                    action.kind.as_str(),
                    "payment_reminder"
                        | "reclaim_equipment_take"
                        | "cancel_carrier_take"
                        | "recover_equipment_take"
                        | "buyer_blacklist"
                )
                || !matches!(
                    action.state.as_str(),
                    "pending"
                        | "prepared"
                        | "enqueued"
                        | "satisfied"
                        | "failed"
                        | "manual_reconcile"
                )
                || action.delivery_id.is_some_and(SettlementId::is_nil)
                || action.failure_code.as_ref().is_some_and(std::string::String::is_empty)
                || action.proof_kind.as_ref().is_some_and(String::is_empty)
                || action.updated_at.timestamp() <= 0
                || !action_ids.insert(action.id.as_uuid())
            {
                return false;
            }
        }
        if recovery.allowed_commands != allowed_recovery_commands(recovery)
            || recovery.allowed_commands.len() > 1
            || recovery.allowed_commands.iter().any(|command| {
                !matches!(command.as_str(), "reclaimed" | "cancelled" | "equipment_recovered")
            })
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use auction_application::recovery::{RecoveryReport, RecoveryReportEntry};
    use auction_domain::{AuctionId, LotId, SettlementId};
    use chrono::Utc;
    use uuid::Uuid;

    use super::{map_optional_recovery_error_code, map_recovery_report, valid_recovery_report};

    fn report(blocker: Option<&str>) -> RecoveryReport {
        let at = Utc::now();
        let auction_id = AuctionId::from_uuid(Uuid::from_u128(2));
        RecoveryReport {
            auction_id,
            recoveries: vec![RecoveryReportEntry {
                id: SettlementId::from_uuid(Uuid::from_u128(1)),
                auction_id,
                lot_id: LotId::from_uuid(Uuid::from_u128(3)),
                lot_number: 1,
                world_id: "main".to_owned(),
                equipment_name: "Fixture".to_owned(),
                equipment_category: "weapon".to_owned(),
                stage: "reclaiming".to_owned(),
                resolution: None,
                state_version: 4,
                payment_deadline_at: at,
                reminder_count: 0,
                reminder_limit: 1,
                cod_return_count: 3,
                cod_return_limit: 3,
                custody_proof_kind: None,
                custody_proof_observed_at: None,
                blocking_code: Some("send_unattributed".to_owned()),
                send_attribution_blocker: blocker.map(ToOwned::to_owned),
                manual_reconcile_delivery_id: None,
                relisted_auction_id: None,
                relisted_lot_id: None,
                actions: Vec::new(),
                allowed_commands: Vec::new(),
                updated_at: at,
            }],
        }
    }

    /// 发送归属原因只在显式 `include=send_attribution` 时输出；缺省响应的 JSON 不带该键，旧页面
    /// 的闭集解码照常通过。闭集之外的原因让报告校验失败关闭。
    #[test]
    fn send_attribution_blocker_is_projected_only_when_requested() {
        let source = report(Some("send_without_sent_mid"));
        assert!(valid_recovery_report(&source, Uuid::from_u128(2)));
        let default = map_recovery_report(source.clone(), false).expect("缺省投影");
        assert_eq!(default.recoveries[0].send_attribution_blocker, None);
        let wire = serde_json::to_value(&default).expect("序列化缺省投影");
        assert!(wire["recoveries"][0].get("send_attribution_blocker").is_none());
        let extended = map_recovery_report(source, true).expect("扩展投影");
        assert_eq!(
            extended.recoveries[0].send_attribution_blocker.as_deref(),
            Some("send_without_sent_mid")
        );

        assert!(!valid_recovery_report(&report(Some("operator_declared")), Uuid::from_u128(2)));
        let mut completed = report(Some("send_in_flight"));
        completed.recoveries[0].stage = "completed".to_owned();
        assert!(!valid_recovery_report(&completed, Uuid::from_u128(2)));
    }

    /// Worker 自动推进因退件不能唯一归属而写下的内部阻断码，在防腐边界归并为已发布的
    /// `recovery_evidence_mismatch`，旧页面按闭集照常解码。
    #[test]
    fn send_attribution_block_maps_to_the_published_evidence_mismatch() {
        assert_eq!(
            map_optional_recovery_error_code(Some("send_unattributed".to_owned())),
            Ok(Some("recovery_evidence_mismatch".to_owned()))
        );
    }
}
