//! Admin 人工 Sent 证明对话框。
//!
//! 只允许选择该投递维护流程已写入的 Sent 证据（`mailbox_sweep` 或
//! `sent_reference_sighting`），并把冻结尝试与观察组合成 `auction-contracts` 的闭集请求。
//! 预览无副作用；提交沿用页面命令宿主的稳定 `command_id`，服务端仍会重新判定准入，
//! 不把预览当作成功证明，也不重发邮件或改写原失败命令。

use auction_web_shared::{Dialog, legacy_t as t};
use chrono::Timelike;
use dioxus::prelude::*;

use crate::api::{
    DeliveryAttempt, DeliveryDetail, DeliveryEvidence, DeliverySentEvidence,
    DeliverySentProofPreview, MAX_JSON_SAFE_INTEGER, preview_delivery_sent_proof,
};
use crate::app::surface::admin_world_label;
use crate::app::surface::pending::set_if_alive;

/// 人工证明可以引用的证据种类；草稿丢弃与既有确认不能被再次引用。
const SELECTABLE_EVIDENCE_KINDS: [&str; 2] = ["mailbox_sweep", "sent_reference_sighting"];

/// 发送分钟统一使用带 `Z` 的 UTC 整分钟文本。
const SENT_MINUTE_FORMAT: &str = "%Y-%m-%dT%H:%M:00Z";

/// 详情页转交的一次性提交；`command_id` 由命令宿主生成并在重试间保持。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SentProofSubmission {
    pub(super) evidence_reference: String,
    pub(super) evidence: DeliverySentEvidence,
    pub(super) reason: String,
    pub(super) expected_fencing_version: i64,
    pub(super) accept_collected_assets: bool,
    pub(super) accept_attempt_history: bool,
}

/// 操作员可编辑的观察草稿；预填只复制证据中已有的字段，不伪造未观测值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SentProofDraft {
    pub(super) evidence_reference: String,
    pub(super) sender_name: String,
    pub(super) sender_uid: String,
    pub(super) sent_minute: String,
    pub(super) remaining_credits: String,
    pub(super) cod_pending: String,
    pub(super) collected: bool,
    pub(super) receipt: String,
    pub(super) reason: String,
}

impl SentProofDraft {
    pub(super) fn for_evidence(evidence: Option<&DeliveryEvidence>) -> Self {
        Self {
            evidence_reference: evidence.map(|item| item.reference.clone()).unwrap_or_default(),
            sender_name: String::new(),
            sender_uid: String::new(),
            sent_minute: evidence
                .and_then(|item| item.sent_at.as_deref())
                .and_then(sent_minute_from_timestamp)
                .unwrap_or_default(),
            remaining_credits: evidence
                .and_then(|item| item.remaining_credits)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            cod_pending: evidence
                .and_then(|item| item.cod_pending)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            collected: evidence.and_then(|item| item.collected).unwrap_or(false),
            receipt: evidence.and_then(|item| item.receipt.clone()).unwrap_or_default(),
            reason: String::new(),
        }
    }

    /// 切换证据时保留操作员已填写的发送者与原因，只替换由证据派生的字段。
    pub(super) fn with_evidence(self, evidence: Option<&DeliveryEvidence>) -> Self {
        let mut next = Self::for_evidence(evidence);
        next.sender_name = self.sender_name;
        next.sender_uid = self.sender_uid;
        next.reason = self.reason;
        next
    }
}

/// 草稿形状问题；只在浏览器本地阻止提交，语义准入仍由服务端预览与事务裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SentProofIssue {
    Evidence,
    Attempt,
    Digest,
    Recipient,
    Subject,
    SentMinute,
    SenderName,
    SenderUid,
    Amount,
    Receipt,
}

