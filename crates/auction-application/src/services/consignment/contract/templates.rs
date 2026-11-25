//! 寄售发送意图：模板名、变量与规范主题。
//!
//! 七个模板是代码冻结正文，不在运营可编辑的模板目录里；变量只来自已冻结的合同事实。变量里的
//! `dispatch_ref` 固定为轮 ID（`dispatch_round_id`）的小写带连字符 UUID，**首派也写**
//! （拍卖首封省略 dispatch 的字节另行保持）。正文渲染归 integrations，本模块不拼正文。

use std::collections::BTreeMap;

use auction_domain::{ConsignmentContractId, SettlementAmount, SettlementId};
use auction_time::Timestamp;
use thiserror::Error;
use uuid::Uuid;

use crate::ports::settlement::{ConsignmentSubjectKind, canonical_consignment_subject};
use crate::services::public::format_deadline;

/// 种类对应的发送模板名；已发布后不可修改。
pub const fn consignment_send_template_name(kind: ConsignmentSubjectKind) -> &'static str {
    match kind {
        ConsignmentSubjectKind::EquipmentCod => "consignment_equipment_cod",
        ConsignmentSubjectKind::PaymentCarrier => "consignment_payment_carrier",
        ConsignmentSubjectKind::EquipmentHandoff => "consignment_equipment_handoff",
        ConsignmentSubjectKind::SellerPayout => "consignment_seller_payout",
        ConsignmentSubjectKind::BuyerRefund => "consignment_buyer_refund",
        ConsignmentSubjectKind::EquipmentWithdrawal => "consignment_equipment_withdrawal",
        ConsignmentSubjectKind::PaymentReminder => "consignment_payment_reminder",
    }
}

/// 模板名反查种类；非寄售模板返回 `None`。
pub fn consignment_send_template_kind(template: &str) -> Option<ConsignmentSubjectKind> {
    ConsignmentSubjectKind::ALL
        .into_iter()
        .find(|kind| consignment_send_template_name(*kind) == template)
}

/// 构造发送意图的已冻结事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentSendFacts<'a> {
    pub kind: ConsignmentSubjectKind,
    pub obligation_id: SettlementId,
    pub contract_id: ConsignmentContractId,
    /// 本次派发轮 ID；首派同样必填。
    pub dispatch_round_id: Uuid,
    pub item_name: &'a str,
    /// 金额类邮件（COD、载体、卖款、退款、提醒）必填，其余必须为空。
    pub amount: Option<SettlementAmount>,
    /// 付款类邮件（COD、载体、提醒）冻结的首寄期限 D0，其余必须为空。
    pub payment_deadline: Option<Timestamp>,
}

/// 寄售发送意图：模板名、规范主题与模板变量（键集合按模板固定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentSendIntent {
    pub template: &'static str,
    pub subject: String,
    pub variables: BTreeMap<&'static str, String>,
}

/// 发送意图的事实与模板不匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConsignmentTemplateError {
    #[error("寄售发送事实含零标识或空装备名")]
    InvalidIdentity,
    #[error("寄售发送金额与模板不匹配")]
    AmountMismatch,
    #[error("寄售发送付款期限与模板不匹配")]
    DeadlineMismatch,
}

/// 按种类校验事实并生成发送意图。金额与期限的有无由种类固定，不静默补默认值
/// （不变量 6：非法金额、身份失败关闭）。
pub fn build_consignment_send_intent(
    facts: &ConsignmentSendFacts<'_>,
) -> Result<ConsignmentSendIntent, ConsignmentTemplateError> {
    let ConsignmentSendFacts {
        kind,
        obligation_id,
        contract_id,
        dispatch_round_id,
        item_name,
        amount,
        payment_deadline,
    } = facts;
    if obligation_id.is_nil()
        || contract_id.is_nil()
        || dispatch_round_id.is_nil()
        || item_name.trim().is_empty()
        || *item_name != item_name.trim()
    {
        return Err(ConsignmentTemplateError::InvalidIdentity);
    }
    let money = !matches!(
        kind,
        ConsignmentSubjectKind::EquipmentHandoff | ConsignmentSubjectKind::EquipmentWithdrawal
    );
    let deadline = matches!(
        kind,
        ConsignmentSubjectKind::EquipmentCod
            | ConsignmentSubjectKind::PaymentCarrier
            | ConsignmentSubjectKind::PaymentReminder
    );
    let mut variables = BTreeMap::from([
        ("reference", obligation_id.to_string()),
        ("contract", contract_id.to_string()),
        ("dispatch_ref", dispatch_round_id.to_string()),
        ("item_name", (*item_name).to_owned()),
    ]);
    match (money, amount) {
        (true, Some(amount)) if amount.value() > 0 => {
            variables.insert("amount", amount.value().to_string());
        }
        (false, None) => {}
        _ => return Err(ConsignmentTemplateError::AmountMismatch),
    }
    match (deadline, payment_deadline) {
        (true, Some(deadline)) => {
            variables.insert("payment_deadline", format_deadline(*deadline));
        }
        (false, None) => {}
        _ => return Err(ConsignmentTemplateError::DeadlineMismatch),
    }
    Ok(ConsignmentSendIntent {
        template: consignment_send_template_name(*kind),
        subject: canonical_consignment_subject(*kind, *obligation_id),
        variables,
    })
}

