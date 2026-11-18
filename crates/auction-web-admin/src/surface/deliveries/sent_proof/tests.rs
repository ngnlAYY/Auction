use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::future::poll_fn;
use std::rc::Rc;
use std::task::{Poll, Waker};

use auction_contracts::{
    AdminDeliveryAttemptResponse, AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
    AdminDeliveryEvidenceResponse, AdminDeliveryResponse, AdminDeliverySentProofPreviewResponse,
};
use auction_time::Timestamp;
use auction_web_shared::i18n::{DEFAULT_ADMIN_LOCALE, build_bundle};
use dioxus::dioxus_core::{AttributeValue, Mutation, Runtime};
use dioxus::prelude::*;
use dioxus_i18n::prelude::try_use_init_i18n_from_bundle;
use uuid::Uuid;

use super::evidence::{decode_hex_digest, selectable_evidence};
use super::preview::run_preview;
use super::render::SentProofDialog;
use super::state::{
    SentProofDraft, SentProofIssue, SentProofPreviewError, SentProofRequest, SentProofReview,
};
use super::validation::{build_evidence, sent_minute_from_input, sent_minute_from_timestamp};
use crate::api::{AdminApiFailure, LocalTimeError};
use crate::surface::ADMIN_LOCALE_CATALOG;
use crate::surface::test_events::{dispatch_click_event, dispatch_form_event};

fn time(minute: u32) -> String {
    Timestamp::from_ymd_hms(2026, 11, 2, 1, minute, 0).expect("timestamp").to_rfc3339()
}

fn evidence() -> AdminDeliveryEvidenceResponse {
    AdminDeliveryEvidenceResponse {
        reference: "mailbox_sweep:7000001".to_owned(),
        kind: "mailbox_sweep".to_owned(),
        external_mail_id: Some("7000001".to_owned()),
        observed_at: Some(time(2)),
        sent_at: Some(time(1)),
        recipient: Some("Sample".to_owned()),
        subject: Some("Auction settlement".to_owned()),
        body_digest: Some("cd".repeat(32)),
        collected: Some(false),
        cod_pending: Some(0),
        remaining_credits: Some(300_000),
        remaining_equipment_count: Some(0),
        remaining_equipment_ids: Some(Some(Vec::new())),
        pre_collection_equipment_ids: Some(None),
        receipt: None,
    }
}

fn detail() -> AdminDeliveryDetailResponse {
    AdminDeliveryDetailResponse {
        delivery: AdminDeliveryResponse {
            id: Uuid::from_bytes([1; 16]).to_string(),
            origin_kind: "settlement".to_owned(),
            origin_id: "42".to_owned(),
            recipient: "Sample".to_owned(),
            cod: Some(0),
            status: "manual_reconcile".to_owned(),
            template: Some("settlement_seller_payout".to_owned()),
            category: Some("settlement".to_owned()),
            world: Some("main".to_owned()),
            retirement_reason: None,
            suppression_reason: None,
            lease_expires_at: None,
            fencing_version: 3,
            attempt_count: 1,
            claimed_at: Some(time(0)),
            sending_at: Some(time(0)),
            completed_at: Some(time(2)),
            external_reference: None,
            failure_code: Some("write_unknown".to_owned()),
            reason_code: None,
            take_required: false,
            returned_mail_id: None,
            created_at: time(0),
            updated_at: time(2),
        },
        attempts: vec![AdminDeliveryAttemptResponse {
            operation: "send".to_owned(),
            fencing_version: 3,
            world_id: "main".to_owned(),
            sending_at: time(0),
            recipient: Some("Sample".to_owned()),
            subject: Some("Auction settlement".to_owned()),
            body_digest: Some("ab".repeat(32)),
            credits: Some(300_000),
            cod: Some(0),
            equipment_ids: Vec::new(),
            external_mail_id: None,
            credential: None,
            bound_sent_mail_id: None,
        }],
        evidence: vec![evidence()],
        custody: AdminDeliveryCustodyResponse {
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(300_000),
            receipt: None,
            observed_at: Some(time(2)),
            operator_confirmed_collected: false,
        },
        dispositions: Vec::new(),
        obligation: None,
        recovery: None,
    }
}

