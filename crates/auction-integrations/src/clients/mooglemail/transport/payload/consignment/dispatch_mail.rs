//! 寄售派发邮件的唯一渲染来源：购买与补寄备好事务经 [`render_first_dispatch`] 冻结首轮 / 新轮内容，
//! 撤回事务经 [`render_withdrawal_dispatch`] 冻结撤回件首轮内容，下游首轮生产者经
//! [`render_downstream_dispatch`] 冻结 isekai 原件交割、卖款与退款首轮内容，Worker 发信前经
//! [`render_frozen_dispatch`] 按轮上冻结身份重新渲染正文，并以冻结摘要核对。
//!
//! 两处走同一条路径（合同事实 → 发送意图 → 代码冻结正文），保证冻结时的摘要与发信时的正文逐字
//! 同源。任一不一致都失败关闭：冻结时整笔购买回滚（503，同命令 ID 重试），发信前不起写、不 POST
//! （不变量 4、6）。本模块不写任何行、不外呼；正文只交回调用方的发送请求，不进日志与错误。

use std::collections::BTreeMap;
use std::fmt;

use auction_application::consignment::{
    ConsignmentError, ConsignmentSendFacts, build_consignment_send_intent,
    consignment_send_template_kind,
};
use auction_application::settlement::{ConsignmentSubjectKind, returned_mail_body_digest};
use auction_domain::consignment::dispatch::{
    DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref,
};
use auction_domain::consignment::{ConsignmentObligation, ListingPrice};
use auction_domain::{
    ConsignmentContractId, SettlementAmount, SettlementId, SettlementObligationKind,
    SettlementWorld,
};
use auction_time::Timestamp;
use uuid::Uuid;

use super::render_consignment_send;

/// 冻结一轮付款邮件所需的合同事实；不含装备 key、URL 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct FirstDispatchFacts<'a> {
    /// 派发世界。
    pub world: SettlementWorld,
    /// 成交价 P。
    pub price: ListingPrice,
    /// 轮引用，即轮 ID 的小写带连字符 UUID。
    pub round_ref: &'a str,
    /// 付款期限 D0。
    pub payment_deadline_at: Timestamp,
    /// 装备显示名（挂单登记时的安全快照）。
    pub item_name: &'a str,
    /// 规范装备编号。
    pub canonical_eid: &'a str,
    /// 本轮付款义务（原件 COD 或付款载体）。
    pub payment: &'a ConsignmentObligation,
}

/// 冻结撤回件首轮所需的合同事实；不含装备 key、URL 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct WithdrawalDispatchFacts<'a> {
    /// 派发世界（原件所在世界）。
    pub world: SettlementWorld,
    /// 轮引用，即轮 ID 的小写带连字符 UUID。
    pub round_ref: &'a str,
    /// 装备显示名（挂单登记时的安全快照）。
    pub item_name: &'a str,
    /// 规范装备编号。
    pub canonical_eid: &'a str,
    /// 撤回合同唯一的返还义务。
    pub obligation: &'a ConsignmentObligation,
}

/// 冻结下游首轮（isekai 原件交割、卖款 S、退款 P）所需的合同事实；不含装备 key、URL 或任何凭据。
#[derive(Debug, Clone, Copy)]
pub struct DownstreamDispatchFacts<'a> {
    /// 轮引用，即轮 ID 的小写带连字符 UUID。
    pub round_ref: &'a str,
    /// 装备显示名（挂单登记时的安全快照）。
    pub item_name: &'a str,
    /// 规范装备编号。
    pub canonical_eid: &'a str,
    /// 本轮义务。
    pub obligation: &'a ConsignmentObligation,
}

/// 渲染结果：模板名、规范主题、正文与标的装备；`Debug` 不输出正文。
pub struct RenderedDispatchMail {
    /// 冻结模板名。
    pub template: &'static str,
    /// 规范主题。
    pub subject: String,
    /// 代码冻结正文。
    pub body: String,
    /// 标的装备编号（main 为规范装备，isekai 载体为空）。
    pub equipment_ids: Vec<String>,
}

impl RenderedDispatchMail {
    /// 正文摘要：与发送围栏、系统退件比对同一摘要规范。
    pub fn body_digest(&self) -> [u8; 32] {
        returned_mail_body_digest(&self.body)
    }
}

