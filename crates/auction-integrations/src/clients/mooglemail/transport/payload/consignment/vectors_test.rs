//! 七个寄售发送模板的黄金字节与变量闭集（核心：金额 / 正文冻结）。

use std::collections::BTreeMap;

use super::render_consignment_send;

const OBLIGATION: &str = "0f3c8f60-1111-4a2b-8a1d-6f2f5f4b9c11";
const CONTRACT: &str = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
const ROUND: &str = "0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01";
const DEADLINE: &str = "2026-11-10T12:00:00Z";
const TAIL: &str = "\n\nContract: 5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a\nReference: 0f3c8f60-1111-4a2b-8a1d-6f2f5f4b9c11\n\nDispatch reference: 0d6c1d0c-63c4-5d0b-9a4f-1d7d2f8e9a01";

fn variables(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    [
        ("reference", OBLIGATION),
        ("contract", CONTRACT),
        ("dispatch_ref", ROUND),
        ("item_name", "Radiant Phase Shoes"),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect()
}

fn golden(template: &str, extra: &[(&str, &str)], subject: &str, lead: &str) {
    let (actual_subject, body) =
        render_consignment_send(template, &variables(extra)).expect("冻结变量必须可渲染");
    assert_eq!(actual_subject, format!("{subject} [{OBLIGATION}]"));
    assert_eq!(body, format!("{lead}{TAIL}"), "{template} 正文字节已冻结");
}

#[test]
fn equipment_cod_body_is_frozen() {
    golden(
        "consignment_equipment_cod",
        &[("amount", "1234567"), ("payment_deadline", DEADLINE)],
        "Consignment equipment COD",
        "Your consignment purchase is ready to collect.\n\nItem: Radiant Phase Shoes\n\nAmount due (CoD): 1,234,567 Credits\nPayment deadline: 2026-11-10T12:00:00Z\n\nCollect the attached equipment to pay the CoD\nand receive your item.",
    );
}

#[test]
fn payment_carrier_body_is_frozen() {
    golden(
        "consignment_payment_carrier",
        &[("amount", "1001"), ("payment_deadline", DEADLINE)],
        "Consignment payment carrier",
        "This mail collects payment for your Isekai consignment purchase.\n\nItem: Radiant Phase Shoes\nCoD: 1,001 Credits\nPayment deadline: 2026-11-10T12:00:00Z\n\nCollect the attached Credits to pay the CoD.\nYour equipment will be sent separately in Isekai\nafter payment is confirmed.",
    );
}

#[test]
fn equipment_handoff_and_withdrawal_bodies_are_frozen() {
    golden(
        "consignment_equipment_handoff",
        &[],
        "Consignment equipment handoff",
        "The following consignment equipment is attached.\n\nItem: Radiant Phase Shoes\n\nPlease collect the attachment.",
    );
    golden(
        "consignment_equipment_withdrawal",
        &[],
        "Consignment equipment withdrawal",
        "Your withdrawn consignment equipment is attached.\n\nItem: Radiant Phase Shoes\n\nPlease collect the attachment.",
    );
}

#[test]
fn payout_and_refund_bodies_are_frozen() {
    golden(
        "consignment_seller_payout",
        &[("amount", "5250000")],
        "Consignment seller payout",
        "Your consignment payout is attached.\n\nItem: Radiant Phase Shoes\nAmount: 5,250,000 Credits\n\nPlease collect the attached Credits.",
    );
    golden(
        "consignment_buyer_refund",
        &[("amount", "5250000")],
        "Consignment buyer refund",
        "Your consignment refund is attached.\n\nItem: Radiant Phase Shoes\nAmount: 5,250,000 Credits\n\nPlease collect the attached Credits.",
    );
}

#[test]
fn payment_reminder_body_is_frozen() {
    golden(
        "consignment_payment_reminder",
        &[("amount", "1001"), ("payment_deadline", DEADLINE)],
        "Consignment payment reminder",
        "Payment for this consignment purchase is overdue.\n\nItem: Radiant Phase Shoes\nAmount due (CoD): 1,001 Credits\nPayment deadline: 2026-11-10T12:00:00Z\n\nPlease check your original consignment payment mail.\nIf you have already paid, contact the auction\noperator with the references below.",
    );
}

/// 失败关闭：缺轮引用（首派也必须带）、多余键、非规范金额 / 期限 / UUID、拍卖模板名都不渲染。
#[test]
fn render_rejects_non_closed_variable_sets() {
    let payout = [("amount", "5250000")];
    assert!(render_consignment_send("consignment_seller_payout", &variables(&payout)).is_some());
    let mut missing = variables(&payout);
    missing.remove("dispatch_ref");
    assert!(render_consignment_send("consignment_seller_payout", &missing).is_none());
    let mut extra = variables(&payout);
    extra.insert("payment_deadline".to_owned(), DEADLINE.to_owned());
    assert!(render_consignment_send("consignment_seller_payout", &extra).is_none());
    for bad in ["0", "-5", "007", "1e3", ""] {
        assert!(
            render_consignment_send("consignment_seller_payout", &variables(&[("amount", bad)]))
                .is_none(),
            "{bad}"
        );
    }
    let cod_with = |deadline: &'static str| {
        render_consignment_send(
            "consignment_equipment_cod",
            &variables(&[("amount", "10"), ("payment_deadline", deadline)]),
        )
    };
    assert!(cod_with(DEADLINE).is_some());
    assert!(cod_with("2026-11-10").is_none());
    let mut upper_ref = variables(&payout);
    upper_ref.insert("dispatch_ref".to_owned(), ROUND.to_ascii_uppercase());
    assert!(render_consignment_send("consignment_seller_payout", &upper_ref).is_none());
    assert!(render_consignment_send("settlement_seller_payout", &variables(&payout)).is_none());
}