fn filled_draft(detail: &AdminDeliveryDetailResponse) -> SentProofDraft {
    let mut draft = SentProofDraft::for_evidence(selectable_evidence(detail).first().copied());
    draft.sender_name = "Sample".to_owned();
    draft.sender_uid = "12345".to_owned();
    draft.reason = "verified in Sent".to_owned();
    draft
}

#[test]
fn decodes_lowercase_hex_digest_only() {
    assert_eq!(decode_hex_digest(&"ab".repeat(32)), Some([0xab; 32]));
    assert_eq!(decode_hex_digest(&"AB".repeat(32)), None);
    assert_eq!(decode_hex_digest("ab"), None);
    assert_eq!(decode_hex_digest(&"zz".repeat(32)), None);
}

#[test]
fn normalizes_sent_minute_to_utc_zero_seconds() {
    assert_eq!(sent_minute_from_input("2026-11-02T01:14"), Ok("2026-11-02T01:14:00Z".to_owned()));
    assert_eq!(sent_minute_from_input(""), Err(LocalTimeError::Invalid));
    assert_eq!(sent_minute_from_timestamp("2026-11-02T01:14:37Z"), None);
    assert_eq!(
        sent_minute_from_timestamp("2026-11-02T01:14:00.000000Z").as_deref(),
        Some("2026-11-02T01:14:00.000000Z")
    );
}

#[test]
fn prefilled_sent_minute_is_a_local_datetime_value_without_offset() {
    let draft = SentProofDraft::for_evidence(Some(&evidence()));
    let minute = draft.sent_minute.as_str();
    // 形如 `YYYY-MM-DDTHH:MM`（到分钟、不带偏移）：`datetime-local` 会把带 `Z` 或 `+08:00` 之类
    // 时区偏移的值净化为空串。
    assert_eq!(minute.len(), 16);
    assert!(minute.bytes().enumerate().all(|(index, byte)| match index {
        4 | 7 => byte == b'-',
        10 => byte == b'T',
        13 => byte == b':',
        _ => byte.is_ascii_digit(),
    }));
    assert!(!draft.sent_minute_edited);
}

#[test]
fn untouched_prefill_submits_observed_utc_minute() {
    let mut detail = detail();
    detail.evidence[0].sent_at = Some("2027-11-07T09:30:00Z".to_owned());
    let mut draft = filled_draft(&detail);
    // 模拟本地展示值经旧换算落到较早的 UTC 分钟。
    draft.sent_minute = "2027-11-07T08:30".to_owned();
    let built = build_evidence(&detail, &draft).expect("未编辑的预填证据");
    assert_eq!(built.sent_minute, "2027-11-07T09:30:00Z");

    draft.sent_minute_edited = true;
    assert_eq!(
        build_evidence(&detail, &draft).expect("已编辑的时间").sent_minute,
        "2027-11-07T08:30:00Z"
    );
    draft.sent_minute.clear();
    assert_eq!(
        build_evidence(&detail, &draft),
        Err(SentProofIssue::SentMinute(LocalTimeError::Invalid))
    );
}

#[test]
fn builds_evidence_from_frozen_attempt_and_observation() {
    let detail = detail();
    let built = build_evidence(&detail, &filled_draft(&detail)).expect("evidence");
    assert_eq!(built.schema_version, 1);
    assert_eq!(built.world_id, "main");
    assert_eq!(built.external_mail_id, "7000001");
    assert_eq!(built.sender_uid, "12345");
    assert_eq!(built.recipient, "Sample");
    assert_eq!(built.sent_minute, time(1));
    assert_eq!(built.original_body_digest, [0xab; 32]);
    assert_eq!(built.observed_body_digest, [0xcd; 32]);
    assert_eq!(built.remaining_credits, 300_000);
    assert!(built.equipment_ids.is_empty());
}

#[test]
fn rejects_draft_without_recorded_evidence_or_sender() {
    let detail = detail();
    let mut draft = filled_draft(&detail);
    draft.evidence_reference = "mailbox_sweep:999".to_owned();
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::Evidence));

    let mut draft = filled_draft(&detail);
    draft.sender_uid = "0".to_owned();
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::SenderUid));

    let mut draft = filled_draft(&detail);
    draft.sender_name = "  ".to_owned();
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::SenderName));
}

