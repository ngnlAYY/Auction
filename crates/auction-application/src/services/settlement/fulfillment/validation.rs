//! 结算履约 claim、外部标识和金额边界校验。

use crate::{
    ApplicationError, RepositoryError, SettlementFulfillmentClaim,
    SettlementFulfillmentDeliveryStatus, SettlementFulfillmentState,
};
use auction_domain::SettlementObligationKind;

/// 校验数据库 claim 的 identity、租约和阶段字段保持一致。
pub(super) fn validate_claim(claim: &SettlementFulfillmentClaim) -> Result<(), ApplicationError> {
    if claim.obligation_id.is_nil()
        || claim.auction_id.is_nil()
        || claim.lot_id.is_nil()
        || claim.origin_command_id.is_nil()
        || claim.equipment_id.is_nil()
        || claim.recipient.is_empty()
        || claim.recipient != claim.recipient.trim()
        || !valid_external_equipment_id(&claim.external_equipment_id)
        || claim.equipment_name.is_empty()
        || claim.equipment_name != claim.equipment_name.trim()
        || claim.amount < 0
        || claim.claim_token.is_nil()
        || claim.claim_fencing <= 0
        || claim.state_version == 0
        || claim.worker_id.is_empty()
        || claim.worker_id != claim.worker_id.trim()
        || claim.claimed_at >= claim.lease_until
        || claim.recipient_user_id.is_some_and(|user_id| user_id.is_nil())
    {
        return Err(repository_error("数据库返回的履约 claim 不满足不变量"));
    }
    let expects_user_recipient = matches!(
        claim.kind,
        SettlementObligationKind::WinnerEquipmentCod
            | SettlementObligationKind::WinnerPaymentCarrierCod
            | SettlementObligationKind::WinnerEquipmentHandoff
            | SettlementObligationKind::BuyerRefund
    );
    if expects_user_recipient != claim.recipient_user_id.is_some() {
        return Err(repository_error("数据库返回的履约收件人 owner 不一致"));
    }
    match claim.state {
        SettlementFulfillmentState::Pending
            if claim.delivery_id.is_none()
                && claim.prepare_command_id.is_none()
                && claim.prepare_request_hash.is_none()
                && claim.activate_command_id.is_none()
                && claim.activate_request_hash.is_none()
                && claim.delivery_snapshot.is_none() => {}
        SettlementFulfillmentState::Prepared
            if claim.delivery_id.is_some()
                && claim.prepare_command_id.is_some()
                && claim.prepare_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32])
                && claim.activate_command_id.is_none()
                && claim.activate_request_hash.is_none()
                && claim.delivery_snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.status == SettlementFulfillmentDeliveryStatus::Prepared
                }) => {}
        SettlementFulfillmentState::Enqueued
            if claim.delivery_id.is_some()
                && claim.prepare_command_id.is_some()
                && claim.prepare_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32])
                && claim.activate_command_id.is_some()
                && claim.activate_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32])
                && claim.delivery_snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.status != SettlementFulfillmentDeliveryStatus::Prepared
                }) => {}
        _ => return Err(repository_error("数据库返回的履约阶段 identity 不一致")),
    }
    if let Some(snapshot) = claim.delivery_snapshot.as_ref() {
        if snapshot.id.is_nil()
            || Some(snapshot.id) != claim.delivery_id
            || snapshot.prepare_command_id.is_nil()
            || snapshot.prepare_request_hash.as_bytes() == [0; 32]
            || snapshot.origin_kind.is_empty()
            || snapshot.origin_kind != snapshot.origin_kind.trim()
            || snapshot.origin_id.is_empty()
            || snapshot.origin_id != snapshot.origin_id.trim()
            || snapshot.recipient.is_empty()
            || snapshot.recipient != snapshot.recipient.trim()
            || !snapshot.payload.is_object()
            || snapshot.external_reference.as_ref().is_some_and(|value| {
                value.is_empty() || value != value.trim() || value.chars().any(char::is_control)
            })
            || snapshot.failure_code.as_ref().is_some_and(|value| {
                value.is_empty() || value != value.trim() || value.chars().any(char::is_control)
            })
        {
            return Err(repository_error("数据库返回的 delivery 快照不满足不变量"));
        }
        match snapshot.status {
            SettlementFulfillmentDeliveryStatus::Prepared
                if snapshot.activate_command_id.is_none()
                    && snapshot.activate_request_hash.is_none()
                    && snapshot.external_reference.is_none()
                    && snapshot.failure_code.is_none() => {}
            SettlementFulfillmentDeliveryStatus::Pending
            | SettlementFulfillmentDeliveryStatus::Claimed
            | SettlementFulfillmentDeliveryStatus::Sending
                if snapshot.activate_command_id.is_some()
                    && snapshot
                        .activate_request_hash
                        .is_some_and(|hash| hash.as_bytes() != [0; 32])
                    && snapshot.external_reference.is_none()
                    && snapshot.failure_code.is_none() => {}
            SettlementFulfillmentDeliveryStatus::Succeeded
                if snapshot.activate_command_id.is_some()
                    && snapshot
                        .activate_request_hash
                        .is_some_and(|hash| hash.as_bytes() != [0; 32])
                    && snapshot.external_reference.is_some()
                    && snapshot.failure_code.is_none() => {}
            SettlementFulfillmentDeliveryStatus::Failed
            | SettlementFulfillmentDeliveryStatus::ManualReconcile
                if snapshot.activate_command_id.is_some()
                    && snapshot
                        .activate_request_hash
                        .is_some_and(|hash| hash.as_bytes() != [0; 32])
                    && snapshot.failure_code.is_some() => {}
            _ => return Err(repository_error("数据库返回的 delivery 状态字段不一致")),
        }
    }
    Ok(())
}

/// 将应用层错误保持为仓储失败类别，避免把数据库/外部实现泄漏到端口。
pub(super) fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}

pub(super) fn valid_external_equipment_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn valid_external_mail_id(value: &str) -> bool {
    auction_domain::valid_text(value, 256)
}