impl SentProofIssue {
    fn message(self) -> String {
        match self {
            Self::Evidence => t!("delivery-sent-proof-issue-evidence"),
            Self::Attempt => t!("delivery-sent-proof-issue-attempt"),
            Self::Digest => t!("delivery-sent-proof-issue-digest"),
            Self::Recipient => t!("delivery-sent-proof-issue-recipient"),
            Self::Subject => t!("delivery-sent-proof-issue-subject"),
            Self::SentMinute => t!("delivery-sent-proof-issue-sent-minute"),
            Self::SenderName => t!("delivery-sent-proof-issue-sender-name"),
            Self::SenderUid => t!("delivery-sent-proof-issue-sender-uid"),
            Self::Amount => t!("delivery-sent-proof-issue-amount"),
            Self::Receipt => t!("delivery-sent-proof-issue-receipt"),
        }
    }
}

/// 只有维护流程写入的两类观察可被引用；既有确认与草稿丢弃不构成新证据。
pub(super) fn selectable_evidence(detail: &DeliveryDetail) -> Vec<&DeliveryEvidence> {
    detail
        .evidence
        .iter()
        .filter(|item| SELECTABLE_EVIDENCE_KINDS.contains(&item.kind.as_str()))
        .collect()
}

/// 冻结的发送尝试按 fence 取最新一条；缺失或只有 Take 尝试时不得引用。
pub(super) fn latest_send_attempt(detail: &DeliveryDetail) -> Option<&DeliveryAttempt> {
    detail
        .attempts
        .iter()
        .filter(|attempt| attempt.operation == "send")
        .max_by_key(|attempt| attempt.fencing_version)
}

/// 解析服务端小写 hex 摘要；长度或字符不合规时返回 `None`，不做大小写兼容。
pub(super) fn decode_hex_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let (chunks, _) = value.as_bytes().as_chunks::<2>();
    let mut digest = [0_u8; 32];
    for (index, chunk) in chunks.iter().enumerate() {
        digest[index] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
    }
    Some(digest)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

/// 把 `datetime-local` 输入换算为带 `Z` 的整分钟；秒或小数非零时不猜测。
pub(super) fn sent_minute_from_input(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let normalized = crate::api::normalize_datetime(value);
    let parsed =
        chrono::DateTime::parse_from_rfc3339(&normalized).ok()?.with_timezone(&chrono::Utc);
    if parsed.second() != 0 || parsed.nanosecond() != 0 {
        return None;
    }
    Some(parsed.format(SENT_MINUTE_FORMAT).to_string())
}

/// 证据里的发送时间戳按整分钟截断；服务端只接受秒与小数均为零的分钟。
pub(super) fn sent_minute_from_timestamp(value: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&chrono::Utc)
        .with_second(0)?
        .with_nanosecond(0)?;
    Some(parsed.format(SENT_MINUTE_FORMAT).to_string())
}

fn valid_sender_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(char::is_control) {
        return None;
    }
    Some(trimmed.to_owned())
}