#[test]
fn rejects_unselectable_kind_and_missing_digest() {
    let mut detail = self::detail();
    detail.evidence[0].kind = "compose_discard".to_owned();
    assert!(selectable_evidence(&detail).is_empty());
    assert_eq!(
        build_evidence(&detail, &filled_draft(&self::detail())),
        Err(SentProofIssue::Evidence)
    );

    let mut detail = self::detail();
    detail.evidence[0].body_digest = None;
    assert_eq!(
        build_evidence(&detail, &filled_draft(&self::detail())),
        Err(SentProofIssue::Digest)
    );
}

#[test]
fn draft_uses_observed_equipment_ids_only_while_uncollected() {
    let mut detail = detail();
    detail.attempts[0].credits = Some(0);
    detail.attempts[0].cod = Some(0);
    detail.attempts[0].equipment_ids = vec!["300753957".to_owned()];
    detail.evidence[0].remaining_equipment_count = Some(1);
    detail.evidence[0].remaining_equipment_ids = Some(Some(vec!["300753958".to_owned()]));
    let uncollected = filled_draft(&detail);
    let built = build_evidence(&detail, &uncollected).expect("evidence");
    assert_eq!(built.equipment_ids, vec!["300753958".to_owned()]);

    let mut draft = filled_draft(&detail);
    draft.collected = true;

    let built = build_evidence(&detail, &draft).expect("evidence");
    assert!(built.collected);
    assert!(built.equipment_ids.is_empty());
}

#[test]
fn uncollected_observation_with_no_ids_cannot_preview() {
    let mut detail = detail();
    detail.attempts[0].equipment_ids = vec!["300753957".to_owned()];
    detail.evidence[0].remaining_equipment_ids = Some(None);
    let mut draft = filled_draft(&detail);
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::MissingObservedEquipmentIds));
    draft.collected = true;
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::MissingObservedEquipmentIds));

    detail.evidence[0].collected = Some(true);
    assert!(build_evidence(&detail, &draft).expect("历史装备回执路径").equipment_ids.is_empty());
}

#[test]
fn only_collected_proof_validates_and_submits_the_receipt() {
    let detail = detail();
    let mut draft = filled_draft(&detail);
    draft.collected = true;
    draft.receipt = "x".repeat(2049);
    assert_eq!(build_evidence(&detail, &draft), Err(SentProofIssue::Receipt));
    draft.collected = false;
    assert!(build_evidence(&detail, &draft).expect("隐藏回执").receipt.is_empty());

    draft.receipt = " Removed attachment: 300,000 Credits ".to_owned();
    assert!(build_evidence(&detail, &draft).expect("未领取的回执为空").receipt.is_empty());
    draft.collected = true;
    assert_eq!(
        build_evidence(&detail, &draft).expect("已领取的回执").receipt,
        "Removed attachment: 300,000 Credits"
    );
}

#[test]
fn invalid_preview_fields_use_their_own_error_locations() {
    let detail = detail();
    let mut draft = filled_draft(&detail);
    draft.sender_uid = "0".to_owned();
    let issue = build_evidence(&detail, &draft).expect_err("无效 UID");
    assert_eq!(issue.field_id(), "delivery-sent-sender-uid");
    draft.sender_uid = "12345".to_owned();
    draft.cod_pending = "-1".to_owned();
    let issue = build_evidence(&detail, &draft).expect_err("无效 COD");
    assert_eq!(issue.field_id(), "delivery-sent-cod-pending");
    draft.cod_pending = "0".to_owned();
    assert!(build_evidence(&detail, &draft).is_ok());
}

type PreviewControls = Cell<Option<(ScopeId, Signal<SentProofDraft>, Signal<SentProofReview>)>>;
type LocalTimeParser = fn(&str) -> Result<Timestamp, LocalTimeError>;
type PreviewRequests = RefCell<Vec<SentProofRequest>>;
type StalePreviews = BTreeMap<String, Rc<StalePreview>>;

