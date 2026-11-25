//! 寄售发送模板的公共面回归：规范主题、系统退件认证与拍卖识别互斥，以及正文末行轮引用可被退件解析取回。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use auction_application::consignment::{
    ConsignmentSendFacts, build_consignment_send_intent, consignment_send_template_name,
};
use auction_application::settlement::{
    ConsignmentSubjectKind, SYSTEM_RETURN_SENDER, parse_returned_settlement_body,
    system_returned_consignment_obligation_id, system_returned_settlement_obligation_id,
};
use auction_domain::{ConsignmentContractId, SettlementAmount, SettlementId};
use auction_integrations::mooglemail::render_consignment_send;
use auction_time::Timestamp;
use uuid::Uuid;

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
        obligation_id: SettlementId::from_uuid(Uuid::from_u128(0x11)),
        contract_id: ConsignmentContractId::from_uuid(Uuid::from_u128(0x22)),
        dispatch_round_id: Uuid::from_u128(0x33),
        item_name: "Radiant Phase Shoes",
        amount: money.then(|| SettlementAmount::new(1_000).unwrap()),
        payment_deadline: deadline
            .then(|| Timestamp::parse_from_rfc3339("2026-11-10T12:00:00Z").unwrap()),
    }
}

/// 应用层生成的意图能被冻结渲染器接受，主题与意图一致；退回主题只被寄售识别函数认回，
/// 拍卖识别函数一律返回 `None`；退件正文末行的轮引用可原样取回（首派也带）。
#[test]
fn intents_render_and_returned_mail_binds_only_to_consignment() {
    for kind in ConsignmentSubjectKind::ALL {
        let intent = build_consignment_send_intent(&facts(kind)).unwrap();
        let variables = intent
            .variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let (subject, body) = render_consignment_send(intent.template, &variables)
            .expect("应用层意图必须被冻结渲染器接受");
        assert_eq!(subject, intent.subject);
        assert_eq!(intent.template, consignment_send_template_name(kind));

        let returned_subject = format!("[RTS] {subject}");
        assert_eq!(
            system_returned_consignment_obligation_id(SYSTEM_RETURN_SENDER, &returned_subject),
            Some((kind, Uuid::from_u128(0x11)))
        );
        assert!(
            system_returned_settlement_obligation_id(SYSTEM_RETURN_SENDER, &returned_subject)
                .is_none(),
            "拍卖识别不得认出寄售主题：{returned_subject}"
        );

        let returned = format!("{body}\n\nThis message was returned from Buyer, kupo!");
        let parsed = parse_returned_settlement_body(&returned).expect("系统退件正文可解析");
        assert_eq!(
            parsed.dispatch_reference.as_deref(),
            Some(intent.variables["dispatch_ref"].as_str())
        );
    }
}

/// 反例：缺 `dispatch_ref`（首派也必填）或沿用拍卖模板名都不能渲染。
#[test]
fn renderer_refuses_missing_dispatch_ref_and_auction_template_names() {
    let intent =
        build_consignment_send_intent(&facts(ConsignmentSubjectKind::SellerPayout)).unwrap();
    let mut variables = intent
        .variables
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    assert!(render_consignment_send("settlement_seller_payout", &variables).is_none());
    variables.remove("dispatch_ref");
    assert!(render_consignment_send(intent.template, &variables).is_none());
}
