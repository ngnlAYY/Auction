//! 结算退件的来源认证与消费判定：只认系统 `[RTS]` + canonical 结算主题，主题绑定
//! 义务种类；恢复流程之外的退件按绑定与履约状态给出闭集处置。只覆盖纯判定，不连接数据库。

use auction_application::settlement::{
    SYSTEM_RETURN_SENDER, SettlementReturnDelivery, SettlementReturnDispatchLookup,
    SettlementReturnFacts, SettlementReturnMatch, SettlementReturnObligation,
    SettlementReturnReason, parse_returned_settlement_body, returned_mail_body_digest,
    settlement_return_dispatch_request, system_return_matches_obligation,
    system_returned_settlement_obligation_id,
};
use auction_domain::{SettlementDispatchReference, SettlementObligationKind};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::json;
use uuid::Uuid;

const OBLIGATION_ID: &str = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
const EQUIPMENT_ID: &str = "300753957";

fn obligation_id() -> Uuid {
    Uuid::parse_str(OBLIGATION_ID).unwrap_or_else(|error| panic!("固定义务标识：{error}"))
}

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 5, 9, minute, 0)
        .single()
        .unwrap_or_else(|| panic!("固定测试时间必须存在"))
}

#[test]
fn only_system_rts_settlement_subjects_resolve_their_obligation() {
    for base in [
        "Auction equipment delivery",
        "Auction payment request",
        "Auction seller payout",
        "Auction buyer refund",
    ] {
        let subject = format!("[RTS] {base} [{OBLIGATION_ID}]");
        assert_eq!(
            system_returned_settlement_obligation_id(SYSTEM_RETURN_SENDER, &subject),
            Some(obligation_id()),
            "{base}"
        );
        // 发件人精确比较：玩家或大小写不同的名字都不是系统退信。
        for sender in ["Player", "mooglemail", "MOOGLEMAIL", "MoogleMail "] {
            assert_eq!(system_returned_settlement_obligation_id(sender, &subject), None);
        }
        // 无前缀兼容入口已撤掉：原 canonical 主题本身不是退件。
        let canonical = format!("{base} [{OBLIGATION_ID}]");
        assert_eq!(
            system_returned_settlement_obligation_id(SYSTEM_RETURN_SENDER, &canonical),
            None
        );
    }
    let upper = format!("[rts] AUCTION SELLER PAYOUT [{}]", OBLIGATION_ID.to_ascii_uppercase());
    assert_eq!(
        system_returned_settlement_obligation_id(SYSTEM_RETURN_SENDER, &upper),
        Some(obligation_id())
    );
}

#[test]
fn malformed_or_foreign_returned_subjects_are_rejected() {
    for subject in [
        "[RTS] Auction payment overdue".to_owned(),
        format!("[RTS] [RTS] Auction payment request [{OBLIGATION_ID}]"),
        format!("[RTS]Auction payment request [{OBLIGATION_ID}]"),
        format!("[RTS] Auction payment request [{OBLIGATION_ID}] "),
        format!("[RTS] Auction payment requests [{OBLIGATION_ID}]"),
        format!("[RTS] Auction payment request [{{{OBLIGATION_ID}}}]"),
        "[RTS] Auction payment request [00000000-0000-0000-0000-000000000000]".to_owned(),
        "[RTS] Auction payment request [5b1c2a9e1f0d4c6b9a7e3d2f1e0c9b8a]".to_owned(),
    ] {
        assert_eq!(
            system_returned_settlement_obligation_id(SYSTEM_RETURN_SENDER, &subject),
            None,
            "{subject}"
        );
    }
}

#[test]
fn returned_subject_must_match_the_obligation_kind() {
    let id = obligation_id();
    let payout = format!("[RTS] Auction seller payout [{OBLIGATION_ID}]");
    assert!(system_return_matches_obligation(
        SYSTEM_RETURN_SENDER,
        &payout,
        SettlementObligationKind::SellerPayout,
        id
    ));
    assert!(!system_return_matches_obligation(
        SYSTEM_RETURN_SENDER,
        &payout,
        SettlementObligationKind::BuyerRefund,
        id
    ));
    assert!(!system_return_matches_obligation(
        "Player",
        &payout,
        SettlementObligationKind::SellerPayout,
        id
    ));
    let delivery = format!("[rts] auction equipment delivery [{OBLIGATION_ID}]");
    for kind in [
        SettlementObligationKind::WinnerEquipmentCod,
        SettlementObligationKind::WinnerEquipmentHandoff,
        SettlementObligationKind::UnsoldReturn,
        SettlementObligationKind::WithdrawnReturn,
    ] {
        assert!(system_return_matches_obligation(SYSTEM_RETURN_SENDER, &delivery, kind, id));
    }
    assert!(!system_return_matches_obligation(
        SYSTEM_RETURN_SENDER,
        &delivery,
        SettlementObligationKind::WinnerPaymentCarrierCod,
        id
    ));
}

