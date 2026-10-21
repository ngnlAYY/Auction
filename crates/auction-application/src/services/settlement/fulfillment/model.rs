//! 结算履约跨 owner 传递的无 secret 模型。

use auction_domain::{
    SettlementLine, SettlementObligationKind, validate_settlement_price_breakdown,
};
use serde::Serialize;
use uuid::Uuid;

/// auction owner 在生成通用 MoogleMail schema-v1 payload 前校验的 typed intent。
/// 保留旧版 `FulfillmentDeliveryIntent` 的 Send 语义。拍卖、标的、投稿等授权上下文只留在
/// fulfillment claim，不得混入 delivery owner 的稳定载荷或请求摘要。
#[derive(Debug)]
pub(super) struct SettlementDeliveryIntent {
    pub(super) obligation_id: Uuid,
    pub(super) kind: SettlementObligationKind,
    pub(super) operation: &'static str,
    pub(super) world: &'static str,
    pub(super) recipient: String,
    pub(super) template: &'static str,
    pub(super) reference: String,
    pub(super) item_name: String,
    pub(super) credits: i64,
    pub(super) cod: i64,
    pub(super) equipment_id: Option<Uuid>,
    /// 新建竞得装备发送时冻结；历史三变量载荷保持原摘要。
    pub(super) settlement_line: Option<SettlementLine>,
}

impl SettlementDeliveryIntent {
    /// 按义务种类校验完整闭集组合，流拍与撤拍返还必须单独发送原装备。
    pub(super) fn validate(&self) -> bool {
        if self.obligation_id.is_nil()
            || self.operation != "send"
            || !matches!(self.world, "main" | "isekai")
            || !trimmed_non_empty(&self.recipient)
            || self.credits < 0
            || self.cod < 0
            || self.settlement_line.as_ref().is_some_and(|line| {
                self.kind != SettlementObligationKind::WinnerEquipmentCod
                    || line.validate().is_err()
                    || line.end_price.value() <= 0
                    || line.buyer_amount.value() != self.cod
                    || line
                        .buyer_amount
                        .value()
                        .checked_add(line.compensation_amount.value())
                        .is_none_or(|adjusted_price| {
                            validate_settlement_price_breakdown(
                                line.end_price.value(),
                                line.adjustment_ppm,
                                adjusted_price,
                                line.compensation_amount.value(),
                            )
                            .is_err()
                        })
            })
        {
            return false;
        }

        let has_equipment = self.equipment_id.is_some_and(|id| !id.is_nil());
        match self.kind {
            SettlementObligationKind::WinnerEquipmentCod => {
                self.operation == "send"
                    && self.world == "main"
                    && self.template == "settlement_winner_equipment_cod"
                    && self.reference == self.obligation_id.to_string()
                    && trimmed_non_empty(&self.item_name)
                    && self.credits == 0
                    && self.cod > 0
                    && has_equipment
            }
            SettlementObligationKind::WinnerPaymentCarrierCod => {
                self.operation == "send"
                    && self.world == "main"
                    && self.template == "settlement_payment_carrier"
                    && self.reference == self.obligation_id.to_string()
                    && self.item_name.is_empty()
                    && self.credits > 0
                    && self.cod > self.credits
                    && self.equipment_id.is_none()
            }
            SettlementObligationKind::WinnerEquipmentHandoff => {
                self.operation == "send"
                    && self.world == "isekai"
                    && self.template == "settlement_equipment_handoff"
                    && self.reference == self.obligation_id.to_string()
                    && trimmed_non_empty(&self.item_name)
                    && self.credits == 0
                    && self.cod == 0
                    && has_equipment
            }
            SettlementObligationKind::SellerPayout => {
                valid_credit_delivery(self, "settlement_seller_payout")
            }
            SettlementObligationKind::BuyerRefund => {
                valid_credit_delivery(self, "settlement_buyer_refund")
            }
            SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
                self.template == "settlement_equipment_handoff"
                    && self.reference == self.obligation_id.to_string()
                    && trimmed_non_empty(&self.item_name)
                    && self.credits == 0
                    && self.cod == 0
                    && has_equipment
            }
        }
    }
}

fn valid_credit_delivery(intent: &SettlementDeliveryIntent, template: &str) -> bool {
    intent.operation == "send"
        && intent.world == "main"
        && intent.template == template
        && intent.reference == intent.obligation_id.to_string()
        && intent.item_name.is_empty()
        && intent.credits > 0
        && intent.cod == 0
        && intent.equipment_id.is_none()
}

fn trimmed_non_empty(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

/// 激活 prepared delivery 时计算的旧版稳定语义载荷。
#[derive(Debug, Serialize)]
pub(super) struct SettlementDeliveryActivation {
    pub(super) schema_version: u8,
    pub(super) delivery_id: String,
    pub(super) obligation_id: String,
    pub(super) prepare_request_digest: [u8; 32],
    pub(super) expected_status: &'static str,
}