#[derive(Default)]
struct StalePreview {
    ready: Cell<bool>,
    waiting: RefCell<Option<Waker>>,
}

impl StalePreview {
    fn deliver(&self) {
        self.ready.set(true);
        self.waiting.borrow_mut().take().expect("预览请求正在等待响应").wake();
    }
}

pub(super) fn local_time_parser() -> Option<LocalTimeParser> {
    Runtime::try_current()?.try_current_scope_id()?;
    try_consume_context()
}

pub(super) async fn preview_response(
    request: &SentProofRequest,
) -> Option<Result<AdminDeliverySentProofPreviewResponse, AdminApiFailure>> {
    try_consume_context::<Rc<PreviewRequests>>()?.borrow_mut().push(request.clone());
    if let Some(stale) = try_consume_context::<StalePreviews>()
        .and_then(|replies| replies.get(&request.evidence_reference).cloned())
    {
        poll_fn(|context| {
            if stale.ready.get() {
                Poll::Ready(())
            } else {
                stale.waiting.replace(Some(context.waker().clone()));
                Poll::Pending
            }
        })
        .await;
        return Some(Err(AdminApiFailure::client("stale_delivery_state")));
    }
    Some(Ok(AdminDeliverySentProofPreviewResponse {
        eligible: true,
        blockers: Vec::new(),
        delivery_id: request.delivery_id.clone(),
        expected_fencing_version: request.fencing_version,
        external_reference: format!("mooglemail:main:send:{}", request.evidence.external_mail_id),
        requires_collected_asset_acceptance: false,
        requires_attempt_history_acceptance: false,
        prior_unbound_attempts: Some(false),
        sent_before_current_attempt: Some(false),
        corrects_admin_failure: false,
    }))
}

pub(super) fn capture_dialog_controls(
    draft: Signal<SentProofDraft>,
    review: Signal<SentProofReview>,
) {
    if let Some(controls) = try_consume_context::<Rc<PreviewControls>>() {
        controls.set(Some((dioxus::dioxus_core::current_scope_id(), draft, review)));
    }
}

fn preview_harness() -> Element {
    let mut input = filled_draft(&detail());
    input.sender_uid.clear();
    let draft = use_signal(|| input);
    let review = use_signal(SentProofReview::default);
    capture_dialog_controls(draft, review);
    rsx! {}
}

#[test]
fn invalid_field_leaves_preview_idle_with_a_local_issue() {
    let controls = Rc::new(PreviewControls::default());
    let mut dom = VirtualDom::new(preview_harness).with_root_context(controls.clone());
    dom.rebuild_to_vec();
    let (scope, draft, review) = controls.get().expect("预览控件");
    dom.in_scope(scope, || {
        run_preview(&detail(), false, draft, review, EventHandler::new(|_| ()));
        assert!(!review.peek().pending);
        assert!(matches!(
            review.peek().outcome,
            Some(Err(SentProofPreviewError::Local(SentProofIssue::SenderUid)))
        ));
    });
}

fn request_and_preview(
    mid: &str,
    prior_unbound_attempts: bool,
    sent_before_current_attempt: bool,
) -> (SentProofRequest, AdminDeliverySentProofPreviewResponse) {
    let mut detail = detail();
    let reference = format!("mailbox_sweep:{mid}");
    detail.evidence[0].external_mail_id = Some(mid.to_owned());
    detail.evidence[0].reference = reference.clone();
    let request = SentProofRequest {
        delivery_id: detail.delivery.id.clone(),
        fencing_version: detail.delivery.fencing_version,
        evidence_reference: reference,
        evidence: build_evidence(&detail, &filled_draft(&detail)).expect("evidence"),
    };
    let preview = AdminDeliverySentProofPreviewResponse {
        eligible: true,
        blockers: Vec::new(),
        delivery_id: request.delivery_id.clone(),
        expected_fencing_version: request.fencing_version,
        external_reference: format!("mooglemail:main:send:{mid}"),
        requires_collected_asset_acceptance: false,
        requires_attempt_history_acceptance: prior_unbound_attempts || sent_before_current_attempt,
        prior_unbound_attempts: Some(prior_unbound_attempts),
        sent_before_current_attempt: Some(sent_before_current_attempt),
        corrects_admin_failure: sent_before_current_attempt,
    };
    (request, preview)
}