/// 卖方货款被退回、原投递已寄出且履约仍等待领取证据的标准事实。
fn payout_facts() -> SettlementReturnFacts {
    SettlementReturnFacts {
        sender: Some(SYSTEM_RETURN_SENDER.to_owned()),
        subject: Some(format!("[RTS] Auction seller payout [{OBLIGATION_ID}]")),
        return_world: "main".to_owned(),
        received_at: at(30),
        metadata: json!({
            "obligation_id": OBLIGATION_ID,
            "credits": 250_000,
            "cod": 0,
            "attachment_ids": [],
        }),
        obligation: Some(SettlementReturnObligation {
            id: obligation_id(),
            kind: "seller_payout".to_owned(),
            amount: 250_000,
            carrier_credit: 0,
            external_equipment_id: Some(EQUIPMENT_ID.to_owned()),
            execution_state: Some("enqueued".to_owned()),
            delivery: Some(SettlementReturnDelivery {
                id: Uuid::from_u128(9),
                world: Some("main".to_owned()),
                operation: Some("send".to_owned()),
                sending_at: Some(at(10) + Duration::seconds(42)),
                dispatch_no: 1,
                current: true,
                recipient: "SellerPlayer".to_owned(),
                body_digests: Vec::new(),
            }),
            dispatch_lookup: SettlementReturnDispatchLookup::Unique,
        }),
    }
}

fn decision(facts: &SettlementReturnFacts) -> (SettlementReturnMatch, SettlementReturnReason) {
    let decision = facts.classify();
    (decision.matched, decision.reason)
}

#[test]
fn bound_return_with_live_fulfillment_goes_to_manual_reconcile() {
    assert_eq!(
        decision(&payout_facts()),
        (SettlementReturnMatch::ManualReconcile, SettlementReturnReason::ReturnedAssetsMatch)
    );
    for state in ["pending", "prepared"] {
        let mut facts = payout_facts();
        if let Some(obligation) = facts.obligation.as_mut() {
            obligation.execution_state = Some(state.to_owned());
        }
        assert_eq!(decision(&facts).0, SettlementReturnMatch::ManualReconcile, "{state}");
    }

    // 流拍寄回：原装备原样退回。
    let mut unsold = payout_facts();
    unsold.subject = Some(format!("[RTS] Auction equipment delivery [{OBLIGATION_ID}]"));
    unsold.metadata = json!({"obligation_id": OBLIGATION_ID, "credits": 0, "cod": 0, "attachment_ids": [EQUIPMENT_ID]});
    if let Some(obligation) = unsold.obligation.as_mut() {
        obligation.kind = "unsold_return".to_owned();
        obligation.amount = 0;
    }
    assert_eq!(
        decision(&unsold),
        (SettlementReturnMatch::ManualReconcile, SettlementReturnReason::ReturnedAssetsMatch)
    );

    // 退回资产与冻结金额不符仍转人工，原因码区分。
    let mut short = payout_facts();
    short.metadata["credits"] = json!(249_999);
    assert_eq!(
        decision(&short),
        (SettlementReturnMatch::ManualReconcile, SettlementReturnReason::ReturnedAssetsMismatch)
    );

    // 收件时间早于原投递开始发送所在分钟再减 2 分钟：不是这次投递的退件。
    let mut early = payout_facts();
    early.received_at = at(7);
    assert_eq!(
        decision(&early),
        (SettlementReturnMatch::ManualReconcile, SettlementReturnReason::ReceivedBeforeSend)
    );
    let mut within_slack = payout_facts();
    within_slack.received_at = at(8);
    assert_eq!(decision(&within_slack).1, SettlementReturnReason::ReturnedAssetsMatch);
}

#[test]
fn terminal_fulfillment_is_only_recorded() {
    for state in ["satisfied", "failed", "manual_reconcile"] {
        let mut facts = payout_facts();
        if let Some(obligation) = facts.obligation.as_mut() {
            obligation.execution_state = Some(state.to_owned());
        }
        assert_eq!(
            decision(&facts),
            (
                SettlementReturnMatch::FulfillmentTerminal,
                SettlementReturnReason::ReturnedAssetsMatch
            ),
            "{state}"
        );
    }
}

#[test]
fn unproven_source_is_isolated_as_untrusted() {
    let mut player = payout_facts();
    player.sender = Some("BuyerPlayer".to_owned());
    let mut prefixless = payout_facts();
    prefixless.subject = Some(format!("Auction seller payout [{OBLIGATION_ID}]"));
    let mut legacy = payout_facts();
    legacy.sender = None;
    legacy.subject = None;
    for facts in [player, prefixless, legacy] {
        assert_eq!(
            decision(&facts),
            (SettlementReturnMatch::Untrusted, SettlementReturnReason::SourceUnproven)
        );
    }
}

