//! 结算履约 claim、外部标识和金额边界校验。

use auction_domain::SettlementObligationKind;

use crate::{
    ApplicationError, RepositoryError, SettlementFulfillmentClaim, SettlementFulfillmentState,
};

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
        || claim.attempt_count == 0
    {
        return Err(repository_error("数据库返回的履约 claim 不满足不变量"));
    }
    match claim.state {
        SettlementFulfillmentState::Pending
            if claim.delivery_id.is_none()
                && claim.prepare_command_id.is_none()
                && claim.prepare_request_hash.is_none() => {}
        SettlementFulfillmentState::Prepared
            if claim.delivery_id.is_some()
                && claim.prepare_command_id.is_some()
                && claim.prepare_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32]) => {}
        SettlementFulfillmentState::Enqueued
            if claim.delivery_id.is_some()
                && claim.prepare_command_id.is_some()
                && claim.prepare_request_hash.is_some_and(|hash| hash.as_bytes() != [0; 32]) => {}
        _ => return Err(repository_error("数据库返回的履约阶段 identity 不一致")),
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
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

pub(super) fn expected_credit_amount(claim: &SettlementFulfillmentClaim) -> i64 {
    match claim.kind {
        SettlementObligationKind::WinnerPaymentCarrierCod
        | SettlementObligationKind::SellerPayout
        | SettlementObligationKind::BuyerRefund => {
            if claim.kind == SettlementObligationKind::WinnerPaymentCarrierCod {
                i64::from(claim.carrier_credit)
            } else {
                claim.amount
            }
        }
        _ => 0,
    }
}