fn dialog_harness() -> Element {
    try_use_init_i18n_from_bundle(|| {
        build_bundle(
            &DEFAULT_ADMIN_LOCALE,
            ADMIN_LOCALE_CATALOG.default_ftl,
            &DEFAULT_ADMIN_LOCALE,
            None,
        )
    })
    .expect("加载管理端中文译文");
    let mut detail = try_consume_context::<AdminDeliveryDetailResponse>().unwrap_or_else(detail);
    let mut other = evidence();
    other.reference = "mailbox_sweep:7000002".to_owned();
    other.external_mail_id = Some("7000002".to_owned());
    detail.evidence.push(other);
    let refreshes = try_consume_context::<Rc<Cell<u32>>>();
    rsx! {
        SentProofDialog {
            detail,
            busy: false,
            on_submit: |_| (),
            on_cancel: |_| (),
            on_stale: move |_| {
                if let Some(refreshes) = &refreshes {
                    refreshes.set(refreshes.get() + 1);
                }
            },
        }
    }
}

fn los_angeles_time(value: &str) -> Result<Timestamp, LocalTimeError> {
    // 宿主只模拟 A9 解析器的出参，时区反查本身由共享解析器的测试覆盖。
    match value {
        "2026-03-08T02:30" => Err(LocalTimeError::Nonexistent),
        "2026-11-01T01:30" => Err(LocalTimeError::Ambiguous),
        _ => crate::api::parse_local_datetime(value),
    }
}

#[test]
fn repeated_wall_minute_keeps_each_prefilled_utc_in_the_preview_request() {
    for utc in ["2026-11-01T08:30:00Z", "2026-11-01T09:30:00Z"] {
        let mut detail = detail();
        detail.evidence[0].sent_at = Some(utc.to_owned());
        let controls = Rc::new(PreviewControls::default());
        let requests = Rc::new(PreviewRequests::default());
        let mut dom = VirtualDom::new(dialog_harness)
            .with_root_context(detail.clone())
            .with_root_context(controls.clone())
            .with_root_context(requests.clone())
            .with_root_context(los_angeles_time as LocalTimeParser);
        dom.rebuild_to_vec();
        dom.process_events();
        let (scope, mut draft, review) = controls.get().expect("对话框信号");
        dom.in_scope(scope, || {
            let mut filled = filled_draft(&detail);
            filled.sent_minute = "2026-11-01T01:30".to_owned();
            draft.set(filled);
            run_preview(&detail, false, draft, review, EventHandler::new(|_| ()));
        });
        dom.render_immediate_to_vec();
        let requests = requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].evidence.sent_minute, utc);
    }
}

#[test]
fn edited_dst_gap_and_fold_render_time_field_errors_without_a_preview_request() {
    for (wall, expected, text) in [
        ("2026-03-08T02:30", LocalTimeError::Nonexistent, "该时刻在本地时区不存在，请另选时间。"),
        (
            "2026-11-01T01:30",
            LocalTimeError::Ambiguous,
            "该时刻在本地时区出现两次，请改选重复时段外的相邻时间。",
        ),
    ] {
        let detail = detail();
        let controls = Rc::new(PreviewControls::default());
        let requests = Rc::new(PreviewRequests::default());
        let mut dom = VirtualDom::new(dialog_harness)
            .with_root_context(controls.clone())
            .with_root_context(requests.clone())
            .with_root_context(los_angeles_time as LocalTimeParser);
        let minute_input = dom
            .rebuild_to_vec()
            .edits
            .into_iter()
            .filter_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == "input" => Some(id),
                _ => None,
            })
            .nth(2)
            .expect("发件人、UID 之后是发送时间控件");
        dom.process_events();
        let (scope, mut draft, review) = controls.get().expect("对话框信号");
        dom.in_runtime(|| draft.set(filled_draft(&detail)));
        dispatch_form_event(&dom, minute_input, "input", wall);
        dom.in_scope(scope, || {
            assert!(draft.peek().sent_minute_edited);
            run_preview(&detail, false, draft, review, EventHandler::new(|_| ()));
            assert!(matches!(
                review.peek().outcome,
                Some(Err(SentProofPreviewError::Local(SentProofIssue::SentMinute(error))))
                    if error == expected
            ));
        });
        let edits = dom.render_immediate_to_vec().edits;
        assert!(edits.iter().any(|edit| matches!(
            edit,
            Mutation::SetAttribute { id, name: "aria-describedby", value: AttributeValue::Text(value), .. }
                if *id == minute_input && value == "delivery-sent-minute-error"
        )));
        assert!(edits.iter().any(|edit| matches!(
            edit, Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
                if value == text
        )));
        assert!(requests.borrow().is_empty());
    }
}