#[cfg(test)]
mod tests {
    use auction_domain::{ConsignmentContractId, SettlementAmount, SettlementId};
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{
        ConsignmentSendFacts, ConsignmentTemplateError, build_consignment_send_intent,
        consignment_send_template_kind, consignment_send_template_name,
    };
    use crate::ports::settlement::ConsignmentSubjectKind;

    fn facts(kind: ConsignmentSubjectKind) -> ConsignmentSendFacts<'static> {
        let money = !matches!(
            kind,
            ConsignmentSubjectKind::EquipmentHandoff | ConsignmentSubjectKind::EquipmentWithdrawal
        );
        let deadline = matches!(
            kind,
            ConsignmentSubjectKind::EquipmentCod
                | ConsignmentSubjectKind::PaymentCarrier
                | ConsignmentSubjectKind::PaymentReminder
        );
        ConsignmentSendFacts {
            kind,
            obligation_id: SettlementId::from_uuid(Uuid::from_u128(1)),
            contract_id: ConsignmentContractId::from_uuid(Uuid::from_u128(2)),
            dispatch_round_id: Uuid::from_u128(3),
            item_name: "Radiant Phase Shoes",
            amount: money.then(|| SettlementAmount::new(1_000).unwrap()),
            payment_deadline: deadline
                .then(|| Timestamp::parse_from_rfc3339("2026-11-10T12:00:00Z").unwrap()),
        }
    }

    /// 七个种类各有唯一模板名，主题与模板互相对应，首派也带轮引用。
    #[test]
    fn every_kind_builds_with_dispatch_ref_and_distinct_template() {
        let mut names = std::collections::BTreeSet::new();
        for kind in ConsignmentSubjectKind::ALL {
            let intent = build_consignment_send_intent(&facts(kind)).unwrap();
            assert_eq!(intent.template, consignment_send_template_name(kind));
            assert_eq!(consignment_send_template_kind(intent.template), Some(kind));
            assert_eq!(intent.variables["dispatch_ref"], "00000000-0000-0000-0000-000000000003");
            assert!(intent.subject.starts_with(kind.base()));
            assert!(names.insert(intent.template));
        }
        assert!(consignment_send_template_kind("settlement_seller_payout").is_none());
    }

    /// 金额与期限有无由种类固定：该有的缺失、不该有的出现、零金额都失败关闭。
    #[test]
    fn amount_and_deadline_must_match_the_kind() {
        let mut missing = facts(ConsignmentSubjectKind::SellerPayout);
        missing.amount = None;
        assert_eq!(
            build_consignment_send_intent(&missing),
            Err(ConsignmentTemplateError::AmountMismatch)
        );
        let mut zero = facts(ConsignmentSubjectKind::SellerPayout);
        zero.amount = Some(SettlementAmount::zero());
        assert_eq!(
            build_consignment_send_intent(&zero),
            Err(ConsignmentTemplateError::AmountMismatch)
        );
        let mut extra = facts(ConsignmentSubjectKind::EquipmentHandoff);
        extra.amount = Some(SettlementAmount::new(1).unwrap());
        assert_eq!(
            build_consignment_send_intent(&extra),
            Err(ConsignmentTemplateError::AmountMismatch)
        );
        let mut no_deadline = facts(ConsignmentSubjectKind::PaymentCarrier);
        no_deadline.payment_deadline = None;
        assert_eq!(
            build_consignment_send_intent(&no_deadline),
            Err(ConsignmentTemplateError::DeadlineMismatch)
        );
        let mut nil_round = facts(ConsignmentSubjectKind::BuyerRefund);
        nil_round.dispatch_round_id = Uuid::nil();
        assert_eq!(
            build_consignment_send_intent(&nil_round),
            Err(ConsignmentTemplateError::InvalidIdentity)
        );
    }
}