#[test]
fn unbindable_system_returns_are_isolated() {
    let mut other_obligation = payout_facts();
    other_obligation.metadata["obligation_id"] = json!(Uuid::from_u128(77).to_string());
    let mut malformed = payout_facts();
    malformed.metadata = json!({"obligation_id": OBLIGATION_ID, "credits": 1});
    let mut unsorted = payout_facts();
    unsorted.metadata["attachment_ids"] = json!(["2", "1"]);
    for facts in [other_obligation, malformed, unsorted] {
        assert_eq!(
            decision(&facts),
            (SettlementReturnMatch::Unbound, SettlementReturnReason::EvidenceInvalid)
        );
    }

    let mut unknown = payout_facts();
    unknown.obligation = None;
    assert_eq!(
        decision(&unknown),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::ObligationUnknown)
    );

    let mut wrong_kind = payout_facts();
    if let Some(obligation) = wrong_kind.obligation.as_mut() {
        obligation.kind = "buyer_refund".to_owned();
    }
    assert_eq!(
        decision(&wrong_kind),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::SubjectKindMismatch)
    );

    let mut no_fulfillment = payout_facts();
    if let Some(obligation) = no_fulfillment.obligation.as_mut() {
        obligation.execution_state = None;
    }
    let mut no_delivery = payout_facts();
    if let Some(obligation) = no_delivery.obligation.as_mut() {
        obligation.delivery = None;
    }
    let mut never_sent = payout_facts();
    if let Some(delivery) =
        never_sent.obligation.as_mut().and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.sending_at = None;
    }
    let mut legacy_return = payout_facts();
    if let Some(delivery) =
        legacy_return.obligation.as_mut().and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.operation = Some("return".to_owned());
    }
    let mut unknown_state = payout_facts();
    if let Some(obligation) = unknown_state.obligation.as_mut() {
        obligation.execution_state = Some("archived".to_owned());
    }
    for facts in [no_fulfillment, no_delivery, never_sent, legacy_return, unknown_state] {
        assert_eq!(
            decision(&facts),
            (SettlementReturnMatch::Unbound, SettlementReturnReason::DeliveryUnsent)
        );
    }

    let mut other_world = payout_facts();
    other_world.return_world = "isekai".to_owned();
    assert_eq!(
        decision(&other_world),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::WorldMismatch)
    );
}

#[test]
fn stable_labels_match_the_database_closed_sets() {
    assert_eq!(
        [
            SettlementReturnMatch::Untrusted,
            SettlementReturnMatch::Unbound,
            SettlementReturnMatch::FulfillmentTerminal,
            SettlementReturnMatch::ManualReconcile,
        ]
        .map(SettlementReturnMatch::as_str),
        ["untrusted", "unbound", "fulfillment_terminal", "manual_reconcile"]
    );
    for reason in [
        SettlementReturnReason::SourceUnproven,
        SettlementReturnReason::EvidenceInvalid,
        SettlementReturnReason::ObligationUnknown,
        SettlementReturnReason::SubjectKindMismatch,
        SettlementReturnReason::DeliveryUnsent,
        SettlementReturnReason::WorldMismatch,
        SettlementReturnReason::ReturnedAssetsMatch,
        SettlementReturnReason::ReturnedAssetsMismatch,
        SettlementReturnReason::ReceivedBeforeSend,
    ] {
        let label = reason.as_str();
        // 与迁移 000111 的 reason_code CHECK 同一形状。
        assert!(
            label.len() <= 64
                && label.starts_with(|first: char| first.is_ascii_lowercase())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
            "{label}"
        );
    }
}

fn hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 补寄派发的退件：原正文末行带派发引用，系统后缀写明原收件人。
fn resend_facts() -> (SettlementReturnFacts, SettlementDispatchReference, [u8; 32]) {
    let reference = SettlementDispatchReference::for_resend(obligation_id(), 2)
        .unwrap_or_else(|| panic!("补寄派发引用必须存在"));
    let original = format!(
        "Your auction payout is attached.\n\nReference: {OBLIGATION_ID}\n\nDispatch reference: {}",
        reference.encode()
    );
    let parsed = parse_returned_settlement_body(&format!(
        "{original}\n\nThis message was returned from sellerplayer, kupo!"
    ))
    .unwrap_or_else(|| panic!("系统后缀必须完整"));
    let digest = returned_mail_body_digest(&original);
    assert_eq!(parsed.body_digest, digest);
    let mut facts = payout_facts();
    facts.metadata = json!({
        "obligation_id": OBLIGATION_ID,
        "credits": 250_000,
        "cod": 0,
        "attachment_ids": [],
        "returned_from": parsed.returned_from,
        "body_digest": hex(&parsed.body_digest),
        "dispatch_reference": parsed.dispatch_reference,
    });
    if let Some(delivery) =
        facts.obligation.as_mut().and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.id = reference.delivery_id;
        delivery.dispatch_no = 2;
        delivery.body_digests = vec![[0x11; 32], digest];
    }
    (facts, reference, digest)
}