#[test]
fn switching_a_to_b_discards_late_a_without_clearing_b_loading_or_preview() {
    let (a, preview_a) = request_and_preview("7000001", true, false);
    let (b, preview_b) = request_and_preview("7000002", false, true);
    let controls = Rc::new(PreviewControls::default());
    let mut dom = VirtualDom::new(dialog_harness).with_root_context(controls.clone());
    let select = dom
        .rebuild_to_vec()
        .edits
        .into_iter()
        .find_map(|edit| match edit {
            Mutation::NewEventListener { name, id } if name == "change" => Some(id),
            _ => None,
        })
        .expect("证据选择控件已挂载");
    dom.process_events();
    let (_, draft, mut review) = controls.get().expect("对话框信号");
    let a_revision = dom.in_runtime(|| review.write().begin());

    dispatch_form_event(&dom, select, "change", &b.evidence_reference);
    dom.in_runtime(|| {
        assert_eq!(draft.peek().evidence_reference, b.evidence_reference);
        assert!(!review.peek().pending, "选中 B 必须使 A 的预览失效");
    });
    let b_revision = dom.in_runtime(|| review.write().begin());

    dom.in_runtime(|| {
        let mut review = review.write();
        review.complete(a_revision, a.clone(), Ok(preview_a.clone()));
        assert!(review.pending);
        assert!(review.outcome.is_none());
        review.complete(b_revision, b.clone(), Ok(preview_b));
        assert!(!review.pending);
        assert!(review.acceptance(a_revision).is_none());
        assert!(review.acceptance(b_revision).is_some());

        review.complete(a_revision, a.clone(), Err(AdminApiFailure::client("old_error")));
        let proof = review.outcome.as_ref().expect("B 结果").as_ref().expect("B 预览");
        assert_eq!(proof.submission("B").evidence, b.evidence);
    });

    dispatch_form_event(&dom, select, "change", &a.evidence_reference);
    dom.in_runtime(|| {
        assert_eq!(draft.peek().evidence_reference, a.evidence_reference);
        let mut review = review.write();
        assert!(review.outcome.is_none(), "选回 A 必须清除 B 的预览");
        assert!(review.acceptance(b_revision).is_none());
        let second_a_revision = review.begin();
        review.complete(a_revision, a.clone(), Ok(preview_a.clone()));
        assert!(review.pending);
        review.complete(second_a_revision, a, Ok(preview_a));
        assert!(!review.pending);
        assert!(review.outcome.as_ref().is_some_and(Result::is_ok));
    });
}

