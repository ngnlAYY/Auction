//! 结算履约外部证据的稳定摘要。

use crate::{
    ApplicationError, RequestHash, SettlementFulfillmentBalanceEvidence,
    SettlementFulfillmentClaim, SettlementFulfillmentSentEvidence,
};

use super::intent::settlement_world_name;
use super::model::SettlementProofDigest;

/// 以 Sent/Balance 观察结果生成不可变 proof digest，供后续审计和状态推进绑定。
pub(super) fn proof_digest(
    claim: &SettlementFulfillmentClaim,
    delivery_id: uuid::Uuid,
    sent: &SettlementFulfillmentSentEvidence,
    balance: Option<&SettlementFulfillmentBalanceEvidence>,
) -> Result<[u8; 32], ApplicationError> {
    let payload = SettlementProofDigest {
        schema_version: 1,
        obligation_id: claim.obligation_id.as_uuid(),
        delivery_id,
        external_mail_id: &sent.external_mail_id,
        world: settlement_world_name(sent.world),
        recipient: &sent.recipient,
        subject: &sent.subject,
        remaining_credits: sent.remaining_credits,
        cod_pending: sent.cod_pending,
        remaining_equipment_ids: &sent.remaining_equipment_ids,
        collected: sent.collected,
        sent_observed_at: sent.observed_at,
        balance_credits: balance.map(|value| value.credits),
        balance_observed_at: balance.map(|value| value.observed_at),
    };
    Ok(RequestHash::from_request(
        "settlement_fulfillment_proof",
        claim.obligation_id.as_uuid(),
        &payload,
    )?
    .as_bytes())
}
