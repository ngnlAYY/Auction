//! MoogleMail Sent/Balance 证据与结算意图的一致性校验。

use auction_domain::SettlementObligationKind;

use crate::{
    ApplicationError, SettlementFulfillmentBalanceEvidence, SettlementFulfillmentClaim,
    SettlementFulfillmentSentEvidence, canonical_settlement_fulfillment_subject,
};

use super::validation::repository_error;

/// 校验 Sent 记录证明了当前义务、收件人和剩余资产已经收口。
pub(super) fn validate_sent_evidence(
    claim: &SettlementFulfillmentClaim,
    external_mail_id: &str,
    evidence: &SettlementFulfillmentSentEvidence,
) -> Result<(), ApplicationError> {
    let expected_subject = match claim.kind {
        SettlementObligationKind::UnsoldReturn => {
            return Err(repository_error("退回义务不应进入发送回执对账"));
        }
        _ => canonical_settlement_fulfillment_subject(claim.kind, claim.obligation_id)
            .ok_or_else(|| repository_error("结算履约主题闭集无效"))?,
    };
    let (remaining_credits, remaining_cod, remaining_equipment) =
        if evidence.collected { (0, 0, Vec::new()) } else { expected_remaining(claim)? };
    if evidence.external_mail_id != external_mail_id
        || evidence.world != claim.world
        || evidence.recipient != claim.recipient
        || evidence.subject != expected_subject
        || evidence.remaining_credits != remaining_credits
        || evidence.cod_pending != remaining_cod
        || evidence.remaining_equipment_ids != remaining_equipment
        || evidence.observed_at < claim.claimed_at
    {
        return Err(repository_error("MoogleMail Sent 证明与结算意图不一致"));
    }
    Ok(())
}

pub(super) fn expected_remaining(
    claim: &SettlementFulfillmentClaim,
) -> Result<(i64, i64, Vec<String>), ApplicationError> {
    match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod => {
            Ok((0, claim.amount, vec![claim.external_equipment_id.clone()]))
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => claim
            .amount
            .checked_add(i64::from(claim.carrier_credit))
            .map(|remaining_cod| (i64::from(claim.carrier_credit), remaining_cod, Vec::new()))
            .ok_or_else(|| repository_error("MoogleMail COD 证据期望金额溢出")),
        SettlementObligationKind::WinnerEquipmentHandoff => {
            Ok((0, 0, vec![claim.external_equipment_id.clone()]))
        }
        SettlementObligationKind::SellerPayout | SettlementObligationKind::BuyerRefund => {
            Ok((claim.amount, 0, Vec::new()))
        }
        SettlementObligationKind::UnsoldReturn => Ok((0, 0, Vec::new())),
        SettlementObligationKind::WithdrawnReturn => {
            Ok((0, 0, vec![claim.external_equipment_id.clone()]))
        }
    }
}

/// Balance 证明至少必须属于同一 world 且不早于 claim，后续金额上限由 evidence reader/owner 保证。
pub(super) fn validate_balance_evidence(
    claim: &SettlementFulfillmentClaim,
    evidence: &SettlementFulfillmentBalanceEvidence,
) -> Result<(), ApplicationError> {
    if evidence.world != claim.world
        || evidence.credits < 0
        || evidence.observed_at < claim.claimed_at
    {
        return Err(repository_error("MoogleMail Balance 证明无效或过期"));
    }
    Ok(())
}