impl fmt::Debug for RenderedDispatchMail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RenderedDispatchMail")
            .field("template", &self.template)
            .field("equipment_count", &self.equipment_ids.len())
            .finish_non_exhaustive()
    }
}

/// 按合同事实渲染一轮付款邮件（首轮与补寄新轮共用）。
///
/// 世界与义务种类不配对、义务本金不等于成交价、金额溢出或渲染不成立时返回 `Unavailable`
/// （不变量 6）。
pub fn render_first_dispatch(
    facts: &FirstDispatchFacts<'_>,
) -> Result<RenderedDispatchMail, ConsignmentError> {
    let payment = facts.payment;
    // main：原件 COD，标的就是挂单的规范装备；isekai：main 世界 1 Credit 付款载体，无装备
    // （D9 §4.1、§5.1）。世界与义务种类不配对说明合同图异常，失败关闭。
    let (kind, equipment_ids) = match (facts.world, payment.world, payment.kind) {
        (
            SettlementWorld::Main,
            SettlementWorld::Main,
            SettlementObligationKind::WinnerEquipmentCod,
        ) if payment.carrier_credit == 0 => {
            (ConsignmentSubjectKind::EquipmentCod, vec![facts.canonical_eid.to_owned()])
        }
        (SettlementWorld::Isekai, SettlementObligationKind::WinnerPaymentCarrierCod) => {
            (ConsignmentSubjectKind::PaymentCarrier, Vec::new())
        }
        _ => return Err(ConsignmentError::Unavailable),
    };
    // 付款义务的本金必须就是成交价 P，否则合同与挂单不一致（不变量 6）。
    if payment.amount.value() != facts.price.value() {
        return Err(ConsignmentError::Unavailable);
    }
    // 统筹方裁定：邮件金额取首轮冻结身份里的 COD——main 为 P，isekai 为 P + 载体 1 Credit——与
    // 冻结身份的 `cod` 同一来源，不另算。
    let cod = payment
        .amount
        .value()
        .checked_add(i64::from(payment.carrier_credit))
        .ok_or(ConsignmentError::Unavailable)?;
    let amount = SettlementAmount::new(cod).map_err(unavailable)?;
    let dispatch_round_id = Uuid::parse_str(facts.round_ref).map_err(unavailable)?;
    let (template, subject, body) = render(&ConsignmentSendFacts {
        kind,
        obligation_id: payment.id,
        contract_id: payment.contract_id,
        dispatch_round_id,
        item_name: facts.item_name,
        amount: Some(amount),
        payment_deadline: Some(facts.payment_deadline_at),
    })
    .ok_or(ConsignmentError::Unavailable)?;
    Ok(RenderedDispatchMail { template, subject, body, equipment_ids })
}

/// 按撤回合同事实渲染撤回件首轮（PIWX 裁定：模板 `consignment_equipment_withdrawal`）。
///
/// 撤回件把原件寄还卖家：义务必须是零金额、零载体的 `withdrawn_return`，标的恰为规范装备，
/// 不带金额与期限；任一不符返回 `Unavailable`（不变量 6）。
pub fn render_withdrawal_dispatch(
    facts: &WithdrawalDispatchFacts<'_>,
) -> Result<RenderedDispatchMail, ConsignmentError> {
    let obligation = facts.obligation;
    if obligation.kind != SettlementObligationKind::WithdrawnReturn
        || obligation.world != facts.world
        || obligation.amount.value() != 0
        || obligation.carrier_credit != 0
    {
        return Err(ConsignmentError::Unavailable);
    }
    let dispatch_round_id = Uuid::parse_str(facts.round_ref).map_err(unavailable)?;
    let (template, subject, body) = render(&ConsignmentSendFacts {
        kind: ConsignmentSubjectKind::EquipmentWithdrawal,
        obligation_id: obligation.id,
        contract_id: obligation.contract_id,
        dispatch_round_id,
        item_name: facts.item_name,
        amount: None,
        payment_deadline: None,
    })
    .ok_or(ConsignmentError::Unavailable)?;
    Ok(RenderedDispatchMail {
        template,
        subject,
        body,
        equipment_ids: vec![facts.canonical_eid.to_owned()],
    })
}