/// 补寄退件只在派发引用属于该义务、解析到同一派发，且原正文摘要与原收件人都与派发冻结事实
/// 一致时绑定；任一不符都隔离为 unbound，不能默认绑定最新投递。
#[test]
fn resend_returns_bind_only_with_dispatch_and_body_proof() {
    let (facts, reference, _) = resend_facts();
    assert_eq!(
        settlement_return_dispatch_request(&facts.metadata),
        auction_application::settlement::SettlementReturnDispatchRequest::Resend(reference)
    );
    assert_eq!(
        decision(&facts),
        (SettlementReturnMatch::ManualReconcile, SettlementReturnReason::ReturnedAssetsMatch)
    );

    let mut wrong_digest = resend_facts().0;
    if let Some(delivery) =
        wrong_digest.obligation.as_mut().and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.body_digests = vec![[0x11; 32]];
    }
    let mut wrong_recipient = resend_facts().0;
    wrong_recipient.metadata["returned_from"] = json!("SomeoneElse");
    let mut missing_digest = resend_facts().0;
    if let Some(metadata) = missing_digest.metadata.as_object_mut() {
        metadata.remove("body_digest");
    }
    for facts in [wrong_digest, wrong_recipient, missing_digest] {
        assert_eq!(
            decision(&facts),
            (SettlementReturnMatch::Unbound, SettlementReturnReason::BodyUnproven)
        );
    }

    let foreign = SettlementDispatchReference::for_resend(Uuid::from_u128(77), 2)
        .expect("其他义务的补寄引用");
    let mut foreign_reference = resend_facts().0;
    foreign_reference.metadata["dispatch_reference"] = json!(foreign.encode());
    let mut malformed = resend_facts().0;
    malformed.metadata["dispatch_reference"] = json!("v1/2/not-a-uuid");
    for facts in [foreign_reference, malformed] {
        assert_eq!(
            decision(&facts),
            (SettlementReturnMatch::Unbound, SettlementReturnReason::EvidenceInvalid)
        );
    }

    let mut other_delivery = resend_facts().0;
    if let Some(delivery) =
        other_delivery.obligation.as_mut().and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.id = Uuid::from_u128(10);
    }
    let mut missing_dispatch = resend_facts().0;
    if let Some(obligation) = missing_dispatch.obligation.as_mut() {
        obligation.delivery = None;
        obligation.dispatch_lookup = SettlementReturnDispatchLookup::Missing;
    }
    for facts in [other_delivery, missing_dispatch] {
        assert_eq!(
            decision(&facts),
            (SettlementReturnMatch::Unbound, SettlementReturnReason::DispatchUnknown)
        );
    }
}

/// 无派发引用的退件只能绑定唯一、仍为当前派发且未被其他退件绑定的首寄。
#[test]
fn first_send_returns_require_a_unique_current_dispatch() {
    let mut ambiguous = payout_facts();
    if let Some(obligation) = ambiguous.obligation.as_mut() {
        obligation.delivery = None;
        obligation.dispatch_lookup = SettlementReturnDispatchLookup::Ambiguous;
    }
    assert_eq!(
        decision(&ambiguous),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::DispatchAmbiguous)
    );
    let mut already_returned = payout_facts();
    if let Some(obligation) = already_returned.obligation.as_mut() {
        obligation.dispatch_lookup = SettlementReturnDispatchLookup::AlreadyReturned;
    }
    assert_eq!(
        decision(&already_returned),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::DispatchAlreadyReturned)
    );
    let mut superseded = payout_facts();
    if let Some(delivery) =
        superseded.obligation.as_mut().and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.current = false;
    }
    assert_eq!(
        decision(&superseded),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::DispatchSuperseded)
    );
    let mut resend_without_reference = payout_facts();
    if let Some(delivery) = resend_without_reference
        .obligation
        .as_mut()
        .and_then(|obligation| obligation.delivery.as_mut())
    {
        delivery.dispatch_no = 2;
    }
    assert_eq!(
        decision(&resend_without_reference),
        (SettlementReturnMatch::Unbound, SettlementReturnReason::DispatchUnknown)
    );
}
