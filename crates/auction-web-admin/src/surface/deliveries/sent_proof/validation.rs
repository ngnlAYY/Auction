//! 草稿的本地形状校验与闭集请求组装。
//!
//! 这里只阻止明显不完整的草稿；真正的准入判定仍由服务端预览与事务完成。

use chrono::Timelike;

use super::evidence::{SELECTABLE_EVIDENCE_KINDS, decode_hex_digest, latest_send_attempt};
use super::state::{SentProofDraft, SentProofIssue};
use crate::api::{DeliveryDetail, DeliverySentEvidence, MAX_JSON_SAFE_INTEGER};

/// 发送分钟统一使用带 `Z` 的 UTC 整分钟文本。
const SENT_MINUTE_FORMAT: &str = "%Y-%m-%dT%H:%M:00Z";

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

pub(super) fn valid_reason(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.len() <= 2048 && !trimmed.chars().any(char::is_control)
}

fn parse_amount(value: &str) -> Option<i64> {
    let parsed = value.trim().parse::<i64>().ok()?;
    (0..=MAX_JSON_SAFE_INTEGER).contains(&parsed).then_some(parsed)
}

/// 组合冻结尝试与已采集证据；任一必需字段缺失都返回本地问题码，不提交半成品。
pub(super) fn build_evidence(
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