/// 按下游义务渲染首轮（PIWP）：isekai 原件交割寄规范原件、不带金额；卖款与退款只带 Credits（义务金额 S 或 P）、
/// 不带装备与期限。义务种类、世界或金额形状不符返回 `Unavailable`（不变量 6）。
pub fn render_downstream_dispatch(
    facts: &DownstreamDispatchFacts<'_>,
) -> Result<RenderedDispatchMail, ConsignmentError> {
    let obligation = facts.obligation;
    let cash = obligation.amount.value() > 0 && obligation.carrier_credit == 0;
    let (kind, amount, equipment_ids) = match obligation.kind {
        SettlementObligationKind::WinnerEquipmentHandoff
            if obligation.world == SettlementWorld::Isekai
                && obligation.amount.value() == 0
                && obligation.carrier_credit == 0 =>
        {
            (ConsignmentSubjectKind::EquipmentHandoff, None, vec![facts.canonical_eid.to_owned()])
        }
        SettlementObligationKind::SellerPayout if cash => {
            (ConsignmentSubjectKind::SellerPayout, Some(obligation.amount), Vec::new())
        }
        SettlementObligationKind::BuyerRefund if cash => {
            (ConsignmentSubjectKind::BuyerRefund, Some(obligation.amount), Vec::new())
        }
        _ => return Err(ConsignmentError::Unavailable),
    };
    let dispatch_round_id = Uuid::parse_str(facts.round_ref).map_err(unavailable)?;
    let (template, subject, body) = render(&ConsignmentSendFacts {
        kind,
        obligation_id: obligation.id,
        contract_id: obligation.contract_id,
        dispatch_round_id,
        item_name: facts.item_name,
        amount,
        payment_deadline: None,
    })
    .ok_or(ConsignmentError::Unavailable)?;
    Ok(RenderedDispatchMail { template, subject, body, equipment_ids })
}

/// 按轮上冻结身份重新渲染派发邮件正文，供发信前核对；只有模板、主题、轮引用与正文摘要全部等于
/// 冻结值时才返回正文。
///
/// 只接受有派发轮的模板：付款类（原件 COD、付款载体）按冻结 COD 与 D0 渲染；撤回件（PIWX）与 isekai 原件
/// 交割不带金额与期限，冻结 Credits 与 COD 必须为零；卖款与退款（PIWP）按冻结 Credits 渲染、COD 必须为零。
/// 付款提醒没有派发轮，出现即失败关闭（不变量 6）。
pub fn render_frozen_dispatch(
    key: &DispatchRoundKey,
    frozen: &FrozenDispatchIdentity,
    item_name: &str,
) -> Option<String> {
    let kind = consignment_send_template_kind(&frozen.template)?;
    if frozen.round_ref != dispatch_ref(key) {
        return None;
    }
    let (amount, payment_deadline) = match kind {
        ConsignmentSubjectKind::EquipmentCod | ConsignmentSubjectKind::PaymentCarrier => {
            (Some(SettlementAmount::new(frozen.cod).ok()?), Some(frozen.d0))
        }
        ConsignmentSubjectKind::EquipmentWithdrawal | ConsignmentSubjectKind::EquipmentHandoff
            if frozen.cod == 0 && frozen.credits == 0 =>
        {
            (None, None)
        }
        ConsignmentSubjectKind::SellerPayout | ConsignmentSubjectKind::BuyerRefund
            if frozen.cod == 0 =>
        {
            (Some(SettlementAmount::new(frozen.credits).ok()?), None)
        }
        _ => return None,
    };
    let (template, subject, body) = render(&ConsignmentSendFacts {
        kind,
        obligation_id: SettlementId::from_uuid(key.obligation_id),
        contract_id: ConsignmentContractId::from_uuid(key.contract_id),
        dispatch_round_id: Uuid::parse_str(&frozen.round_ref).ok()?,
        item_name,
        amount,
        payment_deadline,
    })?;
    (template == frozen.template
        && subject == frozen.subject
        && returned_mail_body_digest(&body) == frozen.body_digest)
        .then_some(body)
}

