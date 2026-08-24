//! 结算履约 delivery intent 与稳定命名辅助。

use auction_domain::{CommandId, SettlementObligationKind, SettlementWorld, deterministic_uuid};
use uuid::Uuid;

use crate::{ApplicationError, SettlementFulfillmentClaim};

use super::model::SettlementDeliveryIntent;
use super::validation::repository_error;

/// 根据数据库 claim 生成无 secret 的 delivery intent。
pub(super) fn build_delivery_intent(
    claim: &SettlementFulfillmentClaim,
) -> Result<SettlementDeliveryIntent, ApplicationError> {
    let mut intent = SettlementDeliveryIntent {
        schema_version: 2,
        operation: claim.operation.as_str(),
        world: settlement_world_name(claim.world),
        obligation_id: claim.obligation_id.as_uuid(),
        auction_id: claim.auction_id.as_uuid(),
        lot_id: claim.lot_id.as_uuid(),
        origin_command_id: claim.origin_command_id.as_uuid(),
        kind: obligation_kind_name(claim.kind),
        recipient: claim.recipient.clone(),
        template: "",
        item_name: String::new(),
        external_equipment_id: claim.external_equipment_id.clone(),
        amount: claim.amount,
        carrier_credit: claim.carrier_credit,
        credits: 0,
        cod: 0,
        equipment_id: claim.equipment_id.as_uuid(),
        submission_id: claim.submission_id.map(|id| id.as_uuid()),
        submission_source_kind: claim.submission_source_kind.clone(),
        source_external_mail_id: claim.submission_external_mail_id.clone(),
    };

    match claim.kind {
        SettlementObligationKind::WinnerEquipmentCod => {
            intent.template = "settlement_winner_equipment_cod";
            intent.item_name = claim.equipment_name.clone();
            intent.cod = claim.amount;
        }
        SettlementObligationKind::WinnerPaymentCarrierCod => {
            intent.template = "settlement_payment_carrier";
            intent.credits = i64::from(claim.carrier_credit);
            intent.cod = claim
                .amount
                .checked_add(intent.credits)
                .ok_or_else(|| repository_error("结算履约 COD 金额溢出"))?;
        }
        SettlementObligationKind::WinnerEquipmentHandoff => {
            intent.template = "settlement_equipment_handoff";
            intent.item_name = claim.equipment_name.clone();
        }
        SettlementObligationKind::SellerPayout => {
            intent.template = "settlement_seller_payout";
            intent.credits = claim.amount;
        }
        SettlementObligationKind::BuyerRefund => {
            intent.template = "settlement_buyer_refund";
            intent.credits = claim.amount;
        }
        SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
            match (
                claim.submission_source_kind.as_deref(),
                claim.submission_external_mail_id.as_deref(),
            ) {
                (Some("mail"), Some(external_mail_id))
                    if !external_mail_id.is_empty()
                        && external_mail_id == external_mail_id.trim() =>
                {
                    intent.operation = "return";
                }
                (Some("admin"), None) => {
                    intent.operation = "send";
                    intent.template = "settlement_equipment_handoff";
                    intent.item_name = claim.equipment_name.clone();
                }
                _ => return Err(repository_error("结算退回来源缺少可执行身份")),
            }
        }
    }

    Ok(intent)
}

/// 从 settlement obligation 生成跨重试稳定的 delivery identity。
pub(super) fn stable_id(
    obligation_id: auction_domain::SettlementId,
    label: &str,
    version: u32,
) -> Uuid {
    deterministic_uuid(CommandId::from_uuid(obligation_id.as_uuid()), label, version)
}

pub(super) fn settlement_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

pub(super) fn obligation_kind_name(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}