#[test]
fn only_the_current_stale_preview_notifies_detail_to_refresh() {
    let a = "mailbox_sweep:7000001";
    let b = "mailbox_sweep:7000002";
    let stale_a = Rc::new(StalePreview::default());
    let stale_b = Rc::new(StalePreview::default());
    let replies =
        StalePreviews::from([(a.to_owned(), stale_a.clone()), (b.to_owned(), stale_b.clone())]);
    let controls = Rc::new(PreviewControls::default());
    let requests = Rc::new(PreviewRequests::default());
    let refreshes = Rc::new(Cell::new(0_u32));
    let mut dom = VirtualDom::new(dialog_harness)
        .with_root_context(controls.clone())
        .with_root_context(requests.clone())
        .with_root_context(refreshes.clone())
        .with_root_context(replies);
    let edits = dom.rebuild_to_vec().edits;
    let select = edits
        .iter()
        .find_map(|edit| match edit {
            Mutation::NewEventListener { name, id } if name == "change" => Some(*id),
            _ => None,
        })
        .expect("选择证据");
    let preview = edits
        .iter()
        .rev()
        .filter_map(|edit| match edit {
            Mutation::NewEventListener { name, id } if name == "click" => Some(*id),
            _ => None,
        })
        .nth(1)
        .expect("取消之前的预览按钮");
    dom.process_events();
    let (_, mut draft, review) = controls.get().expect("对话框信号");
    dom.in_runtime(|| draft.set(filled_draft(&detail())));
    dispatch_click_event(&dom, preview);
    dom.render_immediate_to_vec();
    dispatch_form_event(&dom, select, "change", b);
    dom.render_immediate_to_vec();
    dispatch_click_event(&dom, preview);
    dom.render_immediate_to_vec();
    assert_eq!(
        requests
            .borrow()
            .iter()
            .map(|request| request.evidence_reference.as_str())
            .collect::<Vec<_>>(),
        [a, b]
    );

    stale_a.deliver();
    dom.render_immediate_to_vec();
    assert_eq!(refreshes.get(), 0);
    dom.in_runtime(|| assert!(review.peek().pending));

    stale_b.deliver();
    let edits = dom.render_immediate_to_vec().edits;
    assert_eq!(refreshes.get(), 1);
    assert!(edits.iter().any(|edit| matches!(edit,
        Mutation::CreateTextNode { value, .. } | Mutation::SetText { value, .. }
            if value.contains("请重新选择证据")
    )));
}

#[test]
fn confirmation_uses_reviewed_fields_and_requires_each_triggered_responsibility() {
    let (request, mut preview) = request_and_preview("7000001", true, true);
    preview.requires_collected_asset_acceptance = true;
    let mut review = SentProofReview::default();
    let revision = review.begin();
    review.complete(revision, request.clone(), Ok(preview.clone()));
    {
        let accepted = review.acceptance(revision).expect("预览勾选");
        accepted.collected_assets = true;
        accepted.prior_unbound_attempts = true;
    }
    let ready = |review: &SentProofReview| {
        review.outcome.as_ref().is_some_and(|result| {
            result
                .as_ref()
                .is_ok_and(|proof| proof.can_confirm(&request.delivery_id, request.fencing_version))
        })
    };
    assert!(!ready(&review));
    review.acceptance(revision).unwrap().sent_before_current_attempt = true;
    assert!(ready(&review));
    let proof = review.outcome.as_ref().unwrap().as_ref().unwrap();
    let submission = proof.submission("verified");
    assert_eq!(submission.evidence_reference, request.evidence_reference);
    assert_eq!(submission.evidence, request.evidence);
    assert_eq!(submission.expected_fencing_version, request.fencing_version);
    assert!(submission.accept_collected_assets && submission.accept_attempt_history);
    assert!(!proof.can_confirm(&request.delivery_id, request.fencing_version + 1));

    review.clear();
    let revision = review.begin();
    review.complete(revision, request.clone(), Ok(preview));
    assert!(!ready(&review), "新预览没有继承核对勾选");
}

#[test]
fn preview_with_changed_fence_cannot_be_used_for_confirmation() {
    let (request, mut preview) = request_and_preview("7000001", false, false);
    preview.expected_fencing_version += 1;
    let mut review = SentProofReview::default();
    let revision = review.begin();
    review.complete(revision, request, Ok(preview));
    assert!(matches!(
        review.outcome,
        Some(Err(SentProofPreviewError::Api(error))) if error.code == "stale_delivery_state"
    ));
}

#[test]
fn draft_preserves_operator_fields_across_evidence_switch() {
    let detail = detail();
    let draft = filled_draft(&detail);
    let next = draft.with_evidence(selectable_evidence(&detail).first().copied());
    assert_eq!(next.sender_uid, "12345");
    assert_eq!(next.reason, "verified in Sent");
    assert_eq!(next.remaining_credits, "300000");
}