/// 合同事实异常的统一结论：只给出“不可用”，不透出原错误（其中可能含金额或标识）。
fn unavailable<T>(_error: T) -> ConsignmentError {
    ConsignmentError::Unavailable
}

/// 意图 → 冻结正文；渲染出的主题必须就是意图的规范主题。
fn render(facts: &ConsignmentSendFacts<'_>) -> Option<(&'static str, String, String)> {
    let intent = build_consignment_send_intent(facts).ok()?;
    let variables: BTreeMap<String, String> =
        intent.variables.into_iter().map(|(name, value)| (name.to_owned(), value)).collect();
    let (subject, body) = render_consignment_send(intent.template, &variables)?;
    (subject == intent.subject).then_some((intent.template, subject, body))
}

#[cfg(test)]
mod tests {
    use auction_application::consignment::ConsignmentError;
    use auction_domain::consignment::dispatch::{
        DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref,
    };
    use auction_domain::consignment::{ConsignmentObligation, ListingPrice};
    use auction_domain::{
        ConsignmentContractId, ConsignmentContractKind, SettlementAmount, SettlementId,
        SettlementObligationKind, SettlementObligationState, SettlementWorld,
    };
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{
        FirstDispatchFacts, WithdrawalDispatchFacts, render_first_dispatch, render_frozen_dispatch,
        render_withdrawal_dispatch,
    };