fn valid_sender_uid(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !(1..=64).contains(&trimmed.len())
        || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
        || trimmed.starts_with('0')
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn valid_reason(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.len() <= 2048 && !trimmed.chars().any(char::is_control)
}

fn parse_amount(value: &str) -> Option<i64> {
    let parsed = value.trim().parse::<i64>().ok()?;
    (0..=MAX_JSON_SAFE_INTEGER).contains(&parsed).then_some(parsed)
}

/// 组合冻结尝试与已采集证据；任一必需字段缺失都返回本地问题码，不提交半成品。
fn build_evidence(
    detail: &DeliveryDetail,
    draft: &SentProofDraft,
) -> Result<DeliverySentEvidence, SentProofIssue> {
    let reference = draft.evidence_reference.trim();
    let evidence = detail
        .evidence
        .iter()
        .find(|item| {
            item.reference == reference && SELECTABLE_EVIDENCE_KINDS.contains(&item.kind.as_str())
        })
        .ok_or(SentProofIssue::Evidence)?;
    let attempt = latest_send_attempt(detail).ok_or(SentProofIssue::Attempt)?;
    let external_mail_id = evidence.external_mail_id.clone().ok_or(SentProofIssue::Evidence)?;
    let observed_at = evidence.observed_at.clone().ok_or(SentProofIssue::Evidence)?;
    let observed_body_digest = evidence
        .body_digest
        .as_deref()
        .and_then(decode_hex_digest)
        .ok_or(SentProofIssue::Digest)?;
    let original_body_digest =
        attempt.body_digest.as_deref().and_then(decode_hex_digest).ok_or(SentProofIssue::Digest)?;
    let recipient = evidence
        .recipient
        .clone()
        .or_else(|| attempt.recipient.clone())
        .ok_or(SentProofIssue::Recipient)?;
    let subject = evidence
        .subject
        .clone()
        .or_else(|| attempt.subject.clone())
        .ok_or(SentProofIssue::Subject)?;
    let sent_minute = match draft.sent_minute.trim() {
        "" => evidence.sent_at.as_deref().and_then(sent_minute_from_timestamp),
        value => sent_minute_from_input(value),
    }
    .ok_or(SentProofIssue::SentMinute)?;
    let sender_name = valid_sender_name(&draft.sender_name).ok_or(SentProofIssue::SenderName)?;
    let sender_uid = valid_sender_uid(&draft.sender_uid).ok_or(SentProofIssue::SenderUid)?;
    let remaining_credits = parse_amount(&draft.remaining_credits).ok_or(SentProofIssue::Amount)?;
    let cod_pending = parse_amount(&draft.cod_pending).ok_or(SentProofIssue::Amount)?;
    if draft.receipt.len() > 2048 {
        return Err(SentProofIssue::Receipt);
    }
    // 已领取时附件不在 Sent 中，冻结 EID 不能作为剩余观察；未领取才沿用冻结清单。
    let equipment_ids = if draft.collected { Vec::new() } else { attempt.equipment_ids.clone() };
    Ok(DeliverySentEvidence {
        schema_version: 1,
        world_id: attempt.world_id.clone(),
        external_mail_id,
        sender_uid,
        sender_name,
        recipient,
        subject,
        sent_minute,
        observed_at,
        original_body_digest,
        observed_body_digest,
        remaining_credits,
        cod_pending,
        equipment_ids,
        collected: draft.collected,
        receipt: draft.receipt.trim().to_owned(),
    })
}

/// 服务端闭集 blocker 的译文；未知码原样显示，不猜测语义。
pub(super) fn blocker_label(code: &str) -> String {
    match code {
        "unsupported_delivery_scope" => t!("delivery-sent-proof-blocker-scope"),
        "requires_unclaimed_write_unknown_or_admin_failure" => {
            t!("delivery-sent-proof-blocker-requires-unknown")
        }
        "fulfillment_identity_or_claim_conflict" => t!("delivery-sent-proof-blocker-fulfillment"),
        "conflicting_recovery_disposition" => t!("delivery-sent-proof-blocker-recovery"),
        "existing_binding_retirement_or_conflicting_attempt" => {
            t!("delivery-sent-proof-blocker-binding")
        }
        "requires_unique_ready_sender_funding_account" => {
            t!("delivery-sent-proof-blocker-sender-account")
        }
        "sender_player_name_mismatch" => t!("delivery-sent-proof-blocker-sender-name"),
        "frozen_credential_retired_or_changed" => t!("delivery-sent-proof-blocker-credential"),
        "sent_minute_outside_frozen_window" => t!("delivery-sent-proof-blocker-minute-window"),
        "requires_exact_unknown_seller_reservation" => {
            t!("delivery-sent-proof-blocker-seller-reservation")
        }
        "frozen_identity_body_money_or_assets_mismatch" => {
            t!("delivery-sent-proof-blocker-frozen-identity")
        }
        other => other.to_owned(),
    }
}

/// 证据种类译文；未知种类原样显示。
pub(super) fn evidence_kind_label(kind: &str) -> String {
    match kind {
        "mailbox_sweep" => t!("delivery-detail-evidence-kind-mailbox-sweep"),
        "sent_reference_sighting" => t!("delivery-detail-evidence-kind-sent-sighting"),
        "compose_discard" => t!("delivery-detail-evidence-kind-compose-discard"),
        "sent_operator_confirmed" => t!("delivery-detail-evidence-kind-operator-confirmed"),
        other => other.to_owned(),
    }
}

/// 展示可以从详情证据中选用的人工证明对话框。
#[component]
pub(super) fn SentProofDialog(
    detail: DeliveryDetail,
    busy: bool,
    on_submit: EventHandler<SentProofSubmission>,
    on_cancel: EventHandler<()>,
) -> Element {
    let candidates = selectable_evidence(&detail);
    let draft = use_signal(|| SentProofDraft::for_evidence(candidates.first().copied()));
    let mut preview = use_signal(|| None::<Result<DeliverySentProofPreview, String>>);
    let previewing = use_signal(|| false);
    let mut accept_collected = use_signal(|| false);
    let mut accept_attempts = use_signal(|| false);

    let current = draft();
    let selected =
        candidates.iter().copied().find(|item| item.reference == current.evidence_reference);
    let attempt = latest_send_attempt(&detail);
    let preview_state = preview();
    let requires_collected = matches!(
        preview_state.as_ref(),
        Some(Ok(preview)) if preview.requires_collected_asset_acceptance
    );
    let requires_attempts = matches!(
        preview_state.as_ref(),
        Some(Ok(preview)) if preview.requires_attempt_history_acceptance
    );
    let eligible = matches!(preview_state.as_ref(), Some(Ok(preview)) if preview.eligible);
    let can_confirm = !busy
        && eligible
        && (!requires_collected || accept_collected())
        && (!requires_attempts || accept_attempts())
        && valid_reason(&current.reason);

    let preview_detail = detail.clone();
    let preview_draft = draft;
    let mut preview_signal = preview;
    let mut previewing_signal = previewing;
    let mut run_preview = move || {
        if busy || previewing_signal() {
            return;
        }
        let current = preview_draft();
        match build_evidence(&preview_detail, &current) {
            Err(issue) => preview_signal.set(Some(Err(issue.message()))),
            Ok(evidence) => {
                let reference = current.evidence_reference.trim().to_owned();
                let delivery_id = preview_detail.delivery.id.clone();
                preview_signal.set(None);
                previewing_signal.set(true);
                spawn(async move {
                    let result = preview_delivery_sent_proof(delivery_id, reference, evidence)
                        .await
                        .map_err(|failure| failure.code);
                    set_if_alive(preview_signal, Some(result));
                    set_if_alive(previewing_signal, false);
                });
            }
        }
    };

    let submit_detail = detail.clone();
    let submit_draft = draft;
    let submit_accept_collected = accept_collected;
    let submit_accept_attempts = accept_attempts;

    let world = attempt.map(|attempt| admin_world_label(&attempt.world_id));
    let mid = selected.and_then(|item| item.external_mail_id.clone());
    let recipient = selected
        .and_then(|item| item.recipient.clone())
        .or_else(|| attempt.and_then(|attempt| attempt.recipient.clone()));
    let subject = selected
        .and_then(|item| item.subject.clone())
        .or_else(|| attempt.and_then(|attempt| attempt.subject.clone()));
    let original_digest = attempt.and_then(|attempt| attempt.body_digest.clone());
    let observed_digest = selected.and_then(|item| item.body_digest.clone());
    let select_detail = detail.clone();
    let mut select_draft = draft;
    let mut sender_name_draft = draft;
    let mut sender_uid_draft = draft;
    let mut minute_draft = draft;
    let mut credits_draft = draft;
    let mut cod_draft = draft;
    let mut collected_draft = draft;
    let mut receipt_draft = draft;
    let mut reason_draft = draft;

    rsx! {
        Dialog {
            title: t!("delivery-sent-proof-title"),
            close_label: t!("delivery-cancel"),
            busy,
            on_close: move |_| on_cancel.call(()),
            form { class: "admin-delivery-sent-proof",
                onsubmit: move |event| {
                    event.prevent_default();
                    if busy || !can_confirm {
                        return;
                    }
                    let current = submit_draft();
                    let Ok(evidence) = build_evidence(&submit_detail, &current) else {
                        return;
                    };
                    on_submit.call(SentProofSubmission {
                        evidence_reference: current.evidence_reference.trim().to_owned(),
                        evidence,
                        reason: current.reason.trim().to_owned(),
                        expected_fencing_version: submit_detail.delivery.fencing_version,
                        accept_collected_assets: submit_accept_collected(),
                        accept_attempt_history: submit_accept_attempts(),
                    });
                },
                p { class: "admin-panel__note", {t!("delivery-sent-proof-help")} }
                label { r#for: "delivery-sent-evidence", {t!("delivery-sent-proof-evidence")} }
                select {
                    id: "delivery-sent-evidence",
                    disabled: busy,
                    value: current.evidence_reference.clone(),
                    onchange: move |event| {
                        let next_reference = event.value();
                        let evidence = select_detail
                            .evidence
                            .iter()
                            .find(|item| item.reference == next_reference);
                        select_draft.set(select_draft().with_evidence(evidence));
                        preview.set(None);
                    },
                    option {
                        value: "",
                        disabled: true,
                        selected: current.evidence_reference.is_empty(),
                        {t!("delivery-sent-proof-evidence-placeholder")}
                    }
                    for item in candidates.iter() {
                        option {
                            key: "{item.reference}",
                            value: item.reference.clone(),
                            selected: current.evidence_reference == item.reference,
                            "{evidence_kind_label(&item.kind)} · {item.external_mail_id.clone().unwrap_or_default()}"
                        }
                    }
                }
                dl { class: "admin-delivery-sent-proof__grid",
                    dt { {t!("delivery-sent-proof-world")} }
                    dd { "{world.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-mid")} }
                    dd { "{mid.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-recipient")} }
                    dd { "{recipient.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-subject")} }
                    dd { "{subject.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    dt { {t!("delivery-sent-proof-original-digest")} }
                    dd {
                        code { "{original_digest.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    }
                    dt { {t!("delivery-sent-proof-observed-digest")} }
                    dd {
                        code { "{observed_digest.clone().unwrap_or_else(|| t!(\"value-empty\"))}" }
                    }
                }
                label { r#for: "delivery-sent-sender-name", {t!("delivery-sent-proof-sender-name")} }
                input {
                    id: "delivery-sent-sender-name",
                    r#type: "text",
                    required: true,
                    disabled: busy,
                    value: current.sender_name.clone(),
                    oninput: move |event| {
                        let mut next = sender_name_draft();
                        next.sender_name = event.value();
                        sender_name_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-sender-uid", {t!("delivery-sent-proof-sender-uid")} }
                input {
                    id: "delivery-sent-sender-uid",
                    r#type: "text",
                    inputmode: "numeric",
                    required: true,
                    disabled: busy,
                    value: current.sender_uid.clone(),
                    oninput: move |event| {
                        let mut next = sender_uid_draft();
                        next.sender_uid = event.value();
                        sender_uid_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-minute", {t!("delivery-sent-proof-sent-minute")} }
                input {
                    id: "delivery-sent-minute",
                    r#type: "datetime-local",
                    required: true,
                    disabled: busy,
                    value: current.sent_minute.clone(),
                    oninput: move |event| {
                        let mut next = minute_draft();
                        next.sent_minute = event.value();
                        minute_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-remaining-credits", {t!("delivery-sent-proof-remaining-credits")} }
                input {
                    id: "delivery-sent-remaining-credits",
                    r#type: "number",
                    min: "0",
                    required: true,
                    disabled: busy,
                    value: current.remaining_credits.clone(),
                    oninput: move |event| {
                        let mut next = credits_draft();
                        next.remaining_credits = event.value();
                        credits_draft.set(next);
                        preview.set(None);
                    },
                }
                label { r#for: "delivery-sent-cod-pending", {t!("delivery-sent-proof-cod-pending")} }
                input {
                    id: "delivery-sent-cod-pending",
                    r#type: "number",
                    min: "0",
                    required: true,
                    disabled: busy,
                    value: current.cod_pending.clone(),
                    oninput: move |event| {
                        let mut next = cod_draft();
                        next.cod_pending = event.value();
                        cod_draft.set(next);
                        preview.set(None);
                    },
                }
                label { class: "admin-delivery-sent-proof__check",
                    input {
                        r#type: "checkbox",
                        disabled: busy,
                        checked: current.collected,
                        onchange: move |event| {
                            let mut next = collected_draft();
                            next.collected = event.checked();
                            collected_draft.set(next);
                            preview.set(None);
                        },
                    }
                    {t!("delivery-sent-proof-collected")}
                }
                if current.collected {
                    label { r#for: "delivery-sent-receipt", {t!("delivery-sent-proof-receipt")} }
                    textarea {
                        id: "delivery-sent-receipt",
                        rows: "3",
                        disabled: busy,
                        value: current.receipt.clone(),
                        oninput: move |event| {
                            let mut next = receipt_draft();
                            next.receipt = event.value();
                            receipt_draft.set(next);
                            preview.set(None);
                        },
                    }
                }
                label { r#for: "delivery-sent-reason", {t!("delivery-sent-proof-reason")} }
                textarea {
                    id: "delivery-sent-reason",
                    rows: "3",
                    required: true,
                    disabled: busy,
                    value: current.reason.clone(),
                    oninput: move |event| {
                        let mut next = reason_draft();
                        next.reason = event.value();
                        reason_draft.set(next);
                    },
                }
                p { class: "admin-panel__note", {t!("delivery-sent-proof-note")} }
                div { class: "admin-shell__dialog-actions",
                    button {
                        r#type: "button",
                        disabled: busy || previewing(),
                        onclick: move |_| run_preview(),
                        if previewing() {
                            {t!("delivery-sent-proof-previewing")}
                        } else {
                            {t!("delivery-sent-proof-preview")}
                        }
                    }
                    button {
                        r#type: "button",
                        disabled: busy,
                        onclick: move |_| on_cancel.call(()),
                        {t!("delivery-cancel")}
                    }
                    button {
                        r#type: "submit",
                        class: "admin-btn-primary",
                        disabled: busy || !can_confirm,
                        if busy {
                            {t!("admin-action-working")}
                        } else {
                            {t!("delivery-sent-proof-confirm")}
                        }
                    }
                }
                if let Some(result) = preview_state {
                    match result {
                        Ok(preview) => rsx! {
                            section { class: "admin-delivery-sent-proof__preview", role: "status",
                                if preview.eligible {
                                    p { {t!("delivery-sent-proof-eligible")} }
                                } else {
                                    p { class: "admin-delivery-reconcile__warning",
                                        {t!("delivery-sent-proof-blocked")}
                                    }
                                }
                                if preview.corrects_admin_failure {
                                    p { class: "admin-panel__note",
                                        {t!("delivery-sent-proof-corrects-failure")}
                                    }
                                }
                                if !preview.blockers.is_empty() {
                                    ul { class: "admin-delivery-sent-proof__blockers",
                                        for blocker in preview.blockers.iter() {
                                            li { key: "{blocker}", {blocker_label(blocker)} }
                                        }
                                    }
                                }
                                if preview.requires_collected_asset_acceptance {
                                    label { class: "admin-delivery-sent-proof__check",
                                        input {
                                            r#type: "checkbox",
                                            disabled: busy,
                                            checked: accept_collected(),
                                            onchange: move |event| accept_collected.set(event.checked()),
                                        }
                                        {t!("delivery-sent-proof-accept-collected")}
                                    }
                                }
                                if preview.requires_attempt_history_acceptance {
                                    label { class: "admin-delivery-sent-proof__check",
                                        input {
                                            r#type: "checkbox",
                                            disabled: busy,
                                            checked: accept_attempts(),
                                            onchange: move |event| accept_attempts.set(event.checked()),
                                        }
                                        {t!("delivery-sent-proof-accept-attempts")}
                                    }
                                }
                            }
                        },
                        Err(code) => rsx! {
                            p { class: "admin-delivery-reconcile__warning", role: "alert",
                                {crate::app::surface::admin_api_error_message(&code)}
                            }
                        },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use auction_contracts::{
        AdminDeliveryAttemptResponse, AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
        AdminDeliveryEvidenceResponse, AdminDeliveryResponse,
    };
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{
        SentProofDraft, SentProofIssue, build_evidence, decode_hex_digest, selectable_evidence,
        sent_minute_from_input, sent_minute_from_timestamp,
    };

    fn time(minute: u32) -> String {
        Utc.with_ymd_and_hms(2026, 11, 2, 1, minute, 0).single().expect("timestamp").to_rfc3339()
    }

    fn evidence() -> AdminDeliveryEvidenceResponse {
        AdminDeliveryEvidenceResponse {
            reference: "mailbox_sweep:3125345".to_owned(),
            kind: "mailbox_sweep".to_owned(),
            external_mail_id: Some("3125345".to_owned()),
            observed_at: Some(time(2)),
            sent_at: Some(time(1)),
            recipient: Some("Restia".to_owned()),
            subject: Some("Auction settlement".to_owned()),
            body_digest: Some("cd".repeat(32)),
            collected: Some(false),
            cod_pending: Some(0),
            remaining_credits: Some(300_000),
            remaining_equipment_count: Some(0),
            receipt: None,
        }
    }

    fn detail() -> AdminDeliveryDetailResponse {
        AdminDeliveryDetailResponse {
            delivery: AdminDeliveryResponse {
                id: Uuid::from_bytes([1; 16]).to_string(),
                origin_kind: "settlement".to_owned(),
                origin_id: "42".to_owned(),
                recipient: "Restia".to_owned(),
                cod: Some(0),
                status: "manual_reconcile".to_owned(),
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
                recipient: Some("Restia".to_owned()),
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
        draft.sender_name = "Restia".to_owned();
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
        assert_eq!(
            sent_minute_from_input("2026-11-02T01:14"),
            Some("2026-11-02T01:14:00Z".to_owned())
        );
        assert_eq!(sent_minute_from_input(""), None);
        assert_eq!(
            sent_minute_from_timestamp("2026-11-02T01:14:37Z"),
            Some("2026-11-02T01:14:00Z".to_owned())
        );
    }

    #[test]
    fn builds_evidence_from_frozen_attempt_and_observation() {
        let detail = detail();
        let built = build_evidence(&detail, &filled_draft(&detail)).expect("evidence");
        assert_eq!(built.schema_version, 1);
        assert_eq!(built.world_id, "main");
        assert_eq!(built.external_mail_id, "3125345");
        assert_eq!(built.sender_uid, "12345");
        assert_eq!(built.recipient, "Restia");
        assert_eq!(built.sent_minute, "2026-11-02T01:01:00Z");
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
    fn collected_draft_drops_frozen_equipment_ids() {
        let mut detail = detail();
        detail.attempts[0].credits = Some(0);
        detail.attempts[0].cod = Some(0);
        detail.attempts[0].equipment_ids = vec!["300753957".to_owned()];
        detail.evidence[0].remaining_equipment_count = Some(0);
        let mut draft = filled_draft(&detail);
        draft.collected = true;

        let built = build_evidence(&detail, &draft).expect("evidence");
        assert!(built.collected);
        assert!(built.equipment_ids.is_empty());

        let mut uncollected = draft;
        uncollected.collected = false;
        let built = build_evidence(&detail, &uncollected).expect("evidence");
        assert_eq!(built.equipment_ids, vec!["300753957".to_owned()]);
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
}