    const OBLIGATION: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0001);
    const CONTRACT: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0002);

    fn key() -> DispatchRoundKey {
        DispatchRoundKey { contract_id: CONTRACT, obligation_id: OBLIGATION, round: 1 }
    }

    fn obligation(world: SettlementWorld) -> ConsignmentObligation {
        let (kind, carrier_credit) = match world {
            SettlementWorld::Main => (SettlementObligationKind::WinnerEquipmentCod, 0),
            SettlementWorld::Isekai => (SettlementObligationKind::WinnerPaymentCarrierCod, 1),
        };
        ConsignmentObligation {
            id: SettlementId::from_uuid(OBLIGATION),
            contract_id: ConsignmentContractId::from_uuid(CONTRACT),
            contract_kind: ConsignmentContractKind::Sale,
            world,
            kind,
            state: SettlementObligationState::Ready,
            state_version: 1,
            policy_version: 1,
            recipient_user_id: None,
            recipient_game_identity: String::new(),
            amount: SettlementAmount::new(100).expect("金额"),
            carrier_credit,
            custody_equipment_id: Uuid::from_u128(0x4000_8000_0000_0000_0003),
            prerequisite_id: None,
        }
    }

    fn facts<'a>(
        world: SettlementWorld,
        payment: &'a ConsignmentObligation,
        round_ref: &'a str,
    ) -> FirstDispatchFacts<'a> {
        FirstDispatchFacts {
            world,
            price: ListingPrice::new(100).expect("价格"),
            round_ref,
            payment_deadline_at: Timestamp::parse_from_rfc3339("2026-12-01T00:00:00Z")
                .expect("期限"),
            item_name: "Fixture Phase Shoes",
            canonical_eid: "123456",
            payment,
        }
    }

    /// 金额取冻结 COD：main 为 P，标的是规范装备；isekai 为 P+1，载体不带装备。
    #[test]
    fn first_mail_amount_is_the_frozen_cod_for_each_world() {
        let round_ref = dispatch_ref(&key());
        let main = obligation(SettlementWorld::Main);
        let mail =
            render_first_dispatch(&facts(SettlementWorld::Main, &main, &round_ref)).expect("main");
        assert_eq!(mail.template, "consignment_equipment_cod");
        assert!(mail.body.contains("Amount due (CoD): 100 Credits"), "{}", mail.body);
        assert_eq!(mail.equipment_ids, ["123456"]);

        let isekai = obligation(SettlementWorld::Isekai);
        let mail = render_first_dispatch(&facts(SettlementWorld::Isekai, &isekai, &round_ref))
            .expect("isekai");
        assert_eq!(mail.template, "consignment_payment_carrier");
        assert!(mail.body.contains("CoD: 101 Credits"), "{}", mail.body);
        assert!(mail.equipment_ids.is_empty());
    }

    /// 义务本金与成交价不符时失败关闭，不渲染任何内容。
    #[test]
    fn principal_that_differs_from_the_price_is_unavailable() {
        let round_ref = dispatch_ref(&key());
        let mut main = obligation(SettlementWorld::Main);
        main.amount = SettlementAmount::new(99).expect("金额");
        assert!(matches!(
            render_first_dispatch(&facts(SettlementWorld::Main, &main, &round_ref)),
            Err(ConsignmentError::Unavailable)
        ));
    }

    /// 发信前按冻结身份重渲染得到与冻结时逐字相同的正文；冻结摘要不符（正文被改）即不给正文。
    #[test]
    fn frozen_identity_rerenders_the_same_body_and_rejects_a_digest_mismatch() {
        let round_ref = dispatch_ref(&key());
        let main = obligation(SettlementWorld::Main);
        let mail =
            render_first_dispatch(&facts(SettlementWorld::Main, &main, &round_ref)).expect("main");
        let mut frozen = FrozenDispatchIdentity {
            rights_holder_identity: "FakeBuyer".to_owned(),
            bot_uid: "12345".to_owned(),
            world: SettlementWorld::Main,
            subject: mail.subject.clone(),
            template: mail.template.to_owned(),
            body_digest: mail.body_digest(),
            round_ref,
            d0: Timestamp::parse_from_rfc3339("2026-12-01T00:00:00Z").expect("期限"),
            credits: 0,
            cod: 100,
            equipment_ids: mail.equipment_ids.clone(),
        };
        let body = render_frozen_dispatch(&key(), &frozen, "Fixture Phase Shoes");
        assert_eq!(body.as_deref(), Some(mail.body.as_str()));

        frozen.body_digest[0] ^= 1;
        assert_eq!(render_frozen_dispatch(&key(), &frozen, "Fixture Phase Shoes"), None);
    }

    /// 撤回件首轮：零金额返还义务渲染出撤回模板、标的为规范装备，发信前按冻结身份重渲染逐字相同；
    /// 反例：冻结身份带 COD 时不给正文，义务带金额时不渲染。
    #[test]
    fn withdrawal_round_renders_without_amount_and_rejects_money() {
        let round_ref = dispatch_ref(&key());
        let mut withdrawn = obligation(SettlementWorld::Main);
        withdrawn.contract_kind = ConsignmentContractKind::Withdrawal;
        withdrawn.kind = SettlementObligationKind::WithdrawnReturn;
        withdrawn.amount = SettlementAmount::zero();
        let facts = WithdrawalDispatchFacts {
            world: SettlementWorld::Main,
            round_ref: &round_ref,
            item_name: "Fixture Phase Shoes",
            canonical_eid: "123456",
            obligation: &withdrawn,
        };
        let mail = render_withdrawal_dispatch(&facts).expect("撤回件");
        assert_eq!(mail.template, "consignment_equipment_withdrawal");
        assert_eq!(mail.equipment_ids, ["123456"]);
        let mut frozen = FrozenDispatchIdentity {
            rights_holder_identity: "FakeSeller".to_owned(),
            bot_uid: "12345".to_owned(),
            world: SettlementWorld::Main,
            subject: mail.subject.clone(),
            template: mail.template.to_owned(),
            body_digest: mail.body_digest(),
            round_ref: round_ref.clone(),
            d0: Timestamp::parse_from_rfc3339("2026-12-01T00:00:00Z").expect("时刻"),
            credits: 0,
            cod: 0,
            equipment_ids: mail.equipment_ids.clone(),
        };
        let body = render_frozen_dispatch(&key(), &frozen, "Fixture Phase Shoes");
        assert_eq!(body.as_deref(), Some(mail.body.as_str()));

        frozen.cod = 1;
        assert_eq!(render_frozen_dispatch(&key(), &frozen, "Fixture Phase Shoes"), None);
        let mut paid = withdrawn.clone();
        paid.amount = SettlementAmount::new(1).expect("金额");
        let facts = WithdrawalDispatchFacts { obligation: &paid, ..facts };
        assert!(matches!(render_withdrawal_dispatch(&facts), Err(ConsignmentError::Unavailable)));
    }
}
